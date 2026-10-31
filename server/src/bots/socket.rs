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
    let context = Context::authenticate_async(&app, &headers).await?;
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
    crate::engine::database::block_on_storage(render_with_interests_async(
        app,
        context,
        changes,
        &mut crate::web::transport::Interests::default(),
    ))
    .map_err(Error::storage)?
}
struct Batch {
    user: String,
    frames: Vec<Utf8Bytes>,
    baseline: bool,
    stamp: crate::engine::DeliveryStamp,
    ids: std::collections::BTreeSet<String>,
    presence: bool,
    refreshed: Option<std::collections::BTreeSet<String>>,
    history: std::sync::Arc<crate::engine::HistoryStore>,
}
fn capture(
    engine: &crate::engine::Engine,
    context: &Context,
    changes: &[Change],
    interests: &crate::web::transport::Interests,
) -> Result<Batch, Error> {
    let user = context.user(engine)?;
    let credential = context.metadata(engine)?;
    let baseline = changes.iter().any(|change| {
        matches!(change, Change::All)
            || matches!(change, Change::Metadata(notice) if notice.targets(&context.account))
            || matches!(change, Change::Read(name) | Change::Account(name) if name == &user)
    });
    let presence = changes.contains(&Change::Presence) && credential.conversations.is_none();
    let refreshed: Option<std::collections::BTreeSet<String>> =
        if interests.refresh_needed(baseline) {
            Some(
                context
                    .snapshot_plan(engine)?
                    .snapshot
                    .conversations
                    .into_iter()
                    .map(|conversation| conversation.id)
                    .collect(),
            )
        } else {
            None
        };
    let mut ids = std::collections::BTreeSet::new();
    if baseline {
        ids.extend(refreshed.iter().flatten().cloned());
    }
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
                    ids.insert(id.clone());
                    frames.push(json!({"kind":"conversation","conversation_id":id,"content_revision":target.revision}).to_string().into());
                }
            }
            Change::Presence if credential.conversations.is_none() => {
                frames.push(engine.presence_frame(&user).to_string().into())
            }
            _ => {}
        }
    }
    let stamp = engine.delivery_stamp(&user, &ids, presence)?;
    Ok(Batch {
        user,
        frames,
        baseline,
        stamp,
        ids,
        presence,
        refreshed,
        history: engine.history_store().clone(),
    })
}
async fn render_with_interests_async(
    app: &App,
    context: &Context,
    changes: &[Change],
    interests: &mut crate::web::transport::Interests,
) -> Result<(String, Vec<Utf8Bytes>), Error> {
    if app.stopping.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(Error::new(Code::Unavailable, "Server stopping."));
    }
    for _ in 0..3 {
        let Batch {
            user,
            mut frames,
            baseline,
            stamp,
            ids,
            presence,
            refreshed,
            history,
        } = app
            .inspect_engine({
                let context = context.clone();
                let changes = changes.to_vec();
                let interests = interests.clone();
                move |engine, _| capture(engine, &context, &changes, &interests)
            })
            .await?;
        let snapshot = if baseline {
            Some(crate::web::snapshots::bot_value_async(app, context).await)
        } else {
            None
        };
        history.ensure_available_async().await?;
        let validated = app
            .inspect_engine({
                let context = context.clone();
                let user = user.clone();
                move |engine, _| -> Result<_, Error> {
                    let current = context.user(engine)?;
                    Ok((current == user
                        && engine
                            .delivery_stamp(&current, &ids, presence)
                            .ok()
                            .as_ref()
                            == Some(&stamp))
                    .then(|| engine.metadata_clock()))
                }
            })
            .await?;
        if let Some(clock) = validated {
            if let Some(snapshot) = snapshot {
                frames.insert(0, snapshot?);
            }
            interests.bind(&context.account);
            if let Some(ids) = refreshed {
                interests.refresh(ids, clock);
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
    let (user, frames, routed) = tokio::spawn(async move {
        let _permit = permit;
        let (user, frames) =
            render_with_interests_async(&app, &context, &changes, &mut routed).await?;
        if user != old_user {
            tokio::task::spawn_blocking({
                let user = user.clone();
                move || app.update_presence(Some(&old_user), Some(&user))
            })
            .await
            .map_err(Error::storage)?
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
    let Ok(user) = context.verify_async(&app).await else {
        return;
    };
    let initial = tokio::task::spawn_blocking({
        let app = app.clone();
        let user = user.clone();
        move || app.update_presence(None, Some(&user))
    })
    .await;
    if !matches!(initial, Ok(Ok(()))) {
        return;
    }
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
                let Ok(user) = context.verify_async(&app).await else {
                    break;
                };
                if user != presence.user {
                    let state = app.clone();
                    let old = presence.user.clone();
                    let current = user.clone();
                    if !matches!(
                        tokio::task::spawn_blocking(
                            move || state.update_presence(Some(&old), Some(&current))
                        )
                        .await,
                        Ok(Ok(()))
                    ) {
                        break;
                    }
                }
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
                changes.retain(|change| !matches!(change, Change::Metadata(notice) if !notice.targets(&context.account)) && !interests.excludes(change) && !matches!(change, Change::BotCommands(_) | Change::BotCatalogues(_) | Change::Avatar(_))
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
                let allowed = async {
                    context.verify_async(&app).await?;
                    app.inspect_engine({
                        let context = context.clone();
                        move |engine, _| -> Result<_, Error> {
                            let user = context.user(engine)?;
                            context.permits(
                                engine,
                                "read",
                                crate::bots::auth::Scope::Conversation(&id),
                            )?;
                            engine.bot_target(&user, &id, Action::Read).map(|_| ())
                        }
                    })
                    .await
                }
                .await;
                if allowed.is_ok()
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
                            async {
                                context.verify_async(&app).await?;
                                let ordering = app.mutations.metadata().await?;
                                app.with_metadata(&ordering, {
                                    let context = context.clone();
                                    move |engine| -> Result<_, Error> {
                                        context.user(engine)?;
                                        engine.admit_bot_account(&context.account).map_err(
                                            |message| Error::new(Code::RateLimited, message),
                                        )
                                    }
                                })
                                .await
                            }
                            .await
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
                            let result = async {
                                admission?;
                                context.verify_async(&app).await?;
                                app.inspect_engine(move |engine, _| -> Result<_, Error> {
                                    if activity.kind != "activity" || !["working", "idle"].contains(&activity.state.as_str()) { return Err(Error::new(Code::InvalidRequest, "Invalid activity.")); }
                                    if activity.draft.as_ref().is_some_and(|text| text.len() > 16000 || text.chars().count() > crate::limits::MAX_MESSAGE_CHARS) { return Err(Error::new(Code::InvalidRequest, "Draft exceeds its limit.")); }
                                    let user = activity_context.user(engine)?;
                                    activity_context.permits(engine, "activity", crate::bots::auth::Scope::Conversation(&activity.conversation_id))?;
                                    engine.bot_target(&user, &activity.conversation_id, Action::Send)?;
                                    let mut frame = json!({"kind":"activity","conversation_id":activity.conversation_id,"account_id":activity_context.account,"state":activity.state,"expires":super::now()+30});
                                    if let Some(draft) = activity.draft { frame["draft"] = json!(draft); }
                                    let _ = activity_app.bot_runtimes.activities.send(frame);
                                    Ok(())
                                }).await
                            }.await;
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
