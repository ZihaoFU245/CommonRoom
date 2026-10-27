//! Native session changes and bounded authentication events.
use super::*;

#[derive(Clone)]
pub(crate) struct AuthStore {
    database: database::Handle,
}
impl AuthStore {
    pub(crate) async fn health(&self) -> Result<(), EngineError> {
        self.database.ensure_available_async().await?;
        Ok(())
    }
    pub(crate) async fn record(&self, event: AuthEvent) -> Result<(), EngineError> {
        self.database
            .save_metadata(&database::MetadataWrite {
                operations: vec![database::MetadataOp::AuthEvent(&event)],
            })
            .await?;
        Ok(())
    }
}
pub(crate) struct SessionWrite {
    store: AuthStore,
    before: BTreeMap<String, Session>,
    after: BTreeMap<String, Session>,
    event: Option<AuthEvent>,
    at: u64,
}
impl SessionWrite {
    pub(crate) async fn save(self) -> Result<Self, EngineError> {
        let batch =
            storage::session_write(&self.before, &self.after, self.at, self.event.as_ref())?;
        self.store.database.save_metadata(&batch).await?;
        drop(batch);
        Ok(self)
    }
}
impl Engine {
    pub(crate) fn auth_store(&self) -> AuthStore {
        AuthStore {
            database: self.store.db.clone(),
        }
    }
    pub(crate) fn prepare_login(
        &self,
        token: String,
        user: &str,
        expected: &str,
        old: Option<&str>,
        event: AuthEvent,
    ) -> Result<SessionWrite, EngineError> {
        Ok(SessionWrite {
            store: self.auth_store(),
            before: self.data.sessions.clone(),
            after: self.login_sessions(token, user, expected, old)?,
            at: now(),
            event: Some(event),
        })
    }
    pub(crate) fn prepare_logout(
        &self,
        token: &str,
        mut event: AuthEvent,
    ) -> (Option<String>, Option<SessionWrite>) {
        let key = session_digest(token);
        if !self.data.sessions.contains_key(&key) {
            return (None, None);
        }
        // An expired but recognized cookie still identifies the session being
        // deleted; do not silently omit its logout from the audit trail.
        let user = self
            .data
            .sessions
            .get(&key)
            .map(|session| session.username.clone());
        event.username = user.clone();
        let mut after = self.data.sessions.clone();
        after.remove(&key);
        let event = Some(event);
        (
            user,
            Some(SessionWrite {
                store: self.auth_store(),
                before: self.data.sessions.clone(),
                after,
                event,
                at: now(),
            }),
        )
    }
    pub(crate) fn publish_sessions(&mut self, pending: SessionWrite) -> Result<(), EngineError> {
        if self.data.sessions != pending.before {
            self.recovery_failed = true;
            return Err(EngineError::Unavailable(
                "Committed session state changed; recovery required, retry shortly.".into(),
            ));
        }
        self.data.sessions = pending.after;
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn expired_recognized_logout_is_removed_and_audited_once() {
        let mut engine = super::super::tests::engine();
        engine
            .login("expired".into(), "alice", "test-hash", None)
            .unwrap();
        engine
            .data
            .sessions
            .get_mut(&session_digest("expired"))
            .unwrap()
            .expires = now() - 1;
        let (user, pending) =
            engine.prepare_logout("expired", AuthEvent::new(None, None, AuthOutcome::Logout));
        assert_eq!(user.as_deref(), Some("alice"));
        let pending = pending.unwrap().save().await.unwrap();
        engine.publish_sessions(pending).unwrap();
        assert!(
            !engine
                .data
                .sessions
                .contains_key(&session_digest("expired"))
        );
        let events: u64 = engine
            .store
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM auth_events WHERE username='alice' AND outcome='logout'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(events, 1);
        assert!(
            engine
                .prepare_logout("expired", AuthEvent::new(None, None, AuthOutcome::Logout))
                .1
                .is_none()
        );
    }
}
