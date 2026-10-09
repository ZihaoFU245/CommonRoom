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
    let username = authenticate(&app, &headers)?;
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
    let value = app
        .engine()
        .ok()
        .and_then(|engine| engine.snapshot(user))
        .and_then(|snapshot| serde_json::to_string(&snapshot).ok());
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
impl Drop for Presence {
    fn drop(&mut self) {
        if let Ok(mut engine) = self.app.engine() {
            engine.disconnect(&self.user);
        }
        let _ = self.app.changes.send(Change::All);
    }
}
pub(super) async fn connection(app: App, headers: HeaderMap, user: String, mut socket: WebSocket) {
    let mut changes = app.changes.subscribe();
    match app.engine() {
        Ok(mut engine) => engine.connect(&user),
        Err(_) => return,
    }
    let _presence = Presence {
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
                if authenticate(&app, &headers).is_err() || last_seen.elapsed() > Duration::from_secs(75) { break; }
                if !send(&mut socket, Message::Ping(Vec::new().into())).await { break; }
            },
            change = changes.recv() => {
                if matches!(&change, Ok(Change::Read(name)) if name != &user) { continue; }
                if matches!(change, Err(broadcast::error::RecvError::Closed)) || authenticate(&app, &headers).is_err() { break; }
                if matches!(change, Ok(Change::Read(_))) {
                    let unread = match app.engine() { Ok(engine) => engine.unreads(&user), Err(_) => break };
                    if !send(&mut socket, Message::Text(json!({"kind":"read", "unread":unread}).to_string().into())).await { break; }
                } else if !snapshot(&app, &user, &mut socket).await { break; }
            },
            incoming = socket.recv() => {
                let Some(Ok(message)) = incoming else { break; };
                last_seen = Instant::now();
                if authenticate(&app, &headers).is_err() { break; }
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
                        if matches!(result, Ok((_, true))) { let _ = app.changes.send(Change::All); }
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
