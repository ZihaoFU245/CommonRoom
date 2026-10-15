use super::*;
use axum::{
    extract::{Query, State},
    http::HeaderMap,
};
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct QueryInput {
    conversation_id: String,
}
pub(crate) async fn get(
    State(app): State<App>,
    headers: HeaderMap,
    Query(input): Query<QueryInput>,
) -> Result<Json<serde_json::Value>, ApiError> {
    super::security::check_read_origin_or_bot(&app, &headers)?;
    if super::auth::bearer(&headers).is_some() {
        return Err(error(StatusCode::UNAUTHORIZED, "Browser session required."));
    }
    let permit = app
        .io_permit()
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let engine = app.engine_read_api()?;
        let user = super::auth::identity(&engine, &headers)
            .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Please log in."))?;
        let bots = app
            .bot_runtimes
            .commands
            .visible(&engine, &user, &input.conversation_id)
            .map_err(|e| error(StatusCode::FORBIDDEN, &e))?;
        Ok(Json(
            json!({"conversation_id":input.conversation_id,"bots":bots}),
        ))
    })
    .await
    .map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Command metadata worker unavailable.",
        )
    })?
}
