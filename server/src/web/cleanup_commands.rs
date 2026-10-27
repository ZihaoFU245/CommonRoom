//! Async history cleanup with stable authority and an atomic deletion/audit batch.
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
    let (identity, plan) = super::commands::capture(app, headers, user.clone(), &input, {
        let view = input.room.clone();
        move |engine, text| engine.prepare_browser_cleanup(&user, view.as_deref(), text)
    })
    .await?;
    let _account = app
        .admission
        .http(identity.account.clone())
        .map_err(EngineError::Unavailable)?;
    let ids = plan.ids();
    let ordering = app.mutations.conversations(ids.clone()).await?;
    let final_input = input.clone();
    let mut plan = app
        .with_conversation(&ordering, {
            let identity = identity.clone();
            move |engine| -> Result<_, EngineError> {
                super::commands::phase(engine, &identity, &input, |engine, text| {
                    let plan = engine.prepare_browser_cleanup(
                        &identity.user,
                        input.room.as_deref(),
                        text,
                    )?;
                    if plan.ids() != ids {
                        return Err(EngineError::Conflict(
                            "Cleanup targets changed; retry the operation.".into(),
                        ));
                    }
                    Ok(plan)
                })
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
                super::commands::phase(engine, &identity, &final_input, |engine, _| {
                    engine.prepare_history_cleanup(&identity.user, &plan)
                })
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
