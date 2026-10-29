//! Validated history ranges and revision-keyed immutable pages.
use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Deserialize)]
pub struct HistoryRange {
    pub start: Option<u64>,
    pub around: Option<u64>,
    pub end: Option<u64>,
    pub before: Option<u64>,
    pub after: Option<u64>,
    pub limit: Option<usize>,
}
impl HistoryRange {
    pub fn validate(&self) -> Result<usize, EngineError> {
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
type EncodedPage = Arc<std::sync::OnceLock<Result<Box<serde_json::value::RawValue>, String>>>;

// Responses and cache entries share immutable messages and exact wire-size accounting.
#[derive(Default)]
pub(super) struct PageData {
    messages: Vec<Message>,
    sizes: Vec<usize>,
    // Shared content is encoded once across authorized viewers. Two ranges
    // bound retained wire bytes within the page cache's existing accounting.
    encoded: Mutex<BTreeMap<(usize, usize), EncodedPage>>,
}
#[derive(Clone, Default)]
pub struct MessagePage {
    pub(super) data: Arc<PageData>,
    start: usize,
    end: usize,
}
impl MessagePage {
    pub(super) fn new(messages: Vec<Message>, sizes: Vec<usize>) -> Self {
        let end = messages.len();
        Self {
            data: Arc::new(PageData {
                messages,
                sizes,
                encoded: Mutex::default(),
            }),
            start: 0,
            end,
        }
    }
    pub(super) fn latest(&self, limit: usize) -> Self {
        Self {
            data: self.data.clone(),
            start: self.end.saturating_sub(limit).max(self.start),
            end: self.end,
        }
    }
    pub(in crate::engine) fn bounded_tail(&self, count: &mut usize, bytes: &mut usize) -> Self {
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
    #[cfg(test)]
    pub(in crate::engine) fn shares_storage(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.data, &other.data)
    }
    #[cfg(test)]
    pub(in crate::engine) fn encoded_range_count(&self) -> usize {
        self.data
            .encoded
            .lock()
            .map(|entries| entries.len())
            .unwrap_or(0)
    }
    pub(super) fn accounted_bytes(&self) -> usize {
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
        use serde::ser::Error as _;
        let cell = {
            let mut encoded = self
                .data
                .encoded
                .lock()
                .map_err(|_| S::Error::custom("Page encoding cache unavailable"))?;
            let key = (self.start, self.end);
            if let Some(value) = encoded.get(&key) {
                Some(value.clone())
            } else if encoded.len() < 2 {
                let value = Arc::new(std::sync::OnceLock::new());
                encoded.insert(key, value.clone());
                Some(value)
            } else {
                None
            }
        };
        if let Some(cell) = cell {
            let raw = cell.get_or_init(|| {
                serde_json::value::to_raw_value(&**self)
                    .map_err(|_| "Cannot encode message page".into())
            });
            return raw
                .as_ref()
                .map_err(S::Error::custom)?
                .serialize(serializer);
        }
        serde::Serialize::serialize(&**self, serializer)
    }
}

impl HistoryStore {
    #[cfg(test)]
    pub fn page(
        &self,
        id: &str,
        revision: u64,
        range: HistoryRange,
        view: &str,
    ) -> Result<History, EngineError> {
        self.page_checked(id, revision, range, view)
            .map_err(EngineError::from)
    }
    #[cfg(test)]
    pub fn page_checked(
        &self,
        id: &str,
        revision: u64,
        range: HistoryRange,
        view: &str,
    ) -> Result<History, HistoryError> {
        database::block_on_storage(self.page_checked_async(id, revision, range, view))
            .map_err(EngineError::from)?
    }
    pub(crate) async fn page_checked_async(
        &self,
        id: &str,
        revision: u64,
        mut range: HistoryRange,
        view: &str,
    ) -> Result<History, HistoryError> {
        self.ensure_caches()?;
        let mut fill = None;
        loop {
            let limit = range.validate().map_err(HistoryError::from)?;
            let cached = {
                let mut cache = self.cache[cache_shard(id)]
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
                        name_colors: None,
                        view: view.into(),
                        revision,
                        messages: page.messages.latest(limit),
                        has_before: page.before || page.messages.len() > limit,
                        has_after: page.after,
                    };
                    cache.pages.push_back(page);
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    Some(result)
                } else {
                    None
                }
            };
            if let Some(result) = cached {
                // Hits do not acquire a reader, so they validate ownership here.
                // Misses validate once when acquiring the native reader lease.
                self.database
                    .ensure_available_async()
                    .await
                    .map_err(EngineError::from)?;
                return Ok(result);
            }
            if fill.is_none() {
                use std::hash::{Hash, Hasher};
                let mut hash = std::hash::DefaultHasher::new();
                id.hash(&mut hash);
                revision.hash(&mut hash);
                range.hash(&mut hash);
                let shard = usize::from(hash.finish().to_le_bytes()[0]) % self.page_fills.len();
                fill = Some(self.page_fills[shard].lock().await);
                // A concurrent viewer may have filled this immutable revision.
                continue;
            }
            self.misses.fetch_add(1, Ordering::Relaxed);
            self.next.fetch_add(1, Ordering::Relaxed);
            let page = self
                .database
                .history_page(&database::PageRead {
                    conversation: id,
                    revision,
                    range,
                    limit,
                    max_bytes: PAGE_BYTES,
                })
                .await
                .map_err(EngineError::from)?;
            let result = History {
                name_colors: None,
                view: view.into(),
                revision,
                messages: MessagePage::new(page.messages, page.sizes),
                has_before: page.has_before,
                has_after: page.has_after,
            };
            // Large preceding rows must not prevent around from reaching its anchor.
            if let Some(around) = range.around
                && result.has_after
                && result.messages.last().is_some_and(|m| m.sequence < around)
            {
                range = HistoryRange {
                    start: Some(around),
                    limit: range.limit,
                    ..Default::default()
                };
                continue;
            }
            let bytes = result.messages.accounted_bytes();
            self.cache[cache_shard(id)]
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
            return Ok(result);
        }
    }
}
