//! Per-conversation retained-row counters and pruning.
use super::*;

impl Engine {
    #[cfg(test)]
    pub fn set_limits(
        &mut self,
        max_users: usize,
        max_rooms: usize,
        max_messages: usize,
    ) -> Result<(), EngineError> {
        database::block_on_storage(self.set_limits_async(max_users, max_rooms, max_messages))?
    }
    pub(crate) async fn set_limits_async(
        &mut self,
        max_users: usize,
        max_rooms: usize,
        max_messages: usize,
    ) -> Result<(), EngineError> {
        crate::config::validate_limits(max_users, max_rooms, max_messages)
            .map_err(EngineError::Invalid)?;
        let mut batch = MetadataWrite::default();
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
            batch.operations.push(MetadataOp::Prune {
                kind,
                key: key.clone(),
                conversation: id.clone(),
                revision,
                limit: max_messages,
            });
        }
        let changed = self.store.db.save_metadata(&batch).await?;
        for change in changed {
            if change.kind == "room" {
                if let Some(room) = self.data.rooms.get_mut(&change.key) {
                    room.revision = change.revision;
                }
            } else if let Some(chat) = self.data.private.get_mut(&change.key) {
                chat.revision = change.revision;
            }
        }
        self.max_users = max_users;
        self.max_rooms = max_rooms;
        self.max_messages = max_messages;

        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    #[test]
    fn retention_counters_handle_gaps_limits_cleanup_and_failed_appends() {
        let mut engine = crate::engine::tests::engine();
        engine.set_limits(64, 64, 3).unwrap();
        engine
            .execute(Some("alice"), None, "/room new other")
            .unwrap();
        for n in 0..5 {
            engine
                .execute(Some("alice"), Some("lobby"), &format!("room {n}"))
                .unwrap();
            for _ in 0..10 {
                engine.execute(Some("alice"), Some("other"), "gap").unwrap();
            }
        }
        let id = engine.data.rooms["lobby"].id.clone();
        let page = engine.history("bob", "lobby").unwrap();
        assert_eq!(
            page.messages
                .iter()
                .map(|m| m.text.as_str())
                .collect::<Vec<_>>(),
            ["room 2", "room 3", "room 4"]
        );
        assert_eq!(
            engine
                .store
                .db
                .lock()
                .unwrap()
                .query_row::<(usize, u64), _, _>(
                    "SELECT count,oldest FROM retention WHERE conversation=?1",
                    [&id],
                    |row| Ok((row.get(0)?, row.get(1)?))
                )
                .unwrap(),
            (3, page.messages[0].sequence)
        );
        engine.store.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_retention BEFORE UPDATE ON retention BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        let revision = engine.data.rooms["lobby"].revision;
        assert!(
            engine
                .execute(Some("alice"), Some("lobby"), "rolled back")
                .is_err()
        );
        assert_eq!(engine.data.rooms["lobby"].revision, revision);
        assert_eq!(
            engine.history("bob", "lobby").unwrap().messages[2].text,
            "room 4"
        );
        engine
            .store
            .db
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER reject_retention;")
            .unwrap();
        engine.set_limits(64, 64, 2).unwrap();
        assert_eq!(engine.history("bob", "lobby").unwrap().messages.len(), 2);
        engine.execute(None, None, "/clean -1 #lobby").unwrap();
        assert_eq!(
            engine
                .store
                .db
                .lock()
                .unwrap()
                .query_row::<usize, _, _>(
                    "SELECT count(*) FROM retention WHERE conversation=?1",
                    [&id],
                    |row| row.get(0)
                )
                .unwrap(),
            0
        );
    }
}
