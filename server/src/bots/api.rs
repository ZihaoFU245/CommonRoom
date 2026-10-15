//! V1 HTTP transport; mutations use stable IDs and typed engine errors.
use super::{
    auth::Context,
    error::{Code, Error},
    models,
};
use crate::{
    engine::{HistoryRange, authorization::Action},
    web::{App, Change},
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
                .route("/credentials", post(credential))
                .route("/conversations", post(conversation))
                .route(
                    "/commands",
                    get(super::control::commands).post(super::control::command),
                )
                .route(
                    "/commands/receipts/{key}",
                    get(super::control::command_receipt),
                )
                .route("/message-context", get(super::control::message_context))
                .route("/history", get(history))
                .route("/messages", post(message))
                .route("/read", post(read))
                .route("/reactions", put(react))
                .route("/ws", get(upgrade))
                .route("/schema", get(schema)),
        )
        .layer(axum::extract::DefaultBodyLimit::max(32768))
}

fn authenticate(app: &App, headers: &HeaderMap) -> Result<Context, Error> {
    Context::authenticate(app, headers)
}
pub(super) async fn work<T: Serialize + Send + 'static>(
    app: App,
    headers: HeaderMap,
    job: impl FnOnce(&App, &Context) -> Result<T, Error> + Send + 'static,
) -> Result<Json<T>, Error> {
    let permit = app.io_permit().map_err(Error::storage)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let context = authenticate(&app, &headers)?;
        job(&app, &context).map(Json)
    })
    .await
    .map_err(Error::storage)?
}
async fn me(State(app): State<App>, headers: HeaderMap) -> Result<Json<models::Snapshot>, Error> {
    work(app, headers, |app, context| {
        let engine = app.engine_read().map_err(Error::storage)?;
        context.user(&engine)?;
        context.snapshot(&engine)
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
    message_id: Option<String>,
    limit: Option<usize>,
    content_revision: Option<u64>,
}
async fn history(
    State(app): State<App>,
    headers: HeaderMap,
    Query(input): Query<HistoryQuery>,
) -> Result<Json<Value>, Error> {
    work(app, headers, move |app, context| {
        if input.limit.is_some_and(|limit| limit > 100) { return Err(Error::new(Code::InvalidRequest,"History limit is at most 100.")); }
        let mut range = HistoryRange { start: input.start, end: input.end, before: input.before, after: input.after, around: input.around, limit: input.limit };
        range.validate().map_err(|e| Error::new(Code::InvalidRequest, e))?;
        let (revision, store) = {
            let engine = app.engine_read().map_err(Error::storage)?;
            let user = context.user(&engine)?;
            context.permits(&engine, "read", Some(&input.conversation_id))?;
            (engine.bot_target(&user, &input.conversation_id, Action::Read)?.revision, engine.history_store.clone())
        };
        if input.content_revision.is_some_and(|expected| expected != revision) { return Err(Error::new(Code::Conflict, "Content revision changed; rescan history.")); }
        if let Some(message_id) = input.message_id {
            if !models::valid_id(&message_id) { return Err(Error::new(Code::InvalidRequest,"Invalid message ID.")); }
            if input.start.is_some() || input.end.is_some() || input.before.is_some() || input.after.is_some() || input.around.is_some() { return Err(Error::new(Code::InvalidRequest, "message_id cannot combine with sequence bounds.")); }
            let message = store.locate(&message_id).map_err(Error::storage)?
                .filter(|(id,_)| id == &input.conversation_id).map(|(_,message)|message)
                .ok_or_else(||Error::new(Code::NotFound,"Message is not retained in this conversation."))?;
            range.around = Some(message.sequence);
        }
        let page = store.page_checked(&input.conversation_id, revision, range, "");
        let engine = app.engine_read().map_err(Error::storage)?;
        let user = context.user(&engine)?;
        context.permits(&engine, "read", Some(&input.conversation_id))?;
        if engine.bot_target(&user, &input.conversation_id, Action::Read)?.revision != revision {
            return Err(Error::new(Code::Conflict, "History changed; retry the page."));
        }
        let page = page.map_err(|error| match error {
            crate::engine::HistoryError::Changed => Error::new(Code::Conflict, "History changed; retry the page."),
            crate::engine::HistoryError::InvalidRange(error) => Error::new(Code::InvalidRequest, error),
            crate::engine::HistoryError::Unavailable(error) => Error::storage(error),
        })?;
        Ok(json!({"conversation_id":input.conversation_id,"messages":page.messages.iter().map(models::message_json).collect::<Vec<_>>(),"content_revision":page.revision,"has_before":page.has_before,"has_after":page.has_after}))
    }).await
}
// Capture authorization and revision, release the engine lock for SQLite reads.
pub(super) fn inspect<T>(
    app: &App,
    context: &Context,
    id: &str,
    read: impl FnOnce(&crate::engine::HistoryStore) -> Result<T, String>,
) -> Result<(u64, T), Error> {
    let (revision, store) = {
        let engine = app.engine_read().map_err(Error::storage)?;
        let user = context.user(&engine)?;
        context.permits(&engine, "read", Some(id))?;
        (
            engine.bot_target(&user, id, Action::Read)?.revision,
            engine.history_store.clone(),
        )
    };
    read(&store)
        .map(|value| (revision, value))
        .map_err(Error::storage)
}
pub(super) fn admit(
    engine: &mut crate::engine::Engine,
    context: &Context,
) -> Result<String, Error> {
    let user = context.user(engine)?;
    engine
        .admit_bot_account(&context.account)
        .map_err(|e| Error::new(Code::RateLimited, e))?;
    Ok(user)
}
async fn message(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<models::MessageInput>,
) -> Result<Json<Value>, Error> {
    let key = idempotency_key(&headers)?;
    work(app, headers, move |app, context| {
        let mut engine = app.engine().map_err(Error::storage)?;
        let user = admit(&mut engine, context)?;
        context.permits(&engine, "send", Some(&input.conversation_id))?;
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
    acknowledge_shared_unread: bool,
}
async fn read(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<Read>,
) -> Result<Json<Value>, Error> {
    work(app, headers, move |app, context| {
        let (revision, retained) = inspect(app, context, &input.conversation_id, |store| {
            store.contains_sequence(&input.conversation_id, input.through)
        })?;
        let mut engine = app.engine().map_err(Error::storage)?;
        if input.through > 9_007_199_254_740_991 {
            return Err(Error::new(Code::InvalidRequest, "Invalid sequence."));
        }
        let user = admit(&mut engine, context)?;
        if !input.acknowledge_shared_unread {
            return Err(Error::new(
                Code::InvalidRequest,
                "Explicit shared unread acknowledgement required.",
            ));
        }
        context.permits(&engine, "read_ack", Some(&input.conversation_id))?;
        if engine
            .bot_target(&user, &input.conversation_id, Action::Read)?
            .revision
            != revision
        {
            return Err(Error::new(
                Code::Conflict,
                "Conversation changed; retry acknowledgement.",
            ));
        }
        if engine.bot_read_checked(&user, &input.conversation_id, input.through, retained)? {
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
    work(app, headers, move |app, context| {
        let (revision, original) = inspect(app, context, &input.conversation_id, |store| {
            store.locate(&input.message_id)
        })?;
        let original = original
            .filter(|(id, _)| id == &input.conversation_id)
            .map(|(_, m)| m)
            .ok_or_else(|| Error::new(Code::NotFound, "Message not found in this conversation."))?;
        let mut engine = app.engine().map_err(Error::storage)?;
        let user = admit(&mut engine, context)?;
        context.permits(&engine, "react", Some(&input.conversation_id))?;
        if engine
            .bot_target(&user, &input.conversation_id, Action::Read)?
            .revision
            != revision
        {
            return Err(Error::new(
                Code::Conflict,
                "Conversation changed; retry reaction.",
            ));
        }
        if engine.bot_react_loaded(
            &user,
            &input.conversation_id,
            original,
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
async fn credential(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<models::CredentialInput>,
) -> Result<Json<Value>, Error> {
    work(app, headers, move |app, context| {
        let mut engine = app.engine().map_err(Error::storage)?;
        admit(&mut engine, context)?;
        engine.issue_bot_credential(context, input)
    })
    .await
}
async fn conversation(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<models::ConversationInput>,
) -> Result<Json<Value>, Error> {
    work(app, headers, move |app, context| {
        let mut engine = app.engine().map_err(Error::storage)?;
        let user = admit(&mut engine, context)?;
        context.permits(&engine, "create", None)?;
        if context.metadata(&engine)?.conversations.is_some() {
            return Err(Error::new(
                Code::Forbidden,
                "Creation requires an account-wide credential.",
            ));
        }
        let id = engine.create_bot_conversation(&user, input)?;
        let _ = app.changes.send(Change::All);
        Ok(json!({"conversation_id":id}))
    })
    .await
}
async fn upgrade(
    State(app): State<App>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, Error> {
    super::socket::upgrade(app, headers, ws).await
}

pub(super) fn idempotency_key(headers: &HeaderMap) -> Result<String, Error> {
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
    work(app, headers, |_, _| {
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
