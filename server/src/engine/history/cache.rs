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
#[derive(Default)]
pub(super) struct Cache {
    pub(super) pages: VecDeque<CachedPage>,
    pub(super) messages: usize,
    pub(super) bytes: usize,
}
impl Cache {
    pub(super) fn remove(&mut self, index: usize) {
        if let Some(page) = self.pages.remove(index) {
            self.messages -= page.messages.len();
            self.bytes -= page.bytes;
        }
    }
    pub(super) fn insert(&mut self, page: CachedPage) {
        let id = page.conversation.clone();
        let revision = page.revision;
        // An older reader can finish after a newer revision has filled the cache.
        if self
            .pages
            .iter()
            .any(|cached| cached.conversation == id && cached.revision > revision)
        {
            return;
        }
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
        while self.messages > CACHE_MESSAGES / CACHE_SHARDS
            || self.bytes > CACHE_BYTES / CACHE_SHARDS
            || self.pages.len() > 256 / CACHE_SHARDS
        {
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
}
impl UnreadCache {
    pub(super) fn get(&mut self, id: &str, user: &str, read: u64, revision: u64) -> Option<Unread> {
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
                if users.is_empty() {
                    self.entries.remove(&id);
                }
            }
        }
    }
}
