//! Frozen v1 HTTP transport; mutations use stable IDs and typed engine errors.
use super::{
    error::{Code, Error},
    models,
};
use crate::{
    engine::{HistoryRange, authorization::Action},
    web::{App, Change, auth, socket},
};
use axum::{
    Json, Router,
    extract::{Query, State, WebSocketUpgrade},
    http::{HeaderMap, StatusCode},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(super) fn routes() -> Router<App> {
    Router::new()
        .nest(
            "/api/bot/v1",
            Router::new()
                .route("/me", get(me))
                .route("/history", get(history))
                .route("/messages", post(message))
                .route("/read", post(read))
                .route("/reactions", put(react))
                .route("/ws", get(upgrade))
                .route("/schema", get(schema)),
        )
        .layer(axum::extract::DefaultBodyLimit::max(32768))
}

fn authenticate(app: &App, headers: &HeaderMap) -> Result<String, Error> {
    if app.stopping.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(Error::new(Code::Unavailable, "Server stopping."));
    }
    let secret = auth::bearer(headers)
        .ok_or_else(|| Error::new(Code::Unauthorized, "Bot credential required."))?;
    let engine = app.engine_read().map_err(Error::storage)?;
    let user = engine
        .bot_identity(secret)
        .ok_or_else(|| Error::new(Code::Unauthorized, "Invalid bot credential."))?;
    engine
        .data
        .users
        .get(&user)
        .map(|a| a.id.clone())
        .ok_or_else(|| Error::new(Code::Unauthorized, "Account unavailable."))
}
fn recheck(
    engine: &crate::engine::Engine,
    headers: &HeaderMap,
    account: &str,
) -> Result<String, Error> {
    let user = auth::identity(engine, headers)
        .filter(|name| engine.data.users.get(name).is_some_and(|a| a.id == account))
        .ok_or_else(|| Error::new(Code::Unauthorized, "Credential changed or revoked."))?;
    Ok(user)
}
async fn work<T: Serialize + Send + 'static>(
    app: App,
    headers: HeaderMap,
    job: impl FnOnce(&App, &HeaderMap, &str) -> Result<T, Error> + Send + 'static,
) -> Result<Json<T>, Error> {
    let permit = app.io_permit().map_err(Error::storage)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let user = authenticate(&app, &headers)?;
        job(&app, &headers, &user).map(Json)
    })
    .await
    .map_err(Error::storage)?
}
async fn me(State(app): State<App>, headers: HeaderMap) -> Result<Json<models::Snapshot>, Error> {
    work(app, headers, |app, headers, user| {
        let engine = app.engine_read().map_err(Error::storage)?;
        let user = recheck(&engine, headers, user)?;
        engine.bot_snapshot(&user)
    })
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryQuery {
    conversation_id: String,
    start: Option<u64>,
    end: Option<u64>,
    before: Option<u64>,
    after: Option<u64>,
    around: Option<u64>,
    limit: Option<usize>,
}
async fn history(
    State(app): State<App>,
    headers: HeaderMap,
    Query(input): Query<HistoryQuery>,
) -> Result<Json<Value>, Error> {
    work(app, headers, move |app, headers, user| {
        if input.limit.is_some_and(|limit| limit > 100) { return Err(Error::new(Code::InvalidRequest,"History limit is at most 100.")); }
        let range = HistoryRange { start: input.start, end: input.end, before: input.before, after: input.after, around: input.around, limit: input.limit };
        range.validate().map_err(|e| Error::new(Code::InvalidRequest, e))?;
        let (revision, store) = {
            let engine = app.engine_read().map_err(Error::storage)?;
            let user = recheck(&engine, headers, user)?;
            (engine.bot_target(&user, &input.conversation_id, Action::Read)?.revision, engine.history_store.clone())
        };
        let page = store.page_checked(&input.conversation_id, revision, range, "");
        let engine = app.engine_read().map_err(Error::storage)?;
        let user = recheck(&engine, headers, user)?;
        if engine.bot_target(&user, &input.conversation_id, Action::Read)?.revision != revision {
            return Err(Error::new(Code::Conflict, "History changed; retry the page."));
        }
        let page = page.map_err(|error| match error {
            crate::engine::HistoryError::Changed => Error::new(Code::Conflict, "History changed; retry the page."),
            crate::engine::HistoryError::InvalidRange(error) => Error::new(Code::InvalidRequest, error),
            crate::engine::HistoryError::Unavailable(error) => Error::storage(error),
        })?;
        Ok(json!({"conversation_id":input.conversation_id,"messages":page.messages.iter().map(models::message_json).collect::<Vec<_>>(),"revision":page.revision,"has_before":page.has_before,"has_after":page.has_after}))
    }).await
}
fn admit(
    engine: &mut crate::engine::Engine,
    headers: &HeaderMap,
    account: &str,
) -> Result<String, Error> {
    let user = recheck(engine, headers, account)?;
    engine
        .admit_bot_input(
            auth::bearer(headers)
                .ok_or_else(|| Error::new(Code::Unauthorized, "Bot credential required."))?,
        )
        .map_err(|e| Error::new(Code::RateLimited, e))?;
    Ok(user)
}
async fn message(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<models::MessageInput>,
) -> Result<Json<Value>, Error> {
    let key = idempotency_key(&headers)?;
    work(app, headers, move |app, headers, user| {
        let mut engine = app.engine().map_err(Error::storage)?;
        let user = admit(&mut engine, headers, user)?;
        let sent = engine.send_bot_message(&user, &input, Some(&key))?;
        if !sent.replayed {
            let _ = app
                .changes
                .send(Change::Conversation(input.conversation_id));
        }
        Ok(json!({"message":models::message_json(&sent.message),"replayed":sent.replayed}))
    })
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    conversation_id: String,
    through: u64,
}
async fn read(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<Read>,
) -> Result<Json<Value>, Error> {
    work(app, headers, move |app, headers, user| {
        let mut engine = app.engine().map_err(Error::storage)?;
        if input.through > 9_007_199_254_740_991 {
            return Err(Error::new(Code::InvalidRequest, "Invalid sequence."));
        }
        let user = admit(&mut engine, headers, user)?;
        if engine.bot_read(&user, &input.conversation_id, input.through)? {
            let _ = app.changes.send(Change::Read(user));
        }
        Ok(json!({"ok":true}))
    })
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reaction {
    conversation_id: String,
    message_id: String,
    reaction: String,
    active: bool,
}
async fn react(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<Reaction>,
) -> Result<Json<Value>, Error> {
    if input.reaction.trim() != input.reaction
        || input.reaction.is_empty()
        || input.reaction.chars().count() > 128
        || input.reaction.chars().any(char::is_control)
    {
        return Err(Error::new(
            Code::InvalidRequest,
            "Reactions support 1–128 characters without controls.",
        ));
    }
    work(app, headers, move |app, headers, user| {
        let mut engine = app.engine().map_err(Error::storage)?;
        let user = admit(&mut engine, headers, user)?;
        if engine.bot_react(
            &user,
            &input.conversation_id,
            &input.message_id,
            &input.reaction,
            input.active,
        )? {
            let _ = app
                .changes
                .send(Change::Conversation(input.conversation_id));
        }
        Ok(json!({"ok":true}))
    })
    .await
}
async fn upgrade(
    State(app): State<App>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, Error> {
    if auth::bearer(&headers).is_none() {
        return Err(Error::new(Code::Unauthorized, "Bot credential required."));
    }
    socket::upgrade(State(app), headers, ws)
        .await
        .map_err(|(status, _)| {
            Error::new(
                if status == StatusCode::UNAUTHORIZED {
                    Code::Unauthorized
                } else {
                    Code::Unavailable
                },
                "Socket unavailable.",
            )
        })
}
fn idempotency_key(headers: &HeaderMap) -> Result<String, Error> {
    let mut values = headers.get_all("idempotency-key").iter();
    let key = values.next().and_then(|v| v.to_str().ok()).filter(|key| {
        !key.is_empty()
            && key.len() <= 128
            && key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
    });
    match key {
        Some(key) if values.next().is_none() => Ok(key.into()),
        _ => Err(Error::new(
            Code::InvalidRequest,
            "One Idempotency-Key of 1–128 ASCII letters, digits, dots, underscores, colons or hyphens is required.",
        )),
    }
}
async fn schema(State(app): State<App>, headers: HeaderMap) -> Result<Json<Value>, Error> {
    work(app, headers, |_, _, _| {
        serde_json::from_str(include_str!("../../../docs/bot-v1.schema.json"))
            .map_err(Error::storage)
    })
    .await
}

// Catch framework and ingress errors without exposing parser text or depending
// on human-readable strings. Domain responses already carry their typed code.
pub(crate) async fn contract(
    State(app): State<App>,
    request: axum::extract::Request,
    next: middleware::Next,
) -> Response {
    let base = format!("{}api/bot/v1", app.config.base_url);
    let versioned = request.uri().path() == base || request.uri().path().starts_with(&(base + "/"));
    let mut response = next.run(request).await;
    if !versioned {
        return response;
    }
    if response.status().is_client_error() || response.status().is_server_error() {
        let (parts, body) = response.into_parts();
        let bytes = axum::body::to_bytes(body, 65536).await.unwrap_or_default();
        let typed =
            serde_json::from_slice::<Value>(&bytes).is_ok_and(|v| v["error"]["code"].is_string());
        response = if typed {
            Response::from_parts(parts, axum::body::Body::from(bytes))
        } else {
            let code = match parts.status {
                StatusCode::UNAUTHORIZED => Code::Unauthorized,
                StatusCode::FORBIDDEN => Code::Forbidden,
                StatusCode::NOT_FOUND => Code::NotFound,
                StatusCode::METHOD_NOT_ALLOWED => Code::MethodNotAllowed,
                StatusCode::PAYLOAD_TOO_LARGE => Code::PayloadTooLarge,
                StatusCode::BAD_REQUEST
                | StatusCode::UNSUPPORTED_MEDIA_TYPE
                | StatusCode::UNPROCESSABLE_ENTITY => Code::InvalidRequest,
                _ => Code::Unavailable,
            };
            let mut result = Error::new(
                code,
                parts.status.canonical_reason().unwrap_or("Request failed."),
            )
            .into_response();
            // Keep ingress security headers and the framework's precise 400/422 status.
            *result.status_mut() = parts.status;
            for (name, value) in &parts.headers {
                if name != axum::http::header::CONTENT_LENGTH
                    && name != axum::http::header::CONTENT_TYPE
                {
                    result.headers_mut().insert(name, value.clone());
                }
            }
            result
        };
    }
    response
        .headers_mut()
        .insert("bot-api-version", axum::http::HeaderValue::from_static("1"));
    response
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[path = "tests.rs"]
mod tests;
