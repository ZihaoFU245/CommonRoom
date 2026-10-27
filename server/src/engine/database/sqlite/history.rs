//! Sqlite history SQL and coherent native reader snapshots.
use super::super::{
    Error, Result,
    history::{
        CleanupCounts, PageRead, PageResult, UnreadBatch, UnreadRead, attach_reactions, count,
        decode_message, unsigned,
    },
    message::number,
};
use crate::engine::{Message, Unread};
use sqlx::{Connection as _, SqliteConnection};

pub(crate) const PAGE_ASC_SQL: &str = "SELECT json FROM messages WHERE conversation=?1 AND sequence>=?2 AND sequence<=?3 ORDER BY sequence ASC LIMIT ?4";
const PAGE_DESC_SQL: &str = "SELECT json FROM messages WHERE conversation=?1 AND sequence>=?2 AND sequence<=?3 ORDER BY sequence DESC LIMIT ?4";
pub(crate) const CLEANUP_COUNTS_SQL: &str = "SELECT targets.value,count(DISTINCT m.sequence),count(refs.media_id) FROM json_each(?1) AS targets LEFT JOIN messages AS m ON m.conversation=targets.value AND json_extract(m.json,'$.time')<?2 LEFT JOIN message_media AS refs ON refs.conversation=m.conversation AND refs.sequence=m.sequence GROUP BY targets.value";

