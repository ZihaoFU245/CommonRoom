//! Monotonic cursors and transactional room cursor renames.
use super::*;

impl Engine {
    /// Persist a cursor separately from message state. Caller updates its cache
    /// only after this succeeds; SQL keeps the cursor monotonic across devices.
    #[cfg(test)]
    pub(in crate::engine) fn store_read_position(
        &self,
        user: &str,
        key: &str,
        through: u64,
    ) -> Result<(), EngineError> {
        self.store
            .db
            .advance_cursor_blocking(&database::CursorAdvance {
                user,
                conversation: key,
                through,
            })?;
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
        let mut batch = MetadataWrite {
            operations: vec![MetadataOp::RenameConversationCursors {
                old: format!("room:{name}"),
                new: format!("room:{new_name}"),
            }],
        };
        write_delta_view(&mut batch, &self.data, before.view())?;
        self.store.db.save_metadata_blocking(&batch)?;
        Ok(())
    }
}
