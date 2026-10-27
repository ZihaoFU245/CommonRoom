//! Postgres metadata operations. Native SQL remains independent of other backends.
use super::super::{
    Error, Result,
    history::{decode_message, unsigned},
    message::number,
    metadata::{
        CursorRows, EntityRow, MetadataOp, MetadataSnapshot, MetadataWrite, RevisionChange,
        WriteResult, rewrite_message,
    },
};
use crate::engine::authorization::Audit;
use sqlx::PgConnection;
use std::collections::{BTreeSet, VecDeque};

pub(in crate::engine::database) async fn apply(
    db: &mut PgConnection,
    request: &MetadataWrite<'_>,
) -> Result<WriteResult> {
    let mut result = WriteResult::default();
    for operation in &request.operations {
        result.changed += match operation {
            MetadataOp::Entity { kind, key, json } => super::messages::put_entity(db, kind, key, json).await?,
            MetadataOp::RemoveEntity { kind, key } => sqlx::query("DELETE FROM entities WHERE kind=$1 AND key=$2").bind(*kind).bind(key).execute(&mut *db).await?.rows_affected(),
            MetadataOp::Revision { kind, key, revision } => sqlx::query("UPDATE entities SET revision=$3 WHERE kind=$1 AND key=$2 AND revision!=$3").bind(*kind).bind(key).bind(number(*revision)?).execute(&mut *db).await?.rows_affected(),
            MetadataOp::CheckedRevision { kind, key, conversation, expected, revision } => {
                let changed = sqlx::query("UPDATE entities SET revision=$5 WHERE kind=$1 AND key=$2 AND (projection #>> '{id}')=$3 AND revision=$4").bind(*kind).bind(key).bind(conversation).bind(number(*expected)?).bind(number(*revision)?)
                    .execute(&mut *db).await?.rows_affected();
                if changed != 1 { return Err(Error::RevisionConflict); }
                changed
            }
            MetadataOp::RemoveMessages { conversation } => sqlx::query("DELETE FROM messages WHERE conversation=$1").bind(conversation).execute(&mut *db).await?.rows_affected(),
            MetadataOp::State { json } => sqlx::query("INSERT INTO state VALUES(1,$1) ON CONFLICT(id) DO UPDATE SET json=excluded.json WHERE state.json!=excluded.json").bind(json).execute(&mut *db).await?.rows_affected(),
            MetadataOp::Audit(entries) => audit(db, entries).await?,
            MetadataOp::AppendAudit(entry) => append_audit(db, entry).await?,
            MetadataOp::RemoveAccountReceipts { account } => {
                let changed = sqlx::query("DELETE FROM bot_receipts WHERE account_id=$1").bind(account).execute(&mut *db).await?.rows_affected();
                changed + sqlx::query("DELETE FROM bot_command_receipts WHERE account_id=$1").bind(account).execute(&mut *db).await?.rows_affected()
            }
            MetadataOp::RemoveAccountCursors { user } => sqlx::query("DELETE FROM read_positions WHERE username=$1").bind(user).execute(&mut *db).await?.rows_affected(),
            MetadataOp::RenameAccountCursors { old, new } => {
                let changed = sqlx::query("DELETE FROM read_positions WHERE username=$1").bind(new).execute(&mut *db).await?.rows_affected();
                changed + sqlx::query("UPDATE read_positions SET username=$1 WHERE username=$2").bind(new).bind(old).execute(&mut *db).await?.rows_affected()
            }
            MetadataOp::RemoveConversationCursors { conversation } => sqlx::query("DELETE FROM read_positions WHERE conversation=$1").bind(conversation).execute(&mut *db).await?.rows_affected(),
            MetadataOp::RenameConversationCursors { old, new } => {
                let changed = sqlx::query("DELETE FROM read_positions WHERE conversation=$1").bind(new).execute(&mut *db).await?.rows_affected();
                changed + sqlx::query("UPDATE read_positions SET conversation=$1 WHERE conversation=$2").bind(new).bind(old).execute(&mut *db).await?.rows_affected()
            }
            MetadataOp::RewriteAccount { old, new } => rewrite_account(db, old, new.as_deref()).await?,
            MetadataOp::HistoryChanges { deletes, cleanups } => history_changes(db, deletes, cleanups).await?,
            MetadataOp::Prune { kind, key, conversation, revision, limit } => {
                let deleted = prune(db, conversation, *limit).await?;
                if deleted > 0 {
                    let revision = revision.wrapping_add(1);
                    result.revisions.push(RevisionChange { kind, key: key.clone(), revision });
                    deleted + sqlx::query("UPDATE entities SET revision=$3 WHERE kind=$1 AND key=$2").bind(*kind).bind(key).bind(number(revision)?).execute(&mut *db).await?.rows_affected()
                } else { 0 }
            }
            MetadataOp::CommandReceipt { account, key, digest, created, expired_before, outcome } => {
                super::receipts::save_command(db, account, key, digest, *created, *expired_before, outcome).await?
            }
            MetadataOp::AuthEvent(event) => {
                let sequence: i64 = sqlx::query_scalar("INSERT INTO auth_events(at,client,username,outcome) VALUES($1,$2,$3,$4) RETURNING sequence")
                    .bind(number(event.at)?).bind(&event.client).bind(&event.username).bind(event.outcome.label())
                    .fetch_one(&mut *db).await?;
                1 + sqlx::query("DELETE FROM auth_events WHERE sequence<=$1").bind(sequence.saturating_sub(2048)).execute(&mut *db).await?.rows_affected()
            }
            MetadataOp::ExpireSessions { before } => sqlx::query("DELETE FROM entities WHERE kind='session' AND ((projection #>> '{expires}'))::bigint<=$1").bind(number(*before)?).execute(&mut *db).await?.rows_affected(),
        };
    }
    Ok(result)
}
async fn append_audit(db: &mut PgConnection, entry: &Audit) -> Result<u64> {
    let json = serde_json::to_string(entry)
        .map_err(|_| Error::Conversion("Cannot serialize audit entry"))?;
    let inserted = sqlx::query(
        "INSERT INTO audit_entries(entry_id,json) VALUES($1,$2) ON CONFLICT(entry_id) DO NOTHING",
    )
    .bind(&entry.id)
    .bind(json)
    .execute(&mut *db)
    .await?
    .rows_affected();
    if inserted == 0 {
        return Ok(0);
    }
    let removed = sqlx::query("DELETE FROM audit_entries WHERE sequence < (SELECT sequence FROM audit_entries ORDER BY sequence DESC LIMIT 1 OFFSET 999)").execute(db).await?.rows_affected();
    Ok(inserted + removed)
}
async fn audit(db: &mut PgConnection, entries: &VecDeque<Audit>) -> Result<u64> {
    let last: Option<String> =
        sqlx::query_scalar("SELECT entry_id FROM audit_entries ORDER BY sequence DESC LIMIT 1")
            .fetch_optional(&mut *db)
            .await?;
    let start = last
        .and_then(|id| entries.iter().rposition(|entry| entry.id == id))
        .map_or(0, |n| n + 1);
    let mut inserted = 0;
    for entry in entries.iter().skip(start) {
        let json = serde_json::to_string(entry)
            .map_err(|_| Error::Conversion("Cannot serialize audit entry"))?;
        inserted += sqlx::query("INSERT INTO audit_entries(entry_id,json) VALUES($1,$2) ON CONFLICT(entry_id) DO NOTHING").bind(&entry.id).bind(json).execute(&mut *db).await?.rows_affected();
    }
    let mut changed = inserted;
    if inserted > 0
        && let Some(first) = entries.front()
    {
        changed += sqlx::query("DELETE FROM audit_entries WHERE sequence < (SELECT sequence FROM audit_entries WHERE entry_id=$1)").bind(&first.id).execute(db).await?.rows_affected();
    }
    Ok(changed)
}
async fn rewrite_account(db: &mut PgConnection, old: &str, new: Option<&str>) -> Result<u64> {
    let mut after = 0;
    let mut changed = 0;
    loop {
        let rows: Vec<(String,String)> = sqlx::query_as("SELECT m.conversation,m.json FROM message_accounts a JOIN messages m ON m.conversation=a.conversation AND m.sequence=a.sequence WHERE a.name=$2 AND a.sequence>$1 ORDER BY a.sequence LIMIT 100")
            .bind(number(after)?).bind(old).fetch_all(&mut *db).await?;
        if rows.is_empty() {
            break;
        }
        for (id, raw) in rows {
            let mut message = decode_message(&raw)?;
            after = message.sequence;
            if rewrite_message(&mut message, old, new) {
                changed += super::messages::put_message(db, &id, &message).await?;
            }
        }
    }
    if let Some(new) = new {
        changed += sqlx::query("INSERT INTO reactions(conversation,sequence,reaction,username) SELECT conversation,sequence,reaction,$1 FROM reactions WHERE username=$2 ON CONFLICT DO NOTHING").bind(new).bind(old).execute(&mut *db).await?.rows_affected();
    }
    changed += sqlx::query("DELETE FROM reactions WHERE username=$1")
        .bind(old)
        .execute(db)
        .await?
        .rows_affected();
    Ok(changed)
}
async fn history_changes(
    db: &mut PgConnection,
    deletes: &[(String, u64)],
    cleanups: &[(String, u64)],
) -> Result<u64> {
    let mut changed = 0;
    for (id, sequence) in deletes {
        let target: String = sqlx::query_scalar(
            "SELECT (projection #>> '{id}') FROM messages WHERE conversation=$1 AND sequence=$2",
        )
        .bind(id)
        .bind(number(*sequence)?)
        .fetch_one(&mut *db)
        .await?;
        changed += scrub_replies(db, id, &target).await?;
        changed += sqlx::query("DELETE FROM messages WHERE conversation=$1 AND sequence=$2")
            .bind(id)
            .bind(number(*sequence)?)
            .execute(&mut *db)
            .await?
            .rows_affected();
    }
    for (id, cutoff) in cleanups {
        changed += sqlx::query(
            "DELETE FROM messages WHERE conversation=$1 AND ((projection #>> '{time}'))::bigint<$2",
        )
        .bind(id)
        .bind(number(*cutoff)?)
        .execute(&mut *db)
        .await?
        .rows_affected();
    }
    Ok(changed)
}
async fn scrub_replies(db: &mut PgConnection, conversation: &str, target: &str) -> Result<u64> {
    // Query only the safe projection; escaped NULs in payload text remain text.
    let mut after = 0;
    let mut changed = 0;
    loop {
        let rows: Vec<String> = sqlx::query_scalar("SELECT json FROM messages WHERE conversation=$1 AND (projection #>> '{reply,id}')=$2 AND sequence>$3 ORDER BY sequence LIMIT 100")
            .bind(conversation).bind(target).bind(number(after)?).fetch_all(&mut *db).await?;
        if rows.is_empty() {
            break;
        }
        for raw in rows {
            let mut message = decode_message(&raw)?;
            after = message.sequence;
            message.reply = None;
            changed += super::messages::put_message(db, conversation, &message).await?;
        }
    }
    Ok(changed)
}
async fn prune(db: &mut PgConnection, id: &str, limit: usize) -> Result<u64> {
    let retained: Option<i64> =
        sqlx::query_scalar("SELECT count FROM retention WHERE conversation=$1")
            .bind(id)
            .fetch_optional(&mut *db)
            .await?;
    let limit = u64::try_from(limit).map_err(|_| Error::Conversion("Invalid retention limit"))?;
    let excess = unsigned(retained.unwrap_or(0))?.saturating_sub(limit);
    if excess == 0 {
        return Ok(0);
    }
    Ok(sqlx::query("DELETE FROM messages WHERE conversation=$1 AND sequence IN (SELECT sequence FROM messages WHERE conversation=$1 ORDER BY sequence LIMIT $2)").bind(id).bind(number(excess)?).execute(db).await?.rows_affected())
}
pub(in crate::engine::database) async fn load(db: &mut PgConnection) -> Result<MetadataSnapshot> {
    let state: String = sqlx::query_scalar("SELECT json FROM state WHERE id=1")
        .fetch_one(&mut *db)
        .await?;
    let sequence: i64 = sqlx::query_scalar("SELECT sequence FROM counters WHERE id=1")
        .fetch_one(&mut *db)
        .await?;
    let rows: Vec<(String, String, String, i64)> =
        sqlx::query_as("SELECT kind,key,json,revision FROM entities")
            .fetch_all(&mut *db)
            .await?;
    let entities = rows
        .into_iter()
        .map(|(kind, key, json, revision)| {
            Ok(EntityRow {
                kind,
                key,
                json,
                revision: unsigned(revision)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let audit: Vec<String> = sqlx::query_scalar("SELECT json FROM audit_entries ORDER BY sequence")
        .fetch_all(db)
        .await?;
    Ok(MetadataSnapshot {
        state,
        sequence: unsigned(sequence)?,
        entities,
        audit,
    })
}
pub(in crate::engine::database) async fn cursors(db: &mut PgConnection) -> Result<CursorRows> {
    let rows: Vec<(String, String, i64)> =
        sqlx::query_as("SELECT username,conversation,sequence FROM read_positions")
            .fetch_all(db)
            .await?;
    rows.into_iter()
        .map(|(user, id, sequence)| Ok((user, id, unsigned(sequence)?)))
        .collect()
}
pub(in crate::engine::database) async fn account_conversations(
    db: &mut PgConnection,
    name: &str,
) -> Result<BTreeSet<String>> {
    Ok(sqlx::query_scalar::<_,String>("SELECT conversation FROM message_accounts WHERE name=$1 UNION SELECT conversation FROM reactions WHERE username=$1").bind(name).fetch_all(db).await?.into_iter().collect())
}
