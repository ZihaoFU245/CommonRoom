//! Typed message persistence requests. These contain domain data, never SQL.
use super::{Error, Result};
use crate::engine::{Message, Reply};
use std::collections::BTreeSet;

#[derive(Clone, Copy)]
pub(crate) enum MessageMutation<'a> {
    Append {
        retention: usize,
    },
    Edit {
        reconcile_media: bool,
        actor: &'a str,
    },
    Reaction {
        active: bool,
        reaction: &'a str,
        user: &'a str,
    },
}
/// Receipt input contains only retry identity. The transaction supplies its
/// conversation and assigned sequence, so callers cannot persist a predicted cursor.
#[derive(Clone, Copy)]
pub(crate) struct MessageReceipt<'a> {
    pub(crate) account_id: &'a str,
    pub(crate) key: &'a str,
    pub(crate) digest: &'a str,
    pub(crate) created: u64,
}
#[derive(Clone, Copy)]
pub(crate) struct MessageWrite<'a> {
    pub(crate) conversation: &'a str,
    pub(crate) kind: &'a str,
    pub(crate) key: &'a str,
    pub(crate) revision: u64,
    pub(crate) expected_revision: Option<u64>,
    pub(crate) message: &'a Message,
    pub(crate) mutation: MessageMutation<'a>,
    pub(crate) new_conversation: Option<&'a str>,
    pub(crate) receipt: Option<MessageReceipt<'a>>,
    pub(crate) at: u64,
}

/// Stored identity/version. A legacy outer command transaction must finish its
/// receipt before this result can be published outside the engine lock.
pub(crate) struct MessageCommit {
    pub(crate) conversation: String,
    pub(crate) revision: u64,
    pub(crate) sequence: u64,
}

// Preserve the existing representation: reaction rows are stored separately.
#[derive(serde::Serialize)]
struct MessageRecord<'a> {
    id: &'a str,
    from: &'a str,
    author_id: &'a str,
    markdown: bool,
    edited: bool,
    private_id: &'a str,
    to: &'a Option<String>,
    text: &'a str,
    time: u64,
    sequence: u64,
    reply: &'a Option<Reply>,
    mentions: &'a BTreeSet<String>,
}
pub(super) fn serialize(message: &Message) -> Result<String> {
    serde_json::to_string(&MessageRecord {
        id: &message.id,
        from: &message.from,
        author_id: &message.author_id,
        markdown: message.markdown,
        edited: message.edited,
        private_id: &message.private_id,
        to: &message.to,
        text: &message.text,
        time: message.time,
        sequence: message.sequence,
        reply: &message.reply,
        mentions: &message.mentions,
    })
    .map_err(|_| Error::Conversion("Cannot serialize stored message"))
}
pub(super) fn number(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| Error::Conversion("Integer exceeds database range"))
}
