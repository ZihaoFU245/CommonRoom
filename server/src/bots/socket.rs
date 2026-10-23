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
use std::time::Instant;

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
    let lease = app
        .admission
        .socket(context.account.clone(), crate::web::admission::Profile::Bot)
        .map_err(Error::storage)?;
    Ok(ws
        .max_message_size(crate::web::transport::MAX_FRAME_BYTES)
        .max_frame_size(crate::web::transport::MAX_FRAME_BYTES)
        .on_upgrade(move |socket| async move {
            crate::web::transport::run(&app, lease, connection(app.clone(), context, socket)).await;
        }))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Activity {
    kind: String,
    conversation_id: String,
    state: String,
    draft: Option<String>,
}
/// Normal notification batches authenticate once under one guard. Baselines
/// release it before their owned snapshot performs reader work.
#[cfg(test)]
pub(crate) fn render_changes(
    app: &App,
    context: &Context,
    changes: &[Change],
) -> Result<(String, Vec<Utf8Bytes>), Error> {
    render_with_interests(
        app,
        context,
        changes,
        &mut crate::web::transport::Interests::default(),
    )
}
fn render_with_interests(
    app: &App,
    context: &Context,
    changes: &[Change],
    interests: &mut crate::web::transport::Interests,
) -> Result<(String, Vec<Utf8Bytes>), Error> {
    if app.stopping.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(Error::new(Code::Unavailable, "Server stopping."));
    }
    for _ in 0..3 {
        let engine = app.engine_read().map_err(Error::from)?;
        let user = context.user(&engine)?;
        let credential = context.metadata(&engine)?;
        let baseline = changes.iter().any(|change| {
            matches!(change, Change::All)
                || matches!(change, Change::Metadata(notice) if notice.targets(&context.account))
                || matches!(change, Change::Read(name) | Change::Account(name) if name == &user)
        });
        let stamp = baseline
            .then(|| engine.snapshot_stamp(&user, true))
            .transpose()?;
        let presence = changes
            .contains(&Change::Presence)
            .then(|| engine.presence_revision());
        let refreshed = if interests.refresh_needed(baseline) {
            Some(
                context
                    .snapshot_plan(&engine)?
                    .snapshot
                    .conversations
                    .into_iter()
                    .map(|conversation| conversation.id)
                    .collect(),
            )
        } else {
            None
        };
        let mut frames = Vec::new();
        for change in changes {
            match change {
                Change::Conversation(id) if !baseline => {
                    if credential
                        .operations
                        .as_ref()
                        .is_none_or(|ops| ops.contains("read"))
                        && credential
                            .conversations
                            .as_ref()
                            .is_none_or(|ids| ids.contains(id))
                        && let Ok(target) = engine.bot_target(&user, id, Action::Read)
                    {
                        frames.push(json!({"kind":"conversation","conversation_id":id,"content_revision":target.revision}).to_string().into());
                    }
                }
                Change::Presence if credential.conversations.is_none() => {
                    frames.push(engine.presence_frame(&user).to_string().into())
                }
                _ => {}
            }
        }
        if !baseline {
            interests.bind(&context.account);
            if let Some(ids) = refreshed {
                interests.refresh(ids, engine.metadata_clock());
            }
            return Ok((user, frames));
        }
        drop(engine);
        let snapshot = crate::web::snapshots::bot_value(app, context);
        let engine = app.engine_read().map_err(Error::from)?;
        let current = context.user(&engine)?;
        if current == user
            && engine.snapshot_stamp(&current, true).ok().as_ref() == stamp.as_ref()
            && presence.is_none_or(|revision| revision == engine.presence_revision())
        {
            frames.insert(0, snapshot?);
            interests.bind(&context.account);
            if let Some(ids) = refreshed {
                interests.refresh(ids, engine.metadata_clock());
            }
            return Ok((user, frames));
        }
    }
    Err(Error::new(
        Code::Conflict,
        "Notifications changed; retry shortly.",
    ))
}

