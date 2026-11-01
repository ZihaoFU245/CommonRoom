//! Postgres upload lifecycle, quota coordination and cleanup claims.
use super::super::{
    Error, Result,
    history::unsigned,
    media::{MediaAllocation, MediaColumns, MediaWrite, StickerReach, decode},
    message::number,
    metadata::WriteResult,
};
use crate::engine::media::Media;
use sqlx::PgConnection;

pub(crate) const TOMBSTONES_SQL: &str = "DELETE FROM media WHERE deleted=1 AND staging_clean=1 AND created<$1 AND NOT EXISTS(SELECT 1 FROM message_media WHERE media_id=media.id) AND NOT EXISTS(SELECT 1 FROM sticker_library WHERE media_id=media.id) AND ('/api/avatars/'||media.id||'/file') NOT IN (SELECT ((projection #> '{metadata}') -> '@global' ->> 'avatar') FROM entities WHERE kind='account' AND ((projection #> '{metadata}') -> '@global' ->> 'avatar') IS NOT NULL)";
pub(crate) const STARTUP_CLEANUP_SQL: &str = "UPDATE media SET deleting=1,cleanup_attempt=0 WHERE deleted=0 AND (deleting=1 OR unreferenced<=$1) AND NOT EXISTS(SELECT 1 FROM message_media WHERE media_id=media.id) AND NOT EXISTS(SELECT 1 FROM sticker_library WHERE media_id=media.id) AND ('/api/avatars/'||media.id||'/file') NOT IN (SELECT ((projection #> '{metadata}') -> '@global' ->> 'avatar') FROM entities WHERE kind='account' AND ((projection #> '{metadata}') -> '@global' ->> 'avatar') IS NOT NULL)";
pub(crate) const CLAIM_CLEANUP_SQL: &str = "SELECT json,ready,expires,deleting,deleted FROM media WHERE id IN (\n                SELECT id FROM (SELECT id FROM media WHERE deleted=0 AND deleting=1 AND cleanup_attempt<$4 ORDER BY cleanup_attempt,created LIMIT $3)\n                UNION SELECT id FROM (SELECT id FROM media WHERE deleted=0 AND deleting=0 AND expires<=$1 AND cleanup_attempt<$4 ORDER BY expires,created LIMIT $3)\n                UNION SELECT id FROM (SELECT id FROM media WHERE deleted=0 AND deleting=0 AND unreferenced IS NOT NULL AND (published IS NOT NULL OR unreferenced<=$2) AND NOT EXISTS(SELECT 1 FROM message_media WHERE media_id=media.id) AND NOT EXISTS(SELECT 1 FROM sticker_library WHERE media_id=media.id) AND ('/api/avatars/'||media.id||'/file') NOT IN (SELECT ((projection #> '{metadata}') -> '@global' ->> 'avatar') FROM entities WHERE kind='account' AND ((projection #> '{metadata}') -> '@global' ->> 'avatar') IS NOT NULL) AND cleanup_attempt<$4 ORDER BY unreferenced,created LIMIT $3)\n            ) AND deleted=0 AND cleanup_attempt<$4 AND (deleting=1 OR (deleting=0 AND (expires<=$1 OR (unreferenced IS NOT NULL AND (published IS NOT NULL OR unreferenced<=$2) AND NOT EXISTS(SELECT 1 FROM message_media WHERE media_id=media.id) AND NOT EXISTS(SELECT 1 FROM sticker_library WHERE media_id=media.id) AND ('/api/avatars/'||media.id||'/file') NOT IN (SELECT ((projection #> '{metadata}') -> '@global' ->> 'avatar') FROM entities WHERE kind='account' AND ((projection #> '{metadata}') -> '@global' ->> 'avatar') IS NOT NULL))))) ORDER BY cleanup_attempt,created LIMIT $3 FOR UPDATE SKIP LOCKED";

