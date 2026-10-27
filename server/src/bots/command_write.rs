//! Async command continuations and atomic effect/receipt publication.
use super::{CommandInput, command_account, model_commands, phase};
use crate::{
    bots::{
        api::admit,
        auth::Context,
        error::{Code, Error},
    },
    engine::{EngineError, MetadataPreparation, NativeCommand, PendingCommand},
    web::{App, Change},
};
use axum::http::HeaderMap;
use serde_json::{Value, json};

enum Ordering {
    Metadata(crate::web::mutations::MetadataPermit),
    Conversations(crate::web::mutations::ConversationPermit),
}
impl Ordering {
    async fn with<T: Send + 'static>(
        &self,
        app: &App,
        operation: impl FnOnce(&mut crate::engine::Engine) -> Result<T, Error> + Send + 'static,
    ) -> Result<T, Error> {
        match self {
            Self::Metadata(permit) => app.with_metadata(permit, operation).await,
            Self::Conversations(permit) => app.with_conversation(permit, operation).await,
        }
    }
}
async fn prepare(
    app: &App,
    ordering: &Ordering,
    context: &Context,
    input: &CommandInput,
) -> Result<NativeCommand, Error> {
    ordering
        .with(app, {
            let context = context.clone();
            let input = input.clone();
            move |engine| {
                phase(engine, &context, &input, |engine, user, view, text| {
                    if text.split_whitespace().collect::<Vec<_>>() == ["/help"] {
                        engine.require_command(Some(user), Some(view), "/help")?;
                        return Ok(NativeCommand::Ready(
                            crate::commands::help_for(&model_commands(engine, user, view)),
                            PendingCommand::None,
                        ));
                    }
                    if text.split_whitespace().collect::<Vec<_>>() == ["/configs"] {
                        use crate::engine::authorization::{Action, Scope};
                        engine.require_command(Some(user), Some(view), "/configs")?;
                        engine.require_target_command(Some(user), &Scope::Server, "/configs")?;
                        engine.require(Some(user), &Scope::Server, Action::Config)?;
                        return Ok(NativeCommand::Configs(engine.history_store().clone()));
                    }
                    engine.prepare_native_command(user, view, text, true)
                })
            }
        })
        .await
}
pub(super) async fn execute(
    app: &App,
    headers: HeaderMap,
    input: CommandInput,
    key: String,
) -> Result<Value, Error> {
    let context = Context::authenticate_async(app, &headers).await?;
    let digest = crate::bots::digest(
        &serde_json::to_string(
            &json!({"conversation_id":input.conversation_id,"command":input.command.trim()}),
        )
        .map_err(Error::storage)?,
    );
    let mut ordering = Ordering::Conversations(app.mutations.conversations(vec![]).await?);
    let receipts = ordering
        .with(app, {
            let context = context.clone();
            move |engine| -> Result<_, Error> {
                command_account(engine, &context)?;
                admit(engine, &context)?;
                Ok(engine.command_receipts())
            }
        })
        .await?;
    if let Some((previous, outcome)) = receipts.lookup(&context.account, &key).await? {
        ordering
            .with(app, {
                let context = context.clone();
                move |engine| command_account(engine, &context)
            })
            .await?;
        if previous != digest {
            return Err(Error::new(
                Code::IdempotencyConflict,
                "Key already belongs to a different command request.",
            ));
        }
        return outcome.into_result();
    }
    let mut planned = prepare(app, &ordering, &context, &input).await;
    if let Ok(plan) = &planned {
        let metadata = matches!(plan, NativeCommand::Metadata(_));
        let ids = plan.ordering_ids();
        if metadata || !ids.is_empty() {
            drop(ordering);
            ordering = if metadata {
                Ordering::Metadata(app.mutations.metadata().await)
            } else {
                Ordering::Conversations(app.mutations.conversations(ids.clone()).await?)
            };
            planned = prepare(app, &ordering, &context, &input).await;
            if let Ok(refreshed) = &planned
                && (!metadata && refreshed.ordering_ids() != ids
                    || metadata != matches!(refreshed, NativeCommand::Metadata(_)))
            {
                planned = Err(Error::new(
                    Code::Conflict,
                    "Command targets changed; retry the operation.",
                ));
            }
        }
    }
    let _audit = if planned.as_ref().is_ok_and(NativeCommand::audited) {
        Some(app.mutations.audit.lock().await)
    } else {
        None
    };
    let prepared = match planned {
        Ok(plan) => finish(app, &ordering, &context, &input, plan).await,
        Err(error) => Err(error),
    };
    let (result, effect) = match prepared {
        Ok((reply, effect)) => (Ok(reply), effect),
        Err(error) => (Err(error), PendingCommand::None),
    };
    ordering
        .with(app, {
            let context = context.clone();
            move |engine| command_account(engine, &context)
        })
        .await?;
    let committed = receipts
        .commit(&context.account, &key, &digest, result, effect)
        .await?;
    let changes = app.changes.clone();
    ordering
        .with(app, move |engine| -> Result<_, Error> {
            let (outcome, change) = engine.publish_command(committed)?;
            if let Some(change) = Change::command(change) {
                let _ = changes.send(change);
            }
            outcome.into_result()
        })
        .await
}

