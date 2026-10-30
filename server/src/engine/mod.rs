mod accounts;
mod auth_events;
pub(crate) use auth_events::{AuthEvent, AuthOutcome};
pub mod authorization;
mod avatar_store;
pub(crate) mod avatars;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "tests/benchmarks.rs"]
mod benchmarks;
#[path = "../bots/engine.rs"]
mod bot_api;
#[path = "../bots/commands.rs"]
mod bots;
mod cleanup;
mod command_write;
pub(crate) use command_write::NativeCommand;
mod command_read;
pub(crate) use command_read::CommandRead;
pub(crate) mod colors;
pub(crate) mod database;
mod dispatch;
mod error;
mod helpers;
pub use error::EngineError;
mod read_position;
mod session_write;
pub(crate) use read_position::CursorWrite;
pub(crate) use session_write::AuthStore;
mod history;
mod history_write;
pub(crate) mod media;
mod media_allocation;
mod media_store;
pub(crate) use media_store::MediaStore;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "tests/media.rs"]
mod media_tests;
pub(crate) use history_write::PendingHistory;
mod message_action;
pub(crate) use message_action::BrowserActionPlan;
mod message_write;
pub(crate) use message_write::PendingMessage;
mod messages;
mod metadata_write;
pub(crate) mod preferences;
pub(crate) use metadata_write::{MetadataPreparation, native_metadata_command};
mod models;
mod notices;
pub(crate) use notices::MetadataNotice;
mod presence;
mod profile;
mod queries;
mod read_model;
mod rooms;
pub(crate) mod snapshot;
mod storage;
pub(crate) use storage::PendingCommand;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // Test assertions fail the test on purpose.
mod tests;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "tests/trust.rs"]
mod trust_tests;

use changes::MetadataChange;
use database::Connection;
#[cfg(test)]
use database::params;
pub(crate) use dispatch::CommandChange;
use dispatch::CommandContext;
pub use dispatch::sudo_command;
pub(crate) use helpers::session_digest;
use helpers::*;
pub use helpers::{hash_password, valid_name, verify_password};
pub(crate) use history::HistoryStore;
pub use history::{HistoryError, HistoryRange};
pub use models::*;
pub(crate) use read_model::{DeliveryStamp, MetadataClock, SnapshotStamp};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::Path,
};

const VISIBLE_HISTORY: usize = 50;

pub struct Engine {
    #[cfg(not(test))]
    data: Data,
    #[cfg(test)]
    pub(crate) data: Data,
    pub revision: u64,
    color_revision: u64,
    committed_colors: colors::ColorDirectory,
    recovery_failed: bool,
    recovery_attempted: bool,
    metadata_dirty: bool,
    read_model: read_model::ReadModel,
    presence: presence::Presence,
    pub(crate) read_clock: u64,
    pub(crate) instance: String,
    pub(crate) read_revisions: BTreeMap<String, u64>,
    pub changed_conversation: Option<String>,
    max_users: usize,
    max_rooms: usize,
    max_messages: usize,
    media: media::State,
    store: storage::Store,
    pub(crate) bot_index: crate::bots::models::Index,
    bot_budgets: BTreeMap<String, (std::time::Instant, u32)>,
    authorization: authorization::CompiledPolicy,
}

impl Engine {
    #[cfg(test)]
    #[allow(clippy::unwrap_used)]
    pub(crate) fn test_corrupt_recovery_metadata(&self) {
        self.store
            .db
            .lock()
            .unwrap()
            .execute_batch("UPDATE state SET json='invalid' WHERE id=1")
            .unwrap();
    }
    pub(crate) fn needs_memory_recovery(&self) -> bool {
        self.recovery_failed && !self.recovery_attempted
    }
    pub(crate) fn begin_memory_recovery(&mut self) -> Option<database::Handle> {
        if !self.needs_memory_recovery() {
            return None;
        }
        self.recovery_attempted = true;
        Some(self.store.db.clone())
    }
    pub(crate) fn restore_committed_metadata(
        &mut self,
        snapshot: database::MetadataSnapshot,
        cursors: database::CursorRows,
    ) -> Result<(), EngineError> {
        let mut data = storage::data_from_snapshot(snapshot)?;
        crate::bots::models::validate(&data).map_err(EngineError::unavailable)?;
        let authorization =
            authorization::CompiledPolicy::compile(&data).map_err(EngineError::unavailable)?;
        data.sessions.retain(|_, session| session.expires > now());
        let mut positions = BTreeMap::<String, BTreeMap<String, u64>>::new();
        for (user, view, through) in cursors {
            positions.entry(user).or_default().insert(view, through);
        }
        self.history_store().reset_after_recovery()?;
        self.bot_index = crate::bots::models::Index::build(&data);
        self.data = data;
        self.authorization = authorization;
        self.store.read_positions = positions;
        self.metadata_dirty = false;
        self.changed_conversation = None;

        self.read_clock = self.read_clock.wrapping_add(1);
        self.read_revisions.clear();
        self.revision = self.revision.wrapping_add(1);
        self.read_model.advance();
        self.advance_colors();
        self.recovery_failed = false;
        self.recovery_attempted = false;
        Ok(())
    }
    pub(crate) fn ensure_memory(&self) -> Result<(), EngineError> {
        if self.recovery_failed {
            return Err(EngineError::Unavailable(
                "Server state recovery failed; restart the service.".into(),
            ));
        }
        self.store.db.ensure_state()?;
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn ensure_recovered(&self) -> Result<(), EngineError> {
        self.ensure_memory()?;
        self.store.db.ensure_available()?;
        Ok(())
    }
}

mod changes;
mod objects;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[path = "tests/review.rs"]
mod review_tests;
