//! Bounded revision-keyed history caches above the typed async database API.
use super::*;
mod cache;
mod page;
mod unread;
use cache::*;
#[cfg(test)]
use database::{PAGE_ASC_SQL, history_wire_size as wire_size};
pub use page::{HistoryRange, MessagePage};
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
    database: Arc<database::Database>,
    next: AtomicUsize,
    cache: [Mutex<Cache>; CACHE_SHARDS],
    page_fills: [tokio::sync::Mutex<()>; 64],
    hits: AtomicU64,
    misses: AtomicU64,
    unread_cache: [Mutex<UnreadCache>; CACHE_SHARDS],
    unread_full: AtomicU64,
    unread_incremental: AtomicU64,
}
impl HistoryStore {
    fn ensure_caches(&self) -> Result<(), EngineError> {
        self.database.ensure_state().map_err(|_| {
            EngineError::Unavailable("History storage unavailable; restart the service.".into())
        })?;
        if self.cache.iter().any(Mutex::is_poisoned)
            || self.unread_cache.iter().any(Mutex::is_poisoned)
        {
            return Err(EngineError::Unavailable(
                "History storage poisoned; restart the service.".into(),
            ));
        }
        Ok(())
    }
    pub(super) fn reset_after_recovery(&self) -> Result<(), EngineError> {
        for cache in &self.cache {
            cache
                .lock()
                .map_err(|_| EngineError::unavailable("History cache poisoned"))?
                .clear();
        }
        for cache in &self.unread_cache {
            *cache
                .lock()
                .map_err(|_| EngineError::unavailable("Unread cache poisoned"))? =
                UnreadCache::default();
        }
        Ok(())
    }
    #[cfg(test)]
    pub fn ensure_available(&self) -> Result<(), EngineError> {
        database::block_on_storage(self.ensure_available_async())?
    }
    pub(crate) async fn ensure_available_async(&self) -> Result<(), EngineError> {
        self.ensure_caches()?;
        self.database.ensure_available_async().await?;
        Ok(())
    }
    pub(crate) fn new(database: Arc<database::Database>) -> Self {
        Self {
            database,
            next: AtomicUsize::new(0),
            cache: std::array::from_fn(|_| Mutex::new(Cache::default())),
            page_fills: std::array::from_fn(|_| tokio::sync::Mutex::new(())),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            unread_cache: std::array::from_fn(|_| Mutex::new(UnreadCache::default())),
            unread_full: AtomicU64::new(0),
            unread_incremental: AtomicU64::new(0),
        }
    }
    /// Startup-only tuning; replacing shards evicts payloads without changing storage.
    pub(crate) fn configure_cache(
        &self,
        limits: &crate::config::HistoryCacheConfig,
    ) -> Result<(), EngineError> {
        let bytes = usize::try_from(limits.max_bytes).map_err(|_| {
            EngineError::unavailable("History cache byte limit exceeds this platform")
        })?;
        for shard in &self.cache {
            *shard
                .lock()
                .map_err(|_| EngineError::unavailable("History cache poisoned"))? =
                Cache::with_limits(bytes / CACHE_SHARDS, limits.max_messages / CACHE_SHARDS);
        }
        Ok(())
    }
    // Test compatibility releases cache guards before awaiting native storage.
    #[cfg(test)]
    fn query<T, F>(&self, future: F) -> Result<T, EngineError>
    where
        F: std::future::Future<Output = database::Result<T>> + Send,
        T: Send,
    {
        self.ensure_caches()?;
        self.next.fetch_add(1, Ordering::Relaxed);
        database::block_on_storage(future)?.map_err(EngineError::from)
    }
    #[cfg(test)]
    pub(crate) fn read<T>(
        &self,
        query: impl FnOnce(&Connection) -> Result<T, EngineError>,
    ) -> Result<T, EngineError> {
        self.ensure_available()?;
        self.next.fetch_add(1, Ordering::Relaxed);
        let reader = self.database.reader_lease()?;
        query(&reader)
    }
    #[cfg(test)]
    #[allow(clippy::unwrap_used)] // Tests intentionally saturate every native reader lease.
    fn hold_readers(&self) -> Vec<database::ReaderLease<'_>> {
        (0..4)
            .map(|_| self.database.reader_lease().unwrap())
            .collect()
    }
    #[cfg(test)]
    pub(crate) fn has_author_in(
        &self,
        conversations: &[String],
        author: &str,
    ) -> Result<bool, EngineError> {
        if conversations.is_empty() {
            return Ok(false);
        }
        self.query(self.database.history_has_author(conversations, author))
    }
    pub(crate) async fn has_author_in_async(
        &self,
        conversations: &[String],
        author: &str,
    ) -> Result<bool, EngineError> {
        if conversations.is_empty() {
            return Ok(false);
        }
        self.ensure_caches()?;
        self.next.fetch_add(1, Ordering::Relaxed);
        Ok(self
            .database
            .history_has_author(conversations, author)
            .await?)
    }
    #[cfg(test)]
    pub(crate) fn has_author(&self, id: &str, author: &str) -> Result<bool, EngineError> {
        self.has_author_in(&[id.into()], author)
    }
    #[cfg(test)]
    pub(crate) fn contains_sequence(&self, id: &str, sequence: u64) -> Result<bool, EngineError> {
        self.query(self.database.history_contains_sequence(id, sequence))
    }
    pub(crate) async fn contains_sequence_async(
        &self,
        id: &str,
        sequence: u64,
    ) -> Result<bool, EngineError> {
        self.ensure_caches()?;
        self.next.fetch_add(1, Ordering::Relaxed);
        Ok(self
            .database
            .history_contains_sequence(id, sequence)
            .await?)
    }
    #[cfg(test)]
    pub(crate) fn cleanup_counts(
        &self,
        ids: &[String],
        cutoff: u64,
    ) -> Result<BTreeMap<String, (usize, usize)>, EngineError> {
        self.query(self.database.history_cleanup_counts(ids, cutoff))
    }
    pub(crate) async fn cleanup_counts_async(
        &self,
        ids: &[String],
        cutoff: u64,
    ) -> Result<BTreeMap<String, (usize, usize)>, EngineError> {
        self.ensure_caches()?;
        self.next.fetch_add(1, Ordering::Relaxed);
        Ok(self.database.history_cleanup_counts(ids, cutoff).await?)
    }
    #[cfg(test)]
    pub fn locate(&self, message_id: &str) -> Result<Option<(String, Message)>, EngineError> {
        self.query(self.database.history_locate(message_id))
    }
    pub(crate) async fn locate_async(
        &self,
        message_id: &str,
    ) -> Result<Option<(String, Message)>, EngineError> {
        self.ensure_caches()?;
        self.next.fetch_add(1, Ordering::Relaxed);
        Ok(self.database.history_locate(message_id).await?)
    }
    #[cfg(test)]
    pub fn stats(&self) -> Result<serde_json::Value, EngineError> {
        self.ensure_available()?;
        self.cache_stats()
    }
    pub(crate) async fn stats_async(&self) -> Result<serde_json::Value, EngineError> {
        self.ensure_available_async().await?;
        self.cache_stats()
    }
    fn cache_stats(&self) -> Result<serde_json::Value, EngineError> {
        let (mut messages, mut bytes, mut message_limit, mut byte_limit) = (0, 0, 0, 0);
        for shard in &self.cache {
            let cache = shard.lock().map_err(|_| {
                EngineError::Unavailable("History storage poisoned; restart the service.".into())
            })?;
            messages += cache.messages;
            bytes += cache.bytes;
            message_limit += cache.message_limit;
            byte_limit += cache.byte_limit;
        }
        Ok(
            serde_json::json!({"unread_full":self.unread_full.load(Ordering::Relaxed),"unread_incremental":self.unread_incremental.load(Ordering::Relaxed),"hits":self.hits.load(Ordering::Relaxed),"misses":self.misses.load(Ordering::Relaxed),"messages":messages,"accounted_bytes":bytes,"message_limit":message_limit,"byte_limit":byte_limit,"per_conversation_limit":ROOM_CACHE_MESSAGES,"shards":CACHE_SHARDS,"pools":self.database.pool_stats()}),
        )
    }
}

#[cfg(test)]
pub(super) fn load_reactions(
    db: &Connection,
    id: &str,
    message: &mut Message,
) -> Result<(), EngineError> {
    let mut query = db
        .prepare_cached(if db.is_postgres() {
            "SELECT reaction,username FROM reactions WHERE conversation=$1 AND sequence=$2"
        } else {
            "SELECT reaction,username FROM reactions WHERE conversation=?1 AND sequence=?2"
        })
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)] // Deliberately poison cache/reader ownership.
#[path = "history/failures.rs"]
mod snapshot_failure_tests;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
