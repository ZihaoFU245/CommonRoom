mod accounts;
mod dispatch;
mod helpers;
mod messages;
mod models;
mod queries;
mod rooms;
mod storage;
#[cfg(test)]
mod tests;

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
    read_positions: BTreeMap<String, BTreeMap<String, u64>>,
    _lock: Option<std::fs::File>,
}
