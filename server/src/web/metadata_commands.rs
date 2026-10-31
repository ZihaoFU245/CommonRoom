//! Administrative metadata owns its fence, transaction and committed publication.
use super::commands::Input;
use super::*;
use crate::engine::{EngineError, sudo_command};

pub(super) async fn execute(
    app: &App,
    headers: axum::http::HeaderMap,
    user: String,
    input: Input,
) -> Result<(String, Option<Change>), EngineError> {
    let account = app
        .inspect_engine({
            let user = user.clone();
            let headers = headers.clone();
            move |engine, config| -> Result<_, EngineError> {
                if auth::identity(engine, config, &headers).as_deref() != Some(&user) {
                    return Err("Please log in.".into());
                }
                Ok(engine
                    .active_account_id(&user)
                    .ok_or("Account unavailable.")?
                    .to_owned())
            }
        })
        .await?;
    let _account = app
        .admission
        .http(account.clone())
        .map_err(EngineError::Unavailable)?;
    let identity = auth::Identity {
        headers,
        user,
        account,
        config: app.config.clone(),
    };
    let ordering = app.mutations.metadata().await?;
    let (prepared, history) = app
        .with_metadata(&ordering, {
            let identity = identity.clone();
            let input = input.clone();
            move |engine| -> Result<_, EngineError> {
                identity.verify(engine)?;
                let elevated = sudo_command(&input.text)?;
                let prepared = engine.with_command_authority(
                    Some(&identity.user),
                    input.room.as_deref(),
                    elevated.is_some(),
                    |engine| {
                        engine.prepare_metadata(
                            Some(&identity.user),
                            input.room.as_deref(),
                            elevated.unwrap_or(&input.text),
                        )
                    },
                )?;
                Ok((prepared, engine.history_store().clone()))
            }
        })
        .await?;
    let (reply, pending) = match prepared {
        crate::engine::MetadataPreparation::Ready(reply, pending) => {
            (reply, pending.map(|pending| *pending))
        }
        crate::engine::MetadataPreparation::Rename(read) => {
            let references = read.read().await?;
            app.with_metadata(&ordering, {
                let identity = identity.clone();
                move |engine| -> Result<_, EngineError> {
                    identity.verify(engine)?;
                    let elevated = sudo_command(&input.text)?;
                    let text = elevated.unwrap_or(&input.text);
                    let (reply, pending) = engine.with_command_authority(
                        Some(&identity.user),
                        input.room.as_deref(),
                        elevated.is_some(),
                        |engine| {
                            engine.require_command(
                                Some(&identity.user),
                                input.room.as_deref(),
                                text.split_whitespace().next().unwrap_or(""),
                            )?;
                            engine.prepare_referenced_rename(Some(&identity.user), references)
                        },
                    )?;
                    Ok((reply, Some(pending)))
                }
            })
            .await?
        }
    };
    let Some(pending) = pending else {
        history.ensure_available_async().await?;
        return Ok((reply, None));
    };
    let pending = pending.save().await?;
    let changes = app.changes.clone();
    app.with_metadata(&ordering, move |engine| -> Result<_, EngineError> {
        // Verify against the old committed state: the command may revoke its own
        // credential or authority as part of publication.
        let delivery = identity.verify(engine);
        engine.publish_metadata(pending)?;
        let _ = changes.send(Change::Metadata(engine.metadata_change()));
        delivery?;
        Ok((reply, None))
    })
    .await
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "identity_tests.rs"]
mod identity_tests;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "metadata_tests.rs"]
mod tests;
