use super::auth::identity;
#[path = "credential_commands.rs"]
mod credentials;
use super::*;
use crate::engine::EngineError;
use crate::engine::sudo_command;
use axum::http::HeaderMap;
use serde::Deserialize;

#[derive(Clone, Deserialize)]
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
    let inner = sudo_command(&input.text)
        .map_err(String::from)?
        .unwrap_or(&input.text);
    if inner.split_whitespace().next() == Some("/clean")
        && inner.split_whitespace().last() != Some("--dry-run")
    {
        return tokio::spawn(async move {
            let _permit = permit;
            super::cleanup_commands::execute(&app, headers, user, input)
                .await
                .map_err(String::from)
        })
        .await
        .map_err(|_| "Command worker unavailable.")?;
    }
    if !inner.trim_start().starts_with('/')
        || matches!(
            inner.split_whitespace().next(),
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
    if super::query_commands::supports(&input.text).map_err(String::from)? {
        return tokio::spawn(async move {
            let _permit = permit;
            super::query_commands::execute(&app, headers, user, input)
                .await
                .map_err(String::from)
        })
        .await
        .map_err(|_| "Command worker unavailable.")?;
    }
    if let Some(request) = credentials::parse(&input).map_err(String::from)? {
        return tokio::spawn(async move {
            let _permit = permit;
            credentials::execute(&app, headers, user, request)
                .await
                .map_err(String::from)
        })
        .await
        .map_err(|_| "Command worker unavailable.")?;
    }
    if crate::engine::native_metadata_command(
        sudo_command(&input.text)
            .map_err(String::from)?
            .unwrap_or(&input.text),
    ) {
        return tokio::spawn(async move {
            let _permit = permit;
            super::metadata_commands::execute(&app, headers, user, input)
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
/// Elevation lasts only for this synchronous phase. Every preparation phase
/// checks the credential and sudo grant again; owned plans carry no authority.
pub(super) fn phase<T>(
    engine: &mut Engine,
    identity: &auth::Identity,
    input: &Input,
    operation: impl FnOnce(&mut Engine, &str) -> Result<T, EngineError>,
) -> Result<T, EngineError> {
    identity.verify(engine)?;
    let elevated = sudo_command(&input.text)?;
    engine.with_command_authority(
        Some(&identity.user),
        input.room.as_deref(),
        elevated.is_some(),
        |engine| operation(engine, elevated.unwrap_or(&input.text)),
    )
}

pub(super) async fn capture<T: Send + 'static>(
    app: &App,
    headers: HeaderMap,
    user: String,
    input: &Input,
    operation: impl FnOnce(&Engine, &str) -> Result<T, EngineError> + Send + 'static,
) -> Result<(auth::Identity, T), EngineError> {
    let config = app.config.clone();
    let input = input.clone();
    let identify = move |engine: &Engine| -> Result<auth::Identity, EngineError> {
        if identity(engine, &config, &headers).as_deref() != Some(&user) {
            return Err("Please log in.".into());
        }
        let account = engine
            .active_account_id(&user)
            .ok_or("Account unavailable.")?
            .to_owned();
        Ok(auth::Identity {
            headers,
            user,
            account,
            config,
        })
    };
    if sudo_command(&input.text)?.is_some() {
        let ordering = app.mutations.conversations(vec![]).await?;
        app.with_conversation(&ordering, move |engine| {
            let identity = identify(engine)?;
            let result = phase(engine, &identity, &input, |engine, text| {
                operation(engine, text)
            })?;
            Ok((identity, result))
        })
        .await
    } else {
        app.inspect_engine(move |engine, _| {
            let identity = identify(engine)?;
            let result = operation(engine, &input.text)?;
            Ok((identity, result))
        })
        .await
    }
}

// Keep accepted native writes alive through publication if the socket closes.
async fn execute_message(
    app: &App,
    headers: HeaderMap,
    user: String,
    input: Input,
) -> Result<(String, Option<Change>), EngineError> {
    let (identity, plan) = capture(app, headers, user.clone(), &input, {
        let view = input.room.clone();
        move |engine, text| {
            if matches!(
                text.split_whitespace().next(),
                Some("/reply" | "/edit" | "/react" | "/retract")
            ) {
                engine
                    .browser_action_plan(&user, view.as_deref(), text)
                    .map(MessagePlan::Action)
            } else {
                engine
                    .browser_message_conversation(&user, view.as_deref(), text)
                    .map(MessagePlan::Append)
            }
        }
    })
    .await?;
    let _account_permit = app
        .admission
        .http(identity.account.clone())
        .map_err(EngineError::Unavailable)?;
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
    let media_input = input.clone();
    let (pending, reply) = match plan {
        MessagePlan::Append(_) => {
            app.with_conversation(&ordering, {
                let identity = identity.clone();
                move |engine| -> Result<_, EngineError> {
                    phase(engine, &identity, &input, |engine, text| {
                        let pending = engine.prepare_browser_message(
                            &identity.user,
                            input.room.as_deref(),
                            text,
                            input.markdown,
                        )?;
                        if pending.conversation != id {
                            return Err(EngineError::Conflict(
                                "Conversation changed; retry the operation.".into(),
                            ));
                        }
                        let reply = if text.split_whitespace().next() == Some("/tell") {
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
                    })
                }
            })
            .await?
        }
        MessagePlan::Action(plan) => {
            app.with_conversation(&ordering, {
                let identity = identity.clone();
                let action = plan.action.clone();
                let input = input.clone();
                let id = id.clone();
                move |engine| -> Result<_, EngineError> {
                    phase(engine, &identity, &input, |engine, _| {
                        engine.authorize_browser_action(
                            &identity.user,
                            input.room.as_deref(),
                            &action,
                            &id,
                        )
                    })
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
                    phase(engine, &identity, &input, |engine, _| {
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
                                        pending.map(|pending| {
                                            PreparedWrite::Message(Box::new(pending))
                                        }),
                                        reply,
                                    )
                                })
                        }
                    })
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
                phase(engine, &identity, &media_input, |engine, _| {
                    engine.validate_append_media(&pending, &media)?;
                    Ok(pending)
                })
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
    let routed = crate::commands::route(text);
    let parts: Vec<_> = routed.split_whitespace().collect();
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
    Ok((outcome.reply, Change::command(outcome.change)))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "elevated_write_tests.rs"]
mod tests;
