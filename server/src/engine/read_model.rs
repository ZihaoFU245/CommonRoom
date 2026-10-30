//! Bounded immutable metadata projections, independent of message revisions.
use super::*;
use snapshot::{BotSnapshotPlan, SnapshotPlan};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_PROFILES: usize = 128;
type Key = (String, bool);
#[derive(Clone)]
enum Template {
    Browser(Box<SnapshotPlan>),
    Bot(BotSnapshotPlan),
}
#[derive(Default)]
struct Cache {
    entries: BTreeMap<Key, (Arc<Template>, usize)>,
    order: VecDeque<Key>,
    bytes: usize,
}
impl Cache {
    fn get(&mut self, key: &Key) -> Option<Arc<Template>> {
        let value = self.entries.get(key)?.0.clone();
        self.order.retain(|old| old != key);
        self.order.push_back(key.clone());
        Some(value)
    }
    fn insert(&mut self, key: Key, value: Template, bytes: usize) {
        if bytes > MAX_BYTES {
            return;
        }
        if let Some((_, previous)) = self.entries.remove(&key) {
            self.bytes -= previous;
        }
        self.order.retain(|old| old != &key);
        self.order.push_back(key.clone());
        self.entries.insert(key, (Arc::new(value), bytes));
        self.bytes += bytes;
        while self.bytes > MAX_BYTES || self.entries.len() > MAX_PROFILES {
            if let Some(key) = self.order.pop_front()
                && let Some((_, bytes)) = self.entries.remove(&key)
            {
                self.bytes -= bytes;
            } else {
                break;
            }
        }
    }
}
// Account for container/element allocations as well as text. Wire length alone
// undercounts short names in member sets and directory vectors.
fn metadata_bytes(value: &serde_json::Value) -> usize {
    use serde_json::Value;
    128usize.saturating_add(match value {
        Value::String(text) => text.len().saturating_mul(2),
        Value::Array(values) => values.iter().fold(0usize, |bytes, value| {
            bytes.saturating_add(metadata_bytes(value))
        }),
        Value::Object(values) => values.iter().fold(0usize, |bytes, (key, value)| {
            bytes
                .saturating_add(128)
                .saturating_add(key.len().saturating_mul(2))
                .saturating_add(metadata_bytes(value))
        }),
        _ => 0,
    })
}
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct SnapshotStamp {
    pub(crate) metadata: u64,
    pub(crate) history: Vec<(String, u64)>,
    pub(crate) read: u64,
    elevated: bool,
}
/// Read-only routing clock. Only the metadata owner can advance it.
#[derive(Clone, Default)]
pub(crate) struct MetadataClock(Arc<AtomicU64>);
impl MetadataClock {
    pub(crate) fn current(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }
}
#[derive(Default)]
pub(super) struct ReadModel {
    pub(super) revision: u64,
    clock: MetadataClock,
    notice: Option<(u64, Arc<MetadataNotice>)>,
    cache: Mutex<Cache>,
    suspended: u32,
    pub(super) builds: AtomicU64,
}
impl ReadModel {
    pub(super) fn record_notice(&mut self, revision: u64, notice: MetadataNotice) {
        self.notice = Some((revision, Arc::new(notice)));
    }
    pub(super) fn notice(&self, revision: u64) -> Arc<MetadataNotice> {
        self.notice
            .as_ref()
            .filter(|(recorded, _)| *recorded == revision)
            .map(|(_, notice)| notice.clone())
            .unwrap_or_else(|| Arc::new(MetadataNotice::all()))
    }
    pub(super) fn suspend(&mut self) {
        self.suspended = self.suspended.saturating_add(1);
    }
    pub(super) fn resume(&mut self) {
        self.suspended = self.suspended.saturating_sub(1);
    }
    pub(super) fn advance(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.clock.0.store(self.revision, Ordering::Release);
        self.cache = Mutex::default();
        self.notice = None;
    }
    pub(super) fn refresh_profiles(&mut self, profiles: &BTreeMap<String, Option<String>>) {
        self.revision = self.revision.wrapping_add(1);
        self.clock.0.store(self.revision, Ordering::Release);
        self.notice = None;
        let Ok(mut cache) = self.cache.lock() else {
            return;
        };
        for (template, bytes) in cache.entries.values_mut() {
            if let Template::Browser(plan) = Arc::make_mut(template) {
                for (name, value) in profiles {
                    if let Some(previous) = plan.avatars.remove(name) {
                        *bytes = bytes.saturating_sub((name.len() + previous.len()) * 2 + 128);
                    }
                    if let Some(value) = value {
                        plan.avatars.insert(name.clone(), value.clone());
                        *bytes = bytes.saturating_add((name.len() + value.len()) * 2 + 128);
                    }
                }
            }
        }
        cache.bytes = cache.entries.values().map(|(_, bytes)| bytes).sum();
        while cache.bytes > MAX_BYTES {
            if let Some(key) = cache.order.pop_front()
                && let Some((_, bytes)) = cache.entries.remove(&key)
            {
                cache.bytes -= bytes;
            } else {
                break;
            }
        }
    }
    fn conversations(&self, id: &str, bot: bool) -> Option<Vec<String>> {
        if self.suspended > 0 {
            return None;
        }
        let template = self.cache.lock().ok()?.get(&(id.into(), bot))?;
        Some(match template.as_ref() {
            Template::Browser(plan) => plan.jobs.iter().map(|job| job.id.clone()).collect(),
            Template::Bot(plan) => plan
                .snapshot
                .conversations
                .iter()
                .map(|conversation| conversation.id.clone())
                .collect(),
        })
    }
    pub(super) fn browser(&self, id: &str) -> Option<SnapshotPlan> {
        if self.suspended > 0 {
            return None;
        }
        let template = self.cache.lock().ok()?.get(&(id.into(), false))?;
        match template.as_ref() {
            Template::Browser(plan) => Some(plan.as_ref().clone()),
            _ => None,
        }
    }
    pub(super) fn bot(&self, id: &str) -> Option<BotSnapshotPlan> {
        if self.suspended > 0 {
            return None;
        }
        let template = self.cache.lock().ok()?.get(&(id.into(), true))?;
        match template.as_ref() {
            Template::Bot(plan) => Some(plan.clone()),
            _ => None,
        }
    }
    pub(super) fn remember_browser(&self, id: String, plan: &SnapshotPlan) {
        if self.suspended > 0 {
            return;
        }
        self.builds.fetch_add(1, Ordering::Relaxed);
        // Include owned query/lookup data and allocation overhead, beyond wire
        // metadata. History pages are never kept in metadata templates.
        let Ok(metadata) = serde_json::to_value(&plan.snapshot) else {
            return;
        };
        let bytes = metadata_bytes(&metadata)
            .saturating_add(plan.jobs.len().saturating_mul(512))
            .saturating_add(
                plan.avatars
                    .iter()
                    .map(|(name, url)| (name.len() + url.len()) * 2 + 128)
                    .sum::<usize>(),
            )
            .saturating_add(plan.colors.bytes());
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(
                (id, false),
                Template::Browser(Box::new(plan.clone())),
                bytes,
            );
        }
    }
    pub(super) fn remember_bot(&self, id: String, plan: &BotSnapshotPlan) {
        if self.suspended > 0 {
            return;
        }
        self.builds.fetch_add(1, Ordering::Relaxed);
        let Ok(metadata) = serde_json::to_value(&plan.snapshot) else {
            return;
        };
        let bytes =
            metadata_bytes(&metadata).saturating_add(plan.cursors.len().saturating_mul(512));
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert((id, true), Template::Bot(plan.clone()), bytes);
        }
    }
}
#[derive(PartialEq, Eq)]
pub(crate) struct DeliveryStamp {
    metadata: u64,
    read: u64,
    presence: Option<u64>,
    history: Vec<(String, u64)>,
}
impl Engine {
    pub(crate) fn metadata_clock(&self) -> MetadataClock {
        self.read_model.clock.clone()
    }

