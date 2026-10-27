//! Sqlite upload lifecycle, quota coordination and cleanup claims.
use super::super::{
    Error, Result,
    history::unsigned,
    media::{MediaAllocation, MediaColumns, MediaWrite, decode},
    message::number,
    metadata::WriteResult,
};
use crate::engine::media::Media;
use sqlx::SqliteConnection;

pub(crate) const TOMBSTONES_SQL: &str = "DELETE FROM media WHERE deleted=1 AND staging_clean=1 AND created<?1 AND NOT EXISTS(SELECT 1 FROM message_media WHERE media_id=media.id) AND ('/api/avatars/'||media.id||'/file') NOT IN (SELECT (json_extract(json,'$.metadata') -> '@global' ->> 'avatar') FROM entities WHERE kind='account' AND (json_extract(json,'$.metadata') -> '@global' ->> 'avatar') IS NOT NULL)";
pub(crate) const STARTUP_CLEANUP_SQL: &str = "UPDATE media SET deleting=1,cleanup_attempt=0 WHERE deleted=0 AND (deleting=1 OR unreferenced<=?1) AND NOT EXISTS(SELECT 1 FROM message_media WHERE media_id=media.id) AND ('/api/avatars/'||media.id||'/file') NOT IN (SELECT (json_extract(json,'$.metadata') -> '@global' ->> 'avatar') FROM entities WHERE kind='account' AND (json_extract(json,'$.metadata') -> '@global' ->> 'avatar') IS NOT NULL)";
pub(crate) const CLAIM_CLEANUP_SQL: &str = "SELECT json,ready,expires,deleting,deleted FROM media WHERE id IN (\n                SELECT id FROM (SELECT id FROM media WHERE deleted=0 AND deleting=1 AND cleanup_attempt<?4 ORDER BY cleanup_attempt,created LIMIT ?3)\n                UNION SELECT id FROM (SELECT id FROM media WHERE deleted=0 AND deleting=0 AND expires<=?1 AND cleanup_attempt<?4 ORDER BY expires,created LIMIT ?3)\n                UNION SELECT id FROM (SELECT id FROM media WHERE deleted=0 AND deleting=0 AND unreferenced IS NOT NULL AND (published IS NOT NULL OR unreferenced<=?2) AND NOT EXISTS(SELECT 1 FROM message_media WHERE media_id=media.id) AND ('/api/avatars/'||media.id||'/file') NOT IN (SELECT (json_extract(json,'$.metadata') -> '@global' ->> 'avatar') FROM entities WHERE kind='account' AND (json_extract(json,'$.metadata') -> '@global' ->> 'avatar') IS NOT NULL) AND cleanup_attempt<?4 ORDER BY unreferenced,created LIMIT ?3)\n            ) ORDER BY cleanup_attempt,created LIMIT ?3";

