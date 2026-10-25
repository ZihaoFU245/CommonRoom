//! SQLite is authoritative. Reader connections and revision-keyed pages are bounded.
use super::*;
mod cache;
mod page;
mod unread;
use cache::*;
pub use page::{HistoryRange, MessagePage};
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
const CACHE_SHARDS: usize = 4;
const UNREAD_ENTRIES_PER_SHARD: usize = 4096 / CACHE_SHARDS;
const PAGE_ASC_SQL: crate::engine::database::Sql<'static> = sql(
    "SELECT json FROM messages WHERE conversation=?1 AND sequence>=?2 AND sequence<=?3 ORDER BY sequence ASC LIMIT ?4",
    "SELECT json FROM messages WHERE conversation=$1 AND sequence>=$2 AND sequence<=$3 ORDER BY sequence ASC LIMIT $4",
);
const PAGE_DESC_SQL: crate::engine::database::Sql<'static> = sql(
    "SELECT json FROM messages WHERE conversation=?1 AND sequence>=?2 AND sequence<=?3 ORDER BY sequence DESC LIMIT ?4",
    "SELECT json FROM messages WHERE conversation=$1 AND sequence>=$2 AND sequence<=$3 ORDER BY sequence DESC LIMIT $4",
);
const PAGE_EDGES_SQL: crate::engine::database::Sql<'static> = sql(
    "SELECT EXISTS(SELECT 1 FROM messages WHERE conversation=?1 AND sequence<?2),EXISTS(SELECT 1 FROM messages WHERE conversation=?1 AND sequence>?3)",
    "SELECT EXISTS(SELECT 1 FROM messages WHERE conversation=$1 AND sequence<$2),EXISTS(SELECT 1 FROM messages WHERE conversation=$1 AND sequence>$3)",
);

// Count exact serialized bytes without allocating a temporary JSON buffer per row.
#[derive(Default)]
struct WireSize(usize);
impl std::io::Write for WireSize {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn wire_size(message: &Message) -> Result<usize, EngineError> {
    let mut size = WireSize::default();
    serde_json::to_writer(&mut size, message)?;
    Ok(size.0)
}

fn cache_shard(id: &str) -> usize {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hasher);
    usize::from(hasher.finish().to_le_bytes()[0]) % CACHE_SHARDS
}

#[derive(Debug)]
pub enum HistoryError {
    Changed,
    InvalidRange(String),
    Unavailable(String),
}
impl From<EngineError> for HistoryError {
    fn from(error: EngineError) -> Self {
        match error {
            EngineError::Invalid(message) => Self::InvalidRange(message),
            EngineError::Conflict(_) => Self::Changed,
            error => Self::Unavailable(error.message().to_owned()),
        }
    }
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

pub struct HistoryStore {
    readers: Vec<Mutex<Connection>>,
    next: AtomicUsize,
    cache: [Mutex<Cache>; CACHE_SHARDS],
    hits: AtomicU64,
    misses: AtomicU64,
    unread_cache: [Mutex<UnreadCache>; CACHE_SHARDS],
    unread_full: AtomicU64,
    unread_incremental: AtomicU64,
}
impl HistoryStore {
    pub fn ensure_available(&self) -> Result<(), EngineError> {
        if self.cache.iter().any(Mutex::is_poisoned)
            || self.unread_cache.iter().any(Mutex::is_poisoned)
            || self.readers.iter().any(Mutex::is_poisoned)
        {
            tracing::error!("History storage poisoned; restart the service.");
            return Err(EngineError::Unavailable(
                "History storage poisoned; restart the service.".into(),
            ));
        }
        Ok(())
    }
    pub fn open_backend(location: &str, memory: bool, postgres: bool) -> Result<Self, EngineError> {
        let mut readers = Vec::new();
        for _ in 0..4 {
            let flags = if memory {
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_URI
            } else {
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI
            };
            let db = if postgres {
                Connection::postgres(location, true)?
            } else {
                let db = Connection::open_with_flags(location, flags)?;
                db.busy_timeout(std::time::Duration::from_secs(2))?;
                db.execute_batch("PRAGMA query_only=ON; PRAGMA cache_size=-2048;")?;
                db
            };
            readers.push(Mutex::new(db));
        }
        Ok(Self {
            readers,
            next: AtomicUsize::new(0),
            cache: std::array::from_fn(|_| Mutex::new(Cache::default())),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            unread_cache: std::array::from_fn(|_| Mutex::new(UnreadCache::default())),
            unread_full: AtomicU64::new(0),
            unread_incremental: AtomicU64::new(0),
        })
    }
    pub fn read<T>(
        &self,
        query: impl FnOnce(&Connection) -> Result<T, EngineError>,
    ) -> Result<T, EngineError> {
        self.ensure_available()?;
        let index = self.next.fetch_add(1, Ordering::Relaxed) % self.readers.len();
        for offset in 0..self.readers.len() {
            if let Ok(db) = self.readers[(index + offset) % self.readers.len()].try_lock() {
                return query(&db);
            }
        }
        let db = self.readers[index].lock().map_err(|_| {
            EngineError::Unavailable("History storage poisoned; restart the service.".into())
        })?;
        query(&db)
    }
    pub(crate) fn has_author_in(
        &self,
        conversations: &[String],
        author: &str,
    ) -> Result<bool, EngineError> {
        if conversations.is_empty() {
            return Ok(false);
        }
        self.read(|db| {
            Ok(db.query_row(sql("SELECT EXISTS(SELECT 1 FROM json_each(?1) AS c JOIN messages AS m ON m.conversation=c.value WHERE json_extract(m.json,'$.from')=?2)", "SELECT EXISTS(SELECT 1 FROM jsonb_array_elements_text($1::text::jsonb) AS c JOIN messages AS m ON m.conversation=c.value WHERE (m.projection #>> '{from}')=$2)"), params![serde_json::to_string(conversations)?, author], |row| row.get(0))?)
        })
    }

