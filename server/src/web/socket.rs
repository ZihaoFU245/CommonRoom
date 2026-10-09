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
        .engine
        .lock()
        .unwrap()
        .snapshot(user)
        .map(|s| serde_json::to_string(&s).unwrap());
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
pub(super) async fn connection(app: App, headers: HeaderMap, user: String, mut socket: WebSocket) {
    let mut changes = app.changes.subscribe();
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
                    let unread = app.engine.lock().unwrap().unreads(&user);
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
