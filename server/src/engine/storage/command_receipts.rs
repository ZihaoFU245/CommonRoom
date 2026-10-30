//! Command effects and typed outcome persistence share one outer transaction.
use super::*;
use crate::bots::error::{Code, Error};
use serde::{Deserialize, Serialize};

const TTL: u64 = crate::bots::models::RECEIPT_TTL;
const MAX_RESULT: usize = 65536;
#[cfg(test)]
const MAX_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct CommandOutcome {
    pub result: Option<String>,
    pub error: Option<Error>,
    pub created: u64,
    pub expires: u64,
    pub replayed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_authority: Option<crate::system::ReportAccess>,
}
impl CommandOutcome {
    pub(crate) fn authorize(&self, engine: &Engine, user: &str) -> Result<(), Error> {
        if let Some(access) = self.system_authority {
            access.authorize(engine, user)?;
        }
        Ok(())
    }
    pub fn into_result(self) -> Result<serde_json::Value, Error> {
        if let Some(error) = self.error {
            return Err(error);
        }
        Ok(
            serde_json::json!({"result":self.result.unwrap_or_default(),"replayed":self.replayed,"expires":self.expires}),
        )
    }
}
pub(crate) struct CommandReceipts {
    database: database::Handle,
}
pub(crate) enum PendingCommand {
    None,
    Message(Box<PendingMessage>),
    Metadata(Box<metadata_write::PendingMetadata>),
    History(PendingHistory),
}
pub(crate) struct CommittedCommand {
    effect: PendingCommand,
    commit: database::CommandCommit,
    outcome: CommandOutcome,
}
impl CommandReceipts {
    pub(crate) async fn commit(
        &self,
        account: &str,
        key: &str,
        digest: &str,
        result: Result<String, Error>,
        mut effect: PendingCommand,
        system_authority: Option<crate::system::ReportAccess>,
    ) -> Result<CommittedCommand, Error> {
        let result = result.and_then(|reply| {
            if reply.len() > MAX_RESULT / 2 {
                Err(Error::new(
                    Code::PayloadTooLarge,
                    "Command result is too large; use paged history.",
                ))
            } else {
                Ok(reply)
            }
        });
        if let Err(error) = &result {
            if error.code == Code::Unavailable {
                return Err(error.clone());
            }
            effect = PendingCommand::None;
        }
        let created = now();
        let outcome = CommandOutcome {
            result: result.as_ref().ok().cloned(),
            error: result.err(),
            created,
            expires: created + TTL,
            replayed: false,
            system_authority,
        };
        let raw = serde_json::to_string(&outcome).map_err(Error::storage)?;
        let commit = {
            let message;
            let metadata;
            let write = match &mut effect {
                PendingCommand::None => database::CommandEffect::None,
                PendingCommand::Message(pending) => {
                    message = pending.write(None).map_err(Error::storage)?;
                    database::CommandEffect::Message(&message)
                }
                PendingCommand::Metadata(pending) => {
                    metadata = pending.write();
                    database::CommandEffect::Metadata(&metadata)
                }
                PendingCommand::History(pending) => {
                    metadata = pending.write();
                    database::CommandEffect::Metadata(&metadata)
                }
            };
            self.database
                .save_command(&database::CommandWrite {
                    account,
                    key,
                    digest,
                    created,
                    expired_before: created.saturating_sub(TTL),
                    outcome: &raw,
                    effect: write,
                })
                .await
                .map_err(|error| match error {
                    database::Error::ReceiptConflict => Error::new(
                        Code::IdempotencyConflict,
                        "Key already belongs to a different command request.",
                    ),
                    database::Error::ReceiptPayload => {
                        Error::new(Code::PayloadTooLarge, "Command receipt is too large.")
                    }
                    database::Error::ReceiptCapacity => Error::new(
                        Code::IdempotencyCapacity,
                        "Wait for command receipts to expire.",
                    ),
                    error => Error::from(EngineError::from(error)),
                })?
        };
        let mut outcome: CommandOutcome =
            serde_json::from_str(&commit.outcome).map_err(Error::storage)?;
        outcome.replayed = commit.replayed;
        Ok(CommittedCommand {
            effect,
            commit,
            outcome,
        })
    }
    pub(crate) async fn health(&self) -> Result<(), Error> {
        self.database
            .ensure_available_async()
            .await
            .map_err(Error::storage)
    }
    pub(crate) async fn lookup(
        &self,
        account: &str,
        key: &str,
    ) -> Result<Option<(String, CommandOutcome)>, Error> {
        let row = self
            .database
            .command_receipt(&database::ReceiptRead {
                account,
                key,
                after: now().saturating_sub(TTL),
            })
            .await
            .map_err(Error::storage)?;
        row.map(|(digest, raw)| {
            serde_json::from_str(&raw)
                .map(|mut outcome: CommandOutcome| {
                    outcome.replayed = true;
                    (digest, outcome)
                })
                .map_err(Error::storage)
        })
        .transpose()
    }
}
impl Engine {
    pub(crate) fn publish_command(
        &mut self,
        committed: CommittedCommand,
    ) -> Result<(CommandOutcome, CommandChange), EngineError> {
        let change = if committed.commit.replayed {
            CommandChange::Unchanged
        } else {
            match committed.effect {
                PendingCommand::None => CommandChange::Unchanged,
                PendingCommand::Metadata(pending) => {
                    self.publish_metadata(*pending)?;
                    CommandChange::Metadata(self.metadata_change())
                }
                PendingCommand::History(pending) => self.publish_history(pending)?,
                PendingCommand::Message(pending) => {
                    let created = pending.creates_conversation();
                    let id = pending.conversation.clone();
                    let commit = committed.commit.message.ok_or_else(|| {
                        EngineError::unavailable("Missing committed command message")
                    })?;
                    self.publish_message(*pending, commit)?;
                    if created {
                        CommandChange::Metadata(self.metadata_change())
                    } else {
                        CommandChange::Conversation(id)
                    }
                }
            }
        };
        Ok((committed.outcome, change))
    }
    pub(crate) fn command_receipts(&self) -> CommandReceipts {
        CommandReceipts {
            database: self.store.db.clone(),
        }
    }
    #[cfg(test)]
    pub(crate) fn bot_command_receipt(
        &self,
        account: &str,
        key: &str,
    ) -> Result<Option<(String, CommandOutcome)>, Error> {
        database::block_on_storage(self.command_receipts().lookup(account, key))
            .map_err(Error::storage)?
    }
    #[cfg(test)]
    pub(crate) fn bot_command_once(
        &mut self,
        account: &str,
        key: &str,
        digest: &str,
        execute: impl FnOnce(&mut Self) -> Result<String, Error>,
    ) -> Result<CommandOutcome, Error> {
        let original = self.store.db.clone();
        self.store.db = original.command_scope_blocking().map_err(Error::storage)?;
        let result = self.bot_command_in_context(account, key, digest, execute);
        self.store.db = original;
        result
    }
    #[cfg(test)]
    pub(in crate::engine) fn bot_command_in_context(
        &mut self,
        account: &str,
        key: &str,
        digest: &str,
        execute: impl FnOnce(&mut Self) -> Result<String, Error>,
    ) -> Result<CommandOutcome, Error> {
        if let Some((previous, outcome)) = self.bot_command_receipt(account, key)? {
            return if previous == digest {
                Ok(outcome)
            } else {
                Err(Error::new(
                    Code::IdempotencyConflict,
                    "Key already belongs to a different command request.",
                ))
            };
        }
        let created = now();
        {
            let (count, bytes) = self
                .store
                .db
                .command_receipt_usage_blocking(account, created.saturating_sub(TTL))
                .map_err(Error::storage)?;
            if count >= crate::bots::models::RECEIPT_LIMIT || bytes + MAX_RESULT > MAX_BYTES {
                return Err(Error::new(
                    Code::IdempotencyCapacity,
                    "Wait for command receipts to expire.",
                ));
            }
            self.store
                .db
                .lock()
                .map_err(Error::storage)?
                .begin_command()
                .map_err(Error::storage)?;
        }
        let revision = self.revision;
        let changed = self.changed_conversation.clone();
        let outcome = execute(self).and_then(|result| {
            if result.len() > MAX_RESULT / 2 {
                return Err(Error::new(
                    Code::PayloadTooLarge,
                    "Command result is too large; use paged history.",
                ));
            }
            Ok(result)
        });
        // SQLITE_FULL/IOERR and RAISE(ROLLBACK) may abort the whole outer
        // transaction. Restore memory even when there is no savepoint left.
        if self
            .store
            .db
            .lock()
            .map_err(Error::storage)?
            .is_autocommit()
        {
            self.restore_command_state(revision, changed)?;
            return Err(Error::storage("Command transaction aborted."));
        }
        // A command may have touched multiple savepoints. Roll back the complete
        // effect on rejection, then persist the rejected outcome in the receipt.
        if outcome.is_err() {
            self.store
                .db
                .lock()
                .map_err(Error::storage)?
                .rollback_command_effect()
                .map_err(Error::storage)?;
            self.restore_command_state(revision, changed.clone())?;
        }
        // Availability failures are transient; no effect or terminal receipt.
        if let Err(error) = &outcome
            && error.code == Code::Unavailable
        {
            self.store
                .db
                .lock()
                .map_err(Error::storage)?
                .finish_command()
                .map_err(Error::storage)?;
            return Err(error.clone());
        }
        let outcome = CommandOutcome {
            result: outcome.as_ref().ok().cloned(),
            error: outcome.err(),
            created,
            expires: created + TTL,
            replayed: false,
            system_authority: None,
        };
        let persist = (|| {
            let raw = serde_json::to_string(&outcome).map_err(Error::storage)?;
            if raw.len() > MAX_RESULT {
                return Err(Error::new(
                    Code::PayloadTooLarge,
                    "Command receipt is too large.",
                ));
            }
            let batch = MetadataWrite {
                operations: vec![MetadataOp::CommandReceipt {
                    account,
                    key,
                    digest,
                    created,
                    expired_before: created.saturating_sub(TTL),
                    outcome: &raw,
                }],
            };
            self.store
                .db
                .save_metadata_blocking(&batch)
                .map_err(Error::storage)?;
            self.store
                .db
                .lock()
                .map_err(Error::storage)?
                .finish_command()
                .map_err(Error::storage)
        })();
        if let Err(error) = persist {
            let rollback = {
                let db = self.store.db.lock().map_err(Error::storage)?;
                if db.is_autocommit() {
                    Ok(())
                } else {
                    db.abort_command().map_err(Error::storage)
                }
            };
            self.restore_command_state(revision, changed)?;
            rollback?;
            return Err(error);
        }
        Ok(outcome)
    }
    #[cfg(test)]
    pub(in crate::engine) fn restore_command_state(
        &mut self,
        revision: u64,
        changed: Option<String>,
    ) -> Result<(), Error> {
        self.recovery_failed = true;
        // Rollback is rare. Recover authoritative metadata/cursors after SQLite
        // rollback instead of cloning the whole instance for every native call.
        let (data, cursors) = {
            let db = self.store.db.lock().map_err(Error::storage)?;
            let data = load_data(&db).map_err(Error::storage)?;
            let cursors = load_read_positions(&db).map_err(Error::storage)?;
            (data, cursors)
        };
        self.authorization =
            authorization::CompiledPolicy::compile(&data).map_err(Error::storage)?;
        self.bot_index = crate::bots::models::Index::build(&data);
        self.read_model.advance();
        self.data = data;
        self.revision = revision;
        self.changed_conversation = changed;
        self.store.read_positions = cursors;
        self.recovery_failed = false;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn fixture(path: &Path) -> Engine {
        let mut engine = Engine::open(path).unwrap();
        engine.execute(None, None, "/new room support").unwrap();
        engine
            .execute(None, None, "/bot new helper #support")
            .unwrap();
        engine.execute(None, None, "/grant helper admin").unwrap();
        engine
    }
    #[test]
    fn receipt_and_effect_are_atomic_and_replay_survives_restart() {
        let dir =
            std::env::temp_dir().join(format!("chat-command-receipt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("chat.sqlite");
        let mut engine = fixture(&path);
        let account = engine.data.users["helper"].id.clone();
        let run = |engine: &mut Engine| {
            engine
                .execute_formatted(Some("helper"), Some("support"), "/new room effect", true)
                .map_err(Error::storage)
        };
        let first = engine
            .bot_command_once(&account, "key", "digest", run)
            .unwrap();
        assert!(!first.replayed);
        let revision = engine.revision;
        assert!(
            engine
                .bot_command_once(&account, "key", "digest", run)
                .unwrap()
                .replayed
        );
        assert_eq!(engine.revision, revision);
        assert_eq!(
            engine
                .bot_command_once(&account, "key", "different", run)
                .unwrap_err()
                .code,
            Code::IdempotencyConflict
        );
        drop(engine);
        let mut engine = Engine::open(&path).unwrap();
        let replay = engine
            .bot_command_once(&account, "key", "digest", run)
            .unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.result, first.result);
        engine.store.db.lock().unwrap().execute_batch("CREATE TEMP TRIGGER fail_command_receipt BEFORE INSERT ON bot_command_receipts BEGIN SELECT RAISE(FAIL,'injected'); END;").unwrap();
        let revision = engine.revision;
        let before = engine.data.clone();
        assert!(
            engine
                .bot_command_once(&account, "rollback", "rollback", |engine| engine
                    .execute_formatted(Some("helper"), Some("support"), "/new room rollback", true)
                    .map_err(Error::storage))
                .is_err()
        );
        assert!(engine.data == before);
        assert_eq!(engine.revision, revision);
        assert!(
            engine
                .bot_command_receipt(&account, "rollback")
                .unwrap()
                .is_none()
        );
        let stored: bool = engine
            .store
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM entities WHERE kind='room' AND key='rollback')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!stored);
        // Message writes use nested savepoints too, and must roll back together.
        let sequence = engine.data.next_sequence;
        assert!(
            engine
                .bot_command_once(&account, "message", "message", |engine| engine
                    .execute_formatted(
                        Some("helper"),
                        Some("support"),
                        "/tell helper must roll back",
                        true
                    )
                    .map_err(Error::storage))
                .is_err()
        );
        assert_eq!(engine.data.next_sequence, sequence);
        assert!(engine.data.private.is_empty());
        engine.store.db.lock().unwrap().execute_batch("DROP TRIGGER fail_command_receipt; CREATE TEMP TRIGGER abort_command_receipt BEFORE INSERT ON bot_command_receipts BEGIN SELECT RAISE(ROLLBACK,'injected full transaction abort'); END;").unwrap();
        let before = engine.data.clone();
        let revision = engine.revision;
        assert!(
            engine
                .bot_command_once(&account, "aborted", "aborted", |engine| engine
                    .execute_formatted(Some("helper"), Some("support"), "/new room aborted", true)
                    .map_err(Error::storage))
                .is_err()
        );
        assert!(engine.data == before);
        assert_eq!(engine.revision, revision);
        assert!(!engine.data.rooms.contains_key("aborted"));
        assert!(engine.store.db.lock().unwrap().is_autocommit());
        drop(engine);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod limits_tests {
    use super::*;
    #[test]
    fn capacity_rejects_before_effect_and_expired_receipts_are_not_replayed() {
        let mut engine = Engine::open(Path::new(":memory:")).unwrap();
        engine.execute(None, None, "/new room support").unwrap();
        engine
            .execute(None, None, "/bot new helper #support")
            .unwrap();
        let account = engine.data.users["helper"].id.clone();
        let created = now();
        engine.store.db.lock().unwrap().execute("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<4096) INSERT INTO bot_command_receipts SELECT ?1,CAST(x AS TEXT),'digest',?2,'{}' FROM n",params![account,created]).unwrap();
        let result =
            engine.bot_command_once(&account, "new", "digest", |_| Ok("should never run".into()));
        assert_eq!(result.unwrap_err().code, Code::IdempotencyCapacity);
        engine
            .store
            .db
            .lock()
            .unwrap()
            .execute(
                "UPDATE bot_command_receipts SET created=?1",
                [created.saturating_sub(TTL + 1)],
            )
            .unwrap();
        assert!(engine.bot_command_receipt(&account, "1").unwrap().is_none());
        assert!(
            !engine
                .bot_command_once(&account, "1", "digest", |_| Ok("fresh invocation".into()))
                .unwrap()
                .replayed
        );
        let count: usize = engine
            .store
            .db
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM bot_command_receipts", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
    }
}
