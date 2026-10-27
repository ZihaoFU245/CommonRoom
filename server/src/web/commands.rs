use super::auth::identity;
#[path = "credential_commands.rs"]
mod credentials;
use super::*;
use crate::engine::EngineError;
use crate::engine::{
    authorization::{Action, Scope},
    sudo_command,
};
use axum::http::HeaderMap;
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct Input {
    pub(crate) id: u64,
    pub(crate) room: Option<String>,
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) markdown: bool,
}
pub(crate) async fn execute_input(
    app: &App,
    headers: &HeaderMap,
    user: &str,
    input: &Input,
) -> Result<(String, Option<Change>), String> {
    crate::limits::validate_input(&input.text)?;
    let permit = app.io_permit()?;
    let app = app.clone();
    let headers = headers.clone();
    let user = user.to_owned();
    let input = Input {
        id: input.id,
        markdown: input.markdown,
        room: input.room.clone(),
        text: input.text.clone(),
    };
    if input.text.split_whitespace().next() == Some("/clean") {
        return tokio::spawn(async move {
            let _permit = permit;
            super::cleanup_commands::execute(&app, headers, user, input)
                .await
                .map_err(String::from)
        })
        .await
        .map_err(|_| "Command worker unavailable.")?;
    }
    if !input.text.trim_start().starts_with('/')
        || matches!(
            input.text.split_whitespace().next(),
            Some("/tell" | "/reply" | "/edit" | "/react" | "/retract")
        )
    {
        return tokio::spawn(async move {
            let _permit = permit;
            execute_message(&app, headers, user, input)
                .await
                .map_err(String::from)
        })
        .await
        .map_err(|_| "Command worker unavailable.")?;
    }
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let account = app
            .engine_read()?
            .active_account_id(&user)
            .ok_or("Account unavailable.")?
            .to_owned();
        let _account_permit = app.admission.http(account)?;
        execute_input_inner(&app, &headers, &user, &input).map_err(String::from)
    })
    .await
    .map_err(|_| "Command worker unavailable.")?
}
enum PreparedWrite {
    Message(Box<crate::engine::PendingMessage>),
    History(crate::engine::PendingHistory),
}
enum MessagePlan {
    Append(String),
    Action(crate::engine::BrowserActionPlan),
}
// Keep accepted native writes alive through publication if the socket closes.
async fn execute_message(
    app: &App,
    headers: HeaderMap,
    user: String,
    input: Input,
) -> Result<(String, Option<Change>), EngineError> {
    let (account, plan) = app
        .inspect_engine({
            let headers = headers.clone();
            let user = user.clone();
            let view = input.room.clone();
            let text = input.text.clone();
            move |engine, config| -> Result<_, EngineError> {
                if identity(engine, config, &headers).as_deref() != Some(&user) {
                    return Err("Please log in.".into());
                }
                let account = engine
                    .active_account_id(&user)
                    .ok_or("Account unavailable.")?
                    .to_owned();
                let plan = if matches!(
                    text.split_whitespace().next(),
                    Some("/reply" | "/edit" | "/react" | "/retract")
                ) {
                    MessagePlan::Action(engine.browser_action_plan(
                        &user,
                        view.as_deref(),
                        &text,
                    )?)
                } else {
                    MessagePlan::Append(engine.browser_message_conversation(
                        &user,
                        view.as_deref(),
                        &text,
                    )?)
                };
                Ok((account, plan))
            }
        })
        .await?;
    let _account_permit = app
        .admission
        .http(account.clone())
        .map_err(EngineError::Unavailable)?;
    let identity = auth::Identity {
        headers,
        user,
        account,
        config: app.config.clone(),
    };
    let id = match &plan {
        MessagePlan::Append(id) => id.clone(),
        MessagePlan::Action(plan) => match &plan.conversation {
            Some(id) => id.clone(),
            None => plan
                .history
                .locate_async(&plan.action.id)
                .await?
                .map(|(id, _)| id)
                .ok_or("Message not found in this conversation.")?,
        },
    };
    let ordering = app.mutations.conversation(id.clone()).await?;
    let _audit = if matches!(&plan, MessagePlan::Action(plan) if plan.action.is_retraction()) {
        Some(app.mutations.audit.lock().await)
    } else {
        None
    };
    let (pending, reply) = match plan {
        MessagePlan::Append(_) => {
            app.with_conversation(&ordering, {
                let identity = identity.clone();
                move |engine| -> Result<_, EngineError> {
                    identity.verify(engine)?;
                    let pending = engine.prepare_browser_message(
                        &identity.user,
                        input.room.as_deref(),
                        &input.text,
                        input.markdown,
                    )?;
                    if pending.conversation != id {
                        return Err(EngineError::Conflict(
                            "Conversation changed; retry the operation.".into(),
                        ));
                    }
                    let reply = if input.text.split_whitespace().next() == Some("/tell") {
                        format!(
                            "Private message sent to {}.",
                            pending
                                .message
                                .to
                                .as_deref()
                                .ok_or("Private message has no recipient.")?
                        )
                    } else {
                        String::new()
                    };
                    Ok((Some(PreparedWrite::Message(Box::new(pending))), reply))
                }
            })
            .await?
        }
        MessagePlan::Action(plan) => {
            app.with_conversation(&ordering, {
                let identity = identity.clone();
                let action = plan.action.clone();
                let view = input.room.clone();
                let id = id.clone();
                move |engine| -> Result<_, EngineError> {
                    identity.verify(engine)?;
                    engine.authorize_browser_action(&identity.user, view.as_deref(), &action, &id)
                }
            })
            .await?;
            let original = plan
                .history
                .locate_async(&plan.action.id)
                .await?
                .filter(|(conversation, _)| conversation == &id)
                .map(|(_, message)| message)
                .ok_or("Message not found in this conversation (it may have expired).")?;
            app.with_conversation(&ordering, {
                let identity = identity.clone();
                move |engine| -> Result<_, EngineError> {
                    identity.verify(engine)?;
                    if plan.action.is_retraction() {
                        engine
                            .prepare_browser_retraction(
                                &identity.user,
                                input.room.as_deref(),
                                &plan.action,
                                &id,
                                &original,
                            )
                            .map(|pending| {
                                (
                                    Some(PreparedWrite::History(pending)),
                                    "Message deleted.".into(),
                                )
                            })
                    } else {
                        engine
                            .prepare_browser_action(
                                &identity.user,
                                input.room.as_deref(),
                                plan.action,
                                &id,
                                original,
                                input.markdown,
                            )
                            .map(|(pending, reply)| {
                                (
                                    pending
                                        .map(|pending| PreparedWrite::Message(Box::new(pending))),
                                    reply,
                                )
                            })
                    }
                }
            })
            .await?
        }
    };
    let Some(pending) = pending else {
        return Ok((reply, None));
    };
    let pending = match pending {
        PreparedWrite::Message(pending) => *pending,
        PreparedWrite::History(pending) => {
            pending.save().await?;
            let changes = app.changes.clone();
            return app
                .with_conversation(&ordering, move |engine| -> Result<_, EngineError> {
                    let effect = engine.publish_history(pending)?;
                    if let Some(change) = Change::command(effect) {
                        let _ = changes.send(change);
                    }
                    identity.verify(engine)?;
                    Ok((reply, None))
                })
                .await;
        }
    };
    let media = pending.media_records().await.map_err(EngineError::from)?;
    let pending = if media.is_empty() {
        pending
    } else {
        app.with_conversation(&ordering, {
            let identity = identity.clone();
            move |engine| -> Result<_, EngineError> {
                identity.verify(engine)?;
                engine.validate_append_media(&pending, &media)?;
                Ok(pending)
            }
        })
        .await?
    };
    let commit = pending.save(None).await?;
    let changes = app.changes.clone();
    app.with_conversation(&ordering, move |engine| -> Result<_, EngineError> {
        let id = pending.conversation.clone();
        let created = pending.creates_conversation();
        engine.publish_message(pending, commit)?;
        let change = if created {
            Change::Metadata(engine.metadata_change())
        } else {
            Change::Conversation(id)
        };
        let _ = changes.send(change);
        identity.verify(engine)?;
        Ok((reply, None))
    })
    .await
}
fn execute_input_inner(
    app: &App,
    headers: &HeaderMap,
    user: &str,
    input: &Input,
) -> Result<(String, Option<Change>), EngineError> {
    {
        let engine = app.engine_read()?;
        if identity(&engine, &app.config, headers).as_deref() != Some(user) {
            return Err("Please log in.".into());
        }
    }
    let elevated = sudo_command(&input.text)?;
    let text = elevated.unwrap_or(&input.text);
    let sudo = elevated.is_some();
    let command = text.split_whitespace().next().unwrap_or("");
    let routed = crate::commands::route(text);
    let parts: Vec<_> = routed.split_whitespace().collect();
    let provision = crate::commands::provision_command(&parts).map_err(EngineError::Invalid)?;
    if sudo && matches!(parts.first(), Some(&"/clear" | &"/logout")) {
        return app.engine()?.with_command_authority(
            Some(user),
            input.room.as_deref(),
            true,
            |_| {
                if parts.len() != 1 {
                    return Err(EngineError::Invalid(format!("Usage: {}", parts[0])));
                }
                Ok(("Done.".into(), None))
            },
        );
    }
    if parts.first() == Some(&"/configs") {
        let mut engine = app.engine()?;
        if identity(&engine, &app.config, headers).as_deref() != Some(user) {
            return Err("Please log in.".into());
        }
        return engine.with_command_authority(Some(user), input.room.as_deref(), sudo, |engine| {
            engine.require_command(Some(user), input.room.as_deref(), "/configs")?;
            engine.require_target_command(Some(user), &Scope::Server, "/configs")?;
            engine.require(Some(user), &Scope::Server, Action::Config)?;
            if parts.len() != 1 {
                return Err("Usage: /configs".into());
            }
            app.config
                .display_with_history(engine.history_store().stats()?)
                .map_err(EngineError::Invalid)
                .map(|text| (text, None))
        });
    }
    let passwd = parts.first() == Some(&"/passwd");
    if !passwd && provision.is_none() {
        if !sudo {
            let engine = app.engine_read()?;
            if identity(&engine, &app.config, headers).as_deref() != Some(user) {
                return Err("Please log in.".into());
            }
            if let Some(result) =
                engine.read_command(Some(user), input.room.as_deref(), &input.text)
            {
                return result.map(|reply| (reply, None));
            }
        }
        let mut engine = app.engine()?;
        if identity(&engine, &app.config, headers).as_deref() != Some(user) {
            return Err("Please log in.".into());
        }
        let outcome = engine.execute_command(
            Some(user),
            input.room.as_deref(),
            &input.text,
            input.markdown,
        )?;
        let change = Change::command(outcome.change);
        return Ok((outcome.reply, change));
    }
    credentials::execute(app, headers, user, input, sudo, command, &parts)
}
