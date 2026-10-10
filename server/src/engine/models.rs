use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Account {
    pub hash: String,
    #[serde(default)]
    pub id: String,
    pub disabled: bool,
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
    pub online: Vec<String>,
    pub rooms: Vec<RoomView>,
    pub direct: Vec<Message>,
    pub private_peers: Vec<String>,
    pub commands: Vec<crate::commands::Command>,
    pub available_rooms: Vec<String>,
    pub unread: BTreeMap<String, Unread>,
}