async fn finish(
    app: &App,
    ordering: &Ordering,
    context: &Context,
    input: &CommandInput,
    plan: NativeCommand,
) -> Result<(String, PendingCommand), Error> {
    let ready = match plan {
        NativeCommand::Ready(reply, effect) => (reply, effect),
        NativeCommand::Read(plan) => (plan.finish_async().await?, PendingCommand::None),
        NativeCommand::Configs(history) => (
            app.config
                .display_with_history(history.stats_async().await?)
                .map_err(|e| Error::from(EngineError::Invalid(e)))?,
            PendingCommand::None,
        ),
        NativeCommand::Metadata(MetadataPreparation::Ready(reply, pending)) => (
            reply,
            pending.map_or(PendingCommand::None, PendingCommand::Metadata),
        ),
        NativeCommand::Metadata(MetadataPreparation::Rename(read)) => {
            let references = read.read().await?;
            ordering
                .with(app, {
                    let context = context.clone();
                    let input = input.clone();
                    move |engine| {
                        phase(engine, &context, &input, |engine, user, _, _| {
                            engine
                                .prepare_referenced_rename(Some(user), references)
                                .map(|(reply, pending)| {
                                    (reply, PendingCommand::Metadata(Box::new(pending)))
                                })
                        })
                    }
                })
                .await?
        }
        NativeCommand::Action(plan) => {
            let original = plan
                .history
                .locate_async(&plan.action.id)
                .await?
                .filter(|(id, _)| {
                    plan.conversation
                        .as_ref()
                        .is_none_or(|expected| expected == id)
                })
                .ok_or_else(|| {
                    Error::new(
                        Code::NotFound,
                        "Message not found in this conversation (it may have expired).",
                    )
                })?;
            ordering
                .with(app, {
                    let context = context.clone();
                    let input = input.clone();
                    move |engine| {
                        phase(engine, &context, &input, |engine, user, view, _| {
                            engine.prepare_native_action(
                                user,
                                view,
                                plan,
                                original.1,
                                &original.0,
                                true,
                            )
                        })
                    }
                })
                .await?
        }
        NativeCommand::Cleanup(mut plan) => {
            plan.count().await?;
            let reply = plan.report();
            ordering
                .with(app, {
                    let context = context.clone();
                    let input = input.clone();
                    move |engine| {
                        phase(engine, &context, &input, |engine, user, _, _| {
                            engine.prepare_history_cleanup(user, &plan).map(|pending| {
                                (
                                    reply,
                                    pending.map_or(PendingCommand::None, PendingCommand::History),
                                )
                            })
                        })
                    }
                })
                .await?
        }
    };
    if let PendingCommand::Message(pending) = &ready.1 {
        let media = pending.media_records().await?;
        if !media.is_empty() {
            return ordering
                .with(app, {
                    let context = context.clone();
                    let input = input.clone();
                    move |engine| {
                        phase(engine, &context, &input, |engine, _, _, _| {
                            if let PendingCommand::Message(pending) = &ready.1 {
                                engine.validate_append_media(pending, &media)?;
                            }
                            Ok(ready)
                        })
                    }
                })
                .await;
        }
    }
    Ok(ready)
}
