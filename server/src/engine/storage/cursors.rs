//! Monotonic cursors and transactional room cursor renames.
use super::*;

impl Engine {
    /// Persist a cursor separately from message state. Caller updates its cache
    /// only after this succeeds; SQL keeps the cursor monotonic across devices.
    pub(in crate::engine) fn store_read_position(
        &self,
        user: &str,
        key: &str,
        through: u64,
    ) -> Result<(), EngineError> {
        self.store.db.lock().map_err(EngineError::unavailable)?.execute("INSERT INTO read_positions VALUES(?1,?2,?3) ON CONFLICT(username,conversation) DO UPDATE SET sequence=MAX(sequence,excluded.sequence)",
            params![user,key,through])?;
        Ok(())
    }
    /// Room metadata and name-based read cursors move together; history and
    /// grants remain attached to the unchanged room ID.
    pub(in crate::engine) fn store_room_rename(
        &mut self,
        name: &str,
        new_name: &str,
        before: &MetadataChange,
    ) -> Result<(), EngineError> {
        let writer = self.store.db.clone();
        let mut db = writer.lock().map_err(EngineError::unavailable)?;
        let transaction = db.savepoint().map_err(EngineError::from)?;
        // A reused room name must not inherit another room's orphaned cursors.
        transaction
            .execute(
                "DELETE FROM read_positions WHERE conversation=?1",
                params![format!("room:{new_name}")],
            )
            .map_err(EngineError::from)?;
        transaction
            .execute(
                "UPDATE read_positions SET conversation=?1 WHERE conversation=?2",
                params![format!("room:{new_name}"), format!("room:{name}")],
            )
            .map_err(EngineError::from)?;
        write_delta_view(&transaction, &self.data, before.view())?;
        transaction.commit().map_err(EngineError::from)
    }
}
