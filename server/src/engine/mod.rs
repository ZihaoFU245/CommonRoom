mod accounts;
mod auth_events;
pub(crate) use auth_events::{AuthEvent, AuthOutcome};
pub mod authorization;
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
pub(crate) mod colors;
mod dispatch;
mod error;
mod helpers;
pub use error::EngineError;
mod history;
pub(crate) mod media;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "tests/media.rs"]
mod media_tests;
mod messages;
mod models;
mod notices;
pub(crate) use notices::MetadataNotice;
mod presence;
mod queries;
mod read_model;
mod rooms;
pub(crate) mod snapshot;
mod storage;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // Test assertions fail the test on purpose.
mod tests;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "tests/trust.rs"]
mod trust_tests;

use changes::MetadataChange;
pub(crate) use dispatch::CommandChange;
use dispatch::CommandContext;
pub use dispatch::sudo_command;
pub(crate) use helpers::session_digest;
use helpers::*;
pub use helpers::{hash_password, valid_name, verify_password};
pub(crate) use history::HistoryStore;
pub use history::{HistoryError, HistoryRange};
pub use models::*;
pub(crate) use read_model::{MetadataClock, SnapshotStamp};
use rusqlite::{Connection, params};
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
    working: Option<messages::WorkingMessage>,
    pub revision: u64,
    color_revision: u64,
    committed_colors: colors::ColorDirectory,
    recovery_failed: bool,
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
    pub(crate) fn ensure_recovered(&self) -> Result<(), EngineError> {
        if self.recovery_failed {
            return Err(EngineError::Unavailable(
                "Server state recovery failed; restart the service.".into(),
            ));
        }
        Ok(())
    }
}

mod changes;
mod objects;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[path = "tests/review.rs"]
mod review_tests;
