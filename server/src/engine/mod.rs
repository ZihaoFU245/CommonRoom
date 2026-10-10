mod accounts;
pub mod authorization;
mod dispatch;
mod helpers;
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
use helpers::*;
pub use helpers::{hash_password, verify_password};
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
    max_users: usize,
    max_rooms: usize,
    max_messages: usize,
    db: Connection,
    online_connections: BTreeMap<String, usize>,
    read_positions: BTreeMap<String, BTreeMap<String, u64>>,
    authorization: authorization::CompiledPolicy,
    _lock: Option<std::fs::File>,
}
