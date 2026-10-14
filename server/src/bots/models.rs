use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageInput {
    pub conversation_id: String,
    pub text: String,
    pub reply_to: Option<String>,
    #[serde(default)]
    pub markdown: bool,
}

pub struct SendResult {
    pub message: crate::engine::Message,
    pub replayed: bool,
}

#[derive(Serialize)]
pub struct Snapshot {
    pub kind: &'static str,
    pub account: Account,
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
    pub unread: crate::engine::Unread,
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
// An explicit projection insulates frozen fields from browser/model additions.
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
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Credential {
    pub label: String,
    pub digest: String,
    pub created: u64,
}

pub(crate) fn validate(data: &crate::engine::Data) -> Result<(), String> {
    let mut digests = std::collections::BTreeSet::new();
    for (id, registration) in &data.bots {
        if !data.users.values().any(|account| account.id == *id) || registration.tokens.len() > 8 {
            return Err("Invalid bot registration.".into());
        }
        let mut labels = std::collections::BTreeSet::new();
        for (token_id, token) in &registration.tokens {
            if uuid::Uuid::parse_str(token_id).is_err()
                || token.label.is_empty()
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
