//! Authentication audit persistence, atomic with session changes.
use super::*;

pub(super) const SCHEMA: &str = "CREATE TABLE auth_events(sequence INTEGER PRIMARY KEY AUTOINCREMENT,at INTEGER NOT NULL,client TEXT,username TEXT,outcome TEXT NOT NULL CHECK(outcome IN ('success','failure','logout')));";
pub(super) fn write_auth_event(db: &Connection, event: &AuthEvent) -> Result<(), EngineError> {
    let sequence: i64 = db.query_row(
        sql("INSERT INTO auth_events(at,client,username,outcome) VALUES(?1,?2,?3,?4) RETURNING sequence", "INSERT INTO auth_events(at,client,username,outcome) VALUES($1,$2,$3,$4) RETURNING sequence"),
        params![
            event.at,
            event.client,
            event.username,
            event.outcome.label()
        ], |row| row.get(0),
    )?;
    db.execute(
        sql(
            "DELETE FROM auth_events WHERE sequence<=?1",
            "DELETE FROM auth_events WHERE sequence<=$1",
        ),
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

impl Engine {
    pub(in crate::engine) fn save_sessions(
        &mut self,
        before: &BTreeMap<String, Session>,
        event: Option<&AuthEvent>,
    ) -> Result<(), EngineError> {
        let writer = self.store.db.clone();
        let mut db = writer.lock().map_err(EngineError::unavailable)?;
        let tx = db.savepoint().map_err(EngineError::from)?;
        write_map_delta(&tx, "session", &self.data.sessions, Some(before))?;
        tx.execute(
            sql("DELETE FROM entities WHERE kind='session' AND json_extract(json,'$.expires')<=?1", "DELETE FROM entities WHERE kind='session' AND ((projection #>> '{expires}'))::bigint<=$1"),
            [now()],
        )
        .map_err(EngineError::from)?;
        if let Some(event) = event {
            auth::write_auth_event(&tx, event)?;
        }
        tx.commit().map_err(EngineError::from)?;
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
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
