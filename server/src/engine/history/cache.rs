//! Bounded page and unread caches.
use super::*;

pub(super) struct CachedPage {
    pub(super) conversation: String,
    pub(super) revision: u64,
    pub(super) range: HistoryRange,
    pub(super) messages: MessagePage,
    pub(super) bytes: usize,
    pub(super) before: bool,
    pub(super) after: bool,
}
impl CachedPage {
    pub(super) fn reuse(
        &self,
        range: HistoryRange,
        limit: usize,
    ) -> Option<(MessagePage, bool, bool, bool)> {
        if self.range == range
            || range.is_tail()
                && self.range.is_tail()
                && !self.after
                && (self.messages.len() >= limit || !self.before)
        {
            return Some((
                self.messages.latest(limit),
                self.before || self.messages.len() > limit,
                self.after,
                false,
            ));
        }
        // Reuse a fully covered bounded interval without creating another cache
        // entry or copying message bodies. Partial/around ranges still use SQL.
        let (lower, upper) = range.start.zip(range.end)?;
        if range.before.is_some()
            || range.after.is_some()
            || range.around.is_some()
            || self.messages.first()?.sequence > lower
            || self.messages.last()?.sequence < upper
        {
            return None;
        }
        let start = self
            .messages
            .partition_point(|message| message.sequence < lower);
        let end = self
            .messages
            .partition_point(|message| message.sequence <= upper)
            .min(start.saturating_add(limit));
        if start >= end {
            return None;
        }
        Some((
            self.messages.slice(start, end),
            self.before || start > 0,
            self.after || end < self.messages.len(),
            true,
        ))
    }
}
#[derive(Default)]
pub(super) struct CacheMetrics {
    pub(super) cold_misses: u64,
    pub(super) revision_misses: u64,
    pub(super) range_misses: u64,
    pub(super) coalesced_hits: u64,
    pub(super) subset_hits: u64,
    pub(super) tail_updates: u64,
    pub(super) updated_pages: u64,
    pub(super) carried_pages: u64,
    pub(super) invalidated_pages: u64,
    pub(super) oversized_skips: u64,
    pub(super) stale_fills: u64,
    pub(super) byte_evictions: u64,
    pub(super) message_evictions: u64,
    pub(super) page_evictions: u64,
    pub(super) conversation_evictions: u64,
}
impl CacheMetrics {
    pub(super) fn stats(&self) -> serde_json::Value {
        serde_json::json!({"cold_misses":self.cold_misses,"revision_misses":self.revision_misses,
            "range_misses":self.range_misses,"coalesced_hits":self.coalesced_hits,"subset_hits":self.subset_hits,
            "tail_updates":self.tail_updates,"updated_pages":self.updated_pages,"carried_pages":self.carried_pages,"invalidated_pages":self.invalidated_pages,
            "oversized_skips":self.oversized_skips,"stale_fills":self.stale_fills,"byte_evictions":self.byte_evictions,
            "message_evictions":self.message_evictions,"page_evictions":self.page_evictions,"conversation_evictions":self.conversation_evictions})
    }
}
pub(super) struct Cache {
    pub(super) pages: VecDeque<CachedPage>,
    pub(super) messages: usize,
    pub(super) bytes: usize,
    pub(super) byte_limit: usize,
    pub(super) message_limit: usize,
    pub(super) metrics: CacheMetrics,
}
impl Default for Cache {
    fn default() -> Self {
        Self::with_limits(CACHE_BYTES / CACHE_SHARDS, CACHE_MESSAGES / CACHE_SHARDS)
    }
}
impl Cache {
    pub(super) fn with_limits(byte_limit: usize, message_limit: usize) -> Self {
        Self {
            pages: VecDeque::new(),
            messages: 0,
            bytes: 0,
            byte_limit,
            message_limit,
            metrics: CacheMetrics::default(),
        }
    }
    pub(super) fn clear(&mut self) {
        self.metrics.invalidated_pages += u64::try_from(self.pages.len()).unwrap_or(u64::MAX);
        self.pages.clear();
        self.bytes = 0;
        self.messages = 0;
    }
    pub(super) fn remove(&mut self, index: usize) {
        if let Some(page) = self.pages.remove(index) {
            self.messages -= page.messages.len();
            self.bytes -= page.bytes;
        }
    }
    pub(super) fn insert(&mut self, page: CachedPage) {
        if page.bytes > self.byte_limit || page.messages.len() > self.message_limit {
            self.metrics.oversized_skips += 1;
            return;
        }
        let id = page.conversation.clone();
        let revision = page.revision;
        // An older reader can finish after a newer revision has filled the cache.
        if self
            .pages
            .iter()
            .any(|cached| cached.conversation == id && cached.revision > revision)
        {
            self.metrics.stale_fills += 1;
            return;
        }
        let mut index = 0;
        while index < self.pages.len() {
            if self.pages[index].conversation == id && self.pages[index].revision != revision {
                self.remove(index);
                self.metrics.invalidated_pages += 1;
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
                self.metrics.conversation_evictions += 1;
            } else {
                break;
            }
        }
        while self.messages > self.message_limit
            || self.bytes > self.byte_limit
            || self.pages.len() > 256 / CACHE_SHARDS
        {
            if self.bytes > self.byte_limit {
                self.metrics.byte_evictions += 1;
            } else if self.messages > self.message_limit {
                self.metrics.message_evictions += 1;
            } else {
                self.metrics.page_evictions += 1;
            }
            self.remove(0);
        }
    }
}
#[derive(Clone)]
pub(super) struct UnreadEntry {
    pub(super) read: u64,
    pub(super) revision: u64,
    pub(super) unread: Unread,
    pub(super) used: u64,
}
#[derive(Default)]
pub(super) struct UnreadCache {
    pub(super) entries:
        std::collections::HashMap<String, std::collections::HashMap<String, UnreadEntry>>,
    pub(super) recency: BTreeMap<u64, (String, String)>,
    pub(super) clock: u64,
    pub(super) hits: u64,
    pub(super) misses: u64,
    pub(super) evictions: u64,
    pub(super) carried: u64,
}
impl UnreadCache {
    pub(super) fn get(&mut self, id: &str, user: &str, read: u64, revision: u64) -> Option<Unread> {
        let Some(entry) = self
            .entries
            .get_mut(id)
            .and_then(|users| users.get_mut(user))
        else {
            self.misses += 1;
            return None;
        };
        if entry.read != read || entry.revision != revision {
            self.misses += 1;
            return None;
        }
        let key = self.recency.remove(&entry.used)?;
        self.hits += 1;
        self.clock += 1;
        entry.used = self.clock;
        self.recency.insert(entry.used, key);
        Some(entry.unread.clone())
    }
    pub(super) fn insert(
        &mut self,
        id: &str,
        user: &str,
        read: u64,
        revision: u64,
        unread: Unread,
    ) {
        let users = self.entries.entry(id.into()).or_default();
        if users
            .get(user)
            .is_some_and(|entry| entry.revision > revision || entry.read > read)
        {
            return;
        }
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
        while self.recency.len() > UNREAD_ENTRIES_PER_SHARD {
            if let Some((_, (id, user))) = self.recency.pop_first()
                && let Some(users) = self.entries.get_mut(&id)
            {
                users.remove(&user);
                self.evictions += 1;
                if users.is_empty() {
                    self.entries.remove(&id);
                }
            }
        }
    }
}