async fn value(
    app: &App,
    context: &Context,
    old_user: &str,
    changes: Vec<Change>,
    interests: &mut crate::web::transport::Interests,
) -> Result<(String, Vec<Utf8Bytes>), Error> {
    let app = app.clone();
    let context = context.clone();
    let old_user = old_user.to_owned();
    let permit = app.io_permit().map_err(Error::storage)?;
    let mut routed = std::mem::take(interests);
    let (user, frames, routed) = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let (user, frames) = render_with_interests(&app, &context, &changes, &mut routed)?;
        if user != old_user {
            app.update_presence(Some(&old_user), Some(&user))
                .map_err(Error::storage)?;
        }
        Ok::<_, Error>((user, frames, routed))
    })
    .await
    .map_err(Error::storage)??;
    *interests = routed;
    Ok((user, frames))
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
    let mut events = crate::web::transport::Events::new(&app, true);
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
    let mut interests = crate::web::transport::Interests::default();
    let mut presence = Presence {
        app: app.clone(),
        user,
    };
    if !emit(
        &mut socket,
        value(
            &app,
            &context,
            &presence.user,
            vec![Change::All],
            &mut interests,
        )
        .await,
        &mut presence,
    )
    .await
    {
        return;
    }

    let mut budget = crate::web::transport::Budget::new();
    let mut commands = super::discovery::Runtime::default();
    loop {
        while let Some(frame) = commands.next() {
            if !send(&mut socket, Message::Text(frame.to_string().into())).await {
                return;
            }
        }
        match events.next(&mut socket).await {
            crate::web::transport::Event::Maintenance => {}
            crate::web::transport::Event::Heartbeat(alive) => {
                if !alive {
                    break;
                }
                let check_app = app.clone();
                let check_context = context.clone();
                let old_user = presence.user.clone();
                let checked = tokio::task::spawn_blocking(move || {
                    let user =
                        check_context.user(&*check_app.engine_read().map_err(Error::storage)?)?;
                    if user != old_user {
                        check_app
                            .update_presence(Some(&old_user), Some(&user))
                            .map_err(Error::storage)?;
                    }
                    Ok::<_, Error>(user)
                })
                .await;
                let Ok(Ok(user)) = checked else {
                    break;
                };
                presence.user = user;
                if !send(&mut socket, Message::Ping(Vec::new().into())).await {
                    break;
                }
            }
            crate::web::transport::Event::Change(change) => {
                let change = match change {
                    Ok(change) => change,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => Change::All,
                    Err(_) => break,
                };
                let mut changes = events.take_batch(change);
                if changes.contains(&Change::Shutdown) {
                    break;
                }
                changes.retain(|change| !matches!(change, Change::Metadata(notice) if !notice.targets(&context.account)) && !interests.excludes(change) && !matches!(change, Change::BotCommands(_) | Change::Avatar(_))
                    && !matches!(change, Change::Read(name) | Change::Account(name) if name != &presence.user));
                if changes.is_empty() {
                    continue;
                }
                if changes
                    .iter()
                    .any(|change| matches!(change, Change::All | Change::Metadata(_)))
                    && commands.active()
                    && commands.apply(&app, &context, None).await.is_err()
                {
                    break;
                }
                if !emit(
                    &mut socket,
                    value(&app, &context, &presence.user, changes, &mut interests).await,
                    &mut presence,
                )
                .await
                {
                    break;
                }
            }
            crate::web::transport::Event::Activity(activity) => {
                let Ok(activity) = activity else {
                    continue;
                };
                let id = activity["conversation_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                if interests.excludes(&Change::Conversation(id.clone())) {
                    continue;
                }
                let check_app = app.clone();
                let check_context = context.clone();
                let allowed = tokio::task::spawn_blocking(move || {
                    let engine = check_app.engine_read().map_err(Error::storage)?;
                    let user = check_context.user(&engine)?;
                    check_context.permits(&engine, "read", Some(&id))?;
                    engine.bot_target(&user, &id, Action::Read).map(|_| ())
                })
                .await;
                if matches!(allowed, Ok(Ok(())))
                    && !send(&mut socket, Message::Text(activity.to_string().into())).await
                {
                    break;
                }
            }
            crate::web::transport::Event::Incoming(incoming) => {
                let Some(Ok(message)) = incoming else {
                    break;
                };
                match message {
                    Message::Text(raw) => {
                        let discovery =
                            serde_json::from_str::<super::discovery::Control>(&raw).ok();
                        let expected = discovery
                            .as_ref()
                            .is_some_and(|control| commands.expected(control));
                        let admission = if expected {
                            Ok(())
                        } else if !budget.admit(Instant::now()) {
                            Err(Error::new(Code::RateLimited, "Control budget exhausted."))
                        } else {
                            let charge_app = app.clone();
                            let charge_context = context.clone();
                            tokio::task::spawn_blocking(move || {
                                let mut engine = charge_app.engine().map_err(Error::storage)?;
                                charge_context.user(&engine)?;
                                engine
                                    .admit_bot_account(&charge_context.account)
                                    .map_err(|message| Error::new(Code::RateLimited, message))
                            })
                            .await
                            .map_err(Error::storage)
                            .and_then(|result| result)
                        };
                        let result = if let Some(control) = discovery {
                            let id = match &control {
                                super::discovery::Control::Response { id, .. } => Some(id.clone()),
                                _ => None,
                            };
                            let result = if let Err(error) = admission {
                                Err(error)
                            } else {
                                commands
                                    .apply(&app, &context, Some(control))
                                    .await
                                    .map(|_| ())
                            };
                            (id, result)
                        } else if let Ok(activity) = serde_json::from_str::<Activity>(&raw) {
                            let activity_app = app.clone();
                            let activity_context = context.clone();
                            let result = tokio::task::spawn_blocking(move || {
                                admission?;
                                if activity.kind != "activity" || !["working", "idle"].contains(&activity.state.as_str()) { return Err(Error::new(Code::InvalidRequest, "Invalid activity.")); }
                                if activity.draft.as_ref().is_some_and(|text| text.len() > 16000 || text.chars().count() > crate::limits::MAX_MESSAGE_CHARS) { return Err(Error::new(Code::InvalidRequest, "Draft exceeds its limit.")); }
                                let engine = activity_app.engine_read().map_err(Error::storage)?;
                                let user = activity_context.user(&engine)?;
                                activity_context.permits(&engine, "activity", Some(&activity.conversation_id))?;
                                engine.bot_target(&user, &activity.conversation_id, Action::Send)?;
                                let mut frame = json!({"kind":"activity","conversation_id":activity.conversation_id,"account_id":activity_context.account,"state":activity.state,"expires":super::now()+30});
                                if let Some(draft) = activity.draft { frame["draft"] = json!(draft); }
                                let _ = activity_app.bot_runtimes.activities.send(frame);
                                Ok(())
                            }).await.map_err(Error::storage).and_then(|r| r);
                            (None, result)
                        } else {
                            (
                                None,
                                Err(Error::new(
                                    Code::InvalidRequest,
                                    "Only command discovery and activity frames are accepted; send chat through HTTP.",
                                )),
                            )
                        };
                        let frame = match result {
                            (id, Ok(())) => json!({"kind":"config_ack","id":id}),
                            (id, Err(e)) => json!({"kind":"error","id":id,"error":e}),
                        };
                        if !send(&mut socket, Message::Text(frame.to_string().into())).await {
                            break;
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        }
    }
    let _ = send(&mut socket, Message::Close(None)).await;
}
async fn emit(
    socket: &mut WebSocket,
    result: Result<(String, Vec<Utf8Bytes>), Error>,
    presence: &mut Presence,
) -> bool {
    match result {
        Ok((user, frames)) => {
            presence.user = user;
            for frame in frames {
                if !send(socket, Message::Text(frame)).await {
                    return false;
                }
            }
            true
        }
        Err(error) => {
            let _ = send(
                socket,
                Message::Text(
                    json!({"kind":"error","id":null,"error":error})
                        .to_string()
                        .into(),
                ),
            )
            .await;
            false
        }
    }
}
