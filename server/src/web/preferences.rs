use super::*;
use axum::{extract::State, http::HeaderMap};
use std::collections::BTreeMap;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Input {
    values: BTreeMap<String, Option<String>>,
    #[serde(default)]
    initialize: bool,
}

pub(crate) async fn set(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<Input>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_origin(&app, &headers)?;
    let permit = app
        .io_permit()
        .map_err(|cause| error(StatusCode::SERVICE_UNAVAILABLE, &cause))?;
    admission::spawn_io(permit, async move {
        let (identity, store) = media::native_context(&app, media::Access::Browser(headers)).await?;
        let ordering = app.mutations.metadata().await;
        let pending = app.with_metadata(&ordering, {
            let identity = identity.clone();
            move |engine| -> Result<_, ApiError> {
                media::verify_identity(&identity, engine)?;
                Ok(engine.prepare_preferences(&identity.user, input.values, input.initialize)?)
            }
        }).await?;
        if let Some(pending) = pending {
            let pending = pending.save().await?;
            let identity = identity.clone();
            let changes = app.changes.clone();
            app.with_metadata(&ordering, move |engine| -> Result<_, ApiError> {
                engine.publish_metadata(pending)?;
                let _ = changes.send(Change::Metadata(engine.metadata_change()));
                media::verify_identity(&identity, engine)
            }).await?;
        } else { store.health().await?; }
        app.inspect_engine(move |engine, _| -> Result<_, ApiError> {
            media::verify_identity(&identity, engine)?;
            Ok(Json(json!({"values":engine.preferences(&identity.user), "sync": engine.preference_sync()})))
        }).await
    }).await.map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "Preferences worker unavailable."))?
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#[path = "preferences_tests.rs"]
mod tests;
