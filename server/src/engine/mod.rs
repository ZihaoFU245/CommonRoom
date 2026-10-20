mod accounts;
pub mod authorization;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod benchmarks;
#[path = "../bots/engine.rs"]
mod bot_api;
#[path = "../bots/commands.rs"]
mod bots;
mod dispatch;
mod helpers;
mod history;
pub(crate) mod media;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod media_tests;
mod messages;
mod models;
mod queries;
mod rooms;
mod storage;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // Test assertions fail the test on purpose.
mod tests;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod trust_tests;

use dispatch::CommandContext;
pub use dispatch::sudo_command;
use helpers::*;
pub use helpers::{hash_password, valid_name, verify_password};
pub(crate) use history::HistoryStore;
pub use history::{HistoryError, HistoryRange};
pub use models::*;
use rusqlite::{Connection, params};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::Path,
};

const VISIBLE_HISTORY: usize = 50;

pub struct Engine {
    pub data: Data,
    pub revision: u64,
    recovery_failed: bool,
    pub(crate) presence_revision: u64,
    pub(crate) read_clock: u64,
    pub(crate) instance: String,
    pub(crate) read_revisions: BTreeMap<String, u64>,
    pub changed_conversation: Option<String>,
    max_users: usize,
    max_rooms: usize,
    max_messages: usize,
    pub(crate) media_config: crate::config::MediaConfig,
    db: std::sync::Arc<std::sync::Mutex<Connection>>,
    pub(crate) history_store: std::sync::Arc<history::HistoryStore>,
    pending_deletes: Vec<(String, u64)>,
    pending_cleanups: Vec<(String, u64)>,
    online_connections: BTreeMap<String, usize>,
    pub(crate) bot_index: crate::bots::models::Index,
    bot_budgets: BTreeMap<String, (std::time::Instant, u32)>,
    read_positions: BTreeMap<String, BTreeMap<String, u64>>,
    authorization: authorization::CompiledPolicy,
    _lock: Option<std::fs::File>,
}

impl Engine {
    pub(crate) fn ensure_recovered(&self) -> Result<(), String> {
        if self.recovery_failed {
            return Err("Server state recovery failed; restart the service.".into());
        }
        Ok(())
    }
}
