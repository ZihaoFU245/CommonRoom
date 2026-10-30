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
}
