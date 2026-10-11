use super::commands::{Input, execute_input};
use super::*;
use super::{auth::authenticate, security::check_origin};
use axum::{
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::HeaderMap,
    response::Response,
};
use std::time::Duration;

pub(super) async fn upgrade(
    State(app): State<App>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    check_origin(&app, &headers)?;
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
pub(super) async fn snapshot(app: &App, user: &str, socket: &mut WebSocket) -> bool {
    let app = app.clone();
    let user = user.to_owned();
    let value = tokio::task::spawn_blocking(move || {
        let snapshot = app.engine_read().ok()?.snapshot(&user)?;
        serde_json::to_string(&snapshot).ok()
    })
    .await
    .ok()
    .flatten();
    match value {
        Some(value) => send(socket, Message::Text(value.into())).await,
        None => false,
    }
}
pub(super) async fn send(socket: &mut WebSocket, message: Message) -> bool {
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
                let mut engine = app
                    .engine()
                    .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
                engine.disconnect(&old);
                engine.connect(&current);
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
            if let Ok(mut engine) = app.engine() {
                engine.disconnect(&user);
            }
            let _ = app.changes.send(Change::All);
        });
    }
}
pub(super) async fn connection(
    app: App,
    headers: HeaderMap,
    mut user: String,
    mut socket: WebSocket,
) {
    let mut changes = app.changes.subscribe();
    let presence_app = app.clone();
    let presence_user = user.clone();
    if !matches!(
        tokio::task::spawn_blocking(move || {
            presence_app.engine()?.connect(&presence_user);
            Ok::<_, String>(())
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
    let _ = app.changes.send(Change::All);
    if !snapshot(&app, &user, &mut socket).await {
        return;
    }
    let mut heartbeat = tokio::time::interval(Duration::from_secs(25));
    let mut budget = (Instant::now(), 0u32);
    let mut last_seen = Instant::now();
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if presence.authenticate(&headers).await.is_err() { break; }
                if last_seen.elapsed() > Duration::from_secs(75) { break; }
                if !send(&mut socket, Message::Ping(Vec::new().into())).await { break; }
            },
            change = changes.recv() => {
                user = match presence.authenticate(&headers).await { Ok(user) => user, Err(_) => break };
                if matches!(&change, Ok(Change::Read(name)) if name != &user) { continue; }
                if matches!(change, Err(broadcast::error::RecvError::Closed)) { break; }
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
                } else if !snapshot(&app, &user, &mut socket).await { break; }
            },
            incoming = socket.recv() => {
                let Some(Ok(message)) = incoming else { break; };
                last_seen = Instant::now();
                user = match presence.authenticate(&headers).await { Ok(user) => user, Err(_) => break };
                match message {
                    Message::Text(raw) => {
                        if budget.0.elapsed() > Duration::from_secs(10) { budget = (Instant::now(), 0); }
                        budget.1 += 1;
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
}