    pub(crate) fn delivery_stamp(
        &self,
        user: &str,
        ids: &BTreeSet<String>,
        presence: bool,
    ) -> Result<DeliveryStamp, EngineError> {
        let account = self
            .active_account_id(user)
            .ok_or_else(|| EngineError::Forbidden("Account unavailable.".into()))?;
        let history = ids
            .iter()
            .map(|id| {
                self.conversation_position(user, id)
                    .map(|(revision, _)| (id.clone(), revision))
                    .ok_or_else(|| {
                        EngineError::Conflict("Conversation changed; retry shortly.".into())
                    })
            })
            .collect::<Result<_, _>>()?;
        Ok(DeliveryStamp {
            metadata: self.read_model.revision,
            read: self.read_revisions.get(account).copied().unwrap_or(0),
            presence: presence.then(|| self.presence_revision()),
            history,
        })
    }
    pub(crate) fn snapshot_stamp(
        &self,
        user: &str,
        bot: bool,
    ) -> Result<SnapshotStamp, EngineError> {
        let account = self
            .data
            .users
            .get(user)
            .filter(|account| !account.disabled)
            .ok_or_else(|| EngineError::Forbidden("Account unavailable.".into()))?;
        let ids = if let Some(ids) = self.read_model.conversations(&account.id, bot) {
            ids
        } else if bot {
            self.bot_snapshot_plan(user)
                .map_err(EngineError::unavailable)?
                .snapshot
                .conversations
                .into_iter()
                .map(|conversation| conversation.id)
                .collect()
        } else {
            self.snapshot_plan_for(user, false)
                .ok_or("Snapshot unavailable.")?
                .jobs
                .into_iter()
                .map(|job| job.id)
                .collect()
        };
        let history = ids
            .into_iter()
            .map(|id| {
                let (revision, _) = self
                    .conversation_position(user, &id)
                    .ok_or("Conversation metadata unavailable.")?;
                Ok((id, revision))
            })
            .collect::<Result<_, EngineError>>()?;
        Ok(SnapshotStamp {
            metadata: self.read_model.revision,
            elevated: self.read_model.suspended > 0,
            history,
            read: self.read_revisions.get(&account.id).copied().unwrap_or(0),
        })
    }
    // These lookups refresh only dynamic cursors/content revisions on an already
    // authorized template. Authority changes invalidate templates; profile-only
    // commits refresh their bounded profile values without rebuilding authority.
    pub(super) fn conversation_position(&self, user: &str, id: &str) -> Option<(u64, u64)> {
        let view = self.bot_index.conversations.get(id)?;
        let (revision, key) = if let Some(key) = view.strip_prefix("@private:") {
            (self.data.private.get(key)?.revision, format!("dm:{key}"))
        } else {
            (self.data.rooms.get(view)?.revision, format!("room:{view}"))
        };
        Some((revision, self.read_position(user, &key)))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
