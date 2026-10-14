use super::commands::{Input, execute_input};
use super::*;
use super::{auth::authenticate, security::check_origin_or_bot};
use axum::{
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::HeaderMap,
    response::Response,
};
use std::time::Duration;

pub(crate) async fn upgrade(
    State(app): State<App>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    check_origin_or_bot(&app, &headers)?;
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
    let permit = app
        .connections
        .clone()
        .try_acquire_owned()
        .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "Connection limit reached."))?;
    Ok(ws
        .max_message_size(65536)
        .max_frame_size(65536)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            connection(app, headers, username, socket).await;
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
                json!({"kind":"error", "id":0, "text":error})
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
            Ok(current)
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
    let mut changes = app.changes.subscribe();
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
    let mut heartbeat = tokio::time::interval(Duration::from_secs(25));
    let mut budget = (Instant::now(), 0u32);
    let mut last_seen = Instant::now();
    let (runtime_sender, mut runtime_requests) =
        tokio::sync::mpsc::channel::<crate::bots::runtime::Request>(8);
    let mut runtime = Runtime::default();
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if presence.authenticate(&headers).await.is_err() { break; }
                runtime.pending.retain(|_, request| !request.reply.is_closed());
                if last_seen.elapsed() > Duration::from_secs(75) { break; }
                if !send(&mut socket, Message::Ping(Vec::new().into())).await { break; }
            },
            change = changes.recv() => {
                user = match presence.authenticate(&headers).await { Ok(user) => user, Err(_) => break };
                if matches!(&change, Ok(Change::Read(name)) if name != &user) { continue; }
                if matches!(change, Err(broadcast::error::RecvError::Closed)) { break; }
                if matches!(change, Ok(Change::Presence)) {
                    if bot { continue; }
                    let read_app = app.clone(); let read_user = user.clone();
                    let online = tokio::task::spawn_blocking(move || Ok::<_, String>(read_app.engine_read()?.online(&read_user))).await;
                    match online {
                        Ok(Ok(online)) => { if !send(&mut socket, Message::Text(json!({"kind":"presence","online":online}).to_string().into())).await { break; } },
                        _ => break,
                    }
                    continue;
                }
                if bot {
                    if let Ok(Change::Conversation(id)) = &change {
                        let read_app=app.clone(); let read_headers=headers.clone(); let id=id.clone();
                        let update=tokio::task::spawn_blocking(move || {
                            let engine=read_app.engine_read()?;
                            let current=super::auth::identity(&engine,&read_headers).ok_or("Credential revoked.")?;
                            Ok::<_,String>(engine.bot_target(&current,&id,crate::engine::authorization::Action::Read).is_ok().then(|| json!({"kind":"conversation","conversation_id":id})))
                        }).await;
                        match update { Ok(Ok(Some(value))) => { if !send(&mut socket, Message::Text(value.to_string().into())).await {break;} }, Ok(Ok(None))=>{}, _=>break }
                    } else if !snapshot(&app,&mut socket,true,&headers).await { break; }
                    continue;
                }
                if matches!(change, Ok(Change::Read(_))) {
                    let read_app=app.clone(); let read_user=user.clone();
                    let unread=match tokio::task::spawn_blocking(move ||read_app.engine_read()?.unreads(&read_user)).await {Ok(Ok(unread))=>unread,_=>break};
                    if !send(&mut socket, Message::Text(json!({"kind":"read", "unread":unread}).to_string().into())).await { break; }
                } else if let Ok(Change::Conversation(id))=change {
                    let read_app=app.clone(); let read_user=user.clone();
                    let update=tokio::task::spawn_blocking(move ||read_app.engine_read()?.conversation_update(&read_user,&id)).await;
                    match update {
                        Ok(Ok(Some(value)))=>{if !send(&mut socket,Message::Text(value.to_string().into())).await {break;}},
                        Ok(Ok(None))=>{},
                        _=>break,
                    }
                } else if !snapshot(&app, &mut socket, bot, &headers).await { break; }
            },
            request = runtime_requests.recv() => {
                let Some(request) = request else { break; };
                if request.reply.is_closed() { continue; }
                runtime.pending.retain(|_, request| !request.reply.is_closed());
                if presence.authenticate(&headers).await.is_err() { break; }
                let authorization_app = app.clone();
                let authority = request.authority.clone();
                let command = request.command.clone();
                let authorized = tokio::task::spawn_blocking(move || authority.authorize(&authorization_app, &command)).await.unwrap_or_else(|_| Err("Authorization worker unavailable.".into()));
                if authorized.as_ref() != Ok(&request.target) {
                    let _ = request.reply.send(Err(authorized.err().unwrap_or_else(|| "Bot account changed.".into())));
                    continue;
                }
                if runtime.pending.len() >= 8 {
                    let _ = request.reply.send(Err("Bot configuration runtime busy.".into()));
                    continue;
                }
                let frame = request.command.frame(&request.id).to_string();
                if !send(&mut socket, Message::Text(frame.into())).await { break; }
                runtime.pending.insert(request.id.clone(), request);
            },
            incoming = socket.recv() => {
                let Some(Ok(message)) = incoming else { break; };
                last_seen = Instant::now();
                user = match presence.authenticate(&headers).await { Ok(user) => user, Err(_) => break };
                match message {
                    Message::Text(raw) => {
                        if budget.0.elapsed() > Duration::from_secs(10) { budget = (Instant::now(), 0); }
                        budget.1 += 1;
                        if bot {
                            let (id,result) = runtime.control(&app, &headers, &raw, budget.1, &runtime_sender).await;
                            let frame = match result {
                                Ok(()) => json!({"kind":"config_ack","id":id}),
                                Err(error) => json!({"kind":"error","id":id,"error":error}),
                            };
                            if !send(&mut socket,Message::Text(frame.to_string().into())).await {break;}
                            continue;
                        }
                        let parsed = serde_json::from_str::<Input>(&raw);
                        let (id, result) = match parsed {
                            Ok(input) if budget.1 <= 30 => (input.id, execute_input(&app, &headers, &user, &input).await),
                            Ok(input) => (input.id, Err("Slow down; at most 30 messages per 10 seconds.".into())),
                            Err(_) => (0, Err("Invalid message format.".into())),
                        };
                        let reply = match &result { Ok((text, _)) => json!({ "kind": "notice", "id": id, "text": text }), Err(text) => json!({ "kind": "error", "id": id, "text": text }) };
                        if !send(&mut socket, Message::Text(reply.to_string().into())).await { break; }
                        if let Ok((_,Some(change)))=result { let _=app.changes.send(change); }
                    },
                    Message::Close(_) => break,
                    Message::Binary(_) => break,
                    _ => {},
                }
            }
        }
    }
    let _ = send(&mut socket, Message::Close(None)).await;
    let _job = tokio::task::spawn_blocking(move || drop(runtime));
}