    pub(crate) fn has_author(&self, id: &str, author: &str) -> Result<bool, EngineError> {
        self.read(|db| db.query_row(sql("SELECT EXISTS(SELECT 1 FROM messages WHERE conversation=?1 AND json_extract(json,'$.from')=?2)", "SELECT EXISTS(SELECT 1 FROM messages WHERE conversation=$1 AND (projection #>> '{from}')=$2)"), params![id, author], |row| row.get(0)).map_err(EngineError::unavailable))
    }
    pub(crate) fn contains_sequence(&self, id: &str, sequence: u64) -> Result<bool, EngineError> {
        self.read(|db| {
            db.query_row(
                sql(
                    "SELECT EXISTS(SELECT 1 FROM messages WHERE conversation=?1 AND sequence=?2)",
                    "SELECT EXISTS(SELECT 1 FROM messages WHERE conversation=$1 AND sequence=$2)",
                ),
                params![id, sequence],
                |r| r.get(0),
            )
            .map_err(EngineError::unavailable)
        })
    }
    pub fn stats(&self) -> Result<serde_json::Value, EngineError> {
        self.ensure_available()?;
        let (mut messages, mut bytes) = (0, 0);
        for shard in &self.cache {
            let cache = shard.lock().map_err(|_| {
                EngineError::Unavailable("History storage poisoned; restart the service.".into())
            })?;
            messages += cache.messages;
            bytes += cache.bytes;
        }
        Ok(
            serde_json::json!({"unread_full":self.unread_full.load(Ordering::Relaxed),"unread_incremental":self.unread_incremental.load(Ordering::Relaxed),"hits":self.hits.load(Ordering::Relaxed), "misses":self.misses.load(Ordering::Relaxed), "messages":messages,"accounted_bytes":bytes,"message_limit":CACHE_MESSAGES,"byte_limit":CACHE_BYTES,"per_conversation_limit":ROOM_CACHE_MESSAGES,"shards":CACHE_SHARDS}),
        )
    }
    pub fn locate(&self, message_id: &str) -> Result<Option<(String, Message)>, EngineError> {
        self.read(|db| {
            use crate::engine::database::OptionalExtension;
            let tx = db
                .unchecked_transaction()
                .map_err(EngineError::unavailable)?;
            let row: Option<(String, String)> = tx
                .query_row(
                    sql(
                        "SELECT conversation,json FROM messages WHERE json_extract(json,'$.id')=?1",
                        "SELECT conversation,json FROM messages WHERE (projection #>> '{id}')=$1",
                    ),
                    [message_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(EngineError::unavailable)?;
            row.map(|(id, raw)| {
                let mut message = serde_json::from_str(&raw).map_err(EngineError::unavailable)?;
                load_reactions(&tx, &id, &mut message)?;
                Ok((id, message))
            })
            .transpose()
        })
    }
}

pub(super) fn load_reactions(
    db: &Connection,
    id: &str,
    message: &mut Message,
) -> Result<(), EngineError> {
    let mut query = db
        .prepare_cached(sql(
            "SELECT reaction,username FROM reactions WHERE conversation=?1 AND sequence=?2",
            "SELECT reaction,username FROM reactions WHERE conversation=$1 AND sequence=$2",
        ))
        .map_err(EngineError::unavailable)?;
    let rows = query
        .query_map(params![id, message.sequence], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(EngineError::unavailable)?;
    for row in rows {
        let (reaction, user) = row.map_err(EngineError::unavailable)?;
        message.reactions.entry(reaction).or_default().insert(user);
    }
    Ok(())
}

fn load_page_reactions(
    db: &Connection,
    id: &str,
    messages: &mut [Message],
) -> Result<(), EngineError> {
    let Some(low) = messages.iter().map(|m| m.sequence).min() else {
        return Ok(());
    };
    let high = messages.iter().map(|m| m.sequence).max().unwrap_or(low);
    let mut positions: BTreeMap<u64, usize> = BTreeMap::new();
    for (index, message) in messages.iter().enumerate() {
        positions.insert(message.sequence, index);
    }
    let mut query = db.prepare_cached(sql("SELECT sequence,reaction,username FROM reactions WHERE conversation=?1 AND sequence BETWEEN ?2 AND ?3", "SELECT sequence,reaction,username FROM reactions WHERE conversation=$1 AND sequence BETWEEN $2 AND $3")).map_err(EngineError::unavailable)?;
    let rows = query
        .query_map(params![id, low, high], |r| {
            Ok((
                r.get::<_, u64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(EngineError::unavailable)?;
    for row in rows {
        let (sequence, reaction, user) = row.map_err(EngineError::unavailable)?;
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
#[allow(clippy::unwrap_used, clippy::panic)] // Deliberately poison each lock type.
#[path = "history/failures.rs"]
mod snapshot_failure_tests;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
