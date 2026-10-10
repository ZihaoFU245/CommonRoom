mod accounts;
mod agents;
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

pub(crate) use agents::AgentScope;
// The registry and default model are re-exported for the resolver and for
// tests; the binary itself reaches them through those callers.
#[allow(unused_imports)]
pub use agents::{
    AGENT_ERROR_PREFIX, DEFAULT_MODEL, KNOWN_PROVIDERS, agent_system_prompt, resolve_provider,
};
use dispatch::CommandContext;
pub use dispatch::sudo_command;
use helpers::*;
pub use helpers::{hash_password, verify_password};
pub use models::*;
use rusqlite::{Connection, params};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::Path,
};

const VISIBLE_HISTORY: usize = 50;
/// Conversation turns handed to an agent as context.
pub(super) const AGENT_CONTEXT: usize = 20;

/// One agent reply the caller must produce outside the engine lock.
///
/// The provider credential travels with the trigger so the network call never
/// runs while the engine mutex is held. Never log or serialize this value.
#[derive(Clone, PartialEq, Eq)]
pub struct AgentJob {
    pub name: String,
    pub api_key: String,
    /// Web-search credential, empty unless the agent may search.
    pub search_key: String,
    /// Whether the agent may search the web for this answer.
    pub search: bool,
    /// How the agent decides which sources the answer shows.
    pub sources: SourceMode,
    /// The owner's personality text, appended to the base rules.
    pub prompt: String,
    /// Provider name, base URL, and model id the answer is requested from.
    pub provider: String,
    pub base_url: String,
    pub model: String,
    /// Conversation to reply in, as a view string such as `room:lobby`.
    pub view: String,
    /// Retained messages used as conversation context, oldest first.
    pub context: Vec<Message>,
    /// The message that triggered this reply.
    pub trigger: String,
}
/// Result of one dispatched command: its console text plus any agent work the
/// caller owns once the engine lock is released.
#[derive(Debug)]
pub struct Execution {
    pub reply: String,
    pub agents: Vec<AgentJob>,
}

// Debug output must never expose a provider or search credential.
impl std::fmt::Debug for AgentJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentJob")
            .field("name", &self.name)
            .field("api_key", &"[redacted]")
            .field("search_key", &"[redacted]")
            .field("search", &self.search)
            .field("sources", &self.sources)
            .field("prompt", &self.prompt)
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("view", &self.view)
            .field("context", &self.context.len())
            .field("trigger", &self.trigger)
            .finish()
    }
}

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
    /// Agents with a reply being generated right now, so one trigger produces
    /// exactly one answer.
    agent_pending: BTreeSet<String>,
    /// Agent work queued by the command that just ran.
    agent_queue: Vec<AgentJob>,
    _lock: Option<std::fs::File>,
}
