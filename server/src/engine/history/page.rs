//! Validated history ranges and revision-keyed immutable pages.
use super::*;

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
// Responses and cache entries share immutable messages and exact wire-size accounting.
#[derive(Default)]
pub(super) struct PageData {
    messages: Vec<Message>,
    sizes: Vec<usize>,
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
            data: Arc::new(PageData { messages, sizes }),
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
        serde::Serialize::serialize(&**self, serializer)
    }
}

impl HistoryStore {
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
            if bytes <= CACHE_BYTES / CACHE_SHARDS {
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
            }
            return Ok(result);
        }
    }
}
