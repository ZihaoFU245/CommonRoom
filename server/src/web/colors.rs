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
pub(crate) async fn set_as(
    State(app): State<App>,
    access: super::media::Access,
    Json(input): Json<SetColor>,
) -> Result<Json<serde_json::Value>, ApiError> {
    super::profiles::update(
        &app,
        access,
        super::profiles::Update::Color {
            room: input.room,
            value: input.color,
        },
    )
    .await?;
    Ok(Json(json!({"ok":true})))
}

pub(crate) async fn set(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<SetColor>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_origin(&app, &headers)?;
    set_as(
        State(app),
        super::media::Access::Browser(headers),
        Json(input),
    )
    .await
}
