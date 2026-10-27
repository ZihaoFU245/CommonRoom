//! Postgres message transactions. SQL and native binding stay in this backend.
use super::super::{
    Error, MessageCommit, Result,
    message::{self, MessageMutation, MessageWrite, number},
    metadata::WriteResult,
};
use crate::engine::{Message, media::media_references};
use sqlx::PgConnection;
use std::{borrow::Cow, collections::BTreeSet};

pub(in crate::engine::database) async fn put_entity(
    db: &mut PgConnection,
    kind: &str,
    key: &str,
    json: &str,
) -> Result<u64> {
    let projection = super::super::entity_projection(json)?;
    Ok(sqlx::query("INSERT INTO entities(kind,key,json,projection) VALUES($1,$2,$3,$4::text::jsonb) ON CONFLICT(kind,key) DO UPDATE SET json=excluded.json,projection=excluded.projection WHERE entities.json!=excluded.json")
        .bind(kind).bind(key).bind(json).bind(projection)
        .execute(db).await?.rows_affected())
}
pub(in crate::engine::database) async fn put_message(
    db: &mut PgConnection,
    conversation: &str,
    message: &Message,
) -> Result<u64> {
    let json = message::serialize(message)?;
    let projection = super::super::message_projection(&json)?;
    Ok(sqlx::query("INSERT INTO messages(conversation,sequence,json,projection) VALUES($1,$2,$3,$4::text::jsonb) ON CONFLICT(conversation,sequence) DO UPDATE SET json=excluded.json,projection=excluded.projection WHERE messages.json!=excluded.json")
        .bind(conversation).bind(number(message.sequence)?).bind(&json).bind(projection)
        .execute(db).await?.rows_affected())
}

// The owning dispatcher provides the transaction, including any outer command
// receipt transaction. No operation can independently commit part of a message.
pub(in crate::engine::database) async fn apply_message(
    db: &mut PgConnection,
    request: &MessageWrite<'_>,
) -> Result<WriteResult> {
    if let Some(receipt) = request.receipt {
        check_receipt(db, &request.message.from, receipt).await?;
    }
    let mut changed = 0;
    // Updating the transactional counter acquires the append-order lock before
    // conversation locks. Rollback restores the counter; commit releases it only
    // after the message, references, revision and receipt are all durable.
    let message = if matches!(request.mutation, MessageMutation::Append { .. }) {
        let sequence: Option<i64> = sqlx::query_scalar("UPDATE counters SET sequence=sequence+1 WHERE id=1 AND sequence<9007199254740991 RETURNING sequence")
            .fetch_optional(&mut *db).await?;
        let mut message = request.message.clone();
        message.sequence = sequence
            .and_then(|value| u64::try_from(value).ok())
            .ok_or(Error::InvalidRequest("Message sequence exhausted."))?;
        changed += 1;
        Cow::Owned(message)
    } else {
        Cow::Borrowed(request.message)
    };
    let request = &MessageWrite {
        message: &message,
        ..*request
    };
    let sequence = number(request.message.sequence)?;
    if let Some(json) = request.new_conversation {
        if request.expected_revision.is_some() {
            return Err(Error::RevisionConflict);
        }
        let projection = super::super::entity_projection(json)?;
        let inserted = sqlx::query("INSERT INTO entities(kind,key,json,projection) VALUES($1,$2,$3,$4::text::jsonb) ON CONFLICT(kind,key) DO NOTHING")
            .bind(request.kind).bind(request.key).bind(json).bind(projection).execute(&mut *db).await?.rows_affected();
        if inserted != 1 {
            return Err(Error::RevisionConflict);
        }
        changed += inserted;
    } else {
        let stored: Option<(i64, String)> = sqlx::query_as("SELECT revision,projection #>> '{id}' FROM entities WHERE kind=$1 AND key=$2 FOR UPDATE")
            .bind(request.kind).bind(request.key).fetch_optional(&mut *db).await?;
        let expected = request.expected_revision.map(number).transpose()?;
        if !stored
            .is_some_and(|(revision, id)| Some(revision) == expected && id == request.conversation)
        {
            return Err(Error::RevisionConflict);
        }
    }
    match &request.mutation {
        MessageMutation::Reaction {
            active,
            reaction,
            user,
        } => {
            let sql = if *active {
                "INSERT INTO reactions VALUES($1,$2,$3,$4)"
            } else {
                "DELETE FROM reactions WHERE conversation=$1 AND sequence=$2 AND reaction=$3 AND username=$4"
            };
            changed += sqlx::query(sql)
                .bind(request.conversation)
                .bind(sequence)
                .bind(*reaction)
                .bind(*user)
                .execute(&mut *db)
                .await?
                .rows_affected();
        }
        MessageMutation::Append { .. } => {
            changed += put_message(db, request.conversation, request.message).await?;
            changed += reconcile(db, request, false).await?;
        }
        MessageMutation::Edit {
            reconcile_media, ..
        } => {
            changed += put_message(db, request.conversation, request.message).await?;
            if *reconcile_media {
                changed += reconcile(db, request, true).await?;
            }
        }
    }
    if let MessageMutation::Append { retention } = request.mutation {
        let count: Option<i64> =
            sqlx::query_scalar("SELECT count FROM retention WHERE conversation=$1")
                .bind(request.conversation)
                .fetch_optional(&mut *db)
                .await?;
        let count = u64::try_from(count.unwrap_or(0))
            .map_err(|_| Error::Conversion("Invalid retention count"))?;
        let limit =
            u64::try_from(retention).map_err(|_| Error::Conversion("Invalid retention limit"))?;
        let excess = count.saturating_sub(limit);
        if excess > 0 {
            changed += sqlx::query("DELETE FROM messages WHERE conversation=$1 AND sequence IN (SELECT sequence FROM messages WHERE conversation=$1 ORDER BY sequence LIMIT $2)").bind(request.conversation).bind(number(excess)?)
                .execute(&mut *db).await?.rows_affected();
        }
    }
    let revised =
        sqlx::query("UPDATE entities SET revision=$3 WHERE kind=$1 AND key=$2 AND revision=$4")
            .bind(request.kind)
            .bind(request.key)
            .bind(number(request.revision)?)
            .bind(number(request.expected_revision.unwrap_or(0))?)
            .execute(&mut *db)
            .await?
            .rows_affected();
    if revised != 1 {
        return Err(Error::RevisionConflict);
    }
    changed += revised;
    if let Some(receipt) = request.receipt {
        changed += sqlx::query("DELETE FROM bot_receipts WHERE created<=$1")
            .bind(number(
                receipt
                    .created
                    .saturating_sub(crate::bots::models::RECEIPT_TTL),
            )?)
            .execute(&mut *db)
            .await?
            .rows_affected();
        let inserted = sqlx::query("INSERT INTO bot_receipts VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(account_id,key) DO NOTHING")
            .bind(receipt.account_id)
            .bind(receipt.key)
            .bind(receipt.digest)
            .bind(request.conversation)
            .bind(sequence)
            .bind(number(receipt.created)?)
            .execute(&mut *db)
            .await?
            .rows_affected();
        if inserted != 1 {
            return Err(Error::ReceiptConflict);
        }
        changed += inserted;
    }
    Ok(WriteResult {
        changed,
        message: Some(MessageCommit {
            conversation: request.conversation.into(),
            revision: request.revision,
            sequence: request.message.sequence,
        }),
        ..Default::default()
    })
}

