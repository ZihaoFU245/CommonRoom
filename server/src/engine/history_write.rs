//! Owned history deletion and audit state, published only after native commit.
use super::*;
use authorization::{Audit, Scope};
use database::{MetadataOp, MetadataWrite};

pub(crate) struct PendingHistory {
    database: database::Handle,
    targets: Vec<Target>,
    deletes: Vec<(String, u64)>,
    cleanups: Vec<(String, u64)>,
    audit: Audit,
}
struct Target {
    id: String,
    key: String,
    private: bool,
    revision: u64,
}
impl Engine {
    /// The caller holds conversation ordering and the shared audit permit until
    /// commit and publication. Legacy metadata writers hold the exclusive fence.
    pub(super) fn prepare_history_write(
        &self,
        user: &str,
        operation: &str,
        audit_target: &str,
        deletes: Vec<(String, u64)>,
        cleanups: Vec<(String, u64)>,
    ) -> Result<PendingHistory, EngineError> {
        let ids: BTreeSet<_> = deletes.iter().chain(&cleanups).map(|(id, _)| id).collect();
        let mut targets = Vec::with_capacity(ids.len());
        for id in ids {
            let view = self
                .bot_index
                .conversations
                .get(id)
                .ok_or("Conversation not found.")?;
            let private = view.starts_with("@private:");
            let key = view.strip_prefix("@private:").unwrap_or(view);
            let revision = if private {
                self.data.private.get(key).map(|chat| chat.revision)
            } else {
                self.data.rooms.get(key).map(|room| room.revision)
            }
            .ok_or("Conversation not found.")?;
            revision
                .checked_add(1)
                .ok_or("Conversation revision exhausted.")?;
            targets.push(Target {
                id: id.clone(),
                key: key.into(),
                private,
                revision,
            });
        }
        let audit = Audit {
            id: uuid::Uuid::new_v4().to_string(),
            time: now(),
            actor_id: self
                .principal_id(Some(user))
                .ok_or("Account unavailable.")?
                .into(),
            operation: operation.into(),
            target: audit_target.into(),
        };
        Ok(PendingHistory {
            database: self.store.db.clone(),
            targets,
            deletes,
            cleanups,
            audit,
        })
    }
    pub(crate) fn publish_history(
        &mut self,
        pending: PendingHistory,
    ) -> Result<CommandChange, EngineError> {
        for target in &pending.targets {
            let stored = if target.private {
                self.data
                    .private
                    .get_mut(&target.key)
                    .map(|chat| (&chat.id, &mut chat.revision))
            } else {
                self.data
                    .rooms
                    .get_mut(&target.key)
                    .map(|room| (&room.id, &mut room.revision))
            };
            let Some((_, revision)) =
                stored.filter(|(id, revision)| *id == &target.id && **revision == target.revision)
            else {
                self.recovery_failed = true;
                return Err(EngineError::Unavailable(
                    "Committed history metadata changed; restart the service.".into(),
                ));
            };
            *revision = target.revision + 1;
        }
        if self.data.policy.audit.len() >= 1000 {
            self.data.policy.audit.pop_front();
        }
        self.data.policy.audit.push_back(pending.audit);
        self.revision = self.revision.wrapping_add(1);
        let scopes = pending
            .targets
            .iter()
            .map(|target| {
                if target.private {
                    Scope::Private(target.id.clone())
                } else {
                    Scope::Room(target.id.clone())
                }
            })
            .collect();
        self.record_metadata(self.scope_notice(scopes));
        self.changed_conversation =
            (pending.targets.len() == 1).then(|| pending.targets[0].id.clone());
        Ok(match &self.changed_conversation {
            Some(id) => CommandChange::Conversation(id.clone()),
            None => CommandChange::Metadata(self.metadata_change()),
        })
    }
}
impl PendingHistory {
    pub(crate) async fn save(&self) -> Result<(), EngineError> {
        let mut batch = MetadataWrite::default();
        for target in &self.targets {
            batch.operations.push(MetadataOp::CheckedRevision {
                kind: if target.private { "private" } else { "room" },
                key: target.key.clone(),
                conversation: target.id.clone(),
                expected: target.revision,
                revision: target.revision + 1,
            });
        }
        batch.operations.push(MetadataOp::HistoryChanges {
            deletes: &self.deletes,
            cleanups: &self.cleanups,
        });
        batch.operations.push(MetadataOp::AppendAudit(&self.audit));
        self.database.save_metadata(&batch).await?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    #[tokio::test]
    async fn stale_history_revision_rejects_deletion_and_audit_together() {
        let mut engine = super::super::tests::engine();
        engine
            .execute(Some("alice"), Some("lobby"), "retained")
            .unwrap();
        let message = super::super::tests::room_messages(&engine, "lobby")[0].clone();
        let id = engine.data.rooms["lobby"].id.clone();
        let pending = engine
            .prepare_history_write(
                "alice",
                "/retract",
                &message.id,
                vec![(id, message.sequence)],
                vec![],
            )
            .unwrap();
        engine
            .execute(Some("alice"), Some("lobby"), "raced")
            .unwrap();
        let before = engine.data.clone();
        let audits: i64 = engine
            .store
            .db
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM audit_entries", [], |row| row.get(0))
            .unwrap();
        assert!(matches!(
            pending.save().await,
            Err(crate::engine::EngineError::Conflict(_))
        ));
        assert!(engine.data == before);
        assert_eq!(engine.history("alice", "lobby").unwrap().messages.len(), 2);
        assert_eq!(
            engine
                .store
                .db
                .lock()
                .unwrap()
                .query_row("SELECT count(*) FROM audit_entries", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            audits
        );
    }
}
