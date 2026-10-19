//! Generic command effects and their outcomes share one outer SQLite transaction.
use super::*;
use crate::bots::error::{Code, Error};
use serde::{Deserialize, Serialize};

pub(super) const SCHEMA: &str = "CREATE TABLE bot_command_receipts(account_id TEXT NOT NULL,key TEXT NOT NULL,digest TEXT NOT NULL,created INTEGER NOT NULL,outcome TEXT NOT NULL,PRIMARY KEY(account_id,key)) WITHOUT ROWID; CREATE INDEX bot_command_receipts_created ON bot_command_receipts(created);";
const TTL: u64 = crate::bots::models::RECEIPT_TTL;
const MAX_RESULT: usize = 65536;
const MAX_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct CommandOutcome {
    pub result: Option<String>,
    pub error: Option<Error>,
    pub created: u64,
    pub expires: u64,
    pub replayed: bool,
}
impl CommandOutcome {
    pub fn into_result(self) -> Result<serde_json::Value, Error> {
        if let Some(error) = self.error {
            return Err(error);
        }
        Ok(
            serde_json::json!({"result":self.result.unwrap_or_default(),"replayed":self.replayed,"expires":self.expires}),
        )
    }
}
impl Engine {
    pub(crate) fn bot_command_receipt(
        &self,
        account: &str,
        key: &str,
    ) -> Result<Option<(String, CommandOutcome)>, Error> {
        use rusqlite::OptionalExtension;
        let db = self.db.lock().map_err(Error::storage)?;
        let row: Option<(String,String)> = db.query_row("SELECT digest,outcome FROM bot_command_receipts WHERE account_id=?1 AND key=?2 AND created>?3",params![account,key,now().saturating_sub(TTL)],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(Error::storage)?;
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
    pub(crate) fn bot_command_once(
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
            let db = self.db.lock().map_err(Error::storage)?;
            let (count,bytes):(usize,usize)=db.query_row("SELECT count(*),coalesce(sum(length(CAST(outcome AS BLOB))),0) FROM bot_command_receipts WHERE account_id=?1 AND created>?2",params![account,created.saturating_sub(TTL)],|r|Ok((r.get(0)?,r.get(1)?))).map_err(Error::storage)?;
            if count >= crate::bots::models::RECEIPT_LIMIT || bytes + MAX_RESULT > MAX_BYTES {
                return Err(Error::new(
                    Code::IdempotencyCapacity,
                    "Wait for command receipts to expire.",
                ));
            }
            db.execute_batch("SAVEPOINT bot_command_receipt;")
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
        if self.db.lock().map_err(Error::storage)?.is_autocommit() {
            self.restore_command_state(revision, changed)?;
            return Err(Error::storage("Command transaction aborted."));
        }
        // A command may have touched multiple savepoints. Roll back the complete
        // effect on rejection, then persist the rejected outcome in the receipt.
        if outcome.is_err() {
            self.db
                .lock()
                .map_err(Error::storage)?
                .execute_batch("ROLLBACK TO bot_command_receipt;")
                .map_err(Error::storage)?;
            self.restore_command_state(revision, changed.clone())?;
        }
        // Availability failures are transient; no effect or terminal receipt.
        if let Err(error) = &outcome
            && error.code == Code::Unavailable
        {
            self.db
                .lock()
                .map_err(Error::storage)?
                .execute_batch("RELEASE bot_command_receipt;")
                .map_err(Error::storage)?;
            return Err(error.clone());
        }
        let outcome = CommandOutcome {
            result: outcome.as_ref().ok().cloned(),
            error: outcome.err(),
            created,
            expires: created + TTL,
            replayed: false,
        };
        let persist = (|| {
            let raw = serde_json::to_string(&outcome).map_err(Error::storage)?;
            if raw.len() > MAX_RESULT {
                return Err(Error::new(
                    Code::PayloadTooLarge,
                    "Command receipt is too large.",
                ));
            }
            let db = self.db.lock().map_err(Error::storage)?;
            db.execute(
                "DELETE FROM bot_command_receipts WHERE created<=?1",
                [created.saturating_sub(TTL)],
            )
            .map_err(Error::storage)?;
            db.execute(
                "INSERT INTO bot_command_receipts SELECT ?1,?2,?3,?4,?5 WHERE EXISTS(SELECT 1 FROM entities WHERE kind='account' AND json_extract(json,'$.id')=?1)",
                params![account, key, digest, created, raw],
            )
            .map_err(Error::storage)?;
            db.execute_batch("RELEASE bot_command_receipt;")
                .map_err(Error::storage)
        })();
        if let Err(error) = persist {
            let rollback = {
                let db = self.db.lock().map_err(Error::storage)?;
                if db.is_autocommit() {
                    Ok(())
                } else {
                    db.execute_batch(
                        "ROLLBACK TO bot_command_receipt; RELEASE bot_command_receipt;",
                    )
                    .map_err(Error::storage)
                }
            };
            self.restore_command_state(revision, changed)?;
            rollback?;
            return Err(error);
        }
        Ok(outcome)
    }
    pub(in crate::engine) fn restore_command_state(
        &mut self,
        revision: u64,
        changed: Option<String>,
    ) -> Result<(), Error> {
        self.recovery_failed = true;
        // Rollback is rare. Recover authoritative metadata/cursors after SQLite
        // rollback instead of cloning the whole instance for every native call.
        let (data, cursors) = {
            let db = self.db.lock().map_err(Error::storage)?;
            let raw: String = db
                .query_row("SELECT json FROM state WHERE id=1", [], |r| r.get(0))
                .map_err(Error::storage)?;
            let mut data: Data = serde_json::from_str(&raw).map_err(Error::storage)?;
            load_entities(&db, &mut data).map_err(Error::storage)?;
            let mut cursors: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
            let mut query = db
                .prepare("SELECT username,conversation,sequence FROM read_positions")
                .map_err(Error::storage)?;
            let rows = query
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, u64>(2)?,
                    ))
                })
                .map_err(Error::storage)?;
            for row in rows {
                let (user, view, through) = row.map_err(Error::storage)?;
                cursors.entry(user).or_default().insert(view, through);
            }
            (data, cursors)
        };
        self.authorization =
            authorization::CompiledPolicy::compile(&data).map_err(Error::storage)?;
        self.bot_index = crate::bots::models::Index::build(&data);
        self.data = data;
        self.revision = revision;
        self.changed_conversation = changed;
        self.read_positions = cursors;
        self.clear_working_messages();
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
            .execute(None, None, "/bot new helper support")
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
        engine.db.lock().unwrap().execute_batch("CREATE TEMP TRIGGER fail_command_receipt BEFORE INSERT ON bot_command_receipts BEGIN SELECT RAISE(FAIL,'injected'); END;").unwrap();
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
        engine.db.lock().unwrap().execute_batch("DROP TRIGGER fail_command_receipt; CREATE TEMP TRIGGER abort_command_receipt BEFORE INSERT ON bot_command_receipts BEGIN SELECT RAISE(ROLLBACK,'injected full transaction abort'); END;").unwrap();
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
        assert!(engine.db.lock().unwrap().is_autocommit());
        drop(engine);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn v9_migration_preserves_metadata_and_message_receipts_and_failed_ddl_rolls_back() {
        let dir = std::env::temp_dir().join(format!("chat-v9-upgrade-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("chat.sqlite");
        let engine = fixture(&path);
        engine
            .db
            .lock()
            .unwrap()
            .execute_batch("DROP TABLE bot_command_receipts; UPDATE state SET json=json_set(json,'$.policy.audit',json(coalesce((SELECT json_group_array(json(json)) FROM (SELECT json FROM audit_entries ORDER BY sequence)),'[]'))); DROP TABLE audit_entries; DROP INDEX messages_reply; DROP TRIGGER message_accounts_insert; DROP TRIGGER message_accounts_update; DROP TABLE message_accounts; DROP INDEX reactions_username; PRAGMA user_version=9;")
            .unwrap();
        let before = engine.data.clone();
        drop(engine);
        let engine = Engine::open(&path).unwrap();
        assert!(engine.data == before);
        assert_eq!(
            engine
                .db
                .lock()
                .unwrap()
                .query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                .unwrap(),
            14
        );
        engine
            .db
            .lock()
            .unwrap()
            .execute_batch("PRAGMA user_version=9;")
            .unwrap();
        drop(engine);
        assert!(Engine::open(&path).is_err());
        let db = Connection::open(&path).unwrap();
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                .unwrap(),
            9
        );
        drop(db);
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
            .execute(None, None, "/bot new helper support")
            .unwrap();
        let account = engine.data.users["helper"].id.clone();
        let created = now();
        engine.db.lock().unwrap().execute("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<4096) INSERT INTO bot_command_receipts SELECT ?1,CAST(x AS TEXT),'digest',?2,'{}' FROM n",params![account,created]).unwrap();
        let result =
            engine.bot_command_once(&account, "new", "digest", |_| Ok("should never run".into()));
        assert_eq!(result.unwrap_err().code, Code::IdempotencyCapacity);
        engine
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