async fn reconcile(
    db: &mut PgConnection,
    request: &MessageWrite<'_>,
    editing: bool,
) -> Result<u64> {
    let actor = match request.mutation {
        MessageMutation::Edit { actor, .. } => actor,
        _ => request.message.author_id.as_str(),
    };
    let ids = media_references(&request.message.text);
    if !editing && ids.is_empty() {
        return Ok(0);
    }
    let sequence = number(request.message.sequence)?;
    let previous: BTreeSet<String> = if editing {
        sqlx::query_scalar(
            "SELECT media_id FROM message_media WHERE conversation=$1 AND sequence=$2",
        )
        .bind(request.conversation)
        .bind(sequence)
        .fetch_all(&mut *db)
        .await?
        .into_iter()
        .collect()
    } else {
        BTreeSet::new()
    };
    for id in ids.difference(&previous) {
        let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM media WHERE id=$1 AND conversation=$2 AND ready=1 AND deleting=0 AND deleted=0 AND (expires IS NULL OR expires>$4) AND (uploader=$3 OR EXISTS(SELECT 1 FROM message_media WHERE media_id=$1 AND conversation=$2)) FOR UPDATE)")
            .bind(id).bind(request.conversation).bind(actor).bind(number(request.at)?)
            .fetch_one(&mut *db).await?;
        if !valid {
            return Err(Error::MissingMedia);
        }
    }
    let mut changed = 0;
    for id in previous.difference(&ids) {
        changed += sqlx::query(
            "DELETE FROM message_media WHERE conversation=$1 AND sequence=$2 AND media_id=$3",
        )
        .bind(request.conversation)
        .bind(sequence)
        .bind(id)
        .execute(&mut *db)
        .await?
        .rows_affected();
    }
    for id in ids.difference(&previous) {
        changed += sqlx::query("INSERT INTO message_media VALUES($1,$2,$3)")
            .bind(request.conversation)
            .bind(sequence)
            .bind(id)
            .execute(&mut *db)
            .await?
            .rows_affected();
        changed += sqlx::query(
            "UPDATE media SET unreferenced=NULL,published=COALESCE(published,$2) WHERE id=$1",
        )
        .bind(id)
        .bind(number(request.at)?)
        .execute(&mut *db)
        .await?
        .rows_affected();
    }
    Ok(changed)
}

// Serialize quota and duplicate-key checks per account, before the append
// counter. Different accounts use independent row locks on PostgreSQL.
async fn check_receipt(
    db: &mut PgConnection,
    user: &str,
    receipt: super::super::MessageReceipt<'_>,
) -> Result<()> {
    let account: Option<String> = sqlx::query_scalar("SELECT key FROM entities WHERE kind='account' AND key=$1 AND (projection #>> '{id}')=$2 FOR UPDATE").bind(user).bind(receipt.account_id).fetch_optional(&mut *db).await?;
    if account.is_none() {
        return Err(Error::RevisionConflict);
    }
    let after = number(
        receipt
            .created
            .saturating_sub(crate::bots::models::RECEIPT_TTL),
    )?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM bot_receipts WHERE account_id=$1 AND key=$2 AND created>$3)",
    )
    .bind(receipt.account_id)
    .bind(receipt.key)
    .bind(after)
    .fetch_one(&mut *db)
    .await?;
    if exists {
        return Err(Error::ReceiptConflict);
    }
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM bot_receipts WHERE account_id=$1 AND created>$2")
            .bind(receipt.account_id)
            .bind(after)
            .fetch_one(db)
            .await?;
    if u64::try_from(count).map_err(|_| Error::Conversion("Invalid receipt count"))?
        >= u64::try_from(crate::bots::models::RECEIPT_LIMIT)
            .map_err(|_| Error::Conversion("Invalid receipt limit"))?
    {
        return Err(Error::ReceiptCapacity);
    }
    Ok(())
}