pub(in crate::engine::database) async fn apply(
    db: &mut SqliteConnection,
    request: &MediaWrite<'_>,
) -> Result<WriteResult> {
    let mut result = WriteResult::default();
    result.changed = match request {
        MediaWrite::Allocate(request) => allocate(db, request).await?,
        MediaWrite::AvatarAllocate {
            allocation,
            pending_ttl,
        } => {
            let reserved = avatar_allocate(db, allocation, *pending_ttl).await?;
            result.media = reserved.media;
            reserved.changed
        }
        MediaWrite::Ready { media, at } => {
            let json = serde_json::to_string(media)
                .map_err(|_| Error::Conversion("Cannot serialize media record"))?;
            let changed = sqlx::query("UPDATE media SET ready=1,json=?2,unreferenced=?3,expires=?4,published=CASE WHEN conversation='@avatar' THEN ?3 ELSE published END WHERE id=?1 AND uploader=?5 AND ready=0 AND deleting=0 AND deleted=0").bind(&media.id).bind(json).bind(number(*at)?)
                .bind(media.expires.map(number).transpose()?).bind(&media.uploader).execute(db).await?.rows_affected();
            if changed != 1 {
                return Err(Error::Conflict("Upload changed; retry completion."));
            }
            changed
        }
        MediaWrite::PrepareCleanup { pending_before } => sqlx::query(STARTUP_CLEANUP_SQL)
            .bind(number(*pending_before)?)
            .execute(db)
            .await?
            .rows_affected(),
        MediaWrite::Abandon { id, uploader } => sqlx::query(
            "UPDATE media SET deleting=1 WHERE id=?1 AND uploader=?2 AND published IS NULL AND deleted=0",
        )
        .bind(*id)
        .bind(*uploader)
        .execute(db)
        .await?
        .rows_affected(),
        MediaWrite::ClaimCleanup {
            at,
            pending_before,
            attempted_before,
            limit,
        } => {
            let mut changed = tombstones(db, *pending_before).await?;
            let limit =
                i64::try_from(*limit).map_err(|_| Error::Conversion("Invalid cleanup limit"))?;
            let rows: Vec<MediaColumns> = sqlx::query_as(CLAIM_CLEANUP_SQL)
                .bind(number(*at)?)
                .bind(number(*pending_before)?)
                .bind(limit)
                .bind(number((*attempted_before).min(i64::MAX.unsigned_abs()))?)
                .fetch_all(&mut *db)
                .await?;
            for columns in rows {
                let mut media = decode(columns)?;
                changed +=
                    sqlx::query("UPDATE media SET deleting=1,cleanup_attempt=?2 WHERE id=?1")
                        .bind(&media.id)
                        .bind(number(*at)?)
                        .execute(&mut *db)
                        .await?
                        .rows_affected();
                media.deleting = true;
                result.media.push(media);
            }
            changed
        }
        MediaWrite::ClaimStaging {
            at,
            pending_before,
            attempted_before,
            limit,
        } => {
            let limit =
                i64::try_from(*limit).map_err(|_| Error::Conversion("Invalid staging limit"))?;
            let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM media WHERE staging_clean=0 AND created<=?1 AND staging_attempt<?3 ORDER BY staging_attempt,created LIMIT ?2").bind(number(*pending_before)?).bind(limit)
                .bind(number((*attempted_before).min(i64::MAX.unsigned_abs()))?).fetch_all(&mut *db).await?;
            let mut changed = 0;
            for id in &ids {
                changed += sqlx::query("UPDATE media SET staging_attempt=?2 WHERE id=?1")
                    .bind(id)
                    .bind(number(*at)?)
                    .execute(&mut *db)
                    .await?
                    .rows_affected();
            }
            result.staging = ids;
            changed
        }
        MediaWrite::CleanupDone { records, staging, tombstones_before } => {
            let ids: Vec<_> = records.iter().map(|(id, _)| id).collect();
            let clean: Vec<_> = records.iter().filter(|(_, clean)| *clean).map(|(id, _)| id).collect();
            let json = |values: &[&String]| serde_json::to_string(values).map_err(|_| Error::Conversion("Cannot encode cleanup IDs"));
            let mut changed = sqlx::query("UPDATE media SET deleted=1,deleting=0,ready=0,staging_clean=CASE WHEN id IN (SELECT value FROM json_each(?2)) THEN 1 ELSE staging_clean END WHERE id IN (SELECT value FROM json_each(?1)) AND deleting=1")
                .bind(json(&ids)?).bind(json(&clean)?).execute(&mut *db).await?.rows_affected();
            let staging: Vec<_> = staging.iter().collect();
            changed += sqlx::query("UPDATE media SET staging_clean=1 WHERE id IN (SELECT value FROM json_each(?1)) AND staging_clean=0")
                .bind(json(&staging)?).execute(&mut *db).await?.rows_affected();
            changed + tombstones(db, *tombstones_before).await?
        }
        #[cfg(test)]
        MediaWrite::RemoveRecord {
            id,
            tombstones_before,
            staging_clean,
        } => {
            let changed = sqlx::query(
                "UPDATE media SET deleted=1,deleting=0,ready=0,staging_clean=CASE WHEN ?2 THEN 1 ELSE staging_clean END WHERE id=?1 AND deleting=1",
            )
            .bind(*id)
            .bind(*staging_clean)
            .execute(&mut *db)
            .await?
            .rows_affected();
            changed + tombstones(db, *tombstones_before).await?
        }
    };
    Ok(result)
}
async fn tombstones(db: &mut SqliteConnection, before: u64) -> Result<u64> {
    Ok(sqlx::query(TOMBSTONES_SQL)
        .bind(number(before)?)
        .execute(db)
        .await?
        .rows_affected())
}
async fn allocate(db: &mut SqliteConnection, request: &MediaAllocation<'_>) -> Result<u64> {
    let (bytes,pending,recent): (i64,i64,i64) = sqlx::query_as("SELECT COALESCE(SUM(CASE WHEN deleted=0 THEN size ELSE 0 END),0),COALESCE(SUM(deleted=0 AND published IS NULL),0),COALESCE(SUM(created>?2),0) FROM media WHERE uploader=?1")
        .bind(&request.media.uploader).bind(number(request.at.saturating_sub(60))?).fetch_one(&mut *db).await?;
    if unsigned(bytes)?.saturating_add(request.media.size) > request.quota {
        return Err(Error::InvalidRequest(
            "Upload byte quota reached; remove attachments or wait for cleanup.",
        ));
    }
    if unsigned(pending)? >= request.pending_limit {
        return Err(Error::InvalidRequest(
            "Pending upload limit reached; send or remove attachments and wait for cleanup.",
        ));
    }
    if unsigned(recent)? >= request.rate_limit {
        return Err(Error::InvalidRequest(
            "Upload rate limit reached; retry in a minute.",
        ));
    }
    let mut changed = 0;
    if let Some((key, json)) = &request.private {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM entities WHERE kind='private' AND key=?1)",
        )
        .bind(*key)
        .fetch_one(&mut *db)
        .await?;
        if !exists {
            changed += super::messages::put_entity(db, "private", key, json).await?;
        }
    }
    let media = request.media;
    let json = serde_json::to_string(media)
        .map_err(|_| Error::Conversion("Cannot serialize media record"))?;
    changed += sqlx::query("INSERT INTO media(id,uploader,conversation,size,created,ready,unreferenced,staging_clean,json) VALUES(?1,?2,?3,?4,?5,0,?5,0,?6)").bind(&media.id).bind(&media.uploader).bind(&media.conversation)
        .bind(number(media.size)?).bind(number(media.created)?).bind(json).execute(db).await?.rows_affected();
    Ok(changed)
}
async fn avatar_allocate(
    db: &mut SqliteConnection,
    request: &MediaAllocation<'_>,
    pending_ttl: u64,
) -> Result<WriteResult> {
    let existing: Option<MediaColumns> =
        sqlx::query_as("SELECT json,ready,expires,deleting,deleted FROM media WHERE id=?1")
            .bind(&request.media.id)
            .fetch_optional(&mut *db)
            .await?;
    if let Some(existing) = existing {
        let media = super::super::media::avatar_replay(decode(existing)?, request, pending_ttl)?;
        return Ok(WriteResult {
            media: vec![media],
            ..Default::default()
        });
    }
    let changed = allocate(db, request).await?;
    Ok(WriteResult {
        changed,
        media: vec![request.media.clone()],
        ..Default::default()
    })
}
pub(in crate::engine::database) async fn load(
    db: &mut SqliteConnection,
    id: &str,
) -> Result<Option<(Media, bool)>> {
    let row: Option<(String,i64,Option<i64>,i64,i64,bool)> = sqlx::query_as("SELECT json,ready,expires,deleting,deleted,EXISTS(SELECT 1 FROM message_media WHERE media_id=media.id) FROM media WHERE id=?1").bind(id).fetch_optional(db).await?;
    row.map(|(json, ready, expires, deleting, deleted, referenced)| {
        Ok((
            decode((json, ready, expires, deleting, deleted))?,
            referenced,
        ))
    })
    .transpose()
}
