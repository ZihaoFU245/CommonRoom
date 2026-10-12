use crate::web::{ApiError, App, Change, auth, commands, error, history, socket};
use axum::{
    Json, Router,
    extract::{Query, State, WebSocketUpgrade},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::json;

pub(super) fn routes() -> Router<App> {
    Router::new()
        .route("/api/bot/me", get(me))
        .route("/api/bot/history", get(get_history))
        .route("/api/bot/read", post(read))
        .route("/api/bot/commands", post(command))
        .route("/api/bot/messages", post(message))
        .route("/api/bot/ws", get(upgrade))
        .layer(axum::extract::DefaultBodyLimit::max(32768))
}

fn require_bearer(headers: &HeaderMap) -> Result<(), ApiError> {
    auth::bearer(headers)
        .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Bot bearer credential required."))
        .map(|_| ())
}
async fn authenticate(app: &App, headers: &HeaderMap) -> Result<String, ApiError> {
    let permit = app
        .io_permit()
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    let app = app.clone();
    let headers = headers.clone();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        auth::authenticate_bot(&app, &headers)
    })
    .await
    .map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Authentication worker unavailable.",
        )
    })?
}

async fn me(
    State(app): State<App>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_bearer(&headers)?;
    auth::me(State(app), headers).await
}
async fn get_history(
    State(app): State<App>,
    headers: HeaderMap,
    query: Query<history::HistoryQuery>,
) -> Result<Json<crate::engine::History>, ApiError> {
    require_bearer(&headers)?;
    history::history(State(app), headers, query).await
}
async fn read(
    State(app): State<App>,
    headers: HeaderMap,
    input: Json<history::Read>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_bearer(&headers)?;
    history::read(State(app), headers, input).await
}
async fn upgrade(
    State(app): State<App>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    require_bearer(&headers)?;
    socket::upgrade(State(app), headers, ws).await
}

fn action_error(message: String) -> ApiError {
    let status = if message.starts_with("Slow down;") {
        StatusCode::TOO_MANY_REQUESTS
    } else if message.contains("unavailable") || message.contains("busy") {
        StatusCode::SERVICE_UNAVAILABLE
    } else if message == "Please log in." || message == "Credential revoked." {
        StatusCode::UNAUTHORIZED
    } else {
        StatusCode::FORBIDDEN
    };
    error(status, &message)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandRequest {
    id: u64,
    view: Option<String>,
    command: String,
}
async fn command(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<CommandRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = authenticate(&app, &headers).await?;
    if !input.command.trim_start().starts_with('/') || input.command.chars().count() > 4100 {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "Expected a slash command of at most 4100 characters.",
        ));
    }
    let frame = commands::Input {
        id: input.id,
        room: input.view,
        text: input.command,
    };
    let (text, change) = commands::execute_input(&app, &headers, &user, &frame)
        .await
        .map_err(action_error)?;
    if let Some(change) = change {
        let _ = app.changes.send(change);
    }
    Ok(Json(json!({"ok":true,"id":input.id,"text":text})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageRequest {
    id: u64,
    view: String,
    text: String,
    reply_to: Option<String>,
}
async fn message(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<MessageRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user = authenticate(&app, &headers).await?;
    if input.text.trim().is_empty() || input.text.chars().count() > 4000 {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "Messages support 1–4000 characters.",
        ));
    }
    if let Some(reply_to) = input.reply_to {
        if reply_to.is_empty() || reply_to.len() > 128 || reply_to.chars().any(char::is_whitespace)
        {
            return Err(error(StatusCode::BAD_REQUEST, "Invalid reply message ID."));
        }
        let frame = commands::Input {
            id: input.id,
            room: Some(input.view),
            text: format!("/reply {reply_to} {}", input.text),
        };
        let (text, change) = commands::execute_input(&app, &headers, &user, &frame)
            .await
            .map_err(action_error)?;
        if let Some(change) = change {
            let _ = app.changes.send(change);
        }
        return Ok(Json(json!({"ok":true,"id":input.id,"text":text})));
    }
    let permit = app
        .io_permit()
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let mut engine = app
            .engine()
            .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
        if auth::identity(&engine, &headers).as_deref() != Some(&user) {
            return Err(error(StatusCode::UNAUTHORIZED, "Credential revoked."));
        }
        engine
            .admit_bot_input(
                auth::bearer(&headers)
                    .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Bot credential required."))?,
            )
            .map_err(action_error)?;
        let message = engine
            .send_literal(&user, &input.view, &input.text)
            .map_err(action_error)?;
        let change = engine
            .changed_conversation
            .clone()
            .map_or(Change::All, Change::Conversation);
        drop(engine);
        let _ = app.changes.send(change);
        Ok(Json(json!({"ok":true,"id":input.id,"message":message})))
    })
    .await
    .map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Message worker unavailable.",
        )
    })?
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[path = "tests.rs"]
mod tests;
