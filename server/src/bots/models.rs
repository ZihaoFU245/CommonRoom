use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageInput {
    pub conversation_id: String,
    pub text: String,
    pub reply_to: Option<String>,
    #[serde(default = "default_markdown")]
    pub markdown: bool,
}

fn default_markdown() -> bool {
    true
}

pub struct SendResult {
    pub message: crate::engine::Message,
    pub replayed: bool,
}

#[derive(Serialize)]
pub struct Snapshot {
    pub kind: &'static str,
    pub account: Account,
    pub credential: serde_json::Value,
    pub conversations: Vec<Conversation>,
}
#[derive(Serialize)]
pub struct Account {
    pub id: String,
    pub username: String,
}
#[derive(Serialize)]
pub struct Conversation {
    pub id: String,
    pub view: String,
    pub permissions: Permissions,
    pub members: Vec<String>,
    pub unread: Option<crate::engine::Unread>,
    pub content_revision: u64,
}
#[derive(Serialize)]
pub struct Permissions {
    pub send: bool,
    pub reply: bool,
    pub react: bool,
}

pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
}
// An explicit projection insulates bot fields from browser/model additions.
pub(crate) fn message_json(m: &crate::engine::Message) -> serde_json::Value {
    serde_json::json!({"id":m.id,"from":m.from,"author_id":m.author_id,"text":m.text,
        "markdown":m.markdown,"edited":m.edited,"time":m.time,"sequence":m.sequence,
        "reply":m.reply.as_ref().map(|r| serde_json::json!({"id":r.id,"from":r.from,"text":r.text})),"mentions":m.mentions,"reactions":m.reactions})
}

pub(crate) struct Receipt {
    pub account_id: String,
    pub key: String,
    pub digest: String,
    pub conversation: String,
    pub sequence: u64,
    pub created: u64,
}

pub(crate) const RECEIPT_TTL: u64 = 86400;
pub(crate) const RECEIPT_LIMIT: usize = 4096;

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Registration {
    pub tokens: BTreeMap<String, Credential>,
    #[serde(default)]
    pub owner_id: Option<String>,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Credential {
    pub label: String,
    pub digest: String,
    pub created: u64,
    #[serde(default)]
    pub expires: Option<u64>,
    #[serde(default)]
    pub conversations: Option<std::collections::BTreeSet<String>>,
    #[serde(default)]
    pub operations: Option<std::collections::BTreeSet<String>>,
}

pub(crate) fn validate(data: &crate::engine::Data) -> Result<(), String> {
    let mut digests = std::collections::BTreeSet::new();
    for (id, registration) in &data.bots {
        if registration
            .owner_id
            .as_ref()
            .is_some_and(|owner| !data.users.values().any(|account| &account.id == owner))
        {
            return Err("Invalid bot owner.".into());
        }
        if !data.users.values().any(|account| account.id == *id) || registration.tokens.len() > 8 {
            return Err("Invalid bot registration.".into());
        }
        let mut labels = std::collections::BTreeSet::new();
        for (token_id, token) in &registration.tokens {
            if uuid::Uuid::parse_str(token_id).is_err()
                || token.label.is_empty()
                || token.expires.is_some_and(|at| at > 9_007_199_254_740_991)
                || token
                    .conversations
                    .as_ref()
                    .is_some_and(|ids| ids.iter().any(|id| !valid_id(id)))
                || token.operations.as_ref().is_some_and(|ops| {
                    ops.is_empty() || ops.iter().any(|op| !OPERATIONS.contains(&op.as_str()))
                })
                || token.digest.len() != 64
                || !token.digest.bytes().all(|b| b.is_ascii_hexdigit())
                || !digests.insert(&token.digest)
                || !labels.insert(&token.label)
            {
                return Err("Invalid bot credential metadata.".into());
            }
        }
    }
    Ok(())
}

/// Bot-only lookup tables, rebuilt after metadata commits, never per heartbeat.
#[derive(Default)]
pub(crate) struct Index {
    pub accounts: std::collections::HashMap<String, String>,
    pub credentials: std::collections::HashMap<String, (String, String)>,
    pub conversations: std::collections::HashMap<String, String>,
}
impl Index {
    pub fn build(data: &crate::engine::Data) -> Self {
        Self {
            accounts: data
                .users
                .iter()
                .map(|(name, a)| (a.id.clone(), name.clone()))
                .collect(),
            credentials: data
                .bots
                .iter()
                .flat_map(|(account, r)| {
                    r.tokens
                        .iter()
                        .map(move |(id, c)| (c.digest.clone(), (account.clone(), id.clone())))
                })
                .collect(),
            conversations: data
                .rooms
                .iter()
                .map(|(name, r)| (r.id.clone(), name.clone()))
                .chain(
                    data.private
                        .iter()
                        .map(|(key, c)| (c.id.clone(), format!("@private:{key}"))),
                )
                .collect(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CredentialInput {
    pub label: String,
    pub expires: u64,
    pub conversations: std::collections::BTreeSet<String>,
    pub operations: std::collections::BTreeSet<String>,
}
pub(crate) const OPERATIONS: &[&str] = &[
    "read", "send", "react", "read_ack", "activity", "config", "commands",
];
#[derive(Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub(crate) enum ConversationInput {
    #[serde(rename = "room")]
    Room { name: String },
    #[serde(rename = "direct")]
    Direct { username: String },
}
