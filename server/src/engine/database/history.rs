//! Typed read requests/results and backend-independent pagination accounting.
//! SQL, reader snapshots and native decoding belong to the backend modules.
use super::{Backend, Error, Result};
use crate::engine::{HistoryRange, Message, Unread};
use std::collections::BTreeMap;

pub(crate) struct PageRead<'a> {
    pub(crate) conversation: &'a str,
    pub(crate) revision: u64,
    pub(crate) range: HistoryRange,
    pub(crate) limit: usize,
    pub(crate) max_bytes: usize,
}
pub(crate) struct PageResult {
    pub(crate) messages: Vec<Message>,
    pub(crate) sizes: Vec<usize>,
    pub(crate) has_before: bool,
    pub(crate) has_after: bool,
}
#[derive(Clone)]
pub(crate) struct UnreadHint {
    pub(crate) read: u64,
    pub(crate) unread: Unread,
}
pub(crate) struct UnreadRead<'a> {
    pub(crate) conversation: &'a str,
    pub(crate) read: u64,
    pub(crate) revision: u64,
    pub(crate) previous: Option<UnreadHint>,
}
pub(crate) struct UnreadBatch {
    pub(crate) results: Vec<Result<Unread>>,
    pub(crate) full: u64,
    pub(crate) incremental: u64,
}
pub(crate) type CleanupCounts = BTreeMap<String, (usize, usize)>;

pub(super) fn unsigned(value: i64) -> Result<u64> {
    u64::try_from(value).map_err(|_| Error::Conversion("Invalid stored nonnegative integer"))
}
pub(super) fn count(value: i64) -> Result<usize> {
    usize::try_from(value).map_err(|_| Error::Conversion("Invalid stored count"))
}
pub(super) fn decode_message(raw: &str) -> Result<Message> {
    serde_json::from_str(raw).map_err(|_| Error::Conversion("Invalid stored message JSON"))
}
#[derive(Default)]
struct WireSize(usize);
impl std::io::Write for WireSize {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub(crate) fn wire_size(message: &Message) -> Result<usize> {
    let mut size = WireSize::default();
    serde_json::to_writer(&mut size, message)
        .map_err(|_| Error::Conversion("Cannot size stored message"))?;
    Ok(size.0)
}
impl PageRead<'_> {
    pub(super) fn interval(&self, anchor: u64) -> (u64, u64, bool) {
        let range = self.range;
        let ascending = range.after.is_some() || range.start.is_some() || range.around.is_some();
        let start = if range.around.is_some() {
            anchor
        } else {
            range.start.unwrap_or(0)
        };
        let lower = start.max(range.after.map(|n| n.saturating_add(1)).unwrap_or(1));
        let upper = range.end.unwrap_or(9_007_199_254_740_991).min(
            range
                .before
                .map(|n| n.saturating_sub(1))
                .unwrap_or(9_007_199_254_740_991),
        );
        (lower, upper, ascending)
    }
    pub(super) fn finish(
        &self,
        mut messages: Vec<Message>,
        lower: u64,
        upper: u64,
        ascending: bool,
    ) -> Result<(Vec<Message>, Vec<usize>, u64, u64)> {
        let mut sizes = Vec::new();
        let mut bytes = 0usize;
        for message in &messages {
            let size = wire_size(message)?;
            if !sizes.is_empty() && bytes.saturating_add(size) > self.max_bytes {
                break;
            }
            bytes = bytes.saturating_add(size);
            sizes.push(size);
        }
        messages.truncate(sizes.len());
        if !ascending {
            messages.reverse();
            sizes.reverse();
        }
        let range = self.range;
        let before = messages
            .first()
            .map(|m| m.sequence)
            .unwrap_or(if ascending {
                lower
            } else {
                range.before.or(range.start).unwrap_or(0)
            });
        let after = messages.last().map(|m| m.sequence).unwrap_or(if ascending {
            range.after.or(range.end).unwrap_or(9_007_199_254_740_991)
        } else {
            upper
        });
        Ok((messages, sizes, before, after))
    }
}
pub(super) fn attach_reactions(
    messages: &mut [Message],
    rows: Vec<(i64, String, String)>,
) -> Result<()> {
    let positions: BTreeMap<_, _> = messages
        .iter()
        .enumerate()
        .map(|(i, m)| (m.sequence, i))
        .collect();
    for (sequence, reaction, user) in rows {
        if let Some(index) = positions.get(&unsigned(sequence)?) {
            messages[*index]
                .reactions
                .entry(reaction)
                .or_default()
                .insert(user);
        }
    }
    Ok(())
}

