//! Database ownership and transactional engine coordination.
use super::*;
use database::{MetadataOp, MetadataWrite};
#[cfg(test)]
mod accounts;
mod bootstrap;
#[cfg(test)]
mod cursors;
mod entities;
#[cfg(test)]
mod maintenance;
mod migration;
#[cfg(test)]
mod receipts;
pub(super) use entities::data_from_snapshot;
#[cfg(test)]
pub(super) use entities::load_entities;
use entities::*;
#[cfg(test)]
pub(super) use maintenance::CLEANUP_COUNTS_SQL;
mod auth;
pub(super) use auth::session_write;
mod command_receipts;
pub(crate) use command_receipts::PendingCommand;
mod media;
mod retention;

pub(in crate::engine) fn private_json(chat: &PrivateChat) -> Result<String, EngineError> {
    Ok(serde_json::to_string(&private_metadata(chat))?)
}

/// Shares the database dispatcher and cache lifetime. The dispatcher owns every
/// pool and keeps the exclusive folder lock until the final history owner drops.
pub(super) struct Store {
    history: std::sync::Arc<super::history::HistoryStore>,
    pub(super) db: database::Handle,
    pub(super) read_positions: BTreeMap<String, BTreeMap<String, u64>>,
}

#[cfg(test)]
impl Store {
    fn checkpoint(&self) -> Result<(), EngineError> {
        database::block_on_storage(self.db.checkpoint())?.map_err(EngineError::from)
    }
}

impl Engine {
    pub(super) fn metadata_operations(
        &self,
        before: &MetadataChange,
    ) -> Result<Vec<MetadataOp<'static>>, EngineError> {
        let mut batch = MetadataWrite::default();
        write_delta_entities(&mut batch, &self.data, before.view())?;
        Ok(batch.operations)
    }
    #[cfg(test)]
    pub(super) fn save_delta(&mut self, before: &Data) -> Result<(), EngineError> {
        self.save_metadata(&[], changes::Baseline::Data(before))
    }
    #[cfg(test)]
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
        let mut batch = MetadataWrite::default();
        write_delta_view(&mut batch, &self.data, baseline.view())?;
        for room in rooms {
            batch
                .operations
                .push(MetadataOp::RemoveConversationCursors {
                    conversation: format!("room:{room}"),
                });
        }
        self.store.db.save_metadata_blocking(&batch)?;
        drop(batch);
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
    pub(crate) fn checkpoint_async(
        &self,
    ) -> impl std::future::Future<Output = Result<(), EngineError>> + Send + 'static + use<> {
        let database = self.store.db.clone();
        async move { database.checkpoint().await.map_err(EngineError::from) }
    }
    #[cfg(test)]
    pub fn checkpoint(&self) -> Result<(), EngineError> {
        self.store.checkpoint()
    }
}

impl Engine {
    #[cfg(test)]
    pub(super) fn save_bot_metadata(&mut self, before: &MetadataChange) -> Result<(), EngineError> {
        let notice = MetadataNotice::between(
            &self.data,
            before.view(),
            &self.authorization,
            &self.authorization,
        );
        let mut batch = MetadataWrite::default();
        push_state(&mut batch, &self.data)?;
        self.store.db.save_metadata_blocking(&batch)?;
        self.bot_index = crate::bots::models::Index::build(&self.data);
        self.finish_metadata_commit(notice);
        Ok(())
    }
}
