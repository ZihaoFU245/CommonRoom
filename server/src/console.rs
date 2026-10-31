use crate::{
    commands,
    engine::{EngineError, MetadataPreparation, hash_password, sudo_command},
    web::{App, Change},
};
use std::io::{self, BufRead};

pub(crate) struct Output {
    reply: String,
    secret: bool,
}
struct Request {
    line: String,
    reply: tokio::sync::oneshot::Sender<Result<Option<Output>, EngineError>>,
}
pub fn start(app: App) {
    let (sender, mut requests) = tokio::sync::mpsc::channel::<Request>(1);
    // The blocking stdin owner holds no App/database reference. Shutdown can
    // drain accepted work and release database ownership while stdin stays open.
    std::thread::spawn(move || {
        tracing::info!("Console ready. /help lists commands. Console input is never logged.");
        for line in io::stdin().lock().lines() {
            let line = match line {
                Ok(line) => line,
                Err(error) => {
                    tracing::error!(error = %error, "Console read failed");
                    break;
                }
            };
            let (reply, response) = tokio::sync::oneshot::channel();
            if sender.blocking_send(Request { line, reply }).is_err() {
                break;
            }
            match response.blocking_recv() {
                Ok(Ok(Some(output))) if output.secret => {
                    use std::io::Write;
                    if let Err(error) = writeln!(io::stdout().lock(), "{}", output.reply) {
                        tracing::error!(error = %error, "Could not display credential");
                    }
                }
                Ok(Ok(Some(output))) => tracing::info!("{}", output.reply),
                Ok(Ok(None)) => {}
                Ok(Err(error)) => tracing::warn!("{error}"),
                Err(_) => break,
            }
        }
        tracing::info!("Console stdin closed; web server remains running.");
    });
    let _worker = tokio::spawn(async move {
        loop {
            let shutdown = app.shutdown.notified();
            if app.stopping.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            let request =
                tokio::select! { _ = shutdown => break, request = requests.recv() => request };
            let Some(request) = request else {
                break;
            };
            let result = execute(&app, request.line).await;
            let _ = request.reply.send(result);
        }
    });
}

