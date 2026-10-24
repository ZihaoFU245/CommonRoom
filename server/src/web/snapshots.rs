//! Share baselines by metadata, visible conversation revisions and viewer cursors.
use super::*;
use crate::engine::EngineError;
use axum::extract::ws::Utf8Bytes;
use std::collections::HashMap;
use std::{collections::VecDeque, sync::OnceLock};

type Key = (String, bool, crate::engine::SnapshotStamp, u64, bool);
#[derive(Clone)]
struct Baseline {
    frame: Utf8Bytes,
    complete: bool,
}
type Value = Arc<OnceLock<Result<Baseline, EngineError>>>;
#[derive(Default)]
pub struct SnapshotCache {
    entries: HashMap<Key, (Value, usize)>,
    order: VecDeque<Key>,
    bytes: usize,
}
impl SnapshotCache {
    fn claim(&mut self, key: &Key) -> Value {
        if let Some((value, _)) = self.entries.get(key) {
            return value.clone();
        }
        self.entries.retain(|old, (_, size)| {
            let keep = old.0 != key.0 || old.1 != key.1 || old.4 != key.4;
            if !keep {
                self.bytes = self.bytes.saturating_sub(*size);
            }
            keep
        });
        self.order
            .retain(|old| old.0 != key.0 || old.1 != key.1 || old.4 != key.4);
        let value = Arc::new(OnceLock::new());
        self.entries.insert(key.clone(), (value.clone(), 0));
        self.order.push_back(key.clone());
        self.trim();
        value
    }
    fn finish(&mut self, key: &Key, value: &Value, size: usize) {
        if let Some((current, accounted)) = self.entries.get_mut(key)
            && Arc::ptr_eq(current, value)
            && *accounted == 0
        {
            *accounted = size;
            self.bytes += size;
        }
        self.trim();
    }
    fn trim(&mut self) {
        while self.bytes > 32 * 1024 * 1024 || self.entries.len() > 128 {
            if let Some(key) = self.order.pop_front() {
                if let Some((_, size)) = self.entries.remove(&key) {
                    self.bytes -= size;
                }
            } else {
                break;
            }
        }
    }
}
pub(crate) fn value(
    app: &App,
    headers: &axum::http::HeaderMap,
    bot: bool,
) -> Result<Utf8Bytes, EngineError> {
    let name_colors = super::colors::enabled(headers);
    // Capture only owned metadata under the guard. Disk reads, assembly and
    // serialization run outside it; never deliver a frame after its authority
    // or revision changes. Bounded retry avoids monopolizing a busy writer.
    for _ in 0..3 {
        let initial = {
            let engine = app.engine_read()?;
            let user = super::auth::identity(&engine, &app.config, headers)
                .ok_or_else(|| EngineError::Forbidden("Credential revoked.".into()))?;
            engine.history_store().ensure_available()?;
            key(&engine, user.clone(), bot, &user, name_colors)?
        };
        let result = cached(app, initial.clone(), || {
            let (plan, bot_snapshot) =
                {
                    let engine = app.engine_read()?;
                    let user = super::auth::identity(&engine, &app.config, headers)
                        .ok_or_else(|| EngineError::Forbidden("Credential revoked.".into()))?;
                    if key(&engine, user.clone(), bot, &user, name_colors)? != initial {
                        return Err(EngineError::Conflict(
                            "Snapshot changed; retry shortly.".into(),
                        ));
                    }
                    if bot {
                        (
                            None,
                            Some(engine.bot_snapshot_plan(&user).map_err(EngineError::from)?),
                        )
                    } else {
                        (
                            Some(engine.snapshot_plan_for(&user, name_colors).ok_or_else(
                                || EngineError::Unavailable("Snapshot unavailable.".into()),
                            )?),
                            None,
                        )
                    }
                };
            let (serialized, complete) = if let Some(plan) = plan {
                let (snapshot, complete) = plan.finish()?;
                (serde_json::to_string(&snapshot), complete)
            } else {
                (
                    serde_json::to_string(
                        &bot_snapshot
                            .ok_or_else(|| {
                                EngineError::Unavailable("Snapshot unavailable.".into())
                            })?
                            .finish()
                            .map_err(EngineError::from)?,
                    ),
                    true,
                )
            };
            serialized
                .map(|value| (value, complete))
                .map_err(EngineError::unavailable)
        });
        let engine = app.engine_read()?;
        let user = super::auth::identity(&engine, &app.config, headers)
            .ok_or_else(|| EngineError::Forbidden("Credential revoked.".into()))?;
        engine.history_store().ensure_available()?;
        if key(&engine, user.clone(), bot, &user, name_colors)? == initial {
            return result;
        }
    }
    Err(EngineError::Conflict(
        "Snapshot changed; retry shortly.".into(),
    ))
}

