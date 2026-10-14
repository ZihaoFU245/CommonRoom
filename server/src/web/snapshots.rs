//! Share serialized socket baselines while the engine remains unchanged.
use super::*;
use axum::extract::ws::Utf8Bytes;
use std::{collections::VecDeque, sync::OnceLock};

type Key = (String, bool);
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
    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.bytes = 0;
    }
    fn claim(&mut self, key: &Key) -> Value {
        if let Some((value, _)) = self.entries.get(key) {
            return value.clone();
        }
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
    let key = (user.clone(), bot);
    let value = app
        .snapshots
        .lock()
        .map_err(|_| "Snapshot cache unavailable.")?
        .claim(&key);
    let result = value
        .get_or_init(|| {
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
                .map(|frame| Baseline {
                    frame: frame.into(),
                    complete,
                })
                .map_err(|_| "Snapshot serialization failed.".into())
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
