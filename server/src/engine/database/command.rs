//! One native transaction owns a command effect and its durable retry outcome.
use super::{Error, MessageCommit, MessageWrite, MetadataWrite, Result};

pub(crate) enum CommandEffect<'a> {
    None,
    Message(&'a MessageWrite<'a>),
    Metadata(&'a MetadataWrite<'a>),
}
pub(crate) struct CommandWrite<'a> {
    pub(crate) account: &'a str,
    pub(crate) key: &'a str,
    pub(crate) digest: &'a str,
    pub(crate) created: u64,
    pub(crate) expired_before: u64,
    pub(crate) outcome: &'a str,
    pub(crate) effect: CommandEffect<'a>,
}
pub(crate) struct CommandCommit {
    pub(crate) outcome: String,
    pub(crate) replayed: bool,
    pub(crate) message: Option<MessageCommit>,
}
/// Called while the backend holds the submitting account's receipt lock. The
/// lock precedes any append counter or conversation row to prevent inversion.
pub(super) fn preflight(
    request: &CommandWrite<'_>,
    previous: Option<(String, String)>,
    usage: (usize, usize),
) -> Result<Option<String>> {
    if let Some((digest, outcome)) = previous {
        return if digest == request.digest {
            Ok(Some(outcome))
        } else {
            Err(Error::ReceiptConflict)
        };
    }
    if usage.0 >= crate::bots::models::RECEIPT_LIMIT
        || usage.1.saturating_add(65536) > 4 * 1024 * 1024
    {
        return Err(Error::ReceiptCapacity);
    }
    if request.outcome.len() > 65536 {
        return Err(Error::ReceiptPayload);
    }
    Ok(None)
}
impl super::Handle {
    pub(crate) async fn save_command(&self, request: &CommandWrite<'_>) -> Result<CommandCommit> {
        let mut session = self.acquire().await?;
        let result = session
            .save_write_async(&super::transaction::WriteRequest::Command(request), self)
            .await?;
        let (outcome, replayed) = result
            .command
            .ok_or(Error::Conversion("Missing command commit result"))?;
        Ok(CommandCommit {
            outcome,
            replayed,
            message: result.message,
        })
    }
}