fn key(
    engine: &Engine,
    profile: String,
    bot: bool,
    user: &str,
    name_colors: bool,
) -> Result<Key, EngineError> {
    let presence = if !bot
        && engine.allows(
            Some(user),
            &crate::engine::authorization::Scope::Server,
            crate::engine::authorization::Action::Directory,
        ) {
        engine.presence_revision()
    } else {
        0
    };
    Ok((
        profile,
        bot,
        engine.snapshot_stamp(user, bot)?,
        presence,
        name_colors,
    ))
}

pub(crate) fn bot_value(
    app: &App,
    context: &crate::bots::auth::Context,
) -> Result<Utf8Bytes, crate::bots::error::Error> {
    let profile = format!("{}:{}", context.account, context.credential);
    for _ in 0..3 {
        let initial = {
            let engine = app.engine_read()?;
            let user = context.user(&engine)?;
            engine.history_store().ensure_available()?;
            key(&engine, profile.clone(), true, &user, false)?
        };
        let result = cached(app, initial.clone(), || {
            let plan = {
                let engine = app.engine_read()?;
                let user = context.user(&engine)?;
                if key(&engine, profile.clone(), true, &user, false)? != initial {
                    return Err(EngineError::Conflict(
                        "Snapshot changed; retry shortly.".into(),
                    ));
                }
                context.snapshot_plan(&engine).map_err(EngineError::from)?
            };
            let snapshot = plan.finish().map_err(EngineError::from)?;
            let complete = snapshot
                .conversations
                .iter()
                .all(|conversation| conversation.unread.is_some());
            serde_json::to_string(&snapshot)
                .map(|value| (value, complete))
                .map_err(EngineError::from)
        });
        let engine = app.engine_read()?;
        let user = context.user(&engine)?;
        engine.history_store().ensure_available()?;
        if key(&engine, profile.clone(), true, &user, false)? == initial {
            return result.map_err(crate::bots::error::Error::from);
        }
    }
    Err(crate::bots::error::Error::new(
        crate::bots::error::Code::Conflict,
        "Snapshot changed; retry shortly.",
    ))
}

fn cached(
    app: &App,
    key: Key,
    build: impl FnOnce() -> Result<(String, bool), EngineError>,
) -> Result<Utf8Bytes, EngineError> {
    let value = app
        .snapshots
        .lock()
        .map_err(|_| EngineError::Unavailable("Snapshot cache unavailable.".into()))?
        .claim(&key);
    let result = value
        .get_or_init(|| {
            build().map(|(frame, complete)| Baseline {
                frame: frame.into(),
                complete,
            })
        })
        .clone();
    let mut cache = app
        .snapshots
        .lock()
        .map_err(|_| EngineError::Unavailable("Snapshot cache unavailable.".into()))?;
    if let Ok(baseline) = &result
        && baseline.complete
    {
        cache.finish(&key, &value, baseline.frame.len());
    } else if cache
        .entries
        .get(&key)
        .is_some_and(|(current, _)| Arc::ptr_eq(current, &value))
    {
        cache.entries.remove(&key);
        cache.order.retain(|entry| entry != &key);
    }
    result.map(|baseline| baseline.frame)
}
