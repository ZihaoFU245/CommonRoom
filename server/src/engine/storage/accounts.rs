//! Account identity changes are one typed metadata transaction.
use super::*;

impl Engine {
    pub(in crate::engine) fn account_reference_ids(
        &self,
        name: &str,
    ) -> Result<BTreeSet<String>, EngineError> {
        let db = self.store.db.lock().map_err(EngineError::unavailable)?;
        Ok(db.account_conversations(name)?)
    }
    /// Rename account state and every affected read cursor in one transaction.
    pub(in crate::engine) fn store_account_rename(
        &mut self,
        name: &str,
        new_name: &str,
        conversations: &BTreeMap<String, String>,
        affected: &BTreeSet<String>,
        before: &MetadataChange,
    ) -> Result<(), EngineError> {
        let mut batch = MetadataWrite {
            operations: vec![MetadataOp::RenameAccountCursors {
                old: name.into(),
                new: new_name.into(),
            }],
        };
        for (old, new) in conversations {
            batch
                .operations
                .push(MetadataOp::RenameConversationCursors {
                    old: old.clone(),
                    new: new.clone(),
                });
        }
        batch.operations.push(MetadataOp::RewriteAccount {
            old: name.into(),
            new: Some(new_name.into()),
        });
        for (id, revision) in self
            .data
            .rooms
            .values_mut()
            .map(|r| (&r.id, &mut r.revision))
            .chain(
                self.data
                    .private
                    .values_mut()
                    .map(|c| (&c.id, &mut c.revision)),
            )
        {
            if affected.contains(id) {
                *revision = revision.wrapping_add(1);
            }
        }
        write_delta_view(&mut batch, &self.data, before.view())?;
        self.store.db.save_metadata_blocking(&batch)?;
        Ok(())
    }
    /// Account deletion and its cursor cleanup must commit atomically.
    pub(in crate::engine) fn store_account_deletion(
        &mut self,
        name: &str,
        removed: &BTreeSet<String>,
        before: &MetadataChange,
    ) -> Result<(), EngineError> {
        let mut batch = MetadataWrite {
            operations: vec![MetadataOp::RewriteAccount {
                old: name.into(),
                new: None,
            }],
        };
        write_delta_view(&mut batch, &self.data, before.view())?;
        batch
            .operations
            .push(MetadataOp::RemoveAccountCursors { user: name.into() });
        for key in removed {
            batch
                .operations
                .push(MetadataOp::RemoveConversationCursors {
                    conversation: key.clone(),
                });
        }
        self.store.db.save_metadata_blocking(&batch)?;
        Ok(())
    }
}
