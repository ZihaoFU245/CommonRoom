//! Postgres retry receipt queries and native decoding.
use super::super::{
    Result,
    history::{count, unsigned},
    message::number,
    receipts::ReceiptRead,
};
use crate::bots::models::Receipt;
use sqlx::PgConnection;
pub(in crate::engine::database) async fn message(
    db: &mut PgConnection,
    request: &ReceiptRead<'_>,
) -> Result<Option<Receipt>> {
    let row: Option<(String, i64)> = sqlx::query_as(
        "SELECT digest,sequence FROM bot_receipts WHERE account_id=$1 AND key=$2 AND created>$3",
    )
    .bind(request.account)
    .bind(request.key)
    .bind(number(request.after)?)
    .fetch_optional(db)
    .await?;
    row.map(|(digest, sequence)| {
        Ok(Receipt {
            digest,
            sequence: unsigned(sequence)?,
        })
    })
    .transpose()
}
#[cfg(test)]
pub(in crate::engine::database) async fn message_count(
    db: &mut PgConnection,
    account: &str,
    after: u64,
) -> Result<usize> {
    count(
        sqlx::query_scalar("SELECT count(*) FROM bot_receipts WHERE account_id=$1 AND created>$2")
            .bind(account)
            .bind(number(after)?)
            .fetch_one(db)
            .await?,
    )
}
pub(in crate::engine::database) async fn command(
    db: &mut PgConnection,
    request: &ReceiptRead<'_>,
) -> Result<Option<(String, String)>> {
    Ok(sqlx::query_as("SELECT digest,outcome FROM bot_command_receipts WHERE account_id=$1 AND key=$2 AND created>$3").bind(request.account).bind(request.key).bind(number(request.after)?).fetch_optional(db).await?)
}
pub(in crate::engine::database) async fn command_usage(
    db: &mut PgConnection,
    account: &str,
    after: u64,
) -> Result<(usize, usize)> {
    let (rows,bytes): (i64,i64) = sqlx::query_as("SELECT count(*),coalesce(sum(octet_length(outcome)),0)::bigint FROM bot_command_receipts WHERE account_id=$1 AND created>$2").bind(account).bind(number(after)?).fetch_one(db).await?;
    Ok((count(rows)?, count(bytes)?))
}
pub(in crate::engine::database) async fn save_command(
    db: &mut PgConnection,
    account: &str,
    key: &str,
    digest: &str,
    created: u64,
    expired_before: u64,
    outcome: &str,
) -> Result<u64> {
    let changed = sqlx::query("DELETE FROM bot_command_receipts WHERE created<=$1")
        .bind(number(expired_before)?)
        .execute(&mut *db)
        .await?
        .rows_affected();
    Ok(changed + sqlx::query("INSERT INTO bot_command_receipts SELECT $1,$2,$3,$4,$5 WHERE EXISTS(SELECT 1 FROM entities WHERE kind='account' AND (projection #>> '{id}')=$1)").bind(account).bind(key).bind(digest).bind(number(created)?).bind(outcome).execute(db).await?.rows_affected())
}

pub(in crate::engine::database) async fn apply_command(
    db: &mut PgConnection,
    request: &super::super::CommandWrite<'_>,
) -> Result<super::super::metadata::WriteResult> {
    use super::super::{CommandEffect, Error, metadata::WriteResult};
    let account: Option<String> = sqlx::query_scalar(
        "SELECT key FROM entities WHERE kind='account' AND (projection #>> '{id}')=$1 FOR UPDATE",
    )
    .bind(request.account)
    .fetch_optional(&mut *db)
    .await?;
    if account.is_none() {
        return Err(Error::RevisionConflict);
    }
    let previous = command(
        db,
        &ReceiptRead {
            account: request.account,
            key: request.key,
            after: request.expired_before,
        },
    )
    .await?;
    let usage = if previous.is_none() {
        command_usage(db, request.account, request.expired_before).await?
    } else {
        (0, 0)
    };
    if let Some(outcome) = super::super::command::preflight(request, previous, usage)? {
        return Ok(WriteResult {
            command: Some((outcome, true)),
            ..Default::default()
        });
    }
    let mut result = match &request.effect {
        CommandEffect::None => WriteResult::default(),
        CommandEffect::Message(effect) => super::messages::apply_message(db, effect).await?,
        CommandEffect::Metadata(effect) => super::metadata::apply(db, effect).await?,
    };
    result.changed += save_command(
        db,
        request.account,
        request.key,
        request.digest,
        request.created,
        request.expired_before,
        request.outcome,
    )
    .await?;
    result.command = Some((request.outcome.into(), false));
    Ok(result)
}
