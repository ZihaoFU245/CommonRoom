use super::commands::{Input, execute_input};
use super::security::check_origin;
use super::*;
use axum::{
    extract::{
        Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::HeaderMap,
    response::Response,
};
use std::time::Duration;
use std::time::Instant;

#[derive(serde::Deserialize, Default)]
pub(crate) struct Options {
    colors: Option<u8>,
}

pub(crate) async fn upgrade(
    State(app): State<App>,
    Query(options): Query<Options>,
    mut headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    check_origin(&app, &headers)?;
    if options.colors.is_some_and(|value| value > 1) {
        return Err(error(StatusCode::BAD_REQUEST, "Invalid color preference."));
    }
    headers.remove("x-chat-name-colors");
    if options.colors == Some(1) {
        headers.insert(
            "x-chat-name-colors",
            axum::http::HeaderValue::from_static("1"),
        );
    }
    let identity = super::auth::authenticated_identity(&app, &headers).await?;
    let username = identity.user;
    let account = identity.account;
    let profile = if super::auth::bearer(&headers).is_some() {
        super::admission::Profile::Bot
    } else {
        super::admission::Profile::Browser
    };
    let lease = app
        .admission
        .socket(account, profile)
        .map_err(|message| error(StatusCode::SERVICE_UNAVAILABLE, &message))?;
    Ok(ws
        .max_message_size(crate::web::transport::MAX_FRAME_BYTES)
        .max_frame_size(crate::web::transport::MAX_FRAME_BYTES)
        .on_upgrade(move |socket| async move {
            crate::web::transport::run(
                &app,
                lease,
                connection(app.clone(), headers, username, socket),
            )
            .await;
        }))
}
pub(crate) async fn snapshot(
    app: &App,
    socket: &mut WebSocket,
    bot: bool,
    headers: &HeaderMap,
) -> bool {
    let value = super::snapshots::value_async(app, headers, bot).await;
    match value {
        Ok(value) => send(socket, Message::Text(value)).await,
        Err(error) => {
            let frame = if bot {
                json!({"kind":"error", "id":null, "error":crate::bots::error::Error::storage(error)})
            } else {
                json!({"kind":"error", "id":0, "text":error.message()})
            };
            let _sent = send(socket, Message::Text(frame.to_string().into())).await;
            let _closed = send(socket, Message::Close(None)).await;
            false
        }
    }
}
pub(crate) async fn send(socket: &mut WebSocket, message: Message) -> bool {
    tokio::time::timeout(Duration::from_secs(5), socket.send(message))
        .await
        .is_ok_and(|r| r.is_ok())
}
#[cfg(test)]
pub(crate) fn avatar_frame(
    app: &App,
    headers: &HeaderMap,
    owner: &str,
) -> Result<Option<axum::extract::ws::Utf8Bytes>, crate::engine::EngineError> {
    super::updates::browser(app, headers, false, &[Change::Avatar(owner.into())])
        .map(|(_, frames)| frames.into_iter().next())
}

// Count sockets rather than accounts so closing one tab keeps other tabs online.
struct Presence {
    app: App,
    user: String,
}
impl Presence {
    async fn authenticate(&mut self, headers: &HeaderMap) -> Result<String, ApiError> {
        let current = super::auth::authenticated_identity(&self.app, headers)
            .await?
            .user;
        self.refresh(current).await
    }
    async fn authenticate_command(&mut self, headers: &HeaderMap) -> Result<String, ApiError> {
        let config = self.app.config.clone();
        let headers = headers.clone();
        let current = self
            .app
            .inspect_engine(move |engine, _| {
                super::auth::Identity::capture(engine, config, headers)
                    .map(|identity| identity.user)
                    .ok_or_else(|| {
                        error(StatusCode::UNAUTHORIZED, "Invalid or expired credential.")
                    })
            })
            .await?;
        // Commands check owner health in their own read/commit path. Heartbeats
        // and transient activity retain independent native health checks.
        self.refresh(current).await
    }
    async fn refresh(&mut self, current: String) -> Result<String, ApiError> {
        if current != self.user {
            let app = self.app.clone();
            let old = self.user.clone();
            let current = current.clone();
            tokio::task::spawn_blocking(move || app.update_presence(Some(&old), Some(&current)))
                .await
                .map_err(|_| {
                    error(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "Presence worker unavailable.",
                    )
                })?
                .map_err(|cause| error(StatusCode::SERVICE_UNAVAILABLE, &cause))?;
        }
        self.user.clone_from(&current);
        Ok(current)
    }
}
impl Drop for Presence {
    fn drop(&mut self) {
        let app = self.app.clone();
        let user = self.user.clone();
        let _job = tokio::task::spawn_blocking(move || {
            if let Err(error) = app.update_presence(Some(&user), None) {
                tracing::error!(%error, "Presence cleanup failed");
            }
        });
    }
}

async fn authorize_activity(
    app: &App,
    headers: &HeaderMap,
    old: String,
    id: String,
) -> Result<(String, bool), ApiError> {
    let config = app.config.clone();
    let captured_headers = headers.clone();
    let (identity, allowed) = super::principal::authenticate(
        app,
        None,
        move |engine| {
            super::auth::Identity::capture(engine, config, captured_headers)
                .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Invalid or expired credential."))
        },
        move |identity, engine| {
            identity
                .verify(engine)
                .map_err(|_| error(StatusCode::UNAUTHORIZED, "Invalid or expired credential."))?;
            Ok::<_, ApiError>(
                engine
                    .bot_target(
                        &identity.user,
                        &id,
                        crate::engine::authorization::Action::Read,
                    )
                    .is_ok(),
            )
        },
    )
    .await?;
    let current = identity.user;
    if old != current {
        let state = app.clone();
        let current = current.clone();
        tokio::task::spawn_blocking(move || state.update_presence(Some(&old), Some(&current)))
            .await
            .map_err(|error| ApiError::from(crate::engine::EngineError::unavailable(error)))?
            .map_err(|error| ApiError::from(crate::engine::EngineError::Unavailable(error)))?;
    }
    Ok::<_, ApiError>((current, allowed))
}

// Durable content is recovered with a baseline. Discovery and presence are
// independent of that baseline, so retain one coalesced invalidation for each.
#[derive(Default)]
struct Recovery {
    pending: bool,
    catalogue: Option<Change>,
    presence: bool,
}
impl Recovery {
    fn schedule(&mut self, changes: &[Change]) {
        self.pending = true;
        self.presence |= changes.contains(&Change::Presence);
        for change in changes {
            if matches!(change, Change::BotCommands(_) | Change::BotCatalogues(_))
                || matches!(change, Change::Metadata(notice) if notice.has_catalogues())
            {
                self.catalogue = Some(change.clone());
            }
        }
    }
    fn take(&mut self) -> Option<Vec<Change>> {
        if !std::mem::take(&mut self.pending) {
            return None;
        }
        let mut changes = vec![Change::All];
        changes.extend(self.catalogue.take());
        if std::mem::take(&mut self.presence) {
            changes.push(Change::Presence);
        }
        Some(changes)
    }
}
fn retryable(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::CONFLICT | StatusCode::SERVICE_UNAVAILABLE | StatusCode::INTERNAL_SERVER_ERROR
    )
}
pub(crate) async fn connection(
    app: App,
    headers: HeaderMap,
    mut user: String,
    mut socket: WebSocket,
) {
    let bot = super::auth::bearer(&headers).is_some();
    let mut events = crate::web::transport::Events::new(&app, true);
    let mut recovery = Recovery::default();
    let presence_app = app.clone();
    let presence_user = user.clone();
    if !matches!(
        tokio::task::spawn_blocking(move || {
            presence_app.update_presence(None, Some(&presence_user))
        })
        .await,
        Ok(Ok(()))
    ) {
        return;
    }
    let mut presence = Presence {
        app: app.clone(),
        user: user.clone(),
    };
    if presence.authenticate(&headers).await.is_err() {
        return;
    }
    if !snapshot(&app, &mut socket, bot, &headers).await {
        return;
    }
    let mut budget = crate::web::transport::Budget::new();
    let mut interests = crate::web::transport::Interests::default();

    loop {
        let event = events.next(&mut socket).await;
        match event {
            crate::web::transport::Event::Maintenance | crate::web::transport::Event::Change(_) => {
                let mut changes = match event {
                    crate::web::transport::Event::Maintenance => {
                        let Some(changes) = recovery.take() else {
                            continue;
                        };
                        changes
                    }
                    crate::web::transport::Event::Change(change) => {
                        let change = match change {
                            Ok(change) => change,
                            Err(broadcast::error::RecvError::Lagged(_)) => Change::All,
                            Err(_) => break,
                        };
                        events.take_batch(change)
                    }
                    _ => continue,
                };
                if changes.contains(&Change::Shutdown) {
                    break;
                }
                // Full recovery retries even if no further broadcast arrives.
                if recovery.pending {
                    recovery.schedule(&changes);
                    continue;
                }
                changes.retain(|change| {
                    !interests.excludes(change) && !matches!(change, Change::Read(name) | Change::Account(name) if name != &user)
                        && !(bot
                            && matches!(
                                change,
                                Change::Avatar(_) | Change::BotCommands(_) | Change::Presence
                            ))
                });
                if changes.is_empty() {
                    continue;
                }
                let read_app = app.clone();
                let read_headers = headers.clone();
                let old = user.clone();
                let mut routed = std::mem::take(&mut interests);
                let retry_changes = changes.clone();
                let result = tokio::spawn(async move {
                    let (current, frames) = super::updates::browser_with_interests_async(
                        &read_app,
                        &read_headers,
                        bot,
                        &changes,
                        &mut routed,
                    )
                    .await?;
                    if current != old {
                        tokio::task::spawn_blocking({
                            let current = current.clone();
                            move || read_app.update_presence(Some(&old), Some(&current))
                        })
                        .await
                        .map_err(crate::engine::EngineError::unavailable)?
                        .map_err(crate::engine::EngineError::Unavailable)?;
                    }
                    Ok::<_, crate::engine::EngineError>((current, frames, routed))
                })
                .await;
                let (current, frames, routed) = match result {
                    Ok(Ok(value)) => value,
                    Ok(Err(crate::engine::EngineError::Forbidden(_))) => break,
                    Ok(Err(
                        cause @ (crate::engine::EngineError::Conflict(_)
                        | crate::engine::EngineError::Unavailable(_)
                        | crate::engine::EngineError::Internal(_)),
                    )) => {
                        tracing::warn!(code = cause.code(), "Socket update deferred");
                        recovery.schedule(&retry_changes);
                        continue;
                    }
                    Ok(Err(_)) => break,
                    Err(cause) => {
                        tracing::warn!(%cause, "Socket update worker failed");
                        recovery.schedule(&retry_changes);
                        continue;
                    }
                };
                interests = routed;
                user = current;
                presence.user.clone_from(&user);
                let mut alive = true;
                for frame in frames {
                    if !send(&mut socket, Message::Text(frame)).await {
                        alive = false;
                        break;
                    }
                }
                if !alive {
                    break;
                }
            }
            crate::web::transport::Event::Heartbeat(alive) => {
                if !alive {
                    break;
                }
                match presence.authenticate(&headers).await {
                    Ok(current) => user = current,
                    Err(cause) if retryable(cause.0) => {
                        recovery.schedule(&[]);
                        tracing::warn!(status = %cause.0, "Socket heartbeat authentication deferred");
                    }
                    Err(_) => break,
                }
                if !send(&mut socket, Message::Ping(Vec::new().into())).await {
                    break;
                }
            }

            crate::web::transport::Event::Activity(activity) => {
                let Ok(activity) = activity else {
                    continue;
                };
                if bot {
                    continue;
                }
                let old = user.clone();
                let id = activity["conversation_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                if interests.excludes(&Change::Conversation(id.clone())) {
                    continue;
                }
                let validated = authorize_activity(&app, &headers, old, id).await;
                let (current, allowed) = match validated {
                    Ok(value) => value,
                    Err(cause) if retryable(cause.0) => {
                        tracing::warn!(status = %cause.0, "Socket activity skipped");
                        continue;
                    }
                    Err(_) => break,
                };
                user = current;
                presence.user.clone_from(&user);
                if allowed && !send(&mut socket, Message::Text(activity.to_string().into())).await {
                    break;
                }
            }
            crate::web::transport::Event::Incoming(incoming) => {
                let Some(Ok(message)) = incoming else {
                    break;
                };
                if matches!(message, Message::Close(_) | Message::Binary(_)) {
                    break;
                }
                if !matches!(message, Message::Text(_)) {
                    continue;
                }
                user = match presence.authenticate_command(&headers).await {
                    Ok(user) => user,
                    Err(cause) if retryable(cause.0) => {
                        recovery.schedule(&[]);
                        // Do not discard a command whose acknowledgement the
                        // caller is awaiting. Report its normal retryable error.
                        if let Message::Text(raw) = &message {
                            let id = serde_json::from_str::<Input>(raw)
                                .map(|input| input.id)
                                .unwrap_or(0);
                            let reply = json!({"kind":"error","id":id,"text":"Storage unavailable; retry shortly."});
                            if !send(&mut socket, Message::Text(reply.to_string().into())).await {
                                break;
                            }
                        }
                        continue;
                    }
                    Err(_) => break,
                };
                match message {
                    Message::Text(raw) => {
                        let admitted = budget.admit(Instant::now());
                        if bot {
                            let frame = json!({"kind":"error","id":null,"error":crate::bots::error::Error::new(crate::bots::error::Code::InvalidRequest,"Use /api/bot/v1/ws for bot controls.")});
                            if !send(&mut socket, Message::Text(frame.to_string().into())).await {
                                break;
                            }
                            continue;
                        }
                        let parsed = serde_json::from_str::<Input>(&raw);
                        let (id, result) = match parsed {
                            Ok(input) if admitted => {
                                (input.id, execute_input(&app, &headers, &user, &input).await)
                            }
                            Ok(input) => (
                                input.id,
                                Err("Slow down; at most 30 messages per 10 seconds.".into()),
                            ),
                            Err(_) => (0, Err("Invalid message format.".into())),
                        };
                        let reply = match &result {
                            Ok((text, _)) => json!({ "kind": "notice", "id": id, "text": text }),
                            Err(text) => json!({ "kind": "error", "id": id, "text": text }),
                        };
                        // Persistence is already committed. Notify peers even if
                        // the requester's connection can no longer accept its reply.
                        if let Ok((_, Some(change))) = result {
                            let _ = app.changes.send(change);
                        }
                        if !send(&mut socket, Message::Text(reply.to_string().into())).await {
                            break;
                        }
                    }
                    Message::Close(_) => break,
                    Message::Binary(_) => break,
                    _ => {}
                }
            }
        }
    }
    let _ = send(&mut socket, Message::Close(None)).await;
}

#[cfg(test)]
mod tests;
