//! Shared cached unread projections above typed native reader snapshots.
use super::*;

impl HistoryStore {
    #[cfg(test)]
    pub fn unread(
        &self,
        id: &str,
        user: &str,
        read: u64,
        revision: u64,
    ) -> Result<Unread, EngineError> {
        self.unread_batch(user, [(id, read, revision)])?
            .into_iter()
            .next()
            .ok_or_else(|| EngineError::Internal("Missing unread result.".into()))?
    }
    #[cfg(test)]
    pub(in crate::engine) fn unread_batch<'a>(
        &self,
        user: &str,
        requests: impl IntoIterator<Item = (&'a str, u64, u64)>,
    ) -> Result<Vec<Result<Unread, EngineError>>, EngineError> {
        let requests: Vec<_> = requests.into_iter().collect();
        database::block_on_storage(self.unread_batch_async(user, &requests))?
    }
    pub(crate) async fn unread_batch_async(
        &self,
        user: &str,
        requests: &[(&str, u64, u64)],
    ) -> Result<Vec<Result<Unread, EngineError>>, EngineError> {
        self.ensure_caches()?;
        let mut results = Vec::new();
        let mut misses = Vec::new();
        for &(id, read, revision) in requests {
            let mut cache = self.unread_cache[cache_shard(id)]
                .lock()
                .map_err(|_| EngineError::Unavailable("Unread cache unavailable.".into()))?;
            let cached = cache.get(id, user, read, revision);
            if cached.is_none() {
                let previous =
                    cache
                        .entries
                        .get(id)
                        .and_then(|users| users.get(user))
                        .map(|entry| database::UnreadHint {
                            read: entry.read,
                            unread: entry.unread.clone(),
                        });
                misses.push((
                    results.len(),
                    database::UnreadRead {
                        conversation: id,
                        read,
                        revision,
                        previous,
                    },
                ));
            }
            results.push(cached.map(Ok));
        }
        if misses.is_empty() {
            self.database.ensure_available_async().await?;
        } else {
            let (indices, queries): (Vec<_>, Vec<_>) = misses.into_iter().unzip();
            self.next.fetch_add(1, Ordering::Relaxed);
            let loaded = self.database.history_unreads(user, &queries).await?;
            self.unread_full.fetch_add(loaded.full, Ordering::Relaxed);
            self.unread_incremental
                .fetch_add(loaded.incremental, Ordering::Relaxed);
            for ((index, query), result) in indices.into_iter().zip(queries).zip(loaded.results) {
                if let Ok(unread) = &result {
                    self.unread_cache[cache_shard(query.conversation)]
                        .lock()
                        .map_err(|_| EngineError::Unavailable("Unread cache unavailable.".into()))?
                        .insert(
                            query.conversation,
                            user,
                            query.read,
                            query.revision,
                            unread.clone(),
                        );
                }
                results[index] = Some(result.map_err(EngineError::from));
            }
        }
        results
            .into_iter()
            .map(|result| {
                result.ok_or_else(|| EngineError::Internal("Missing unread result.".into()))
            })
            .collect()
    }
}
