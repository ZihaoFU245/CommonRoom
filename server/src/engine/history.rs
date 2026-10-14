//! SQLite is authoritative. Reader connections and revision-keyed pages are bounded.
use super::*;
use rusqlite::OpenFlags;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};

pub const PAGE_SIZE: usize = 100;
const ROOM_CACHE_MESSAGES: usize = 1000;
const CACHE_MESSAGES: usize = 10_000;
const CACHE_BYTES: usize = 32 * 1024 * 1024;
const PAGE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
pub struct HistoryRange {
    pub start: Option<u64>,
    pub around: Option<u64>,
    pub end: Option<u64>,
    pub before: Option<u64>,
    pub after: Option<u64>,
    pub limit: Option<usize>,
}
impl HistoryRange {
    pub fn validate(&self) -> Result<usize, String> {
        let limit = self.limit.unwrap_or(PAGE_SIZE);
        if !(1..=PAGE_SIZE).contains(&limit)
            || self.before.is_some() && self.after.is_some()
            || self.around.is_some()
                && [self.start, self.end, self.before, self.after]
                    .into_iter()
                    .any(|v| v.is_some())
            || self.start.zip(self.end).is_some_and(|(a, b)| a > b)
            || [self.start, self.end, self.before, self.after, self.around]
                .into_iter()
                .flatten()
                .any(|v| v > 9_007_199_254_740_991)
        {
            return Err(
                "History requires ordered sequence bounds and a page size of 1–100.".into(),
            );
        }
        Ok(limit)
    }
}
struct CachedPage {
    conversation: String,
    revision: u64,
    range: HistoryRange,
    messages: Arc<Vec<Message>>,
    bytes: usize,
    before: bool,
    after: bool,
}
#[derive(Default)]
struct Cache {
    pages: VecDeque<CachedPage>,
    messages: usize,
    bytes: usize,
}
impl Cache {
    fn remove(&mut self, index: usize) {
        if let Some(page) = self.pages.remove(index) {
            self.messages -= page.messages.len();
            self.bytes -= page.bytes;
        }
    }
    fn insert(&mut self, page: CachedPage) {
        let id = page.conversation.clone();
        let revision = page.revision;
        let mut index = 0;
        while index < self.pages.len() {
            if self.pages[index].conversation == id && self.pages[index].revision != revision {
                self.remove(index);
            } else {
                index += 1;
            }
        }
        self.messages += page.messages.len();
        self.bytes += page.bytes;
        self.pages.push_back(page);
        while self
            .pages
            .iter()
            .filter(|p| p.conversation == id)
            .map(|p| p.messages.len())
            .sum::<usize>()
            > ROOM_CACHE_MESSAGES
        {
            if let Some(i) = self.pages.iter().position(|p| p.conversation == id) {
                self.remove(i);
            } else {
                break;
            }
        }
        while self.messages > CACHE_MESSAGES || self.bytes > CACHE_BYTES || self.pages.len() > 256 {
            self.remove(0);
        }
    }
}
type UnreadEntry = (String, String, u64, u64, Unread);
pub struct HistoryStore {
    readers: Vec<Mutex<Connection>>,
    next: AtomicUsize,
    cache: Mutex<Cache>,
    hits: AtomicU64,
    misses: AtomicU64,
    unread_cache: Mutex<VecDeque<UnreadEntry>>,
}
impl HistoryStore {
    pub fn open(location: &str, memory: bool) -> Result<Self, String> {
        let mut readers = Vec::new();
        for _ in 0..4 {
            let flags = if memory {
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_URI
            } else {
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI
            };
            let db = Connection::open_with_flags(location, flags).map_err(|e| e.to_string())?;
            db.busy_timeout(std::time::Duration::from_secs(2))
                .map_err(|e| e.to_string())?;
            db.execute_batch("PRAGMA query_only=ON; PRAGMA cache_size=-2048;")
                .map_err(|e| e.to_string())?;
            readers.push(Mutex::new(db));
        }
        Ok(Self {
            readers,
            next: AtomicUsize::new(0),
            cache: Mutex::new(Cache::default()),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            unread_cache: Mutex::new(VecDeque::new()),
        })
    }
    pub fn read<T>(
        &self,
        query: impl FnOnce(&Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        let index = self.next.fetch_add(1, Ordering::Relaxed) % self.readers.len();
        for offset in 0..self.readers.len() {
            if let Ok(db) = self.readers[(index + offset) % self.readers.len()].try_lock() {
                return query(&db);
            }
        }
        let db = self.readers[index]
            .lock()
            .map_err(|_| "History storage unavailable.")?;
        query(&db)
    }
    pub(super) fn contains_sequence(&self, id: &str, sequence: u64) -> Result<bool, String> {
        self.read(|db| {
            db.query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE conversation=?1 AND sequence=?2)",
                params![id, sequence],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())
        })
    }
    pub fn stats(&self) -> Result<serde_json::Value, String> {
        let cache = self
            .cache
            .lock()
            .map_err(|_| "History cache unavailable.")?;
        Ok(
            serde_json::json!({"hits":self.hits.load(Ordering::Relaxed), "misses":self.misses.load(Ordering::Relaxed), "messages":cache.messages,"accounted_bytes":cache.bytes,"message_limit":CACHE_MESSAGES,"byte_limit":CACHE_BYTES,"per_conversation_limit":ROOM_CACHE_MESSAGES}),
        )
    }
    pub fn page(
        &self,
        id: &str,
        revision: u64,
        range: HistoryRange,
        view: &str,
    ) -> Result<History, String> {
        let limit = range.validate()?;
        {
            let mut cache = self
                .cache
                .lock()
                .map_err(|_| "History cache unavailable.")?;
            if let Some(index) = cache.pages.iter().position(|p| {
                p.conversation == id
                    && p.revision == revision
                    && (p.range == range
                        || range.start.is_none()
                            && range.end.is_none()
                            && range.before.is_none()
                            && range.after.is_none()
                            && range.around.is_none()
                            && p.range.start.is_none()
                            && p.range.end.is_none()
                            && p.range.before.is_none()
                            && p.range.after.is_none()
                            && p.range.around.is_none()
                            && !p.after
                            && p.messages.len() >= limit)
            }) {
                let page = cache.pages.remove(index).ok_or("History cache changed.")?;
                let result = History {
                    view: view.into(),
                    revision,
                    messages: page
                        .messages
                        .iter()
                        .skip(page.messages.len().saturating_sub(limit))
                        .cloned()
                        .collect(),
                    has_before: page.before || page.messages.len() > limit,
                    has_after: page.after,
                };
                cache.pages.push_back(page);
                self.hits.fetch_add(1, Ordering::Relaxed);
                return Ok(result);
            }
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        let result = self.read(|db| {
            let tx = db.unchecked_transaction().map_err(|e| e.to_string())?;
            let stored: u64 = tx.query_row("SELECT revision FROM entities WHERE kind IN ('room','private') AND json_extract(json,'$.id')=?1", [id], |r| r.get(0)).map_err(|e| e.to_string())?;
            if stored != revision { return Err("History changed; retry the current page.".into()); }
            let ascending=range.after.is_some() || range.start.is_some() || range.around.is_some();
            let start=if let Some(around)=range.around {
                use rusqlite::OptionalExtension;
                tx.query_row("SELECT sequence FROM messages WHERE conversation=?1 AND sequence<=?2 ORDER BY sequence DESC LIMIT 1 OFFSET ?3",params![id,around,limit/2],|r|r.get::<_,u64>(0)).optional().map_err(|e|e.to_string())?.unwrap_or(0)
            } else {range.start.unwrap_or(0)};
            let lower=start.max(range.after.map(|sequence|sequence.saturating_add(1)).unwrap_or(1));
            let upper=range.end.unwrap_or(9_007_199_254_740_991).min(range.before.map(|sequence|sequence.saturating_sub(1)).unwrap_or(9_007_199_254_740_991));
            // One normalized interval makes the primary-key seek independent of depth.
            let sql = format!("SELECT json FROM messages WHERE conversation=?1 AND sequence>=?2 AND sequence<=?3 ORDER BY sequence {} LIMIT ?4", if ascending { "ASC" } else { "DESC" });
            let mut query = tx.prepare(&sql).map_err(|e| e.to_string())?;
            let rows = query.query_map(params![id,lower,upper,limit], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
            let mut messages = Vec::new();let mut payload_bytes=0usize;
            for row in rows {
                let mut message: Message = serde_json::from_str(&row.map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
                load_reactions(&tx, id, &mut message)?;
                let size=serde_json::to_vec(&message).map_err(|e|e.to_string())?.len();
                if !messages.is_empty() && payload_bytes.saturating_add(size)>PAGE_BYTES {break;}
                payload_bytes=payload_bytes.saturating_add(size);
                messages.push(message);
            }
            if !ascending { messages.reverse(); }
            let before = messages.first().map(|m|m.sequence).unwrap_or(if ascending { lower } else { range.before.or(range.start).unwrap_or(0) });
            let after = messages.last().map(|m|m.sequence).unwrap_or(if ascending { range.after.or(range.end).unwrap_or(9_007_199_254_740_991) } else { upper });
            let has_before = tx.query_row("SELECT EXISTS(SELECT 1 FROM messages WHERE conversation=?1 AND sequence<?2)", params![id,before], |r| r.get(0)).map_err(|e|e.to_string())?;
            let has_after = tx.query_row("SELECT EXISTS(SELECT 1 FROM messages WHERE conversation=?1 AND sequence>?2)", params![id,after], |r| r.get(0)).map_err(|e|e.to_string())?;
            Ok(History { view:view.into(),messages,revision,has_before,has_after })
        })?;
        // Large preceding rows must not prevent an around request from reaching its anchor.
        if let Some(around) = range.around
            && result.has_after
            && result.messages.last().is_some_and(|m| m.sequence < around)
        {
            return self.page(
                id,
                revision,
                HistoryRange {
                    start: Some(around),
                    limit: range.limit,
                    ..Default::default()
                },
                view,
            );
        }
        let bytes = serde_json::to_vec(&result.messages)
            .map_err(|e| e.to_string())?
            .len()
            .saturating_mul(4)
            .saturating_add(result.messages.len().saturating_mul(512));
        if bytes <= CACHE_BYTES {
            self.cache
                .lock()
                .map_err(|_| "History cache unavailable.")?
                .insert(CachedPage {
                    conversation: id.into(),
                    revision,
                    range,
                    messages: Arc::new(result.messages.clone()),
                    bytes,
                    before: result.has_before,
                    after: result.has_after,
                });
        }
        Ok(result)
    }
    pub fn locate(&self, message_id: &str) -> Result<Option<(String, Message)>, String> {
        self.read(|db| {
            use rusqlite::OptionalExtension;
            let tx = db.unchecked_transaction().map_err(|e| e.to_string())?;
            let row: Option<(String, String)> = tx
                .query_row(
                    "SELECT conversation,json FROM messages WHERE json_extract(json,'$.id')=?1",
                    [message_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            row.map(|(id, raw)| {
                let mut message = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
                load_reactions(&tx, &id, &mut message)?;
                Ok((id, message))
            })
            .transpose()
        })
    }
    pub fn unread(&self, id: &str, user: &str, read: u64, revision: u64) -> Result<Unread, String> {
        {
            let mut cache = self
                .unread_cache
                .lock()
                .map_err(|_| "Unread cache unavailable.")?;
            if let Some(index) = cache
                .iter()
                .position(|(c, u, r, v, _)| c == id && u == user && *r == read && *v == revision)
            {
                let entry = cache.remove(index).ok_or("Unread cache changed.")?;
                let result = entry.4.clone();
                cache.push_back(entry);
                return Ok(result);
            }
        }
        let result=self.read(|db| {
            let tx=db.unchecked_transaction().map_err(|e|e.to_string())?;
            let total:usize=tx.query_row("SELECT count(*) FROM messages WHERE conversation=?1 AND sequence>?2",params![id,read],|r|r.get(0)).map_err(|e|e.to_string())?;
            let own:usize=tx.query_row("SELECT count(*) FROM messages WHERE conversation=?1 AND json_extract(json,'$.from')=?2 AND sequence>?3",params![id,user,read],|r|r.get(0)).map_err(|e|e.to_string())?;
            let count=total.saturating_sub(own);
            let first:Option<u64>=tx.query_row("SELECT (SELECT sequence FROM messages WHERE conversation=?1 AND sequence>?2 AND json_extract(json,'$.from')!=?3 ORDER BY sequence LIMIT 1)",params![id,read,user],|r|r.get(0)).map_err(|e|e.to_string())?;
            let (oldest,through)=tx.query_row("SELECT coalesce((SELECT sequence FROM messages WHERE conversation=?1 ORDER BY sequence LIMIT 1),0),coalesce((SELECT sequence FROM messages WHERE conversation=?1 ORDER BY sequence DESC LIMIT 1),0)",[id],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|e|e.to_string())?;
            Ok(Unread { count,first,oldest,through,revision })
        })?;
        let mut cache = self
            .unread_cache
            .lock()
            .map_err(|_| "Unread cache unavailable.")?;
        cache.retain(|(c, u, _, _, _)| c != id || u != user);
        cache.push_back((id.into(), user.into(), read, revision, result.clone()));
        while cache.len() > 4096 {
            cache.pop_front();
        }
        Ok(result)
    }
}
pub(super) fn load_reactions(
    db: &Connection,
    id: &str,
    message: &mut Message,
) -> Result<(), String> {
    let mut query = db
        .prepare_cached(
            "SELECT reaction,username FROM reactions WHERE conversation=?1 AND sequence=?2",
        )
        .map_err(|e| e.to_string())?;
    let rows = query
        .query_map(params![id, message.sequence], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (reaction, user) = row.map_err(|e| e.to_string())?;
        message.reactions.entry(reaction).or_default().insert(user);
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    fn sequences(page: &History) -> Vec<u64> {
        page.messages
            .iter()
            .map(|message| message.sequence)
            .collect()
    }
    #[test]
    fn pagination_matches_gapped_sequences_in_both_directions() {
        let mut engine = super::super::tests::engine();
        engine
            .execute(Some("alice"), None, "/new room other")
            .unwrap();
        for index in 1..=240 {
            let room = if index % 2 == 0 { "lobby" } else { "other" };
            engine
                .execute(Some("alice"), Some(room), "message")
                .unwrap();
        }
        let all: Vec<u64> = (2..=240).step_by(2).collect();
        let mut forward = Vec::new();
        let mut cursor = 0;
        loop {
            let page = engine
                .history_page(
                    "bob",
                    "lobby",
                    HistoryRange {
                        after: Some(cursor),
                        limit: Some(17),
                        ..Default::default()
                    },
                )
                .unwrap();
            assert_eq!(page.has_before, cursor > 0);
            forward.extend(sequences(&page));
            if !page.has_after {
                break;
            }
            cursor = page.messages.last().unwrap().sequence;
        }
        assert_eq!(forward, all);
        let mut backward = Vec::new();
        let mut before = None;
        loop {
            let page = engine
                .history_page(
                    "bob",
                    "lobby",
                    HistoryRange {
                        before,
                        limit: Some(17),
                        ..Default::default()
                    },
                )
                .unwrap();
            let mut values = sequences(&page);
            values.extend(backward);
            backward = values;
            if !page.has_before {
                break;
            }
            before = Some(page.messages[0].sequence);
        }
        assert_eq!(backward, all);
        for (range, expected) in [
            (
                HistoryRange {
                    start: Some(99),
                    end: Some(105),
                    ..Default::default()
                },
                vec![100, 102, 104],
            ),
            (
                HistoryRange {
                    before: Some(2),
                    ..Default::default()
                },
                vec![],
            ),
            (
                HistoryRange {
                    after: Some(240),
                    ..Default::default()
                },
                vec![],
            ),
            (
                HistoryRange {
                    start: Some(100),
                    end: Some(100),
                    ..Default::default()
                },
                vec![100],
            ),
            (
                HistoryRange {
                    around: Some(100),
                    limit: Some(5),
                    ..Default::default()
                },
                vec![96, 98, 100, 102, 104],
            ),
        ] {
            assert_eq!(
                sequences(&engine.history_page("bob", "lobby", range).unwrap()),
                expected
            );
        }
    }
    #[test]
    fn empty_edge_pages_retain_the_reverse_paging_direction() {
        let mut engine = super::super::tests::engine();
        engine
            .execute(Some("alice"), Some("lobby"), "first")
            .unwrap();
        let older = engine
            .history_page(
                "bob",
                "lobby",
                HistoryRange {
                    before: Some(1),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(older.messages.is_empty());
        assert!(!older.has_before && older.has_after);
        let newer = engine
            .history_page(
                "bob",
                "lobby",
                HistoryRange {
                    after: Some(1),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(newer.messages.is_empty());
        assert!(newer.has_before && !newer.has_after);
    }
    #[test]
    fn range_validation_rejects_ambiguous_and_unsafe_bounds() {
        for range in [
            HistoryRange {
                limit: Some(0),
                ..Default::default()
            },
            HistoryRange {
                limit: Some(101),
                ..Default::default()
            },
            HistoryRange {
                before: Some(10),
                after: Some(1),
                ..Default::default()
            },
            HistoryRange {
                start: Some(10),
                end: Some(9),
                ..Default::default()
            },
            HistoryRange {
                around: Some(5),
                end: Some(9),
                ..Default::default()
            },
            HistoryRange {
                after: Some(u64::MAX),
                ..Default::default()
            },
        ] {
            assert!(range.validate().is_err(), "{range:?}");
        }
        assert_eq!(HistoryRange::default().validate().unwrap(), 100);
        assert!(
            HistoryRange {
                end: Some(9_007_199_254_740_991),
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
    }
    #[test]
    fn unread_and_cached_tails_follow_cursor_retraction_and_retention() {
        let mut engine = super::super::tests::engine();
        for author in ["alice", "bob", "alice", "bob", "alice"] {
            engine
                .execute(Some(author), Some("lobby"), "message")
                .unwrap();
        }
        let page = engine.history("bob", "lobby").unwrap();
        let small = engine
            .history_page(
                "bob",
                "lobby",
                HistoryRange {
                    limit: Some(2),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(sequences(&small), vec![4, 5]);
        assert!(small.has_before && !small.has_after);
        assert_eq!(engine.history_store.stats().unwrap()["hits"], 1);
        let unread = engine.unreads("bob").unwrap();
        assert_eq!(unread["lobby"].count, 3);
        assert_eq!(unread["lobby"].first, Some(1));
        engine.mark_read("bob", "lobby", 3).unwrap();
        assert!(!engine.mark_read("bob", "lobby", 1).unwrap());
        assert!(engine.mark_read("bob", "lobby", 99).is_err());
        assert_eq!(engine.unreads("bob").unwrap()["lobby"].count, 1);
        engine
            .execute(
                Some("alice"),
                Some("lobby"),
                &format!("/retract {}", page.messages[4].id),
            )
            .unwrap();
        assert_eq!(engine.unreads("bob").unwrap()["lobby"].count, 0);
        assert_eq!(
            sequences(&engine.history("bob", "lobby").unwrap()),
            vec![1, 2, 3, 4]
        );
        engine.set_limits(64, 64, 2).unwrap();
        let retained = engine.history("bob", "lobby").unwrap();
        assert_eq!(sequences(&retained), vec![3, 4]);
        assert!(!retained.has_before);
        assert_eq!(engine.unreads("bob").unwrap()["lobby"].oldest, 3);
    }
    #[test]
    fn large_payload_pages_make_progress_and_around_reaches_anchor() {
        let mut engine = super::super::tests::engine();
        for _ in 0..4 {
            engine
                .execute(Some("alice"), Some("lobby"), "message")
                .unwrap();
        }
        // Legacy/imported rows may exceed today's input limit.
        engine
            .db
            .lock()
            .unwrap()
            .execute(
                "UPDATE messages SET json=json_set(json,'$.text',?1)",
                ["x".repeat(PAGE_BYTES)],
            )
            .unwrap();
        let first = engine
            .history_page(
                "bob",
                "lobby",
                HistoryRange {
                    after: Some(0),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(sequences(&first), vec![1]);
        assert!(first.has_after);
        let next = engine
            .history_page(
                "bob",
                "lobby",
                HistoryRange {
                    after: Some(1),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(sequences(&next), vec![2]);
        let anchor = engine
            .history_page(
                "bob",
                "lobby",
                HistoryRange {
                    around: Some(3),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(sequences(&anchor), vec![3]);
    }
    #[test]
    fn cache_enforces_global_bytes_pages_and_lru_without_eager_allocation() {
        let mut engine = super::super::tests::engine();
        assert_eq!(engine.history_store.stats().unwrap()["messages"], 0);
        engine
            .execute(Some("alice"), Some("lobby"), "seed")
            .unwrap();
        let message = engine.history("alice", "lobby").unwrap().messages.remove(0);
        let make = |id: String, revision, count, bytes| CachedPage {
            conversation: id,
            revision,
            range: HistoryRange::default(),
            messages: Arc::new(vec![message.clone(); count]),
            bytes,
            before: false,
            after: false,
        };
        let mut cache = Cache::default();
        for index in 0..120 {
            cache.insert(make(index.to_string(), 1, 100, 1024));
        }
        assert_eq!(cache.messages, CACHE_MESSAGES);
        assert!(!cache.pages.iter().any(|p| p.conversation == "0"));
        cache.insert(make("119".into(), 2, 1, 1024));
        assert_eq!(
            cache
                .pages
                .iter()
                .filter(|p| p.conversation == "119")
                .count(),
            1
        );
        cache.insert(make("large".into(), 1, 1, CACHE_BYTES));
        assert_eq!(cache.pages.len(), 1);
        for index in 0..300 {
            cache.insert(make(index.to_string(), 1, 0, 0));
        }
        assert_eq!(cache.pages.len(), 256);
        assert_eq!(cache.bytes, 0);
        assert_eq!(cache.messages, 0);
    }
    #[test]
    fn account_rename_preserves_private_history_across_streaming_batches_and_name_reuse() {
        let mut engine = super::super::tests::engine();
        for index in 0..205 {
            engine
                .execute(Some("alice"), None, &format!("/tell bob batch {index}"))
                .unwrap();
        }
        let before = engine.history("bob", "@direct:alice").unwrap();
        let original_id = engine.data.private["alice:bob"].id.clone();
        engine.execute(Some("bob"), None, "/rename robert").unwrap();
        assert_eq!(engine.data.private["alice:robert"].id, original_id);
        let all = super::super::tests::private_messages(&engine, "alice:robert");
        assert_eq!(all.len(), 205);
        assert!(
            all.iter()
                .all(|message| message.to.as_deref() == Some("robert")
                    && message.private_id == original_id)
        );
        let after = engine.history("robert", "@direct:alice").unwrap();
        assert_eq!(sequences(&before), sequences(&after));
        assert!(after.revision > before.revision);
        engine
            .provision("bob", "replacement".into(), false, false)
            .unwrap();
        assert!(engine.history("bob", "@direct:alice").is_err());
        engine
            .execute(Some("alice"), None, "/tell bob new identity")
            .unwrap();
        assert_ne!(engine.data.private["alice:bob"].id, original_id);
        assert_eq!(
            engine
                .history("bob", "@direct:alice")
                .unwrap()
                .messages
                .len(),
            1
        );
        assert_eq!(
            engine
                .history("robert", "@direct:alice")
                .unwrap()
                .messages
                .len(),
            100
        );
    }
    #[test]
    fn deep_range_query_uses_a_bounded_primary_key_seek() {
        let engine = super::super::tests::engine();
        engine.history_store.read(|db| {
            let mut query=db.prepare("EXPLAIN QUERY PLAN SELECT json FROM messages WHERE conversation=?1 AND sequence>=?2 AND sequence<=?3 ORDER BY sequence ASC LIMIT ?4").unwrap();
            let plans=query.query_map(params![engine.data.rooms["lobby"].id,1000,1099,100],|row|row.get::<_,String>(3)).unwrap().collect::<Result<Vec<_>,_>>().unwrap().join(" ");
            assert!(plans.contains("SEARCH messages"),"{plans}");
            assert!(plans.contains("sequence>?") && plans.contains("sequence<?"),"{plans}");
            assert!(!plans.contains("TEMP B-TREE"),"{plans}");
            Ok(())
        }).unwrap();
    }
    #[test]
    fn four_reader_connections_execute_simultaneously() {
        let engine = super::super::tests::engine();
        let (started, ready) = std::sync::mpsc::channel();
        let mut releases = Vec::new();
        let mut readers = Vec::new();
        for _ in 0..4 {
            let store = engine.history_store.clone();
            let started = started.clone();
            let (release, wait) = std::sync::mpsc::channel();
            releases.push(release);
            readers.push(std::thread::spawn(move || {
                store.read(|db| {
                    started.send(()).unwrap();
                    wait.recv_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                    db.query_row("SELECT count(*) FROM entities", [], |row| {
                        row.get::<_, usize>(0)
                    })
                    .map_err(|e| e.to_string())
                })
            }));
        }
        let overlapped = (0..4).all(|_| {
            ready
                .recv_timeout(std::time::Duration::from_secs(2))
                .is_ok()
        });
        for release in releases {
            let _ = release.send(());
        }
        for reader in readers {
            assert!(reader.join().unwrap().unwrap() > 0);
        }
        assert!(
            overlapped,
            "All four reader jobs must enter before any is released."
        );
    }

    #[test]
    fn pages_are_independent_bounded_and_revision_checked() {
        let mut engine = super::super::tests::engine();
        engine.set_limits(64, 64, 2000).unwrap();
        for index in 1..=1200 {
            engine
                .execute(Some("alice"), Some("lobby"), &format!("message {index}"))
                .unwrap();
        }
        assert!(engine.data.rooms["lobby"].messages.is_empty());
        let range = HistoryRange {
            start: Some(1000),
            end: Some(1099),
            ..Default::default()
        };
        let page = engine.history_page("alice", "lobby", range).unwrap();
        assert_eq!(page.messages.len(), 100);
        assert_eq!(page.messages[0].sequence, 1000);
        assert_eq!(page.messages[99].sequence, 1099);
        assert!(page.has_before && page.has_after);
        let stats = engine.history_store.stats().unwrap();
        assert_eq!(stats["messages"], 100);
        engine.history_page("alice", "lobby", range).unwrap();
        assert_eq!(engine.history_store.stats().unwrap()["hits"], 1);
        let id = page.messages[0].id.clone();
        engine
            .execute(Some("bob"), Some("lobby"), &format!("/react {id} ok"))
            .unwrap();
        let updated = engine.history_page("alice", "lobby", range).unwrap();
        assert!(updated.messages[0].reactions["ok"].contains("bob"));
        assert!(engine.history_page("eve", "missing", range).is_err());
        for start in (1..1200).step_by(100) {
            engine
                .history_page(
                    "alice",
                    "lobby",
                    HistoryRange {
                        start: Some(start),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        assert!(
            engine.history_store.stats().unwrap()["messages"]
                .as_u64()
                .unwrap()
                <= 1000
        );
    }
    #[test]
    fn current_schema_restart_does_not_load_messages() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("chat.sqlite");
        {
            let mut engine = Engine::open(&path).unwrap();
            engine
                .provision("alice", "hash".into(), true, false)
                .unwrap();
            engine
                .execute(Some("alice"), None, "/new room room")
                .unwrap();
            engine
                .execute(Some("alice"), Some("room"), "preserved")
                .unwrap();
        }
        let engine = Engine::open(&path).unwrap();
        assert!(engine.data.rooms["room"].messages.is_empty());
        assert_eq!(engine.history_store.stats().unwrap()["messages"], 0);
        assert_eq!(
            engine
                .history_page("alice", "room", HistoryRange::default())
                .unwrap()
                .messages[0]
                .text,
            "preserved"
        );
        assert_eq!(
            engine
                .db
                .lock()
                .unwrap()
                .query_row::<u32, _, _>("PRAGMA user_version", [], |r| r.get(0))
                .unwrap(),
            9
        );
    }
    #[test]
    fn file_readers_can_overlap_a_wal_writer() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("chat.sqlite");
        let mut engine = Engine::open(&path).unwrap();
        engine
            .provision("alice", "hash".into(), true, false)
            .unwrap();
        engine
            .execute(Some("alice"), None, "/new room room")
            .unwrap();
        engine
            .execute(Some("alice"), Some("room"), "committed")
            .unwrap();
        let id = engine.data.rooms["room"].id.clone();
        let store = engine.history_store.clone();
        let mut writer = engine.db.lock().unwrap();
        let tx = writer.transaction().unwrap();
        tx.execute("UPDATE counters SET sequence=sequence+1", [])
            .unwrap();
        let revision = engine.data.rooms["room"].revision;
        let reader = std::thread::spawn(move || {
            store
                .page(&id, revision, HistoryRange::default(), "room")
                .unwrap()
        });
        assert_eq!(reader.join().unwrap().messages[0].text, "committed");
        tx.rollback().unwrap();
    }
}
