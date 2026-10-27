//! Authentication audit persistence, atomic with session changes.
use super::*;

impl Engine {
    #[cfg(test)]
    pub(crate) fn record_auth_event(&self, event: &AuthEvent) -> Result<(), EngineError> {
        let batch = MetadataWrite {
            operations: vec![MetadataOp::AuthEvent(event)],
        };
        self.store.db.save_metadata_blocking(&batch)?;
        Ok(())
    }
}

impl Engine {
    #[cfg(test)]
    pub(in crate::engine) fn save_sessions(
        &mut self,
        before: &BTreeMap<String, Session>,
        event: Option<&AuthEvent>,
    ) -> Result<(), EngineError> {
        let batch = session_write(before, &self.data.sessions, now(), event)?;
        self.store.db.save_metadata_blocking(&batch)?;
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }
}

pub(in crate::engine) fn session_write<'a>(
    before: &BTreeMap<String, Session>,
    after: &BTreeMap<String, Session>,
    at: u64,
    event: Option<&'a AuthEvent>,
) -> Result<MetadataWrite<'a>, EngineError> {
    let mut batch = MetadataWrite::default();
    write_map_delta(&mut batch, "session", after, Some(before))?;
    batch
        .operations
        .push(MetadataOp::ExpireSessions { before: at });
    if let Some(event) = event {
        batch.operations.push(MetadataOp::AuthEvent(event));
    }
    Ok(batch)
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
