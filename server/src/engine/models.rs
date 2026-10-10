use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// How an agent decides which messages to answer.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AgentReply {
    /// Answer every message in a conversation the agent belongs to.
    Auto,
    /// Answer only messages that mention the agent. This is the default.
    #[default]
    Mention,
}

/// How an agent decides whether an answer shows its search sources.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SourceMode {
    /// The model decides, and only the results it cites become links. An
    /// everyday answer such as today's weather usually cites nothing.
    #[default]
    Auto,
    /// Always list the results the answer used.
    Always,
    /// Never list sources, even after a search.
    Never,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Account {
    pub hash: String,
    #[serde(default)]
    pub id: String,
    pub disabled: bool,
    #[serde(default)]
    pub agent: bool,
    /// Provider credential used only by the server; never serialized to a client.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    #[serde(default)]
    pub reply: AgentReply,
    /// Web-search credential used only by the server; never sent to a client.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub search_key: String,
    /// Whether this agent may search the web. Off until an owner enables it.
    #[serde(default)]
    pub search: bool,
    /// How this agent decides which sources an answer shows.
    #[serde(default)]
    pub sources: SourceMode,
    /// Extra personality text the owner wrote, appended to the base rules.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prompt: String,
    /// Provider name, or empty for the default provider.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider: String,
    /// Provider base URL when the agent uses a gateway or a custom host.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub base_url: String,
    /// Model id, or empty for the default model.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
}
impl Account {
    /// Accounts that can hold a session and act as a person.
    pub fn is_human(&self) -> bool {
        !self.agent
    }
    /// Agents never receive administrator permission.
    pub fn is_agent(&self) -> bool {
        self.agent
    }
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Message {
    pub id: String,
    pub from: String,
    #[serde(default)]
    pub author_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub private_id: String,
    pub to: Option<String>,
    pub text: String,
    pub time: u64,
    #[serde(default)]
    pub sequence: u64,
    #[serde(default)]
    pub reactions: BTreeMap<String, BTreeSet<String>>,
    #[serde(default)]
    pub reply: Option<Reply>,
    #[serde(default)]
    pub mentions: BTreeSet<String>,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Reply {
    pub id: String,
    pub from: String,
    pub text: String,
}
#[derive(Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Room {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub owner_id: String,
    pub members: BTreeSet<String>,
    pub messages: VecDeque<Message>,
    #[serde(default)]
    pub revision: u64,
}
#[derive(Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PrivateChat {
    #[serde(default)]
    pub id: String,
    pub messages: VecDeque<Message>,
    pub revision: u64,
}
#[derive(Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Data {
    pub users: BTreeMap<String, Account>,
    pub rooms: BTreeMap<String, Room>,
    #[serde(default)]
    pub private: BTreeMap<String, PrivateChat>,
    #[serde(default)]
    pub next_sequence: u64,
    #[serde(default)]
    pub sessions: BTreeMap<String, Session>,
    #[serde(default)]
    pub policy: super::authorization::Policy,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Session {
    pub username: String,
    pub expires: u64,
}
#[derive(Serialize)]
pub struct RoomView {
    pub id: String,
    pub owner: String,
    pub permissions: Vec<String>,
    pub commands: Vec<crate::commands::Command>,
    pub name: String,
    pub members: BTreeSet<String>,
    /// Room members that are agents, so clients can label them.
    pub agents: BTreeSet<String>,
    pub messages: Vec<Message>,
}
#[derive(Serialize)]
pub struct Unread {
    pub count: usize,
    pub first: Option<u64>,
    pub through: u64,
    pub oldest: u64,
    pub revision: u64,
}
#[derive(Serialize)]
pub struct History {
    pub view: String,
    pub messages: Vec<Message>,
    pub revision: u64,
}
#[derive(Serialize)]
pub struct Access {
    pub id: String,
    pub permissions: Vec<String>,
    pub commands: Vec<crate::commands::Command>,
}
#[derive(Serialize)]
pub struct Snapshot {
    pub kind: &'static str,
    pub username: String,
    pub groups: Vec<String>,
    pub permissions: Vec<String>,
    pub account_access: Access,
    pub private_access: BTreeMap<String, Access>,
    pub private_permissions: Vec<String>,
    pub private_commands: Vec<crate::commands::Command>,
    pub policy_revision: u64,
    pub admin: bool,
    pub users: Vec<String>,
    /// Account name to `admin`, `user` or `agent`, for directory labels.
    pub roles: BTreeMap<String, &'static str>,
    /// Personality text per agent. Agents act on it, so it is not a secret.
    pub prompts: BTreeMap<String, String>,
    pub online: Vec<String>,
    pub rooms: Vec<RoomView>,
    pub direct: Vec<Message>,
    pub private_peers: Vec<String>,
    pub commands: Vec<crate::commands::Command>,
    pub available_rooms: Vec<String>,
    pub unread: BTreeMap<String, Unread>,
}
