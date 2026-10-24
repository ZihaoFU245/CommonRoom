//! HEAD v17 to v18: session digests, authentication events and retention cursors.
use super::*;
use rusqlite::OptionalExtension;

pub(super) const SCHEMA: &str = "
CREATE TABLE retention(conversation TEXT PRIMARY KEY, count INTEGER NOT NULL CHECK(count>=0), oldest INTEGER) WITHOUT ROWID;
CREATE TABLE auth_events(sequence INTEGER PRIMARY KEY AUTOINCREMENT,at INTEGER NOT NULL,client TEXT,username TEXT,outcome TEXT NOT NULL CHECK(outcome IN ('success','failure','logout')));
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
pub(super) fn migrate(db: &Connection, data: &mut Data) -> Result<(), EngineError> {
    // The caller owns the complete v17-to-current upgrade transaction.
    let tx = db;
    tx.execute_batch(SCHEMA)?;
    tx.execute("INSERT INTO retention SELECT conversation,count(*),min(sequence) FROM messages GROUP BY conversation", [])?;
    let sessions = data
        .sessions
        .iter()
        .map(|(token, session)| (session_digest(token), session.clone()))
        .collect::<BTreeMap<_, _>>();
    // Reinsert as a set: a legacy token could equal another session's new digest.
    // Row-by-row key updates could otherwise collide despite distinct secrets.
    tx.execute("DELETE FROM entities WHERE kind='session'", [])?;
    for (digest, session) in &sessions {
        put_entity(tx, "session", digest, session)?;
    }
    data.sessions = sessions;
    Ok(())
}
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
pub(super) fn write_auth_event(db: &Connection, event: &AuthEvent) -> Result<(), EngineError> {
    db.execute(
        "INSERT INTO auth_events(at,client,username,outcome) VALUES(?1,?2,?3,?4)",
        params![
            event.at,
            event.client,
            event.username,
            event.outcome.label()
        ],
    )?;
    let sequence = db.last_insert_rowid();
    db.execute(
        "DELETE FROM auth_events WHERE sequence<=?1",
        [sequence.saturating_sub(2048)],
    )?;
    Ok(())
}
impl Engine {
    pub(crate) fn record_auth_event(&self, event: &AuthEvent) -> Result<(), EngineError> {
        let mut db = self.store.db.lock().map_err(EngineError::unavailable)?;
        let tx = db.savepoint()?;
        write_auth_event(&tx, event)?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn legacy(path: &Path) {
        let mut engine = Engine::open(path).unwrap();
        engine
            .provision("alice", "hash".into(), true, false)
            .unwrap();
        engine
            .provision("bob", "hash".into(), false, false)
            .unwrap();
        engine
            .execute(Some("alice"), None, "/room new room")
            .unwrap();
        engine
            .execute(Some("alice"), None, "/room add bob #room")
            .unwrap();
        engine
            .execute(Some("alice"), Some("room"), "retained")
            .unwrap();
        engine.mark_read("bob", "room", 1).unwrap();
        engine.store.db.lock().unwrap().execute_batch("DROP TRIGGER retention_insert; DROP TRIGGER retention_delete; DROP TRIGGER retention_move; DROP TABLE retention; DROP TABLE auth_events; PRAGMA user_version=17;").unwrap();
        let db = engine.store.db.lock().unwrap();
        // The second raw key intentionally collides with the first new digest.
        for raw in ["legacy-cookie".to_owned(), session_digest("legacy-cookie")] {
            put_entity(
                &db,
                "session",
                &raw,
                &Session {
                    username: "alice".into(),
                    expires: now() + 43200,
                },
            )
            .unwrap();
        }
    }
    #[test]
    fn head_upgrade_preserves_sessions_history_policy_and_cursors_without_raw_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite");
        legacy(&path);
        let engine = Engine::open(&path).unwrap();
        assert_eq!(engine.session("legacy-cookie").as_deref(), Some("alice"));
        assert_eq!(
            engine.session(&session_digest("legacy-cookie")).as_deref(),
            Some("alice")
        );
        assert_eq!(
            engine.history("bob", "room").unwrap().messages[0].text,
            "retained"
        );
        assert_eq!(engine.unreads("bob").unwrap()["room"].count, 0);
        assert!(engine.is_admin("alice"));
        let db = engine.store.db.lock().unwrap();
        assert_eq!(
            db.query_row::<u32, _, _>("PRAGMA user_version", [], |row| row.get(0))
                .unwrap(),
            19
        );
        assert_eq!(
            db.query_row::<usize, _, _>("SELECT count FROM retention", [], |row| row.get(0))
                .unwrap(),
            1
        );
        assert_eq!(
            db.query_row::<usize, _, _>(
                "SELECT count(*) FROM entities WHERE kind='session' AND key='legacy-cookie'",
                [],
                |row| row.get(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            db.query_row::<usize, _, _>("SELECT count(*) FROM auth_events", [], |row| row.get(0))
                .unwrap(),
            0
        );
        drop(db);
        drop(engine);
        let engine = Engine::open(&path).unwrap();
        assert_eq!(engine.session("legacy-cookie").as_deref(), Some("alice"));
        assert_eq!(engine.data.sessions.len(), 2);
    }
    #[test]
    fn failed_upgrade_rolls_back_session_keys_tables_triggers_and_schema_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite");
        legacy(&path);
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TRIGGER reject_migration BEFORE INSERT ON entities WHEN NEW.kind='session' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(Engine::open(&path).is_err());
        assert_eq!(
            db.query_row::<u32, _, _>("PRAGMA user_version", [], |row| row.get(0))
                .unwrap(),
            17
        );
        assert_eq!(
            db.query_row::<usize, _, _>(
                "SELECT count(*) FROM entities WHERE kind='session' AND key='legacy-cookie'",
                [],
                |row| row.get(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(db.query_row::<usize,_,_>("SELECT count(*) FROM sqlite_master WHERE name IN ('retention','auth_events','retention_insert','retention_delete','retention_move')",[],|row|row.get(0)).unwrap(),0);
        assert_eq!(
            db.query_row::<usize, _, _>("SELECT count(*) FROM messages", [], |row| row.get(0))
                .unwrap(),
            1
        );
        db.execute_batch("DROP TRIGGER reject_migration;").unwrap();
        assert!(Engine::open(&path).is_ok());
    }
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
    #[test]
    fn auth_audit_is_bounded_durable_and_atomic_with_session_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite");
        let mut engine = Engine::open(&path).unwrap();
        engine
            .provision("alice", "hash".into(), false, false)
            .unwrap();
        let event = AuthEvent::new(
            Some("alice"),
            Some("::ffff:198.51.100.1".parse().unwrap()),
            AuthOutcome::Success,
        );
        engine.store.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_auth BEFORE INSERT ON auth_events BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(
            engine
                .login_recorded("secret-cookie".into(), "alice", "hash", None, Some(&event))
                .is_err()
        );
        assert!(engine.data.sessions.is_empty());
        engine
            .store
            .db
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER reject_auth;")
            .unwrap();
        engine
            .login_recorded("secret-cookie".into(), "alice", "hash", None, Some(&event))
            .unwrap();
        assert!(engine.session(&session_digest("secret-cookie")).is_none());
        for _ in 0..2050 {
            engine
                .record_auth_event(&AuthEvent::new(Some("alice"), None, AuthOutcome::Failure))
                .unwrap();
        }
        drop(engine);
        let mut engine = Engine::open(&path).unwrap();
        assert_eq!(
            engine
                .store
                .db
                .lock()
                .unwrap()
                .query_row::<usize, _, _>("SELECT count(*) FROM auth_events", [], |row| row.get(0))
                .unwrap(),
            2048
        );
        let event = AuthEvent::new(
            Some("alice"),
            Some("198.51.100.1".parse().unwrap()),
            AuthOutcome::Logout,
        );
        engine.store.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_auth BEFORE INSERT ON auth_events BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(
            engine
                .logout_recorded("secret-cookie", Some(&event))
                .is_err()
        );
        assert_eq!(engine.session("secret-cookie").as_deref(), Some("alice"));
        engine
            .store
            .db
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER reject_auth;")
            .unwrap();
        assert_eq!(
            engine
                .logout_recorded("secret-cookie", Some(&event))
                .unwrap()
                .as_deref(),
            Some("alice")
        );
        let db = engine.store.db.lock().unwrap();
        assert_eq!(
            db.query_row::<String, _, _>(
                "SELECT outcome FROM auth_events ORDER BY sequence DESC LIMIT 1",
                [],
                |row| row.get(0)
            )
            .unwrap(),
            "logout"
        );
        assert!(!db.query_row::<String,_,_>("SELECT group_concat(coalesce(username,'')||coalesce(client,'')||outcome) FROM auth_events",[],|row|row.get(0)).unwrap().contains("secret-cookie"));
    }
}
