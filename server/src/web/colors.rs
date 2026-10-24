use super::media::authenticated;
use super::*;
use axum::{extract::State, http::HeaderMap};
use serde::Deserialize;

/// Browser-local opt-in; this controls projection only, never authorization.
pub(crate) fn enabled(headers: &HeaderMap) -> bool {
    headers
        .get("x-chat-name-colors")
        .is_some_and(|value| value == "1")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetColor {
    color: Option<String>,
    /// Stable room ID for a room override; omitted or null selects `@global`.
    #[serde(default)]
    room: Option<String>,
}
pub(crate) async fn set(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<SetColor>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_origin(&app, &headers)?;
    authenticated(&app, &headers, move |app, user| {
        let mut engine = app.engine_api()?;
        let account = engine
            .active_account_id(user)
            .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Account unavailable."))?
            .to_owned();
        let (room, color) = (input.room.as_deref(), input.color.as_deref());
        if engine
            .validate_color(user, room, color)
            .map_err(ApiError::from)?
        {
            app.admission.profile_change(&account)?;
        }
        if engine
            .set_color(user, room, color)
            .map_err(ApiError::from)?
        {
            let _ = app.changes.send(Change::Metadata(engine.metadata_change()));
        }
        Ok(Json(json!({"ok":true})))
    })
    .await
}
