//! Share serialized socket baselines while the engine remains unchanged.
use super::*;
use axum::extract::ws::Utf8Bytes;
use std::{collections::VecDeque, sync::OnceLock};

type Key = (String, bool, u64, u64, u64);
#[derive(Clone)]
struct Baseline {
    frame: Utf8Bytes,
    complete: bool,
}
type Value = Arc<OnceLock<Result<Baseline, String>>>;
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
            let keep = old.0 != key.0 || old.1 != key.1;
            if !keep {
                self.bytes = self.bytes.saturating_sub(*size);
            }
            keep
        });
        self.order.retain(|old| old.0 != key.0 || old.1 != key.1);
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
pub(super) fn value(
    app: &App,
    headers: &axum::http::HeaderMap,
    bot: bool,
) -> Result<Utf8Bytes, String> {
    // Keep this guard through cache publication. Writers invalidate while holding
    // their exclusive guard, and every caller rechecks its own credentials.
    let engine = app.engine_read()?;
    let user = super::auth::identity(&engine, headers).ok_or("Credential revoked.")?;
    engine.history_store.ensure_available()?;
    let key = key(&engine, user.clone(), bot, &user);
    cached(app, key, || {
        let (serialized, complete) = if bot {
            (
                serde_json::to_string(&engine.bot_snapshot(&user).map_err(|e| e.to_string())?),
                true,
            )
        } else {
            let (snapshot, complete) = engine
                .snapshot_baseline(&user)?
                .ok_or("Snapshot unavailable.")?;
            (serde_json::to_string(&snapshot), complete)
        };
        serialized
            .map(|value| (value, complete))
            .map_err(|_| "Snapshot serialization failed.".into())
    })
}
fn key(engine: &Engine, profile: String, bot: bool, user: &str) -> Key {
    let presence = if !bot
        && engine.allows(
            Some(user),
            &crate::engine::authorization::Scope::Server,
            crate::engine::authorization::Action::Directory,
        ) {
        engine.presence_revision
    } else {
        0
    };
    (
        profile,
        bot,
        engine.revision,
        presence,
        engine
            .data
            .users
            .get(user)
            .and_then(|account| engine.read_revisions.get(&account.id))
            .copied()
            .unwrap_or(0),
    )
}
pub(crate) fn bot_value(
    app: &App,
    engine: &Engine,
    context: &crate::bots::auth::Context,
) -> Result<Utf8Bytes, String> {
    let user = context.user(engine).map_err(|e| e.to_string())?;
    engine.history_store.ensure_available()?;
    let key = key(
        engine,
        format!("{}:{}", context.account, context.credential),
        true,
        &user,
    );
    cached(app, key, || {
        let snapshot = context.snapshot(engine).map_err(|e| e.to_string())?;
        let complete = snapshot.conversations.iter().all(|c| c.unread.is_some());
        serde_json::to_string(&snapshot)
            .map(|value| (value, complete))
            .map_err(|e| e.to_string())
    })
}
fn cached(
    app: &App,
    key: Key,
    build: impl FnOnce() -> Result<(String, bool), String>,
) -> Result<Utf8Bytes, String> {
    let value = app
        .snapshots
        .lock()
        .map_err(|_| "Snapshot cache unavailable.")?
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
        .map_err(|_| "Snapshot cache unavailable.")?;
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
