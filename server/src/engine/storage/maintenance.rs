use super::*;

#[cfg(test)]
pub(in crate::engine) const CLEANUP_COUNTS_SQL: &str = database::CLEANUP_COUNTS_SQL;

impl Engine {
    /// One indexed range per target, batched on a single reader connection.
    /// The existing messages_time expression index covers the cutoff predicate.
    pub(in crate::engine) fn cleanup_counts(
        &self,
        ids: &[String],
        cutoff: u64,
    ) -> Result<BTreeMap<String, (usize, usize)>, EngineError> {
        self.history_store().cleanup_counts(ids, cutoff)
    }
    pub(in crate::engine) fn save_history_maintenance(&mut self) -> Result<(), EngineError> {
        if self.store.pending_deletes.is_empty() && self.store.pending_cleanups.is_empty() {
            return Ok(());
        }
        let ids: BTreeSet<_> = self
            .store
            .pending_deletes
            .iter()
            .chain(self.store.pending_cleanups.iter())
            .map(|(id, _)| id.clone())
            .collect();
        let scopes = ids
            .iter()
            .filter_map(|id| {
                self.bot_index.conversations.get(id).map(|view| {
                    if view.starts_with("@private:") {
                        authorization::Scope::Private(id.clone())
                    } else {
                        authorization::Scope::Room(id.clone())
                    }
                })
            })
            .collect();
        let notice = self.scope_notice(scopes);
        let mut batch = MetadataWrite {
            operations: vec![MetadataOp::HistoryChanges {
                deletes: &self.store.pending_deletes,
                cleanups: &self.store.pending_cleanups,
            }],
        };
        for (kind, key, id, revision) in self
            .data
            .rooms
            .iter()
            .map(|(key, r)| ("room", key, &r.id, r.revision))
            .chain(
                self.data
                    .private
                    .iter()
                    .map(|(key, c)| ("private", key, &c.id, c.revision)),
            )
        {
            if ids.contains(id) {
                batch.operations.push(MetadataOp::Revision {
                    expected: revision.saturating_sub(1),
                    kind,
                    key: key.clone(),
                    revision,
                });
            }
        }
        write_audit(&mut batch, &self.data);
        self.store.db.save_metadata_blocking(&batch)?;
        drop(batch);
        self.revision = self.revision.wrapping_add(1);
        self.record_metadata(notice);
        if ids.len() == 1 {
            self.changed_conversation = ids.into_iter().next();
        }
        Ok(())
    }
}
