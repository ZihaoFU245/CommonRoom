//! Async history cleanup with stable authority and an atomic deletion/audit batch.
use super::auth::Identity;
use super::commands::Input;
use super::*;
use crate::engine::EngineError;
use axum::http::HeaderMap;

pub(super) async fn execute(
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
                if auth::identity(engine, config, &headers).as_deref() != Some(&user) {
                    return Err("Please log in.".into());
                }
                let account = engine
                    .active_account_id(&user)
                    .ok_or("Account unavailable.")?
                    .to_owned();
                Ok((
                    account,
                    engine.prepare_browser_cleanup(&user, view.as_deref(), &text)?,
                ))
            }
        })
        .await?;
    let _account = app
        .admission
        .http(account.clone())
        .map_err(EngineError::Unavailable)?;
    let identity = Identity {
        headers,
        user,
        account,
        config: app.config.clone(),
    };
    let ids = plan.ids();
    let ordering = app.mutations.conversations(ids.clone()).await?;
    let mut plan = app
        .with_conversation(&ordering, {
            let identity = identity.clone();
            move |engine| -> Result<_, EngineError> {
                identity.verify(engine)?;
                let plan = engine.prepare_browser_cleanup(
                    &identity.user,
                    input.room.as_deref(),
                    &input.text,
                )?;
                if plan.ids() != ids {
                    return Err(EngineError::Conflict(
                        "Cleanup targets changed; retry the operation.".into(),
                    ));
                }
                Ok(plan)
            }
        })
        .await?;
    plan.count().await?;
    let reply = plan.report();
    // Only audited deletions serialize here. Unrelated native writes do not.
    let _audit = if !plan.preview && plan.has_matches() {
        Some(app.mutations.audit.lock().await)
    } else {
        None
    };
    let pending = app
        .with_conversation(&ordering, {
            let identity = identity.clone();
            move |engine| -> Result<_, EngineError> {
                identity.verify(engine)?;
                engine.prepare_history_cleanup(&identity.user, &plan)
            }
        })
        .await?;
    let Some(pending) = pending else {
        return Ok((reply, None));
    };
    pending.save().await?;
    let changes = app.changes.clone();
    app.with_conversation(&ordering, move |engine| -> Result<_, EngineError> {
        if let Some(change) = Change::command(engine.publish_history(pending)?) {
            let _ = changes.send(change);
        }
        identity.verify(engine)?;
        Ok((reply, None))
    })
    .await
}
