//! Refresh immutable cached pages only after a known committed message is
//! published. Metadata/deletion revisions still invalidate through normal reads.
use super::*;

impl HistoryStore {
    pub(crate) fn publish_committed(
        &self,
        id: &str,
        previous: u64,
        revision: u64,
        message: &Message,
        append: bool,
        retention: usize,
    ) -> Result<(), EngineError> {
        if previous.checked_add(1) != Some(revision) {
            return Ok(());
        }
        let shard = cache_shard(id);
        {
            let mut cache = self.cache[shard]
                .lock()
                .map_err(|_| EngineError::unavailable("History cache poisoned"))?;
            let mut pages = Vec::new();
            let mut index = 0;
            while index < cache.pages.len() {
                if cache.pages[index].conversation == id && cache.pages[index].revision < revision {
                    if let Some(page) = cache.pages.remove(index) {
                        cache.bytes -= page.bytes;
                        cache.messages -= page.messages.len();
                        pages.push(page);
                    }
                } else {
                    index += 1;
                }
            }
            let mut changed_size = None;
            for mut page in pages {
                if page.revision != previous
                    || append
                        && (!page.range.is_tail()
                            || page.after
                            || page
                                .messages
                                .last()
                                .is_some_and(|old| old.sequence >= message.sequence))
                {
                    cache.metrics.invalidated_pages += 1;
                    continue;
                }
                let position = page
                    .messages
                    .iter()
                    .position(|old| old.sequence == message.sequence);
                if !append && position.is_none() {
                    // Edits/reactions do not affect other messages or bounds.
                    page.revision = revision;
                    cache.metrics.carried_pages += 1;
                    cache.insert(page);
                    continue;
                }
                let size = match changed_size {
                    Some(size) => size,
                    None => {
                        let size = wire_size(message)?;
                        changed_size = Some(size);
                        size
                    }
                };
                let mut messages = page.messages.to_vec();
                let mut sizes = page.messages.wire_sizes().to_vec();
                if append {
                    messages.push(message.clone());
                    sizes.push(size);
                } else if let Some(position) = position {
                    messages[position] = message.clone();
                    sizes[position] = size;
                }
                let descending = page.range.start.is_none()
                    && page.range.after.is_none()
                    && page.range.around.is_none();
                let limit = page.range.limit.unwrap_or(PAGE_SIZE).min(if append {
                    retention
                } else {
                    PAGE_SIZE
                });
                let (mut start, mut end) = (0, messages.len());
                let mut bytes = 0usize;
                if descending {
                    start = end;
                    while start > 0 && end - start < limit {
                        let size = sizes[start - 1];
                        if start < end && bytes.saturating_add(size) > PAGE_BYTES {
                            break;
                        }
                        bytes = bytes.saturating_add(size);
                        start -= 1;
                    }
                } else {
                    end = 0;
                    while end < messages.len() && end < limit {
                        let size = sizes[end];
                        if end > 0 && bytes.saturating_add(size) > PAGE_BYTES {
                            break;
                        }
                        bytes = bytes.saturating_add(size);
                        end += 1;
                    }
                }
                page.before = (page.before || start > 0) && (!append || end - start < retention);
                page.after |= end < messages.len();
                messages.truncate(end);
                sizes.truncate(end);
                if start > 0 {
                    messages.drain(..start);
                    sizes.drain(..start);
                }
                page.messages = MessagePage::new(messages, sizes);
                page.bytes = page.messages.accounted_bytes();
                page.revision = revision;
                cache.metrics.updated_pages += 1;
                if append {
                    cache.metrics.tail_updates += 1;
                }
                cache.insert(page);
            }
        }
        if !append {
            // Text edits and reactions preserve sequence/author and therefore
            // every user's unread count, first unread and retention coordinates.
            let mut cache = self.unread_cache[shard]
                .lock()
                .map_err(|_| EngineError::unavailable("Unread cache poisoned"))?;
            let mut carried = 0;
            if let Some(users) = cache.entries.get_mut(id) {
                for entry in users
                    .values_mut()
                    .filter(|entry| entry.revision == previous)
                {
                    entry.revision = revision;
                    entry.unread.revision = revision;
                    carried += 1;
                }
            }
            cache.carried += carried;
        }
        Ok(())
    }
}
