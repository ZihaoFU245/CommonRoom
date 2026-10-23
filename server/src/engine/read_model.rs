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
            );
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
            self.snapshot_plan(user)
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
    // authorized template. Every metadata commit invalidates all templates.
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
mod tests {
    use super::*;
    fn builds(engine: &Engine) -> u64 {
        engine.read_model.builds.load(Ordering::Relaxed)
    }
    fn projections(engine: &Engine, user: &str) -> (serde_json::Value, serde_json::Value) {
        let browser = engine.snapshot_plan(user).unwrap().finish().unwrap().0;
        let bot = engine.bot_snapshot_plan(user).unwrap().finish().unwrap();
        (
            serde_json::to_value(browser).unwrap(),
            serde_json::to_value(bot).unwrap(),
        )
    }
    #[test]
    fn temporary_sudo_projections_never_reuse_or_publish_persisted_authority_templates() {
        let mut engine = super::super::tests::engine();
        for permission in ["/sudo", "x:command.sudo"] {
            engine
                .execute(None, None, &format!("/grant bob @global {permission}"))
                .unwrap();
        }
        let original = projections(&engine, "bob");
        let stamp = engine.snapshot_stamp("bob", false).unwrap();
        let built = builds(&engine);
        for reject in [false, true] {
            let result = engine.with_command_authority(Some("bob"), None, true, |engine| {
                let elevated = projections(engine, "bob");
                assert!(
                    elevated.0["groups"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|group| group == "su")
                );
                assert!(engine.snapshot_stamp("bob", false).unwrap() != stamp);
                if reject {
                    Err("injected rejection".into())
                } else {
                    Ok(())
                }
            });
            assert_eq!(result.is_err(), reject);
            assert_eq!(projections(&engine, "bob"), original);
            assert!(engine.snapshot_stamp("bob", false).unwrap() == stamp);
            assert_eq!(builds(&engine), built);
        }
    }
    #[test]
    fn messages_sessions_presence_and_reads_reuse_metadata_and_refresh_exact_content() {
        let mut engine = super::super::tests::engine();
        let before = projections(&engine, "bob");
        assert_eq!(builds(&engine), 2);
        let metadata = engine.read_model.revision;
        engine
            .execute(Some("alice"), Some("lobby"), "new content")
            .unwrap();
        engine
            .login("cookie".into(), "bob", "test-hash", None)
            .unwrap();
        engine.connect("alice");
        let latest = projections(&engine, "bob");
        assert_eq!(builds(&engine), 2);
        assert_eq!(engine.read_model.revision, metadata);
        assert_ne!(before, latest);
        assert_eq!(latest.0["rooms"][0]["messages"][0]["text"], "new content");
        assert_eq!(latest.0["unread"]["lobby"]["count"], 1);
        assert_eq!(latest.1["conversations"][0]["unread"]["count"], 1);
        let through = engine.data.next_sequence;
        engine.mark_read("bob", "lobby", through).unwrap();
        let read = projections(&engine, "bob");
        assert_eq!(read.0["unread"]["lobby"]["count"], 0);
        assert_eq!(read.1["conversations"][0]["unread"]["count"], 0);
        assert_eq!(builds(&engine), 2);
        // Compare reused projections with a completely cold authorized build.
        engine.read_model.advance();
        assert_eq!(projections(&engine, "bob"), read);
        assert_eq!(builds(&engine), 4);
    }
    #[test]
    fn policy_renames_profiles_private_creation_and_recovery_invalidate_templates() {
        let mut engine = super::super::tests::engine();
        projections(&engine, "bob");
        engine
            .execute(None, None, "/revoke bob user #lobby")
            .unwrap();
        let denied = projections(&engine, "bob");
        assert!(denied.0["rooms"].as_array().unwrap().is_empty());
        assert!(denied.1["conversations"].as_array().unwrap().is_empty());
        assert_eq!(builds(&engine), 4);
        engine
            .execute(None, None, "/grant bob user #lobby")
            .unwrap();
        engine
            .execute(None, None, "/rename room lobby renamed")
            .unwrap();
        engine
            .execute(None, None, "/rename user bob robert")
            .unwrap();
        let renamed = projections(&engine, "robert");
        assert_eq!(renamed.0["username"], "robert");
        assert_eq!(renamed.0["rooms"][0]["name"], "renamed");
        assert_eq!(renamed.1["conversations"][0]["view"], "renamed");
        engine
            .execute(Some("alice"), None, "/tell robert direct")
            .unwrap();
        let direct = projections(&engine, "robert");
        assert_eq!(direct.0["direct"][0]["text"], "direct");
        assert_eq!(direct.1["conversations"].as_array().unwrap().len(), 2);
        engine.set_avatar_hosts(["example.com".into()].into());
        engine
            .set_avatar("alice", Some("https://example.com/a".into()), false)
            .unwrap();
        assert_eq!(
            projections(&engine, "robert").0["avatars"]["alice"],
            "https://example.com/a"
        );
        engine.set_avatar_hosts(BTreeSet::new());
        assert!(
            projections(&engine, "robert").0["avatars"]
                .get("alice")
                .is_none()
        );
        let built = builds(&engine);
        engine.restore_command_state(engine.revision, None).unwrap();
        projections(&engine, "robert");
        assert_eq!(builds(&engine), built + 2);
        engine.execute(None, None, "/disable robert").unwrap();
        assert!(engine.snapshot_plan("robert").is_none());
        assert!(engine.bot_snapshot_plan("robert").is_err());
    }
    #[test]
    fn metadata_cache_bounds_profiles_bytes_and_replacement_without_retained_history() {
        let engine = super::super::tests::engine();
        let plan = engine.snapshot_plan("bob").unwrap();
        assert!(
            plan.snapshot
                .rooms
                .iter()
                .all(|room| room.messages.is_empty())
        );
        assert!(plan.snapshot.unread.is_empty());
        let mut cache = Cache::default();
        for index in 0..MAX_PROFILES + 1 {
            cache.insert(
                (index.to_string(), false),
                Template::Browser(Box::new(plan.clone())),
                1024,
            );
        }
        assert_eq!(cache.entries.len(), MAX_PROFILES);
        assert!(cache.get(&("0".into(), false)).is_none());
        cache.insert(
            ("large".into(), false),
            Template::Browser(Box::new(plan.clone())),
            MAX_BYTES,
        );
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.bytes, MAX_BYTES);
        cache.insert(
            ("large".into(), false),
            Template::Browser(Box::new(plan.clone())),
            1024,
        );
        assert_eq!(cache.bytes, 1024);
        cache.insert(
            ("oversize".into(), false),
            Template::Browser(Box::new(plan)),
            MAX_BYTES + 1,
        );
        assert_eq!(cache.entries.len(), 1);
    }
}
