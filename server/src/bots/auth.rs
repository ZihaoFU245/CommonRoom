//! Stable credential context: hash once, recheck live metadata on every operation.
use super::{
    error::{Code, Error},
    models::Credential,
};
use crate::{
    engine::Engine,
    web::{App, auth},
};
use axum::http::HeaderMap;

#[derive(Clone)]
pub(crate) struct Context {
    pub account: String,
    pub credential: String,
    digest: String,
}
impl Context {
    pub fn authenticate(app: &App, headers: &HeaderMap) -> Result<Self, Error> {
        if app.stopping.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(Error::new(Code::Unavailable, "Server stopping."));
        }
        let engine = app.engine_read().map_err(Error::storage)?;
        Self::from_headers(&engine, headers)
    }
    pub(crate) fn from_headers(engine: &Engine, headers: &HeaderMap) -> Result<Self, Error> {
        let secret = auth::bearer(headers)
            .filter(|s| s.len() == 68 && s.starts_with("bot_"))
            .ok_or_else(|| Error::new(Code::Unauthorized, "Bot credential required."))?;
        let digest = super::digest(secret);
        let (account, credential) = engine
            .bot_index
            .credentials
            .get(&digest)
            .ok_or_else(|| Error::new(Code::Unauthorized, "Invalid bot credential."))?;
        let context = Self {
            account: account.clone(),
            credential: credential.clone(),
            digest,
        };
        context.user(engine)?;
        Ok(context)
    }
    pub fn user(&self, engine: &Engine) -> Result<String, Error> {
        let name = engine
            .active_account_name(&self.account)
            .ok_or_else(|| Error::new(Code::Unauthorized, "Account unavailable."))?;
        let credential = self.metadata(engine)?;
        if credential.digest != self.digest
            || credential
                .expires
                .is_some_and(|at| at <= crate::bots::now())
        {
            return Err(Error::new(
                Code::Unauthorized,
                "Credential expired or revoked.",
            ));
        }
        Ok(name.to_owned())
    }
    pub fn metadata<'a>(&self, engine: &'a Engine) -> Result<&'a Credential, Error> {
        engine
            .bot_credential(&self.account, &self.credential)
            .ok_or_else(|| Error::new(Code::Unauthorized, "Credential revoked."))
    }
    pub fn permits(
        &self,
        engine: &Engine,
        operation: &str,
        conversation: Option<&str>,
    ) -> Result<(), Error> {
        self.user(engine)?;
        let c = self.metadata(engine)?;
        if c.operations
            .as_ref()
            .is_some_and(|ops| !ops.contains(operation))
            || conversation.is_some_and(|id| {
                c.conversations
                    .as_ref()
                    .is_some_and(|ids| !ids.contains(id))
            })
        {
            return Err(Error::new(Code::Forbidden, "Credential scope denied."));
        }
        Ok(())
    }
    pub fn snapshot_plan(
        &self,
        engine: &Engine,
    ) -> Result<crate::engine::snapshot::BotSnapshotPlan, Error> {
        let user = self.user(engine)?;
        let c = self.metadata(engine)?;
        let mut plan = engine.bot_snapshot_plan(&user)?;
        let snapshot = &mut plan.snapshot;
        let permits = |operation: &str, id: &str| {
            c.operations
                .as_ref()
                .is_none_or(|ops| ops.contains(operation))
                && c.conversations.as_ref().is_none_or(|ids| ids.contains(id))
        };
        snapshot.conversations.retain(|v| permits("read", &v.id));
        for v in &mut snapshot.conversations {
            v.permissions.send &= permits("send", &v.id);
            v.permissions.reply &= v.permissions.send;
            v.permissions.react &= permits("react", &v.id);
        }
        snapshot.credential = serde_json::json!({"id":self.credential,"expires":c.expires,"conversations":c.conversations,"operations":c.operations});
        Ok(plan)
    }
}
