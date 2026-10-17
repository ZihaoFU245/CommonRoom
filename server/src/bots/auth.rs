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
        let secret = auth::bearer(headers)
            .filter(|s| s.len() == 68 && s.starts_with("bot_"))
            .ok_or_else(|| Error::new(Code::Unauthorized, "Bot credential required."))?;
        let digest = super::digest(secret);
        let engine = app.engine_read().map_err(Error::storage)?;
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
        context.user(&engine)?;
        Ok(context)
    }
    pub fn user(&self, engine: &Engine) -> Result<String, Error> {
        let name = engine
            .bot_index
            .accounts
            .get(&self.account)
            .filter(|name| {
                engine
                    .data
                    .users
                    .get(*name)
                    .is_some_and(|a| a.id == self.account && !a.disabled)
            })
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
        Ok(name.clone())
    }
    pub fn metadata<'a>(&self, engine: &'a Engine) -> Result<&'a Credential, Error> {
        engine
            .data
            .bots
            .get(&self.account)
            .and_then(|r| r.tokens.get(&self.credential))
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
    pub fn snapshot(&self, engine: &Engine) -> Result<super::models::Snapshot, Error> {
        let user = self.user(engine)?;
        let c = self.metadata(engine)?;
        let mut snapshot = engine.bot_snapshot(&user)?;
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
        Ok(snapshot)
    }
}
