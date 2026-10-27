//! Postgres history SQL and coherent native reader snapshots.
use super::super::{
    Error, Result,
    history::{
        CleanupCounts, PageRead, PageResult, UnreadBatch, UnreadRead, attach_reactions, count,
        decode_message, unsigned,
    },
    message::number,
};
use crate::engine::{Message, Unread};
use sqlx::{Connection as _, PgConnection};

pub(crate) const CLEANUP_COUNTS_SQL: &str = "SELECT targets.value,count(DISTINCT m.sequence),count(refs.media_id) FROM unnest($1::text[]) AS targets(value) LEFT JOIN messages AS m ON m.conversation=targets.value AND ((m.projection #>> '{time}'))::bigint<$2 LEFT JOIN message_media AS refs ON refs.conversation=m.conversation AND refs.sequence=m.sequence GROUP BY targets.value";

pub(in crate::engine::database) async fn page(
    db: &mut PgConnection,
    request: &PageRead<'_>,
) -> Result<PageResult> {
    let limit =
        i64::try_from(request.limit).map_err(|_| Error::Conversion("Invalid history limit"))?;
    let (lower, upper, ascending) = request.interval(0);
    // Only a compile-time direction is interpolated. All request data is bound.
    let direction = if ascending { "ASC" } else { "DESC" };
    let sql = format!(
        r#"
WITH anchor AS (
 SELECT sequence FROM messages WHERE conversation=$1 AND sequence<=$5
 ORDER BY sequence DESC LIMIT 1 OFFSET ($4/2)
), page AS MATERIALIZED (
 SELECT sequence,json FROM messages WHERE conversation=$1
 AND sequence>=CASE WHEN $5::bigint IS NOT NULL THEN greatest(coalesce((SELECT sequence FROM anchor),0),1) ELSE $2 END
 AND sequence<=$3 ORDER BY sequence {direction} LIMIT $4
)
SELECT (SELECT revision FROM entities WHERE kind IN ('room','private') AND projection #>> '{{id}}'=$1),
 ARRAY(SELECT json FROM page ORDER BY sequence {direction}),
 coalesce((SELECT json_agg(json_build_array(sequence,reaction,username)) FROM reactions
           WHERE conversation=$1 AND sequence IN (SELECT sequence FROM page)),'[]'::json),
 (SELECT min(sequence) FROM messages WHERE conversation=$1),
 (SELECT max(sequence) FROM messages WHERE conversation=$1),
 coalesce((SELECT sequence FROM anchor),0)
"#
    );
    type Row = (
        Option<i64>,
        Vec<String>,
        sqlx::types::Json<Vec<(i64, String, String)>>,
        Option<i64>,
        Option<i64>,
        i64,
    );
    let (stored, rows, reactions, oldest, newest, anchor): Row = sqlx::query_as(&sql)
        .bind(request.conversation)
        .bind(number(lower)?)
        .bind(number(upper)?)
        .bind(limit)
        .bind(request.range.around.map(number).transpose()?)
        .fetch_one(db)
        .await?;
    if stored != Some(number(request.revision)?) {
        return Err(Error::RevisionConflict);
    }
    let mut messages = rows
        .iter()
        .map(|raw| decode_message(raw))
        .collect::<Result<Vec<_>>>()?;
    attach_reactions(&mut messages, reactions.0)?;
    let (lower, upper, ascending) = request.interval(unsigned(anchor)?);
    let (messages, sizes, before, after) = request.finish(messages, lower, upper, ascending)?;
    Ok(PageResult {
        messages,
        sizes,
        has_before: oldest
            .map(unsigned)
            .transpose()?
            .is_some_and(|n| n < before),
        has_after: newest.map(unsigned).transpose()?.is_some_and(|n| n > after),
    })
}
async fn reactions(db: &mut PgConnection, id: &str, messages: &mut [Message]) -> Result<()> {
    let Some(low) = messages.iter().map(|m| m.sequence).min() else {
        return Ok(());
    };
    let high = messages.iter().map(|m| m.sequence).max().unwrap_or(low);
    let rows: Vec<(i64,String,String)> = sqlx::query_as("SELECT sequence,reaction,username FROM reactions WHERE conversation=$1 AND sequence BETWEEN $2 AND $3")
        .bind(id).bind(number(low)?).bind(number(high)?).fetch_all(db).await?;
    attach_reactions(messages, rows)
}
pub(in crate::engine::database) async fn has_author(
    db: &mut PgConnection,
    ids: &[String],
    author: &str,
) -> Result<bool> {
    if ids.is_empty() {
        return Ok(false);
    }
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM messages WHERE conversation=ANY($1) AND (projection #>> '{from}')=$2)").bind(ids).bind(author).fetch_one(db).await?)
}
pub(in crate::engine::database) async fn contains_sequence(
    db: &mut PgConnection,
    id: &str,
    sequence: u64,
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM messages WHERE conversation=$1 AND sequence=$2)",
    )
    .bind(id)
    .bind(number(sequence)?)
    .fetch_one(db)
    .await?)
}
pub(in crate::engine::database) async fn locate(
    db: &mut PgConnection,
    id: &str,
) -> Result<Option<(String, Message)>> {
    let mut snapshot = db
        .begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .await?;
    let result = locate_in(&mut snapshot, id).await;
    snapshot.rollback().await?;
    result
}
async fn locate_in(db: &mut PgConnection, id: &str) -> Result<Option<(String, Message)>> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT conversation,json FROM messages WHERE (projection #>> '{id}')=$1")
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
    db: &mut PgConnection,
    ids: &[String],
    cutoff: u64,
) -> Result<CleanupCounts> {
    let mut snapshot = db
        .begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .await?;
    let result = cleanup_counts_in(&mut snapshot, ids, cutoff).await;
    snapshot.rollback().await?;
    result
}
async fn cleanup_counts_in(
    db: &mut PgConnection,
    ids: &[String],
    cutoff: u64,
) -> Result<CleanupCounts> {
    let mut totals = CleanupCounts::new();
    for batch in ids.chunks(256) {
        let rows: Vec<(String, i64, i64)> = sqlx::query_as(CLEANUP_COUNTS_SQL)
            .bind(batch)
            .bind(number(cutoff)?)
            .fetch_all(&mut *db)
            .await?;
        for (id, messages, attachments) in rows {
            totals.insert(id, (count(messages)?, count(attachments)?));
        }
    }
    Ok(totals)
}
// One statement supplies a coherent snapshot for the whole batch. Lateral
// delta scans retain the warm-cache fast path without one round trip per room.
const UNREADS_SQL: &str = r#"
WITH coordinates AS (
 SELECT input.*,e.revision AS stored,coalesce(r.oldest,0) AS oldest,
 coalesce((SELECT sequence FROM messages WHERE conversation=input.conversation
           ORDER BY sequence DESC LIMIT 1),0) AS through
 FROM jsonb_to_recordset($1::jsonb) AS input(
   ordinal bigint,conversation text,read bigint,revision bigint,
   old_read bigint,old_revision bigint,old_count bigint,old_first bigint,
   old_through bigint,old_oldest bigint)
 LEFT JOIN entities e ON e.kind IN ('room','private') AND e.projection #>> '{id}'=input.conversation
 LEFT JOIN retention r ON r.conversation=input.conversation
), hints AS (
 SELECT *,coalesce(stored=revision AND read>=old_read AND through>=old_through
   AND (oldest=old_oldest OR old_through=0),false) AS eligible
 FROM coordinates
), deltas AS (
 SELECT hints.*,a.appended,a.added,d.removed
 FROM hints
 LEFT JOIN LATERAL (
   SELECT count(*) AS appended,
     count(*) FILTER (WHERE sequence>read AND projection #>> '{from}'!=$2) AS added
   FROM messages WHERE eligible AND conversation=hints.conversation AND sequence>old_through
 ) a ON true
 LEFT JOIN LATERAL (
   SELECT count(*) AS removed FROM messages WHERE eligible AND read!=old_read
   AND conversation=hints.conversation AND sequence>old_read
   AND sequence<=least(read,old_through) AND projection #>> '{from}'!=$2
 ) d ON true
), checked AS (
 SELECT *,eligible AND revision-old_revision=appended AND old_count>=removed AS incremental
 FROM deltas
)
SELECT stored,coalesce(incremental,false),oldest,through,
 CASE WHEN incremental THEN old_count-removed+added ELSE totals.total END AS total,
 CASE WHEN incremental THEN
   CASE WHEN old_count-removed+added=0 THEN NULL
        WHEN old_first>read THEN old_first
        ELSE (SELECT min(sequence) FROM messages WHERE conversation=checked.conversation
              AND sequence>read AND projection #>> '{from}'!=$2) END
   ELSE totals.first END AS first
FROM checked
LEFT JOIN LATERAL (
 SELECT count(*) AS total,min(sequence) AS first FROM messages
 WHERE NOT coalesce(incremental,false) AND conversation=checked.conversation
 AND sequence>read AND projection #>> '{from}'!=$2
) totals ON true
ORDER BY ordinal
"#;

pub(in crate::engine::database) async fn unreads(
    db: &mut PgConnection,
    user: &str,
    requests: &[UnreadRead<'_>],
) -> Result<UnreadBatch> {
    let inputs: Vec<_> = requests
        .iter()
        .enumerate()
        .map(|(ordinal, request)| {
            let hint = request.previous.as_ref();
            serde_json::json!({
                "ordinal":ordinal,"conversation":request.conversation,
                "read":request.read,"revision":request.revision,
                "old_read":hint.map(|h| h.read),
                "old_revision":hint.map(|h| h.unread.revision),
                "old_count":hint.map(|h| h.unread.count),
                "old_first":hint.and_then(|h| h.unread.first),
                "old_through":hint.map(|h| h.unread.through),
                "old_oldest":hint.map(|h| h.unread.oldest),
            })
        })
        .collect();
    type Row = (Option<i64>, bool, i64, i64, i64, Option<i64>);
    let rows: Vec<Row> = sqlx::query_as(UNREADS_SQL)
        .bind(sqlx::types::Json(inputs))
        .bind(user)
        .fetch_all(db)
        .await?;
    if rows.len() != requests.len() {
        return Err(Error::Conversion("Invalid unread batch length"));
    }
    let mut batch = UnreadBatch {
        results: Vec::with_capacity(requests.len()),
        full: 0,
        incremental: 0,
    };
    for (request, (stored, incremental, oldest, through, total, first)) in requests.iter().zip(rows)
    {
        let result = (|| {
            if stored != Some(number(request.revision)?) {
                return Err(Error::RevisionConflict);
            }
            Ok(Unread {
                count: count(total)?,
                first: first.map(unsigned).transpose()?,
                oldest: unsigned(oldest)?,
                through: unsigned(through)?,
                revision: request.revision,
            })
        })();
        if result.is_ok() {
            if incremental {
                batch.incremental += 1;
            } else {
                batch.full += 1;
            }
        }
        batch.results.push(result);
    }
    Ok(batch)
}
