//! Cached and incremental unread projections, batched on one reader snapshot.
use super::*;

impl HistoryStore {
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

    pub(in crate::engine) fn unread_batch<'a>(
        &self,
        user: &str,
        requests: impl IntoIterator<Item = (&'a str, u64, u64)>,
    ) -> Result<Vec<Result<Unread, EngineError>>, EngineError> {
        self.ensure_available()?;
        let mut results = Vec::new();
        let mut misses = Vec::new();
        for (id, read, revision) in requests {
            let mut cache = self.unread_cache[cache_shard(id)]
                .lock()
                .map_err(|_| EngineError::Unavailable("Unread cache unavailable.".into()))?;
            let cached = cache.get(id, user, read, revision);
            if cached.is_none() {
                let previous = cache
                    .entries
                    .get(id)
                    .and_then(|users| users.get(user))
                    .cloned();
                misses.push((results.len(), id, read, revision, previous));
            }
            results.push(cached.map(Ok));
        }
        if !misses.is_empty() {
            let loaded = self.read(|db| {
                let tx = db.unchecked_transaction()?;
                Ok(misses
                    .iter()
                    .map(|(_, id, read, revision, previous)| {
                        self.query_unread(&tx, id, user, *read, *revision, previous.clone())
                    })
                    .collect::<Vec<_>>())
            })?;
            for ((index, id, read, revision, _), result) in misses.into_iter().zip(loaded) {
                if let Ok(unread) = &result {
                    self.unread_cache[cache_shard(id)]
                        .lock()
                        .map_err(|_| EngineError::Unavailable("Unread cache unavailable.".into()))?
                        .insert(id, user, read, revision, unread.clone());
                }
                results[index] = Some(result);
            }
        }
        results
            .into_iter()
            .map(|result| {
                result.ok_or_else(|| EngineError::Internal("Missing unread result.".into()))
            })
            .collect()
    }

    fn query_unread(
        &self,
        db: &Connection,
        id: &str,
        user: &str,
        read: u64,
        revision: u64,
        previous: Option<UnreadEntry>,
    ) -> Result<Unread, EngineError> {
        let (oldest, through): (u64, u64) = db.prepare_cached(
                sql("SELECT coalesce((SELECT oldest FROM retention WHERE conversation=?1),0),coalesce((SELECT sequence FROM messages WHERE conversation=?1 ORDER BY sequence DESC LIMIT 1),0)", "SELECT coalesce((SELECT oldest FROM retention WHERE conversation=$1),0),coalesce((SELECT sequence FROM messages WHERE conversation=$1 ORDER BY sequence DESC LIMIT 1),0)")
                )?.query_row([id], |row| Ok((row.get(0)?,row.get(1)?)),
            ).map_err(EngineError::unavailable)?;
        if let Some(previous) = previous.filter(|previous| {
            read >= previous.read
                && through >= previous.unread.through
                && (oldest == previous.unread.oldest || previous.unread.through == 0)
        }) {
            // Every append increments the conversation revision once. Matching
            // the exact number of new rows proves there was no intervening
            // edit, reaction, cleanup or retraction. Global sequence gaps do
            // not affect this proof. Retention moving the oldest row falls
            // back to the complete query.
            let (appended, added): (u64, usize) = db.prepare_cached(
                    sql("SELECT count(*),coalesce(sum(sequence>?3 AND json_extract(json,'$.from')!=?4),0) FROM messages WHERE conversation=?1 AND sequence>?2", "SELECT count(*),coalesce(count(*) FILTER (WHERE sequence>$3 AND (projection #>> '{from}')!=$4),0) FROM messages WHERE conversation=$1 AND sequence>$2")
                    )?.query_row(params![id,previous.unread.through,read,user], |row| Ok((row.get(0)?,row.get(1)?)),
                ).map_err(EngineError::unavailable)?;
            if revision.wrapping_sub(previous.revision) == appended {
                let removed: usize = if read == previous.read {
                    0
                } else {
                    db.prepare_cached(
                            sql("SELECT count(*) FROM messages WHERE conversation=?1 AND sequence>?2 AND sequence<=?3 AND json_extract(json,'$.from')!=?4", "SELECT count(*) FROM messages WHERE conversation=$1 AND sequence>$2 AND sequence<=$3 AND (projection #>> '{from}')!=$4")
                            )?.query_row(params![id,previous.read,read.min(previous.unread.through),user], |row| row.get(0),
                        ).map_err(EngineError::unavailable)?
                };
                if let Some(count) = previous
                    .unread
                    .count
                    .checked_sub(removed)
                    .and_then(|count| count.checked_add(added))
                {
                    let first = if count == 0 {
                        None
                    } else if previous.unread.first.is_some_and(|first| first > read) {
                        previous.unread.first
                    } else {
                        db.prepare_cached(
                                sql("SELECT min(sequence) FROM messages WHERE conversation=?1 AND sequence>?2 AND json_extract(json,'$.from')!=?3", "SELECT min(sequence) FROM messages WHERE conversation=$1 AND sequence>$2 AND (projection #>> '{from}')!=$3")
                                )?.query_row(params![id,read,user], |row|row.get(0),
                            ).map_err(EngineError::unavailable)?
                    };
                    self.unread_incremental.fetch_add(1, Ordering::Relaxed);
                    return Ok(Unread {
                        count,
                        first,
                        oldest,
                        through,
                        revision,
                    });
                }
            }
        }
        let (count,first) = db.prepare_cached(
                sql("SELECT count(*),min(sequence) FROM messages WHERE conversation=?1 AND sequence>?2 AND json_extract(json,'$.from')!=?3", "SELECT count(*),min(sequence) FROM messages WHERE conversation=$1 AND sequence>$2 AND (projection #>> '{from}')!=$3")
                )?.query_row(params![id,read,user], |row|Ok((row.get(0)?,row.get(1)?)),
            ).map_err(EngineError::unavailable)?;
        self.unread_full.fetch_add(1, Ordering::Relaxed);
        Ok(Unread {
            count,
            first,
            oldest,
            through,
            revision,
        })
    }
}
