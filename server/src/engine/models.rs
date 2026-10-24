use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Account {
    pub hash: String,
    pub id: String,
    pub disabled: bool,
    /// Two-level override tree: @global root and sparse stable-room-ID branches.
    /// Inherited/default values are resolved on reads, never copied into branches.
    #[serde(default)]
    pub metadata: BTreeMap<String, BTreeMap<String, String>>,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Message {
    pub id: String,
    pub from: String,
    pub author_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub private_id: String,
    pub to: Option<String>,
    pub text: String,
    #[serde(default)]
    pub markdown: bool,
    #[serde(default)]
    pub edited: bool,
    pub time: u64,
    pub sequence: u64,
    #[serde(default)]
    pub reactions: BTreeMap<String, BTreeSet<String>>,
    pub reply: Option<Reply>,
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
    pub id: String,
    pub owner_id: String,
    pub members: BTreeSet<String>,
    pub revision: u64,
}
#[derive(Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PrivateChat {
    pub id: String,
    pub revision: u64,
}
#[derive(Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Data {
    pub bots: BTreeMap<String, crate::bots::models::Registration>,
    pub users: BTreeMap<String, Account>,
    pub rooms: BTreeMap<String, Room>,
    pub private: BTreeMap<String, PrivateChat>,
    pub next_sequence: u64,
    pub sessions: BTreeMap<String, Session>,
    pub policy: super::authorization::Policy,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Session {
    pub username: String,
    pub expires: u64,
}
#[derive(Clone, Serialize)]
pub struct RoomView {
    pub id: String,
    pub owner: String,
    pub permissions: Vec<String>,
    pub commands: Vec<crate::commands::Command>,
    pub name: String,
    pub members: BTreeSet<String>,
    pub messages: super::history::MessagePage,
    /// Room-scoped color overrides; the snapshot's global `colors` apply otherwise.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub colors: BTreeMap<String, String>,
}
#[derive(Clone, Serialize)]
pub struct Unread {
    pub count: usize,
    pub first: Option<u64>,
    pub through: u64,
    pub oldest: u64,
    pub revision: u64,
}
#[derive(Serialize)]
pub struct History {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name_colors: Option<super::colors::MessageColors>,
    pub view: String,
    pub messages: super::history::MessagePage,
    pub revision: u64,
    pub has_before: bool,
    pub has_after: bool,
}
#[derive(Clone, Serialize)]
pub struct Access {
    pub id: String,
    pub permissions: Vec<String>,
    pub commands: Vec<crate::commands::Command>,
}
#[derive(Clone, Serialize)]
pub struct SyncVersion {
    pub instance: String,
    pub revision: u64,
}
#[derive(Clone, Serialize)]
pub struct Snapshot {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub color_version: String,
    pub sync: SyncVersion,
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
    pub avatars: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub colors: BTreeMap<String, String>,
    pub online: Vec<String>,
    pub rooms: Vec<RoomView>,
    pub direct: Vec<Message>,
    pub private_peers: Vec<String>,
    pub commands: Vec<crate::commands::Command>,
    pub available_rooms: Vec<String>,
    pub unread: BTreeMap<String, Unread>,
}
