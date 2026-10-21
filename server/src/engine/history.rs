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

#[derive(Debug)]
pub enum HistoryError {
    Changed,
    InvalidRange(String),
    Unavailable(String),
}
impl From<String> for HistoryError {
    fn from(error: String) -> Self {
        Self::Unavailable(error)
    }
}
impl From<&str> for HistoryError {
    fn from(error: &str) -> Self {
        Self::Unavailable(error.into())
    }
}
impl std::fmt::Display for HistoryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Changed => formatter.write_str("History changed; retry the current page."),
            Self::InvalidRange(error) | Self::Unavailable(error) => formatter.write_str(error),
        }
    }
}

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
// Responses and cache entries share immutable messages and exact wire-size accounting.
#[derive(Default)]
struct PageData {
    messages: Vec<Message>,
    sizes: Vec<usize>,
}
#[derive(Clone, Default)]
pub struct MessagePage {
    data: Arc<PageData>,
    start: usize,
    end: usize,
}
impl MessagePage {
    fn new(messages: Vec<Message>, sizes: Vec<usize>) -> Self {
        let end = messages.len();
        Self {
            data: Arc::new(PageData { messages, sizes }),
            start: 0,
            end,
        }
    }
    fn latest(&self, limit: usize) -> Self {
        Self {
            data: self.data.clone(),
            start: self.end.saturating_sub(limit).max(self.start),
            end: self.end,
        }
    }
    pub(super) fn bounded_tail(&self, count: &mut usize, bytes: &mut usize) -> Self {
        let mut start = self.end;
        while start > self.start && *count > 0 {
            let size = self.data.sizes[start - 1];
            if size > *bytes {
                break;
            }
            *bytes -= size;
            *count -= 1;
            start -= 1;
        }
        Self {
            data: self.data.clone(),
            start,
            end: self.end,
        }
    }
    fn accounted_bytes(&self) -> usize {
        self.data.sizes[self.start..self.end]
            .iter()
            .sum::<usize>()
            .saturating_add(self.len().saturating_sub(1))
            .saturating_add(2)
            .saturating_mul(4)
            .saturating_add(self.len().saturating_mul(512))
    }
}
impl std::ops::Deref for MessagePage {
    type Target = [Message];
    fn deref(&self) -> &Self::Target {
        &self.data.messages[self.start..self.end]
    }
}
impl serde::Serialize for MessagePage {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serde::Serialize::serialize(&**self, serializer)
    }
}

