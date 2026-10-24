//! Per-conversation retained-row counters and pruning.
use super::*;
use rusqlite::OptionalExtension;

pub(super) const SCHEMA: &str = "
CREATE TABLE retention(conversation TEXT PRIMARY KEY, count INTEGER NOT NULL CHECK(count>=0), oldest INTEGER) WITHOUT ROWID;
CREATE TRIGGER retention_insert AFTER INSERT ON messages BEGIN
 INSERT INTO retention VALUES(NEW.conversation,1,NEW.sequence) ON CONFLICT(conversation) DO UPDATE SET count=count+1,oldest=min(coalesce(oldest,NEW.sequence),NEW.sequence);
END;
CREATE TRIGGER retention_delete AFTER DELETE ON messages BEGIN
 UPDATE retention SET count=count-1,oldest=(SELECT min(sequence) FROM messages WHERE conversation=OLD.conversation) WHERE conversation=OLD.conversation;
 DELETE FROM retention WHERE conversation=OLD.conversation AND count=0;
END;
CREATE TRIGGER retention_move AFTER UPDATE OF conversation,sequence ON messages WHEN OLD.conversation!=NEW.conversation OR OLD.sequence!=NEW.sequence BEGIN
 DELETE FROM retention WHERE conversation IN (OLD.conversation,NEW.conversation);
 INSERT INTO retention SELECT conversation,count(*),min(sequence) FROM messages WHERE conversation IN (OLD.conversation,NEW.conversation) GROUP BY conversation;
END;
";
pub(super) fn prune(db: &Connection, id: &str, limit: usize) -> Result<usize, EngineError> {
    let count: u64 = db
        .query_row(
            "SELECT count FROM retention WHERE conversation=?1",
            [id],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0);
    let excess =
        count.saturating_sub(u64::try_from(limit).map_err(|_| "Invalid retention limit.")?);
    if excess == 0 {
        return Ok(0);
    }
    // Walk only rows being deleted, never the retained window.
    db.execute("DELETE FROM messages WHERE conversation=?1 AND sequence IN (SELECT sequence FROM messages WHERE conversation=?1 ORDER BY sequence LIMIT ?2)", params![id,excess]).map_err(EngineError::from)
}

impl Engine {
    pub fn set_limits(
        &mut self,
        max_users: usize,
        max_rooms: usize,
        max_messages: usize,
    ) -> Result<(), EngineError> {
        crate::config::validate_limits(max_users, max_rooms, max_messages)
            .map_err(EngineError::Invalid)?;
        let writer = self.store.db.clone();
        let mut db = writer.lock().map_err(EngineError::unavailable)?;
        let tx = db.savepoint().map_err(EngineError::from)?;
        let mut changed = Vec::new();
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
            let deleted = retention::prune(&tx, id, max_messages)?;
            if deleted > 0 {
                let revision = revision.wrapping_add(1);
                tx.execute(
                    "UPDATE entities SET revision=?3 WHERE kind=?1 AND key=?2",
                    params![kind, key, revision],
                )
                .map_err(EngineError::from)?;
                changed.push((kind, key.clone(), revision));
            }
        }
        tx.commit().map_err(EngineError::from)?;
        for (kind, key, revision) in changed {
            if kind == "room" {
                if let Some(r) = self.data.rooms.get_mut(&key) {
                    r.revision = revision;
                }
            } else if let Some(c) = self.data.private.get_mut(&key) {
                c.revision = revision;
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
