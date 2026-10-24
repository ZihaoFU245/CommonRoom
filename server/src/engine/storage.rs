//! Database ownership and transactional engine coordination.
use super::*;
mod accounts;
mod bootstrap;
mod cursors;
mod entities;
mod maintenance;
mod messages;
mod receipts;
mod schema;
#[cfg(test)]
pub(super) use entities::load_entities;
use entities::*;
#[cfg(test)]
pub(super) use maintenance::CLEANUP_COUNTS_SQL;
use maintenance::*;
use messages::*;
use schema::*;
mod auth;
mod command_receipts;
mod media;
mod retention;

/// Owns the database lifetime, including readers and the exclusive folder lock.
/// The lock is dropped last, after every connection owned by the engine.
pub(super) struct Store {
    history: std::sync::Arc<super::history::HistoryStore>,
    pub(super) db: std::sync::Arc<std::sync::Mutex<Connection>>,
    pub(super) read_positions: BTreeMap<String, BTreeMap<String, u64>>,
    pub(super) pending_deletes: Vec<(String, u64)>,
    pub(super) pending_cleanups: Vec<(String, u64)>,
    _lock: Option<std::fs::File>,
}

impl Store {
    fn checkpoint(&self) -> Result<(), EngineError> {
        self.db
            .lock()
            .map_err(EngineError::unavailable)?
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(EngineError::from)
    }
}

impl Engine {
    #[cfg(test)]
    pub(super) fn save_delta(&mut self, before: &Data) -> Result<(), EngineError> {
        self.save_metadata(&[], changes::Baseline::Data(before))
    }
    pub(super) fn save_metadata(
        &mut self,
        rooms: &[String],
        baseline: changes::Baseline<'_>,
    ) -> Result<(), EngineError> {
        let authorization = if self.authorization.revision != self.data.policy.revision {
            Some(
                super::authorization::CompiledPolicy::compile(&self.data)
                    .map_err(EngineError::unavailable)?,
            )
        } else {
            None
        };
        let notice = MetadataNotice::between(
            &self.data,
            baseline.view(),
            &self.authorization,
            authorization.as_ref().unwrap_or(&self.authorization),
        );
        let account_keys = baseline.view().keys;
        let previous_users = baseline.view().users;
        let writer = self.store.db.clone();
        let mut db = writer.lock().map_err(EngineError::unavailable)?;
        let tx = db.savepoint().map_err(EngineError::from)?;
        write_delta_view(&tx, &self.data, baseline.view())?;
        for room in rooms {
            tx.execute(
                "DELETE FROM read_positions WHERE conversation=?1",
                params![format!("room:{room}")],
            )
            .map_err(EngineError::from)?;
        }
        write_history_deletions(
            &tx,
            &self.store.pending_deletes,
            &self.store.pending_cleanups,
        )?;
        tx.commit().map_err(EngineError::from)?;
        for room in rooms {
            let key = format!("room:{room}");
            for positions in self.store.read_positions.values_mut() {
                positions.remove(&key);
            }
        }
        if let Some(authorization) = authorization {
            self.authorization = authorization;
        }
        if let Some(keys) = account_keys
            .filter(|keys| keys.rooms.is_empty() && keys.private.is_empty() && keys.bots.is_empty())
        {
            for name in &keys.users {
                if let Some(previous) = previous_users.and_then(|users| users.get(name)) {
                    self.bot_index.accounts.remove(&previous.id);
                }
                if let Some(account) = self.data.users.get(name) {
                    self.bot_index
                        .accounts
                        .insert(account.id.clone(), name.clone());
                }
            }
        } else {
            self.bot_index = crate::bots::models::Index::build(&self.data);
        }
        self.advance_colors();
        self.finish_metadata_commit(notice);
        Ok(())
    }
    pub fn checkpoint(&self) -> Result<(), EngineError> {
        self.store.checkpoint()
    }
}

impl Engine {
    pub(super) fn save_bot_metadata(&mut self, before: &MetadataChange) -> Result<(), EngineError> {
        let notice = MetadataNotice::between(
            &self.data,
            before.view(),
            &self.authorization,
            &self.authorization,
        );
        let writer = self.store.db.clone();
        let mut db = writer.lock().map_err(EngineError::unavailable)?;
        let tx = db.savepoint().map_err(EngineError::from)?;
        write_state(&tx, &self.data)?;
        tx.commit().map_err(EngineError::from)?;
        self.bot_index = crate::bots::models::Index::build(&self.data);
        self.finish_metadata_commit(notice);
        Ok(())
    }

    pub(super) fn save_empty_private(&mut self, key: &str) -> Result<(), EngineError> {
        let chat = self
            .data
            .private
            .get(key)
            .ok_or_else(|| EngineError::NotFound("Private conversation not found.".into()))?;
        let writer = self.store.db.clone();
        let mut db = writer.lock().map_err(EngineError::unavailable)?;
        let tx = db.savepoint().map_err(EngineError::from)?;
        put_entity(&tx, "private", key, &private_metadata(chat))?;
        tx.commit().map_err(EngineError::from)?;
        self.bot_index
            .conversations
            .insert(chat.id.clone(), format!("@private:{key}"));
        self.read_model.advance();
        self.revision = self.revision.wrapping_add(1);
        self.record_metadata(
            self.scope_notice(
                [authorization::Scope::Private(
                    self.data.private[key].id.clone(),
                )]
                .into(),
            ),
        );
        Ok(())
    }
}