pub(crate) async fn execute(app: &App, line: String) -> Result<Option<Output>, EngineError> {
    if app.stopping.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(EngineError::Unavailable("Server stopping.".into()));
    }
    if line.trim().is_empty() {
        return Ok(None);
    }
    let permit = app.io_permit().map_err(EngineError::Unavailable)?;
    let app = app.clone();
    tokio::spawn(async move {
        let _permit = permit;
        execute_inner(&app, line).await.map(Some)
    })
    .await
    .map_err(EngineError::unavailable)?
}
async fn execute_inner(app: &App, line: String) -> Result<Output, EngineError> {
    crate::limits::validate_input(&line).map_err(EngineError::Invalid)?;
    let command = sudo_command(&line)?.unwrap_or(&line).trim();
    let parts = command.split_whitespace().collect::<Vec<_>>();
    let provision = commands::provision_command(&parts).map_err(EngineError::Invalid)?;
    let reply = match (parts.first().copied(), provision) {
        (Some("/help"), _) if parts.len() == 1 => commands::help(true, true),
        (Some("/system"), _) => {
            app.inspect_engine(|engine, _| engine.require_command(None, None, "/system"))
                .await?;
            crate::system::Command::parse(command)?.execute(app).await?
        }
        (_, Some(account)) => {
            let name = account.name.to_owned();
            let password = account.password.to_owned();
            let (admin, reset) = (account.admin, account.reset);
            let _password = app.password_permit()?;
            let hash = tokio::task::spawn_blocking(move || hash_password(&password))
                .await
                .map_err(EngineError::unavailable)??;
            let ordering = app.mutations.metadata().await?;
            let (reply, pending) = app
                .with_metadata(&ordering, move |engine| {
                    engine.prepare_provision(None, &name, hash, admin, reset)
                })
                .await?;
            let pending = pending.save().await?;
            let changes = app.changes.clone();
            app.with_metadata(&ordering, move |engine| {
                engine.publish_metadata(pending)?;
                let _ = changes.send(Change::Metadata(engine.metadata_change()));
                Ok::<_, EngineError>(reply)
            })
            .await?
        }
        (Some("/edit" | "/retract"), _) => action(app, command.to_owned()).await?,
        (Some("/tell"), _) => return Err("Direct messages require a user account.".into()),
        (Some("/reply" | "/react"), _) => {
            return Err("Message actions require a user account.".into());
        }
        _ => ordinary(app, command.to_owned()).await?,
    };
    Ok(Output {
        reply,
        secret: parts.get(0..3) == Some(&["/bot", "token", "new"]),
    })
}
async fn ordinary(app: &App, input: String) -> Result<String, EngineError> {
    let read = app
        .inspect_engine({
            let input = input.clone();
            move |engine, _| -> Result<_, EngineError> {
                let read = engine
                    .prepare_read_command(None, None, &input)
                    .transpose()?;
                Ok((read, engine.history_store().clone()))
            }
        })
        .await?;
    if let (Some(plan), history) = read {
        let reply = plan.finish_async().await?;
        history.ensure_available_async().await?;
        return Ok(reply);
    }
    if input.split_whitespace().next() == Some("/clean") {
        return cleanup(app, input).await;
    }
    if !crate::engine::native_metadata_command(&input) {
        return app
            .inspect_engine(move |engine, _| {
                if !input.starts_with('/') {
                    return Err(
                        if input.chars().count() > crate::limits::MAX_MESSAGE_CHARS {
                            "Messages support at most 4000 characters."
                        } else {
                            "Select a room first."
                        }
                        .into(),
                    );
                }
                engine.reject_command(None, None, &input)
            })
            .await;
    }
    let ordering = app.mutations.metadata().await?;
    let prepared = app
        .with_metadata(&ordering, move |engine| {
            engine.prepare_metadata(None, None, &input)
        })
        .await?;
    let (reply, pending) = match prepared {
        MetadataPreparation::Ready(reply, pending) => (reply, pending.map(|pending| *pending)),
        MetadataPreparation::Rename(read) => {
            let references = read.read().await?;
            let (reply, pending) = app
                .with_metadata(&ordering, move |engine| {
                    engine.prepare_referenced_rename(None, references)
                })
                .await?;
            (reply, Some(pending))
        }
    };
    let Some(pending) = pending else {
        let history = app
            .inspect_engine(|engine, _| Ok::<_, EngineError>(engine.history_store().clone()))
            .await?;
        history.ensure_available_async().await?;
        return Ok(reply);
    };
    let pending = pending.save().await?;
    let changes = app.changes.clone();
    app.with_metadata(&ordering, move |engine| {
        engine.publish_metadata(pending)?;
        let _ = changes.send(Change::Metadata(engine.metadata_change()));
        Ok::<_, EngineError>(reply)
    })
    .await
}
async fn action(app: &App, input: String) -> Result<String, EngineError> {
    let plan = app
        .inspect_engine(move |engine, _| engine.action_plan(None, None, &input))
        .await?;
    let (id, original) = plan
        .history
        .locate_async(&plan.action.id)
        .await?
        .ok_or("Message not found in this conversation (it may have expired).")?;
    let ordering = app.mutations.conversation(id.clone()).await?;
    let _audit = if plan.action.is_retraction() {
        Some(app.mutations.audit().await?)
    } else {
        None
    };
    if plan.action.is_retraction() {
        let pending = app
            .with_conversation(&ordering, move |engine| {
                engine.prepare_retraction(None, None, &plan.action, &id, &original)
            })
            .await?;
        pending.save().await?;
        let changes = app.changes.clone();
        return app
            .with_conversation(&ordering, move |engine| {
                if let Some(change) = Change::command(engine.publish_history(pending)?) {
                    let _ = changes.send(change);
                }
                Ok::<_, EngineError>("Message deleted.".into())
            })
            .await;
    }
    let (pending, reply) = app
        .with_conversation(&ordering, move |engine| {
            engine.prepare_action(None, None, plan.action, &id, original, false)
        })
        .await?;
    let Some(pending) = pending else {
        return Ok(reply);
    };
    let commit = pending.save(None).await?;
    let changes = app.changes.clone();
    app.with_conversation(&ordering, move |engine| {
        let id = pending.conversation.clone();
        engine.publish_message(pending, commit)?;
        let _ = changes.send(Change::Conversation(id));
        Ok::<_, EngineError>(reply)
    })
    .await
}
async fn cleanup(app: &App, input: String) -> Result<String, EngineError> {
    let ids = app
        .inspect_engine({
            let input = input.clone();
            move |engine, _| {
                engine
                    .prepare_cleanup(None, None, &input)
                    .map(|plan| plan.ids())
            }
        })
        .await?;
    let ordering = app.mutations.conversations(ids.clone()).await?;
    let mut plan = app
        .with_conversation(&ordering, move |engine| {
            let plan = engine.prepare_cleanup(None, None, &input)?;
            if plan.ids() != ids {
                return Err(EngineError::Conflict(
                    "Cleanup targets changed; retry the operation.".into(),
                ));
            }
            Ok(plan)
        })
        .await?;
    plan.count().await?;
    let reply = plan.report();
    let _audit = if !plan.preview && plan.has_matches() {
        Some(app.mutations.audit().await?)
    } else {
        None
    };
    let pending = app
        .with_conversation(&ordering, move |engine| {
            engine.prepare_cleanup_write(None, &plan)
        })
        .await?;
    let Some(pending) = pending else {
        return Ok(reply);
    };
    pending.save().await?;
    let changes = app.changes.clone();
    app.with_conversation(&ordering, move |engine| {
        if let Some(change) = Change::command(engine.publish_history(pending)?) {
            let _ = changes.send(change);
        }
        Ok::<_, EngineError>(reply)
    })
    .await
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "console_tests.rs"]
mod tests;
