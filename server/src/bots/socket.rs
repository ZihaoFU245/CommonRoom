//! Bot sockets cache credential identity, but recheck revocation and access live.
use super::{
    auth::Context,
    error::{Code, Error},
};
use crate::{
    engine::authorization::Action,
    web::{App, Change, socket::send},
};
use axum::extract::ws::Utf8Bytes;
use axum::{
    extract::{
        WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::HeaderMap,
    response::Response,
};
use serde::Deserialize;
use serde_json::json;
use std::time::{Duration, Instant};

pub(super) async fn upgrade(
    app: App,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response, Error> {
    let auth_app = app.clone();
    let auth_headers = headers.clone();
    let context =
        tokio::task::spawn_blocking(move || Context::authenticate(&auth_app, &auth_headers))
            .await
            .map_err(Error::storage)??;
    let permit = app
        .connections
        .clone()
        .try_acquire_owned()
        .map_err(Error::storage)?;
    Ok(ws
        .max_message_size(65536)
        .max_frame_size(65536)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            connection(app, context, socket).await;
        }))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Activity {
    kind: String,
    conversation_id: String,
    state: String,
}
async fn value(
    app: &App,
    context: &Context,
    change: Option<Change>,
) -> Result<Option<Utf8Bytes>, Error> {
    let app = app.clone();
    let context = context.clone();
    let permit = app.io_permit().map_err(Error::storage)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        if app.stopping.load(std::sync::atomic::Ordering::SeqCst) { return Err(Error::new(Code::Unavailable, "Server stopping.")); }
        let engine = app.engine_read().map_err(Error::storage)?;
        let user = context.user(&engine)?;
        match change {
            Some(Change::Conversation(id)) => {
                if context.permits(&engine, "read", Some(&id)).is_err() { return Ok(None); }
                Ok(engine.bot_target(&user, &id, Action::Read).ok().map(|t| json!({"kind":"conversation","conversation_id":id,"content_revision":t.revision}).to_string().into()))
            }
            Some(Change::Presence) if context.metadata(&engine)?.conversations.is_some() => Ok(None),
            Some(Change::Presence) => Ok(Some(json!({"kind":"presence","online":engine.online(&user)}).to_string().into())),
            Some(Change::Read(name)) if name != user => Ok(None),
            _ => crate::web::snapshots::bot_value(&app,&engine,&context).map(Some).map_err(Error::storage),
        }
    }).await.map_err(Error::storage)?
}
struct Presence {
    app: App,
    user: String,
}
impl Drop for Presence {
    fn drop(&mut self) {
        let app = self.app.clone();
        let user = self.user.clone();
        let _job = tokio::task::spawn_blocking(move || app.update_presence(Some(&user), None));
    }
}
async fn connection(app: App, context: Context, mut socket: WebSocket) {
    let mut changes = app.changes.subscribe();
    let mut activities = app.bot_runtimes.activities.subscribe();
    let init_app = app.clone();
    let init_context = context.clone();
    let initial = tokio::task::spawn_blocking(move || {
        let user = init_context.user(&*init_app.engine_read().map_err(Error::storage)?)?;
        init_app
            .update_presence(None, Some(&user))
            .map_err(Error::storage)?;
        Ok::<_, Error>(user)
    })
    .await;
    let Ok(Ok(user)) = initial else {
        return;
    };
    let mut presence = Presence {
        app: app.clone(),
        user,
    };
    if !emit(&mut socket, value(&app, &context, None).await).await {
        return;
    }
    let mut heartbeat = tokio::time::interval(Duration::from_secs(25));
    let mut last_seen = Instant::now();
    let mut budget = (Instant::now(), 0u32);
    let mut commands = super::discovery::Runtime::default();
    let mut command_tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        while let Some(frame) = commands.next() {
            if !send(&mut socket, Message::Text(frame.to_string().into())).await {
                return;
            }
        }
        tokio::select! {
            _ = command_tick.tick() => {},
            _ = heartbeat.tick() => {
                if last_seen.elapsed() > Duration::from_secs(75) { break; }
                let check_app = app.clone(); let check_context = context.clone();
                let checked = tokio::task::spawn_blocking(move || {
                    let user = check_context.user(&*check_app.engine_read().map_err(Error::storage)?)?;
                    Ok::<_, Error>(user)
                }).await;
                let Ok(Ok(user)) = checked else { break; };
                if user != presence.user {
                    if app.update_presence(Some(&presence.user), Some(&user)).is_err() { break; }
                    presence.user = user;
                }
                if !send(&mut socket, Message::Ping(Vec::new().into())).await { break; }
            }
            change = changes.recv() => {
                let change = match change { Ok(c) => Some(c), Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => None, Err(_) => break };
                if matches!(change, Some(Change::BotCommands(_))) { continue; }
                if (change.is_none() || matches!(change, Some(Change::All))) && commands.active() && commands.apply(&app, &context, None).await.is_err() { break; }
                if !emit(&mut socket, value(&app, &context, change).await).await { break; }
            }
            activity = activities.recv() => {
                let Ok(activity) = activity else { continue; };
                let id = activity["conversation_id"].as_str().unwrap_or_default().to_owned();
                let check_app = app.clone(); let check_context = context.clone();
                let allowed = tokio::task::spawn_blocking(move || {
                    let engine = check_app.engine_read().map_err(Error::storage)?;
                    let user = check_context.user(&engine)?;
                    check_context.permits(&engine, "read", Some(&id))?;
                    engine.bot_target(&user, &id, Action::Read).map(|_| ())
                }).await;
                if matches!(allowed, Ok(Ok(()))) && !send(&mut socket, Message::Text(activity.to_string().into())).await { break; }
            }
            incoming = socket.recv() => {
                let Some(Ok(message)) = incoming else { break; };
                last_seen = Instant::now();
                match message {
                    Message::Text(raw) => {
                        if budget.0.elapsed() >= Duration::from_secs(10) { budget = (Instant::now(), 0); }
                        let discovery = serde_json::from_str::<super::discovery::Control>(&raw).ok();
                        let expected = discovery.as_ref().is_some_and(|control| commands.expected(control));
                        if !expected { budget.1 += 1; }
                        let result = if let Some(control) = discovery {
                            let id = match &control { super::discovery::Control::Response { id, .. } => Some(id.clone()), _ => None };
                            let result = if budget.1 > 30 && !expected { Err(Error::new(Code::RateLimited, "Control budget exhausted.")) }
                                else { commands.apply(&app, &context, Some(control)).await.map(|_| ()) };
                            (id, result)
                        } else if let Ok(activity) = serde_json::from_str::<Activity>(&raw) {
                            let activity_app = app.clone(); let activity_context = context.clone(); let count = budget.1;
                            let result = tokio::task::spawn_blocking(move || {
                                if count > 30 { return Err(Error::new(Code::RateLimited, "Control budget exhausted.")); }
                                if activity.kind != "activity" || !["working", "idle"].contains(&activity.state.as_str()) { return Err(Error::new(Code::InvalidRequest, "Invalid activity.")); }
                                let engine = activity_app.engine_read().map_err(Error::storage)?;
                                let user = activity_context.user(&engine)?;
                                activity_context.permits(&engine, "activity", Some(&activity.conversation_id))?;
                                engine.bot_target(&user, &activity.conversation_id, Action::Send)?;
                                let _ = activity_app.bot_runtimes.activities.send(json!({"kind":"activity","conversation_id":activity.conversation_id,"account_id":activity_context.account,"state":activity.state,"expires":super::now()+30}));
                                Ok(())
                            }).await.map_err(Error::storage).and_then(|r| r);
                            (None, result)
                        } else {
                            (None, Err(Error::new(Code::InvalidRequest, "Only command discovery and activity frames are accepted; send chat through HTTP.")))
                        };
                        let frame = match result { (id, Ok(())) => json!({"kind":"config_ack","id":id}), (id, Err(e)) => json!({"kind":"error","id":id,"error":e}) };
                        if !send(&mut socket, Message::Text(frame.to_string().into())).await { break; }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        }
    }
    let _ = send(&mut socket, Message::Close(None)).await;
}
async fn emit(socket: &mut WebSocket, result: Result<Option<Utf8Bytes>, Error>) -> bool {
    match result {
        Ok(Some(v)) => send(socket, Message::Text(v)).await,
        Ok(None) => true,
        Err(e) => {
            let _ = send(
                socket,
                Message::Text(
                    json!({"kind":"error","id":null,"error":e})
                        .to_string()
                        .into(),
                ),
            )
            .await;
            false
        }
    }
}