pub(in crate::engine::database) async fn apply(
    db: &mut PgConnection,
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
        MediaWrite::StickerAllocate { allocation, limit } => {
            lock_uploader(db, &allocation.media.uploader).await?;
            sticker_capacity(db, &allocation.media.uploader, *limit).await?;
            allocate_locked(db, allocation).await?
        }
        MediaWrite::DeleteSticker { id, owner, at } => {
            let changed =
                sqlx::query("DELETE FROM sticker_library WHERE owner=$1 AND media_id=$2")
                    .bind(*owner)
                    .bind(*id)
                    .execute(&mut *db)
                    .await?
                    .rows_affected();
            if changed == 0 {
                0
            } else {
                changed + release_sticker(db, id, *at).await?
            }
        }
        MediaWrite::CollectSticker {
            id,
            owner,
            at,
            limit,
        } => {
            lock_uploader(db, owner).await?;
            // The row lock orders this entry against a concurrent cleanup claim.
            let live: Option<String> = sqlx::query_scalar("SELECT id FROM media WHERE id=$1 AND conversation='@sticker' AND ready=1 AND deleting=0 AND deleted=0 FOR UPDATE")
                .bind(*id).fetch_optional(&mut *db).await?;
            if live.is_none() {
                return Err(Error::MissingMedia);
            }
            let present: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM sticker_library WHERE owner=$1 AND media_id=$2)",
            )
            .bind(*owner)
            .bind(*id)
            .fetch_one(&mut *db)
            .await?;
            if present {
                0
            } else {
                sticker_capacity(db, owner, *limit).await?;
                sqlx::query("INSERT INTO sticker_library VALUES($1,$2,$3)")
                    .bind(*owner)
                    .bind(*id)
                    .bind(number(*at)?)
                    .execute(&mut *db)
                    .await?;
                sqlx::query("UPDATE media SET unreferenced=NULL WHERE id=$1")
                    .bind(*id)
                    .execute(&mut *db)
                    .await?
                    .rows_affected()
                    + 1
            }
        }
        MediaWrite::Ready { media, at } => {
            if media.conversation == "@sticker"
                && let Some(digest) = media.content_digest.as_deref()
            {
                let taken: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM sticker_library l JOIN media m ON m.id=l.media_id WHERE l.owner=$1 AND m.ready=1 AND m.deleting=0 AND m.deleted=0 AND m.json->>'content_digest'=$2)",
                )
                .bind(&media.uploader)
                .bind(digest)
                .fetch_one(&mut *db)
                .await?;
                if taken {
                    return Err(Error::Conflict(
                        "This sticker is already in your library.",
                    ));
                }
            }
            let json = serde_json::to_string(media)
                .map_err(|_| Error::Conversion("Cannot serialize media record"))?;
            // A completed sticker is referenced by its uploader's library entry.
            let changed = sqlx::query("UPDATE media SET ready=1,json=$2,unreferenced=CASE WHEN conversation='@sticker' THEN NULL ELSE $3 END,expires=$4,published=CASE WHEN conversation IN ('@avatar','@sticker') THEN $3 ELSE published END WHERE id=$1 AND uploader=$5 AND ready=0 AND deleting=0 AND deleted=0").bind(&media.id).bind(json).bind(number(*at)?)
                .bind(media.expires.map(number).transpose()?).bind(&media.uploader).execute(&mut *db).await?.rows_affected();
            if changed != 1 {
                return Err(Error::Conflict("Upload changed; retry completion."));
            }
            changed
                + sqlx::query("INSERT INTO sticker_library SELECT uploader,id,$2 FROM media WHERE id=$1 AND conversation='@sticker'")
                    .bind(&media.id).bind(number(*at)?).execute(&mut *db).await?.rows_affected()
        }
        MediaWrite::PrepareCleanup { pending_before } => sqlx::query(STARTUP_CLEANUP_SQL)
            .bind(number(*pending_before)?)
            .execute(db)
            .await?
            .rows_affected(),
        MediaWrite::Abandon { id, uploader } => sqlx::query(
            "UPDATE media SET deleting=1 WHERE id=$1 AND uploader=$2 AND published IS NULL AND deleted=0",
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
                    sqlx::query("UPDATE media SET deleting=1,cleanup_attempt=$2 WHERE id=$1")
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
            let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM media WHERE staging_clean=0 AND created<=$1 AND staging_attempt<$3 ORDER BY staging_attempt,created LIMIT $2 FOR UPDATE SKIP LOCKED").bind(number(*pending_before)?).bind(limit)
                .bind(number((*attempted_before).min(i64::MAX.unsigned_abs()))?).fetch_all(&mut *db).await?;
            let mut changed = 0;
            for id in &ids {
                changed += sqlx::query("UPDATE media SET staging_attempt=$2 WHERE id=$1")
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
            let ids: Vec<_> = records.iter().map(|(id, _)| id.clone()).collect();
            let clean: Vec<_> = records.iter().filter(|(_, clean)| *clean).map(|(id, _)| id.clone()).collect();
            let mut changed = sqlx::query("UPDATE media SET deleted=1,deleting=0,ready=0,staging_clean=CASE WHEN id=ANY($2) THEN 1 ELSE staging_clean END WHERE id=ANY($1) AND deleting=1")
                .bind(&ids).bind(&clean).execute(&mut *db).await?.rows_affected();
            changed += sqlx::query("UPDATE media SET staging_clean=1 WHERE id=ANY($1) AND staging_clean=0")
                .bind(*staging).execute(&mut *db).await?.rows_affected();
            changed + tombstones(db, *tombstones_before).await?
        }
        #[cfg(test)]
        MediaWrite::RemoveRecord {
            id,
            tombstones_before,
            staging_clean,
        } => {
            let changed = sqlx::query(
                "UPDATE media SET deleted=1,deleting=0,ready=0,staging_clean=CASE WHEN $2 THEN 1 ELSE staging_clean END WHERE id=$1 AND deleting=1",
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
async fn sticker_capacity(db: &mut PgConnection, owner: &str, limit: u64) -> Result<()> {
    let used: i64 = sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM sticker_library WHERE owner=$1)+(SELECT COUNT(*) FROM media WHERE uploader=$1 AND conversation='@sticker' AND ready=0 AND deleting=0 AND deleted=0)")
        .bind(owner).fetch_one(&mut *db).await?;
    if unsigned(used)? >= limit {
        return Err(Error::InvalidRequest(
            "Sticker limit reached; delete a sticker first.",
        ));
    }
    Ok(())
}
pub(in crate::engine::database) async fn remove_stickers(
    db: &mut PgConnection,
    owner: &str,
    at: u64,
) -> Result<u64> {
    let ids: Vec<String> =
        sqlx::query_scalar("DELETE FROM sticker_library WHERE owner=$1 RETURNING media_id")
            .bind(owner)
            .fetch_all(&mut *db)
            .await?;
    let mut changed = 0;
    for id in &ids {
        changed += 1 + release_sticker(db, id, at).await?;
    }
    Ok(changed)
}
/// Marks a sticker object for cleanup once no library or message references it.
async fn release_sticker(db: &mut PgConnection, id: &str, at: u64) -> Result<u64> {
    Ok(sqlx::query("UPDATE media SET unreferenced=COALESCE(unreferenced,$2) WHERE id=$1 AND NOT EXISTS(SELECT 1 FROM message_media WHERE media_id=$1) AND NOT EXISTS(SELECT 1 FROM sticker_library WHERE media_id=$1)")
        .bind(id).bind(number(at)?).execute(db).await?.rows_affected())
}
async fn tombstones(db: &mut PgConnection, before: u64) -> Result<u64> {
    Ok(sqlx::query(TOMBSTONES_SQL)
        .bind(number(before)?)
        .execute(db)
        .await?
        .rows_affected())
}
async fn allocate(db: &mut PgConnection, request: &MediaAllocation<'_>) -> Result<u64> {
    lock_uploader(db, &request.media.uploader).await?;
    allocate_locked(db, request).await
}
async fn lock_uploader(db: &mut PgConnection, uploader: &str) -> Result<()> {
    // Serialize quota accounting for this uploader only, across all write leases.
    let _: String = sqlx::query_scalar(
        "SELECT key FROM entities WHERE kind='account' AND (projection #>> '{id}')=$1 FOR UPDATE",
    )
    .bind(uploader)
    .fetch_one(&mut *db)
    .await?;
    Ok(())
}
async fn allocate_locked(db: &mut PgConnection, request: &MediaAllocation<'_>) -> Result<u64> {
    let (bytes,pending,recent): (i64,i64,i64) = sqlx::query_as("SELECT COALESCE(SUM(CASE WHEN deleted=0 THEN size ELSE 0 END)::bigint,0),COALESCE(count(*) FILTER (WHERE deleted=0 AND published IS NULL),0),COALESCE(count(*) FILTER (WHERE created>$2),0) FROM media WHERE uploader=$1")
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
            "SELECT EXISTS(SELECT 1 FROM entities WHERE kind='private' AND key=$1)",
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
    changed += sqlx::query("INSERT INTO media(id,uploader,conversation,size,created,ready,unreferenced,staging_clean,json) VALUES($1,$2,$3,$4,$5,0,$5,0,$6)").bind(&media.id).bind(&media.uploader).bind(&media.conversation)
        .bind(number(media.size)?).bind(number(media.created)?).bind(json).execute(db).await?.rows_affected();
    Ok(changed)
}
async fn avatar_allocate(
    db: &mut PgConnection,
    request: &MediaAllocation<'_>,
    pending_ttl: u64,
) -> Result<WriteResult> {
    // Reserve the retry ID before the uploader quota row. Different IDs and
    // different accounts remain independent; the lock lasts through commit.
    sqlx::query("SELECT pg_advisory_xact_lock(1096171860,hashtext($1))")
        .bind(&request.media.id)
        .execute(&mut *db)
        .await?;
    lock_uploader(db, &request.media.uploader).await?;
    let existing: Option<MediaColumns> = sqlx::query_as(
        "SELECT json,ready,expires,deleting,deleted FROM media WHERE id=$1 FOR UPDATE",
    )
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
    let changed = allocate_locked(db, request).await?;
    Ok(WriteResult {
        changed,
        media: vec![request.media.clone()],
        ..Default::default()
    })
}
pub(in crate::engine::database) async fn stickers(
    db: &mut PgConnection,
    owner: &str,
) -> Result<Vec<Media>> {
    let rows: Vec<MediaColumns> = sqlx::query_as("SELECT m.json,m.ready,m.expires,m.deleting,m.deleted FROM sticker_library l JOIN media m ON m.id=l.media_id WHERE l.owner=$1 AND m.ready=1 AND m.deleting=0 AND m.deleted=0 ORDER BY l.created DESC,l.media_id")
        .bind(owner).fetch_all(db).await?;
    rows.into_iter().map(decode).collect()
}
pub(in crate::engine::database) async fn sticker_digest_taken(
    db: &mut PgConnection,
    owner: &str,
    digest: &str,
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sticker_library l JOIN media m ON m.id=l.media_id WHERE l.owner=$1 AND m.ready=1 AND m.deleting=0 AND m.deleted=0 AND m.json->>'content_digest'=$2)",
    )
    .bind(owner)
    .bind(digest)
    .fetch_one(db)
    .await?)
}
pub(in crate::engine::database) async fn sticker_reach(
    db: &mut PgConnection,
    id: &str,
    owner: &str,
) -> Result<StickerReach> {
    let owned: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sticker_library WHERE owner=$1 AND media_id=$2)",
    )
    .bind(owner)
    .bind(id)
    .fetch_one(&mut *db)
    .await?;
    let conversations =
        sqlx::query_scalar("SELECT DISTINCT conversation FROM message_media WHERE media_id=$1")
            .bind(id)
            .fetch_all(db)
            .await?;
    Ok(StickerReach {
        owned,
        conversations,
    })
}
pub(in crate::engine::database) async fn load(
    db: &mut PgConnection,
    id: &str,
) -> Result<Option<(Media, bool)>> {
    let row: Option<(String,i64,Option<i64>,i64,i64,bool)> = sqlx::query_as("SELECT json,ready,expires,deleting,deleted,EXISTS(SELECT 1 FROM message_media WHERE media_id=media.id) FROM media WHERE id=$1").bind(id).fetch_optional(db).await?;
    row.map(|(json, ready, expires, deleting, deleted, referenced)| {
        Ok((
            decode((json, ready, expires, deleting, deleted))?,
            referenced,
        ))
    })
    .transpose()
}