// A native pool lease selects an implementation, never a translated SQL string.
macro_rules! read_call {
    ($session:expr, $method:ident $(, $argument:expr)* $(,)?) => {{
        $session.ensure_available()?;
        let result = match $session.backend.get_mut() {
            Backend::Sqlite(db) => super::sqlite::history::$method(db $(, $argument)*).await,
            Backend::Postgres(db) => super::postgres::history::$method(db $(, $argument)*).await,
            Backend::Closed => Err(Error::Conversion("Database closed")),
        };
        $session.checked(result)
    }};
}
impl super::Connection {
    pub(super) async fn read_page(&mut self, request: &PageRead<'_>) -> Result<PageResult> {
        read_call!(self, page, request)
    }
    pub(super) async fn read_unreads(
        &mut self,
        user: &str,
        requests: &[UnreadRead<'_>],
    ) -> Result<UnreadBatch> {
        let batch = read_call!(self, unreads, user, requests)?;
        for result in &batch.results {
            if let Err(error) = result {
                self.observe_error(error);
            }
        }
        Ok(batch)
    }
    pub(super) async fn read_author(&mut self, ids: &[String], author: &str) -> Result<bool> {
        read_call!(self, has_author, ids, author)
    }
    pub(super) async fn read_sequence(&mut self, id: &str, sequence: u64) -> Result<bool> {
        read_call!(self, contains_sequence, id, sequence)
    }
    pub(super) async fn read_locate(&mut self, id: &str) -> Result<Option<(String, Message)>> {
        read_call!(self, locate, id)
    }
    pub(super) async fn read_cleanup_counts(
        &mut self,
        ids: &[String],
        cutoff: u64,
    ) -> Result<CleanupCounts> {
        read_call!(self, cleanup_counts, ids, cutoff)
    }
}
impl super::Database {
    pub(crate) async fn history_page(&self, request: &PageRead<'_>) -> Result<PageResult> {
        let mut reader = self.acquire_reader().await?;
        reader.read_page(request).await
    }
    pub(crate) async fn history_unreads(
        &self,
        user: &str,
        requests: &[UnreadRead<'_>],
    ) -> Result<UnreadBatch> {
        let mut reader = self.acquire_reader().await?;
        reader.read_unreads(user, requests).await
    }
    pub(crate) async fn history_has_author(&self, ids: &[String], author: &str) -> Result<bool> {
        let mut reader = self.acquire_reader().await?;
        reader.read_author(ids, author).await
    }
    pub(crate) async fn history_contains_sequence(&self, id: &str, sequence: u64) -> Result<bool> {
        let mut reader = self.acquire_reader().await?;
        reader.read_sequence(id, sequence).await
    }
    pub(crate) async fn history_locate(&self, id: &str) -> Result<Option<(String, Message)>> {
        let mut reader = self.acquire_reader().await?;
        reader.read_locate(id).await
    }
    pub(crate) async fn history_cleanup_counts(
        &self,
        ids: &[String],
        cutoff: u64,
    ) -> Result<CleanupCounts> {
        let mut reader = self.acquire_reader().await?;
        reader.read_cleanup_counts(ids, cutoff).await
    }
}
