use super::commands::{Input, execute_input};
use super::*;
use super::{auth::authenticate, security::check_origin_or_bot};
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
    check_origin_or_bot(&app, &headers)?;
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
    let auth_app = app.clone();
    let auth_headers = headers.clone();
    let username = tokio::task::spawn_blocking(move || authenticate(&auth_app, &auth_headers))
        .await
        .map_err(|_| {
            error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Session worker unavailable.",
            )
        })??;
    let account = app
        .engine_read_api()?
        .active_account_id(&username)
        .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Account unavailable."))?
        .to_owned();
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
    let app = app.clone();
    let headers = headers.clone();
    let value = tokio::task::spawn_blocking(move || super::snapshots::value(&app, &headers, bot))
        .await
        .unwrap_or_else(|_| Err("Snapshot worker unavailable.".into()));
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
        let app = self.app.clone();
        let headers = headers.clone();
        let old = self.user.clone();
        let current = tokio::task::spawn_blocking(move || {
            let current = authenticate(&app, &headers)?;
            if current != old {
                app.update_presence(Some(&old), Some(&current))
                    .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
            }
            Ok::<_, ApiError>(current)
        })
        .await
        .map_err(|_| {
            error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Session worker unavailable.",
            )
        })??;
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
pub(crate) async fn connection(
    app: App,
    headers: HeaderMap,
    mut user: String,
    mut socket: WebSocket,
) {
    let bot = super::auth::bearer(&headers).is_some();
    let mut events = crate::web::transport::Events::new(&app, false);
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
        match events.next(&mut socket).await {
            crate::web::transport::Event::Maintenance => {}
            crate::web::transport::Event::Heartbeat(alive) => {
                if !alive {
                    break;
                }
                if presence.authenticate(&headers).await.is_err() {
                    break;
                }
                if !send(&mut socket, Message::Ping(Vec::new().into())).await {
                    break;
                }
            }
            crate::web::transport::Event::Change(change) => {
                let change = match change {
                    Ok(change) => change,
                    Err(broadcast::error::RecvError::Lagged(_)) => Change::All,
                    Err(_) => break,
                };
                let mut changes = events.take_batch(change);
                if changes.contains(&Change::Shutdown) {
                    break;
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
                let result = tokio::task::spawn_blocking(move || {
                    let (current, frames) = super::updates::browser_with_interests(
                        &read_app,
                        &read_headers,
                        bot,
                        &changes,
                        &mut routed,
                    )?;
                    if current != old {
                        read_app
                            .update_presence(Some(&old), Some(&current))
                            .map_err(crate::engine::EngineError::Unavailable)?;
                    }
                    Ok::<_, crate::engine::EngineError>((current, frames, routed))
                })
                .await;
                let Ok(Ok((current, frames, routed))) = result else {
                    break;
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
            crate::web::transport::Event::Activity(activity) => {
                let Ok(activity) = activity else {
                    continue;
                };
                if bot {
                    continue;
                }
                let read_app = app.clone();
                let read_headers = headers.clone();
                let old = user.clone();
                let id = activity["conversation_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                if interests.excludes(&Change::Conversation(id.clone())) {
                    continue;
                }
                let allowed = tokio::task::spawn_blocking(move || {
                    let engine = read_app.engine_read()?;
                    let current = super::auth::identity(&engine, &read_app.config, &read_headers)
                        .ok_or("Credential revoked.")?;
                    let allowed = engine
                        .bot_target(&current, &id, crate::engine::authorization::Action::Read)
                        .is_ok();
                    drop(engine);
                    if old != current {
                        read_app
                            .update_presence(Some(&old), Some(&current))
                            .map_err(crate::engine::EngineError::Unavailable)?;
                    }
                    Ok::<_, crate::engine::EngineError>((current, allowed))
                })
                .await;
                let Ok(Ok((current, allowed))) = allowed else {
                    break;
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
                user = match presence.authenticate(&headers).await {
                    Ok(user) => user,
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