struct CachedPage {
    conversation: String,
    revision: u64,
    range: HistoryRange,
    messages: MessagePage,
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
struct UnreadEntry {
    read: u64,
    revision: u64,
    unread: Unread,
    used: u64,
}
#[derive(Default)]
struct UnreadCache {
    entries: std::collections::HashMap<String, std::collections::HashMap<String, UnreadEntry>>,
    recency: BTreeMap<u64, (String, String)>,
    clock: u64,
}
impl UnreadCache {
    fn get(&mut self, id: &str, user: &str, read: u64, revision: u64) -> Option<Unread> {
        let entry = self.entries.get_mut(id)?.get_mut(user)?;
        if entry.read != read || entry.revision != revision {
            return None;
        }
        let key = self.recency.remove(&entry.used)?;
        self.clock += 1;
        entry.used = self.clock;
        self.recency.insert(entry.used, key);
        Some(entry.unread.clone())
    }
    fn insert(&mut self, id: &str, user: &str, read: u64, revision: u64, unread: Unread) {
        let users = self.entries.entry(id.into()).or_default();
        if let Some(previous) = users.remove(user) {
            self.recency.remove(&previous.used);
        }
        self.clock += 1;
        users.insert(
            user.into(),
            UnreadEntry {
                read,
                revision,
                unread,
                used: self.clock,
            },
        );
        self.recency.insert(self.clock, (id.into(), user.into()));
        while self.recency.len() > 4096 {
            if let Some((_, (id, user))) = self.recency.pop_first()
                && let Some(users) = self.entries.get_mut(&id)
            {
                users.remove(&user);
                if users.is_empty() {
                    self.entries.remove(&id);
                }
            }
        }
    }
}
pub struct HistoryStore {
    readers: Vec<Mutex<Connection>>,
    next: AtomicUsize,
    cache: Mutex<Cache>,
    hits: AtomicU64,
    misses: AtomicU64,
    unread_cache: Mutex<UnreadCache>,
}
impl HistoryStore {
    pub fn ensure_available(&self) -> Result<(), String> {
        if self.cache.is_poisoned()
            || self.unread_cache.is_poisoned()
            || self.readers.iter().any(Mutex::is_poisoned)
        {
            tracing::error!("History storage poisoned; restart the service.");
            return Err("History storage poisoned; restart the service.".into());
        }
        Ok(())
    }
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
            unread_cache: Mutex::new(UnreadCache::default()),
        })
    }
    pub fn read<T>(
        &self,
        query: impl FnOnce(&Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        self.ensure_available()?;
        let index = self.next.fetch_add(1, Ordering::Relaxed) % self.readers.len();
        for offset in 0..self.readers.len() {
            if let Ok(db) = self.readers[(index + offset) % self.readers.len()].try_lock() {
                return query(&db);
            }
        }
        let db = self.readers[index]
            .lock()
            .map_err(|_| "History storage poisoned; restart the service.")?;
        query(&db)
    }
    pub(crate) fn has_author(&self, id: &str, author: &str) -> Result<bool, String> {
        self.read(|db| db.query_row("SELECT EXISTS(SELECT 1 FROM messages WHERE conversation=?1 AND json_extract(json,'$.from')=?2)", params![id, author], |row| row.get(0)).map_err(|e| e.to_string()))
    }
    pub(crate) fn contains_sequence(&self, id: &str, sequence: u64) -> Result<bool, String> {
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
        self.ensure_available()?;
        let cache = self
            .cache
            .lock()
            .map_err(|_| "History storage poisoned; restart the service.")?;
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
        self.page_checked(id, revision, range, view)
            .map_err(|error| error.to_string())
    }
    pub fn page_checked(
        &self,
        id: &str,
        revision: u64,
        range: HistoryRange,
        view: &str,
    ) -> Result<History, HistoryError> {
        self.ensure_available()?;
        let limit = range.validate().map_err(HistoryError::InvalidRange)?;
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
                    messages: page.messages.latest(limit),
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
            if stored != revision { return Ok(None); }
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
            let mut messages = rows.map(|row| {
                let raw = row.map_err(|e| e.to_string())?;
                serde_json::from_str::<Message>(&raw).map_err(|e| e.to_string())
            }).collect::<Result<Vec<_>, String>>()?;
            load_page_reactions(&tx, id, &mut messages)?;
            let mut sizes = Vec::new(); let mut payload_bytes=0usize;
            for message in &messages {
                let size=serde_json::to_vec(message).map_err(|e|e.to_string())?.len();
                if !sizes.is_empty() && payload_bytes.saturating_add(size)>PAGE_BYTES {break;}
                payload_bytes=payload_bytes.saturating_add(size); sizes.push(size);
            }
            messages.truncate(sizes.len());
            if !ascending { messages.reverse(); sizes.reverse(); }
            let before = messages.first().map(|m|m.sequence).unwrap_or(if ascending { lower } else { range.before.or(range.start).unwrap_or(0) });
            let after = messages.last().map(|m|m.sequence).unwrap_or(if ascending { range.after.or(range.end).unwrap_or(9_007_199_254_740_991) } else { upper });
            let has_before = tx.query_row("SELECT EXISTS(SELECT 1 FROM messages WHERE conversation=?1 AND sequence<?2)", params![id,before], |r| r.get(0)).map_err(|e|e.to_string())?;
            let has_after = tx.query_row("SELECT EXISTS(SELECT 1 FROM messages WHERE conversation=?1 AND sequence>?2)", params![id,after], |r| r.get(0)).map_err(|e|e.to_string())?;
            Ok(Some(History { view:view.into(),messages: MessagePage::new(messages, sizes),revision,has_before,has_after }))
        })?.ok_or(HistoryError::Changed)?;
        // Large preceding rows must not prevent an around request from reaching its anchor.
        if let Some(around) = range.around
            && result.has_after
            && result.messages.last().is_some_and(|m| m.sequence < around)
        {
            return self.page_checked(
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
        let bytes = result.messages.accounted_bytes();
        if bytes <= CACHE_BYTES {
            self.cache
                .lock()
                .map_err(|_| "History cache unavailable.")?
                .insert(CachedPage {
                    conversation: id.into(),
                    revision,
                    range,
                    messages: result.messages.clone(),
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
        self.ensure_available()?;
        {
            let mut cache = self
                .unread_cache
                .lock()
                .map_err(|_| "Unread cache unavailable.")?;
            if let Some(unread) = cache.get(id, user, read, revision) {
                return Ok(unread);
            }
        }
        let result=self.read(|db| {
            let tx=db.unchecked_transaction().map_err(|e|e.to_string())?;
            let (count, first, oldest, through) = tx.query_row(
                "SELECT count(*),min(sequence),coalesce((SELECT sequence FROM messages WHERE conversation=?1 ORDER BY sequence LIMIT 1),0),coalesce((SELECT sequence FROM messages WHERE conversation=?1 ORDER BY sequence DESC LIMIT 1),0) FROM messages WHERE conversation=?1 AND sequence>?2 AND json_extract(json,'$.from')!=?3",
                params![id,read,user], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))
            ).map_err(|e|e.to_string())?;
            Ok(Unread { count,first,oldest,through,revision })
        })?;
        let mut cache = self
            .unread_cache
            .lock()
            .map_err(|_| "Unread cache unavailable.")?;
        cache.insert(id, user, read, revision, result.clone());
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

fn load_page_reactions(db: &Connection, id: &str, messages: &mut [Message]) -> Result<(), String> {
    let Some(low) = messages.iter().map(|m| m.sequence).min() else {
        return Ok(());
    };
    let high = messages.iter().map(|m| m.sequence).max().unwrap_or(low);
    let mut positions: BTreeMap<u64, usize> = BTreeMap::new();
    for (index, message) in messages.iter().enumerate() {
        positions.insert(message.sequence, index);
    }
    let mut query = db.prepare_cached("SELECT sequence,reaction,username FROM reactions WHERE conversation=?1 AND sequence BETWEEN ?2 AND ?3").map_err(|e| e.to_string())?;
    let rows = query
        .query_map(params![id, low, high], |r| {
            Ok((
                r.get::<_, u64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (sequence, reaction, user) = row.map_err(|e| e.to_string())?;
        if let Some(index) = positions.get(&sequence) {
            messages[*index]
                .reactions
                .entry(reaction)
                .or_default()
                .insert(user);
        }
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
    fn shared_latest_pages_reuse_payload_and_exact_byte_budgets() {
        let mut engine = super::super::tests::engine();
        for _ in 0..6 {
            engine
                .execute(Some("alice"), Some("lobby"), "Unicode 🦀 \"quoted\"")
                .unwrap();
        }
        let page = engine.history("bob", "lobby").unwrap();
        let latest = engine
            .history_page(
                "bob",
                "lobby",
                HistoryRange {
                    limit: Some(3),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(Arc::ptr_eq(&page.messages.data, &latest.messages.data));
        assert_eq!(sequences(&latest), vec![4, 5, 6]);
        let budget = serde_json::to_vec(&latest.messages[2]).unwrap().len();
        let mut bytes = budget;
        let mut count = 100;
        let tail = latest.messages.bounded_tail(&mut count, &mut bytes);
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].sequence, 6);
        assert_eq!(bytes, 0);
        assert!(Arc::ptr_eq(&tail.data, &page.messages.data));
        assert_eq!(
            serde_json::to_vec(&latest.messages).unwrap(),
            serde_json::to_vec(&latest.messages[..]).unwrap()
        );
    }
    #[test]
    fn indexed_unread_cache_keeps_recency_and_replaces_cursor_revision_entries() {
        let mut cache = UnreadCache::default();
        let unread = Unread {
            count: 3,
            first: Some(1),
            through: 3,
            oldest: 1,
            revision: 1,
        };
        for index in 0..4096 {
            cache.insert(&index.to_string(), "bob", 0, 1, unread.clone());
        }
        assert!(cache.get("0", "bob", 0, 1).is_some());
        cache.insert("new", "bob", 0, 1, unread.clone());
        assert!(cache.get("1", "bob", 0, 1).is_none());
        assert!(cache.get("0", "bob", 0, 1).is_some());
        cache.insert("0", "bob", 2, 2, unread);
        assert!(cache.get("0", "bob", 0, 1).is_none());
        assert!(cache.get("0", "bob", 2, 2).is_some());
        assert!(cache.get("0", "alice", 2, 2).is_none());
        assert_eq!(cache.recency.len(), 4096);
        assert_eq!(
            cache
                .entries
                .values()
                .map(std::collections::HashMap::len)
                .sum::<usize>(),
            4096
        );
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
        let message = engine.history("alice", "lobby").unwrap().messages[0].clone();
        let make = |id: String, revision, count, bytes| CachedPage {
            conversation: id,
            revision,
            range: HistoryRange::default(),
            messages: MessagePage::new(vec![message.clone(); count], vec![1; count]),
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
            17
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)] // Intentionally poison each lock type.
mod snapshot_failure_tests {
    #[test]
    fn one_corrupt_conversation_does_not_abort_healthy_snapshot_data() {
        let mut e = super::super::tests::engine();
        e.execute(Some("alice"), None, "/new room broken").unwrap();
        e.execute(Some("alice"), Some("broken"), "bad record")
            .unwrap();
        e.execute(Some("alice"), Some("lobby"), "healthy record")
            .unwrap();
        let id = e.data.rooms["broken"].id.clone();
        e.db.lock()
            .unwrap()
            .execute(
                "UPDATE messages SET json=json_set(json,'$.time','invalid') WHERE conversation=?1",
                [&id],
            )
            .unwrap();
        let (snapshot, complete) = e.snapshot_baseline("alice").unwrap().unwrap();
        assert!(!complete);
        assert!(
            snapshot
                .rooms
                .iter()
                .find(|r| r.name == "broken")
                .unwrap()
                .messages
                .is_empty()
        );
        assert_eq!(
            snapshot
                .rooms
                .iter()
                .find(|r| r.name == "lobby")
                .unwrap()
                .messages[0]
                .text,
            "healthy record"
        );
        assert!(snapshot.unread.contains_key("lobby"));
        assert!(snapshot.unread.contains_key("broken"));
        // An unavailable unread store still preserves the authorized directory.
        let e = super::super::tests::engine();
        e.db.lock()
            .unwrap()
            .execute_batch("DROP TABLE messages;")
            .unwrap();
        let snapshot = e.try_snapshot("alice").unwrap().unwrap();
        assert_eq!(snapshot.rooms.len(), 1);
        assert!(snapshot.unread.is_empty());
        assert!(e.unreads("alice").is_err());
    }
    #[test]
    fn poisoned_history_locks_fail_explicitly_and_are_never_reused() {
        for lock in 0..3 {
            let e = super::super::tests::engine();
            let store = e.history_store.clone();
            assert!(
                std::thread::spawn(move || {
                    match lock {
                        0 => {
                            let _guard = store.cache.lock().unwrap();
                            panic!("poison page cache");
                        }
                        1 => {
                            let _guard = store.unread_cache.lock().unwrap();
                            panic!("poison unread cache");
                        }
                        _ => {
                            let _guard = store.readers[0].lock().unwrap();
                            panic!("poison reader");
                        }
                    }
                })
                .join()
                .is_err()
            );
            let error = e.try_snapshot("alice").err().unwrap();
            assert!(error.contains("restart the service"));
            assert!(
                e.history_store
                    .stats()
                    .err()
                    .unwrap()
                    .contains("restart the service")
            );
            assert!(
                e.history("alice", "lobby")
                    .err()
                    .unwrap()
                    .contains("restart the service")
            );
            assert!(
                e.unreads("alice")
                    .err()
                    .unwrap()
                    .contains("restart the service")
            );
            assert!(e.history_store.ensure_available().is_err());
        }
    }
}
