//! Typed metadata transactions. Shared coordinators describe changes; backend
//! implementations own SQL, native bindings and transaction-local maintenance.
use super::{Error, Result};
use crate::engine::{AuthEvent, Message, authorization::Audit};
use std::collections::BTreeSet;
#[cfg(test)]
use std::collections::VecDeque;

#[derive(Default)]
pub(crate) struct MetadataWrite<'a> {
    pub(crate) operations: Vec<MetadataOp<'a>>,
}
pub(crate) enum MetadataOp<'a> {
    Entity {
        kind: &'static str,
        key: String,
        json: String,
    },
    RemoveEntity {
        kind: &'static str,
        key: String,
    },
    Revision {
        kind: &'static str,
        key: String,
        expected: u64,
        revision: u64,
    },
    CheckedRevision {
        kind: &'static str,
        key: String,
        conversation: String,
        expected: u64,
        revision: u64,
    },
    RemoveMessages {
        conversation: String,
    },
    State {
        json: String,
    },
    #[cfg(test)]
    Audit(&'a VecDeque<Audit>),
    AppendAudit(&'a Audit),
    RemoveAccountReceipts {
        account: String,
    },
    RemoveAccountCursors {
        user: String,
    },
    RenameAccountCursors {
        old: String,
        new: String,
    },
    RemoveConversationCursors {
        conversation: String,
    },
    RenameConversationCursors {
        old: String,
        new: String,
    },
    RewriteAccount {
        old: String,
        new: Option<String>,
    },
    HistoryChanges {
        deletes: &'a [(String, u64)],
        cleanups: &'a [(String, u64)],
    },
    Prune {
        kind: &'static str,
        key: String,
        conversation: String,
        revision: u64,
        limit: usize,
    },
    #[cfg(test)]
    CommandReceipt {
        account: &'a str,
        key: &'a str,
        digest: &'a str,
        created: u64,
        expired_before: u64,
        outcome: &'a str,
    },
    AuthEvent(&'a AuthEvent),
    ExpireSessions {
        before: u64,
    },
}
impl MetadataWrite<'_> {
    pub(crate) fn entity(
        &mut self,
        kind: &'static str,
        key: &str,
        value: &impl serde::Serialize,
    ) -> Result<()> {
        let json = serde_json::to_string(value)
            .map_err(|_| Error::Conversion("Cannot serialize stored entity"))?;
        self.operations.push(MetadataOp::Entity {
            kind,
            key: key.into(),
            json,
        });
        Ok(())
    }
}
pub(crate) struct RevisionChange {
    pub(crate) kind: &'static str,
    pub(crate) key: String,
    pub(crate) revision: u64,
}
#[derive(Default)]
pub(super) struct WriteResult {
    pub(super) changed: u64,
    pub(super) message: Option<super::MessageCommit>,
    pub(super) command: Option<(String, bool)>,
    pub(crate) revisions: Vec<RevisionChange>,
    pub(super) media: Vec<crate::engine::media::Media>,
    pub(super) staging: Vec<String>,
}
pub(crate) struct EntityRow {
    pub(crate) kind: String,
    pub(crate) key: String,
    pub(crate) json: String,
    pub(crate) revision: u64,
}
pub(crate) struct MetadataSnapshot {
    pub(crate) state: String,
    pub(crate) sequence: u64,
    pub(crate) entities: Vec<EntityRow>,
    pub(crate) audit: Vec<String>,
}
pub(crate) type CursorRows = Vec<(String, String, u64)>;

pub(super) fn rewrite_message(message: &mut Message, old: &str, new: Option<&str>) -> bool {
    let replacement = new
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{old} (deleted)"));
    let mut changed = false;
    if message.from == old {
        message.from = replacement.clone();
        changed = true;
    }
    if new.is_some() && message.to.as_deref() == Some(old) {
        message.to = new.map(str::to_owned);
        changed = true;
    }
    if let Some(reply) = &mut message.reply
        && reply.from == old
    {
        reply.from = replacement;
        changed = true;
    }
    if message.mentions.remove(old) {
        if let Some(new) = new {
            message.mentions.insert(new.into());
        }
        changed = true;
    }
    changed
}

macro_rules! native_metadata {
    ($connection:expr, $method:ident $(, $argument:expr)* $(,)?) => {{
        use super::{Backend, runtime};
        $connection.ensure_available()?;
        let mut backend = $connection.backend.borrow_mut();
        let result = match &mut *backend {
            Backend::Sqlite(db) => runtime::run(super::sqlite::metadata::$method(db $(, $argument)*)),
            Backend::Postgres(db) => runtime::run(super::postgres::metadata::$method(db $(, $argument)*)),
            Backend::Closed => Err(Error::Conversion("Database closed")),
        };
        $connection.checked(result)
    }};
}
impl super::Connection {
    pub(crate) fn metadata_snapshot(&self) -> Result<MetadataSnapshot> {
        native_metadata!(self, load)
    }
    pub(crate) fn cursor_rows(&self) -> Result<CursorRows> {
        native_metadata!(self, cursors)
    }
    #[cfg(test)]
    pub(crate) fn account_conversations(&self, name: &str) -> Result<BTreeSet<String>> {
        native_metadata!(self, account_conversations, name)
    }
}
impl super::Connection {
    async fn account_conversations_async(&mut self, name: &str) -> Result<BTreeSet<String>> {
        self.ensure_available()?;
        let result = match self.backend.get_mut() {
            super::Backend::Sqlite(db) => {
                super::sqlite::metadata::account_conversations(db, name).await
            }
            super::Backend::Postgres(db) => {
                super::postgres::metadata::account_conversations(db, name).await
            }
            super::Backend::Closed => Err(Error::Conversion("Database closed")),
        };
        self.checked(result)
    }
}
impl super::Handle {
    /// The caller holds the exclusive application metadata fence while loading
    /// and publishing this authoritative snapshot. No engine guard crosses I/O.
    pub(crate) async fn recovery_snapshot(&self) -> Result<(MetadataSnapshot, CursorRows)> {
        self.check_owner().await?;
        let mut reader = self.acquire_read().await?;
        let result = match reader.backend.get_mut() {
            super::Backend::Sqlite(db) => {
                let metadata = super::sqlite::metadata::load(db).await?;
                Ok((metadata, super::sqlite::metadata::cursors(db).await?))
            }
            super::Backend::Postgres(db) => {
                let metadata = super::postgres::metadata::load(db).await?;
                Ok((metadata, super::postgres::metadata::cursors(db).await?))
            }
            super::Backend::Closed => Err(Error::RestartRequired),
        };
        self.check_owner().await?;
        result
    }
    pub(crate) async fn account_conversations(&self, name: &str) -> Result<BTreeSet<String>> {
        self.acquire_read()
            .await?
            .account_conversations_async(name)
            .await
    }

    #[cfg(test)]
    pub(crate) fn save_metadata_blocking(
        &self,
        request: &MetadataWrite<'_>,
    ) -> Result<Vec<RevisionChange>> {
        super::runtime::run(self.save_metadata(request))
    }
    pub(crate) async fn save_metadata(
        &self,
        request: &MetadataWrite<'_>,
    ) -> Result<Vec<RevisionChange>> {
        let mut session = self.acquire().await?;
        Ok(session
            .save_write_async(&super::transaction::WriteRequest::Metadata(request), self)
            .await?
            .revisions)
    }
}