#[derive(Default)]
struct Runtime {
    lease: Option<crate::bots::runtime::Lease>,
    pending: std::collections::BTreeMap<String, crate::bots::runtime::Request>,
}
impl Runtime {
    async fn control(
        &mut self,
        app: &App,
        headers: &HeaderMap,
        raw: &str,
        count: u32,
        sender: &tokio::sync::mpsc::Sender<crate::bots::runtime::Request>,
    ) -> (Option<String>, Result<(), crate::bots::error::Error>) {
        use crate::bots::{
            error::{Code, Error},
            runtime::{Control, validate_response},
        };
        if count > 30 {
            return (
                None,
                Err(Error::new(
                    Code::RateLimited,
                    "At most 30 control frames per 10 seconds.",
                )),
            );
        }
        match serde_json::from_str::<Control>(raw) {
            Ok(Control::Ready) => {
                let result = if self.lease.is_some() {
                    Ok(())
                } else {
                    let app = app.clone();
                    let headers = headers.clone();
                    let sender = sender.clone();
                    match tokio::task::spawn_blocking(move || {
                        let engine = app.engine_read().map_err(Error::storage)?;
                        let user = super::auth::identity(&engine, &headers)
                            .ok_or_else(|| Error::new(Code::Unauthorized, "Credential revoked."))?;
                        let id = engine
                            .data
                            .users
                            .get(&user)
                            .ok_or_else(|| Error::new(Code::Unauthorized, "Account unavailable."))?
                            .id
                            .clone();
                        app.bot_runtimes.register(id, sender)
                    })
                    .await
                    {
                        Ok(Ok(lease)) => {
                            self.lease = Some(lease);
                            Ok(())
                        }
                        Ok(Err(error)) => Err(error),
                        Err(_) => Err(Error::storage("Runtime registration worker unavailable.")),
                    }
                };
                (None, result)
            }
            Ok(Control::Response { id, values, error }) => {
                let result = (|| {
                    let request = self.pending.remove(&id).ok_or_else(|| {
                        Error::new(Code::NotFound, "Unknown or expired configuration request.")
                    })?;
                    if request.reply.is_closed() {
                        return Err(Error::new(Code::NotFound, "Configuration request expired."));
                    }
                    let remote_error = values.is_none()
                        && error
                            .as_ref()
                            .is_some_and(|e| !e.is_empty() && e.len() <= 1024);
                    let response = validate_response(values, error);
                    let invalid = response.is_err() && !remote_error
                        || request.command.key.as_ref().is_some_and(|key| {
                            response.as_ref().is_ok_and(|v| v.keys().any(|k| k != key))
                        });
                    let response = if invalid {
                        Err("Invalid runtime configuration response.".into())
                    } else {
                        response
                    };
                    let _ = request.reply.send(response);
                    if invalid {
                        Err(Error::new(
                            Code::InvalidRequest,
                            "Invalid runtime configuration response.",
                        ))
                    } else {
                        Ok(())
                    }
                })();
                (Some(id), result)
            }
            Err(_) => (
                None,
                Err(Error::new(
                    Code::InvalidRequest,
                    "Only config_ready and config_response frames are accepted; send chat through HTTP.",
                )),
            ),
        }
    }
}
