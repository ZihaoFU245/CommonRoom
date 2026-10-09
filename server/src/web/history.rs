use super::*;
use super::{auth::authenticate, security::check_origin};
use axum::{
    extract::{Query, State},
    http::HeaderMap,
};
use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct HistoryQuery {
    view: String,
}
pub(super) async fn history(
    State(app): State<App>,
    headers: HeaderMap,
    Query(input): Query<HistoryQuery>,
) -> Result<Json<crate::engine::History>, ApiError> {
    let user = authenticate(&app, &headers)?;
    app.engine
        .lock()
        .unwrap()
        .history(&user, &input.view)
        .map(Json)
        .map_err(|e| error(StatusCode::FORBIDDEN, &e))
}
#[derive(Deserialize)]
pub(super) struct Read {
    view: String,
    through: u64,
}
pub(super) async fn read(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<Read>,
) -> Result<Json<serde_json::Value>, ApiError> {
    check_origin(&app, &headers)?;
    let user = authenticate(&app, &headers)?;
    let changed = app
        .engine
        .lock()
        .unwrap()
        .mark_read(&user, &input.view, input.through)
        .map_err(|e| error(StatusCode::BAD_REQUEST, &e))?;
    if changed {
        let _ = app.changes.send(Change::Read(user));
    }
    Ok(Json(json!({"ok":true})))
}