pub(in crate::engine::database) async fn page(
    db: &mut SqliteConnection,
    request: &PageRead<'_>,
) -> Result<PageResult> {
    let mut snapshot = db.begin().await?;
    let result = page_in(&mut snapshot, request).await;
    snapshot.rollback().await?;
    result
}
async fn page_in(db: &mut SqliteConnection, request: &PageRead<'_>) -> Result<PageResult> {
    let stored: Option<i64> = sqlx::query_scalar("SELECT revision FROM entities WHERE kind IN ('room','private') AND json_extract(json,'$.id')=?1")
        .bind(request.conversation).fetch_optional(&mut *db).await?;
    if stored != Some(number(request.revision)?) {
        return Err(Error::RevisionConflict);
    }
    let limit =
        i64::try_from(request.limit).map_err(|_| Error::Conversion("Invalid history limit"))?;
    let anchor = if let Some(around) = request.range.around {
        let anchor: Option<i64> = sqlx::query_scalar("SELECT sequence FROM messages WHERE conversation=?1 AND sequence<=?2 ORDER BY sequence DESC LIMIT 1 OFFSET ?3")
            .bind(request.conversation).bind(number(around)?).bind(limit / 2)
            .fetch_optional(&mut *db).await?;
        unsigned(anchor.unwrap_or(0))?
    } else {
        0
    };
    let (lower, upper, ascending) = request.interval(anchor);
    let sql = if ascending {
        PAGE_ASC_SQL
    } else {
        PAGE_DESC_SQL
    };
    let rows: Vec<String> = sqlx::query_scalar(sql)
        .bind(request.conversation)
        .bind(number(lower)?)
        .bind(number(upper)?)
        .bind(limit)
        .fetch_all(&mut *db)
        .await?;
    let mut messages = rows
        .iter()
        .map(|raw| decode_message(raw))
        .collect::<Result<Vec<_>>>()?;
    reactions(db, request.conversation, &mut messages).await?;
    let (messages, sizes, before, after) = request.finish(messages, lower, upper, ascending)?;
    let (has_before, has_after): (bool, bool) = sqlx::query_as("SELECT EXISTS(SELECT 1 FROM messages WHERE conversation=?1 AND sequence<?2),EXISTS(SELECT 1 FROM messages WHERE conversation=?1 AND sequence>?3)")
        .bind(request.conversation).bind(number(before)?).bind(number(after)?).fetch_one(db).await?;
    Ok(PageResult {
        messages,
        sizes,
        has_before,
        has_after,
    })
}
async fn reactions(db: &mut SqliteConnection, id: &str, messages: &mut [Message]) -> Result<()> {
    let Some(low) = messages.iter().map(|m| m.sequence).min() else {
        return Ok(());
    };
    let high = messages.iter().map(|m| m.sequence).max().unwrap_or(low);
    let rows: Vec<(i64,String,String)> = sqlx::query_as("SELECT sequence,reaction,username FROM reactions WHERE conversation=?1 AND sequence BETWEEN ?2 AND ?3")
        .bind(id).bind(number(low)?).bind(number(high)?).fetch_all(db).await?;
    attach_reactions(messages, rows)
}
pub(in crate::engine::database) async fn has_author(
    db: &mut SqliteConnection,
    ids: &[String],
    author: &str,
) -> Result<bool> {
    if ids.is_empty() {
        return Ok(false);
    }
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM json_each(?1) AS c JOIN messages AS m ON m.conversation=c.value WHERE json_extract(m.json,'$.from')=?2)").bind(serde_json::to_string(ids).map_err(|_| Error::Conversion("Cannot serialize conversation selection"))?).bind(author).fetch_one(db).await?)
}
pub(in crate::engine::database) async fn contains_sequence(
    db: &mut SqliteConnection,
    id: &str,
    sequence: u64,
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM messages WHERE conversation=?1 AND sequence=?2)",
    )
    .bind(id)
    .bind(number(sequence)?)
    .fetch_one(db)
    .await?)
}
pub(in crate::engine::database) async fn locate(
    db: &mut SqliteConnection,
    id: &str,
) -> Result<Option<(String, Message)>> {
    let mut snapshot = db.begin().await?;
    let result = locate_in(&mut snapshot, id).await;
    snapshot.rollback().await?;
    result
}
async fn locate_in(db: &mut SqliteConnection, id: &str) -> Result<Option<(String, Message)>> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT conversation,json FROM messages WHERE json_extract(json,'$.id')=?1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?;
    let Some((conversation, raw)) = row else {
        return Ok(None);
    };
    let mut message = decode_message(&raw)?;
    reactions(db, &conversation, std::slice::from_mut(&mut message)).await?;
    Ok(Some((conversation, message)))
}
pub(in crate::engine::database) async fn cleanup_counts(
    db: &mut SqliteConnection,
    ids: &[String],
    cutoff: u64,
) -> Result<CleanupCounts> {
    let mut snapshot = db.begin().await?;
    let result = cleanup_counts_in(&mut snapshot, ids, cutoff).await;
    snapshot.rollback().await?;
    result
}
async fn cleanup_counts_in(
    db: &mut SqliteConnection,
    ids: &[String],
    cutoff: u64,
) -> Result<CleanupCounts> {
    let mut totals = CleanupCounts::new();
    for batch in ids.chunks(256) {
        let rows: Vec<(String, i64, i64)> = sqlx::query_as(CLEANUP_COUNTS_SQL)
            .bind(
                serde_json::to_string(batch)
                    .map_err(|_| Error::Conversion("Cannot serialize cleanup selection"))?,
            )
            .bind(number(cutoff)?)
            .fetch_all(&mut *db)
            .await?;
        for (id, messages, attachments) in rows {
            totals.insert(id, (count(messages)?, count(attachments)?));
        }
    }
    Ok(totals)
}
pub(in crate::engine::database) async fn unreads(
    db: &mut SqliteConnection,
    user: &str,
    requests: &[UnreadRead<'_>],
) -> Result<UnreadBatch> {
    let mut snapshot = db.begin().await?;
    let mut batch = UnreadBatch {
        results: Vec::with_capacity(requests.len()),
        full: 0,
        incremental: 0,
    };
    for request in requests {
        let result = unread_in(&mut snapshot, user, request).await;
        match result {
            Ok((unread, incremental)) => {
                if incremental {
                    batch.incremental += 1;
                } else {
                    batch.full += 1;
                }
                batch.results.push(Ok(unread));
            }
            Err(error) => batch.results.push(Err(error)),
        }
    }
    snapshot.rollback().await?;
    Ok(batch)
}
async fn unread_in(
    db: &mut SqliteConnection,
    user: &str,
    request: &UnreadRead<'_>,
) -> Result<(Unread, bool)> {
    let id = request.conversation;
    let read = request.read;
    let revision = request.revision;
    let stored: Option<i64> = sqlx::query_scalar("SELECT revision FROM entities WHERE kind IN ('room','private') AND json_extract(json,'$.id')=?1").bind(id).fetch_optional(&mut *db).await?;
    if stored != Some(number(revision)?) {
        return Err(Error::RevisionConflict);
    }
    let (oldest,through): (i64,i64) = sqlx::query_as("SELECT coalesce((SELECT oldest FROM retention WHERE conversation=?1),0),coalesce((SELECT sequence FROM messages WHERE conversation=?1 ORDER BY sequence DESC LIMIT 1),0)").bind(id).fetch_one(&mut *db).await?;
    let (oldest, through) = (unsigned(oldest)?, unsigned(through)?);
    if let Some(previous) = request.previous.as_ref().filter(|previous| {
        read >= previous.read
            && through >= previous.unread.through
            && (oldest == previous.unread.oldest || previous.unread.through == 0)
    }) {
        // A revision delta equal to the number of appended rows proves that no
        // edit/reaction/deletion intervened. Global sequence gaps are harmless.
        let (appended,added): (i64,i64) = sqlx::query_as("SELECT count(*),coalesce(sum(sequence>?3 AND json_extract(json,'$.from')!=?4),0) FROM messages WHERE conversation=?1 AND sequence>?2")
            .bind(id).bind(number(previous.unread.through)?).bind(number(read)?).bind(user)
            .fetch_one(&mut *db).await?;
        if revision.wrapping_sub(previous.unread.revision) == unsigned(appended)? {
            let removed = if read == previous.read {
                0
            } else {
                let removed: i64 = sqlx::query_scalar("SELECT count(*) FROM messages WHERE conversation=?1 AND sequence>?2 AND sequence<=?3 AND json_extract(json,'$.from')!=?4")
                    .bind(id).bind(number(previous.read)?).bind(number(read.min(previous.unread.through))?).bind(user)
                    .fetch_one(&mut *db).await?;
                count(removed)?
            };
            let added = count(added)?;
            if let Some(total) = previous
                .unread
                .count
                .checked_sub(removed)
                .and_then(|n| n.checked_add(added))
            {
                let first = if total == 0 {
                    None
                } else if previous.unread.first.is_some_and(|n| n > read) {
                    previous.unread.first
                } else {
                    let first: Option<i64> = sqlx::query_scalar("SELECT min(sequence) FROM messages WHERE conversation=?1 AND sequence>?2 AND json_extract(json,'$.from')!=?3")
                        .bind(id).bind(number(read)?).bind(user).fetch_one(&mut *db).await?;
                    first.map(unsigned).transpose()?
                };
                return Ok((
                    Unread {
                        count: total,
                        first,
                        oldest,
                        through,
                        revision,
                    },
                    true,
                ));
            }
        }
    }
    let (total,first): (i64,Option<i64>) = sqlx::query_as("SELECT count(*),min(sequence) FROM messages WHERE conversation=?1 AND sequence>?2 AND json_extract(json,'$.from')!=?3")
        .bind(id).bind(number(read)?).bind(user).fetch_one(db).await?;
    Ok((
        Unread {
            count: count(total)?,
            first: first.map(unsigned).transpose()?,
            oldest,
            through,
            revision,
        },
        false,
    ))
}
