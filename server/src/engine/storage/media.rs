use super::*;
use crate::engine::media::{Media, media_references};
use rusqlite::OptionalExtension;

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS media(id TEXT PRIMARY KEY, uploader TEXT NOT NULL, conversation TEXT NOT NULL, size INTEGER NOT NULL, created INTEGER NOT NULL, ready INTEGER NOT NULL, unreferenced INTEGER, staging_clean INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL, expires INTEGER, published INTEGER, deleting INTEGER NOT NULL DEFAULT 0, deleted INTEGER NOT NULL DEFAULT 0, cleanup_attempt INTEGER NOT NULL DEFAULT 0, staging_attempt INTEGER NOT NULL DEFAULT 0);
CREATE INDEX IF NOT EXISTS media_expiry ON media(deleted,deleting,expires,unreferenced);
CREATE INDEX IF NOT EXISTS media_uploader ON media(uploader,created);
CREATE TABLE IF NOT EXISTS message_media(conversation TEXT NOT NULL, sequence INTEGER NOT NULL, media_id TEXT NOT NULL REFERENCES media(id), PRIMARY KEY(conversation,sequence,media_id), FOREIGN KEY(conversation,sequence) REFERENCES messages(conversation,sequence) ON DELETE CASCADE) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS message_media_id ON message_media(media_id);
";

pub(super) const INDEXES: &str = "
DROP INDEX IF EXISTS media_expiry;
DROP INDEX IF EXISTS media_uploader;
CREATE INDEX IF NOT EXISTS media_quota ON media(uploader,deleted,published,created,size);
CREATE INDEX IF NOT EXISTS media_due_retry ON media(cleanup_attempt,created) WHERE deleted=0 AND deleting=1;
CREATE INDEX IF NOT EXISTS media_due_expiry ON media(expires,created) WHERE deleted=0 AND deleting=0;
CREATE INDEX IF NOT EXISTS media_due_orphan ON media(unreferenced,created) WHERE deleted=0 AND deleting=0 AND unreferenced IS NOT NULL;
CREATE INDEX IF NOT EXISTS media_staging_due ON media(staging_attempt,created) WHERE staging_clean=0;
CREATE INDEX IF NOT EXISTS media_tombstone ON media(created) WHERE deleted=1 AND staging_clean=1;
CREATE TRIGGER IF NOT EXISTS media_reference_removed AFTER DELETE ON message_media BEGIN
 UPDATE media SET unreferenced=COALESCE(unreferenced,unixepoch()) WHERE id=OLD.media_id AND NOT EXISTS(SELECT 1 FROM message_media WHERE media_id=OLD.media_id);
END;
";

fn delete_tombstones(db: &Connection, cutoff: u64) -> Result<(), String> {
    db.execute("DELETE FROM media WHERE deleted=1 AND staging_clean=1 AND created<?1 AND NOT EXISTS(SELECT 1 FROM message_media WHERE media_id=media.id)", [cutoff]).map_err(|e| e.to_string())?;
    Ok(())
}
// Columns are authoritative for mutable state; legacy JSON copies are ignored.
fn media_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Media> {
    let json: String = row.get(0)?;
    let mut media: Media = serde_json::from_str(&json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    media.ready = row.get(1)?;
    media.expires = row.get(2)?;
    media.deleting = row.get(3)?;
    media.deleted = row.get(4)?;
    Ok(media)
}

pub(super) fn reconcile(
    db: &Connection,
    conversation: &str,
    message: &Message,
) -> Result<(), String> {
    let ids = media_references(&message.text);
    let previous: BTreeSet<String> = {
        let mut query = db
            .prepare("SELECT media_id FROM message_media WHERE conversation=?1 AND sequence=?2")
            .map_err(|e| e.to_string())?;
        query
            .query_map(params![conversation, message.sequence], |row| row.get(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?
    };
    // Validate before replacing references, allowing an editor to preserve another
    // author's attachment that is already published in this conversation.
    for id in &ids {
        if previous.contains(id) {
            continue;
        }
        let valid: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM media WHERE id=?1 AND conversation=?2 AND ready=1 AND deleting=0 AND deleted=0 AND (expires IS NULL OR expires>?4) AND (uploader=?3 OR EXISTS(SELECT 1 FROM message_media WHERE media_id=?1 AND conversation=?2)))", params![id,conversation,message.author_id,now()], |r| r.get(0)).map_err(|e| e.to_string())?;
        if !valid {
            return Err(
                "Attachment is unavailable or belongs to another conversation/user.".into(),
            );
        }
    }
    if ids == previous {
        return Ok(());
    }
    db.execute(
        "DELETE FROM message_media WHERE conversation=?1 AND sequence=?2",
        params![conversation, message.sequence],
    )
    .map_err(|e| e.to_string())?;
    for id in ids {
        db.execute(
            "INSERT INTO message_media VALUES(?1,?2,?3)",
            params![conversation, message.sequence, id],
        )
        .map_err(|e| e.to_string())?;
        db.execute(
            "UPDATE media SET unreferenced=NULL,published=COALESCE(published,?2) WHERE id=?1",
            params![id, now()],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

impl Engine {
    // Called before accepting uploads on restart. SQLite references are authoritative;
    // recent uploads from a previous process no longer have a live reservation.
    pub(crate) fn prepare_media_cleanup(&self) -> Result<(), String> {
        self.db.lock().map_err(|_| "Storage unavailable.")?.execute(
            "UPDATE media SET deleting=1,cleanup_attempt=0 WHERE deleted=0 AND NOT EXISTS(SELECT 1 FROM message_media WHERE media_id=media.id)",
            [],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }
    pub(crate) fn store_media_allocation(&self, media: &Media) -> Result<(), String> {
        let mut db = self.db.lock().map_err(|_| "Storage unavailable.")?;
        let tx = db.savepoint().map_err(|e| e.to_string())?;
        let (bytes, pending, recent): (u64,u64,u64) = tx.query_row("SELECT COALESCE(SUM(CASE WHEN deleted=0 THEN size ELSE 0 END),0),COALESCE(SUM(deleted=0 AND published IS NULL),0),COALESCE(SUM(created>?2),0) FROM media WHERE uploader=?1", params![media.uploader,now().saturating_sub(60)], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|e| e.to_string())?;
        if bytes.saturating_add(media.size) > self.media_config.user_quota {
            return Err(
                "Upload byte quota reached; remove attachments or wait for cleanup.".into(),
            );
        }
        if pending >= self.media_config.pending_limit {
            return Err(
                "Pending upload limit reached; send or remove attachments and wait for cleanup."
                    .into(),
            );
        }
        if recent >= self.media_config.allocations_per_minute {
            return Err("Upload rate limit reached; retry in a minute.".into());
        }
        let json = serde_json::to_string(media).map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO media(id,uploader,conversation,size,created,ready,unreferenced,staging_clean,json) VALUES(?1,?2,?3,?4,?5,0,?5,0,?6)",
            params![
                media.id,
                media.uploader,
                media.conversation,
                media.size,
                media.created,
                json
            ],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
    pub(crate) fn stored_media_with_reference(&self, id: &str) -> Result<(Media, bool), String> {
        let db = self.db.lock().map_err(|_| "Storage unavailable.")?;
        db.query_row("SELECT json,ready,expires,deleting,deleted,EXISTS(SELECT 1 FROM message_media WHERE media_id=media.id) FROM media WHERE id=?1", [id], |row| Ok((media_row(row)?, row.get(5)?)))
            .optional().map_err(|e| e.to_string())?.ok_or_else(|| "Upload not found.".into())
    }
    pub(crate) fn stored_media(&self, id: &str) -> Result<Media, String> {
        self.stored_media_with_reference(id).map(|(media, _)| media)
    }
    pub(crate) fn store_media_ready(&self, media: &Media) -> Result<(), String> {
        let mut media = media.clone();
        media.expires =
            if media.max_age == -1 {
                None
            } else {
                Some(now().saturating_add(
                    u64::try_from(media.max_age).map_err(|_| "Invalid media age.")?,
                ))
            };
        let json = serde_json::to_string(&media).map_err(|e| e.to_string())?;
        let db = self.db.lock().map_err(|_| "Storage unavailable.")?;
        let changed = db
            .execute(
                "UPDATE media SET ready=1,json=?2,unreferenced=?3,expires=?4 WHERE id=?1 AND ready=0 AND deleting=0 AND deleted=0",
                params![media.id, json, now(), media.expires],
            )
            .map_err(|e| e.to_string())?;
        if changed != 1 {
            return Err("Upload changed; retry completion.".into());
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn media_attached(&self, id: &str) -> Result<bool, String> {
        self.db
            .lock()
            .map_err(|_| "Storage unavailable.")?
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM message_media WHERE media_id=?1)",
                [id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())
    }
    pub(crate) fn abandon_media(&self, user: &str, id: &str) -> Result<(), String> {
        let media = self.stored_media(id)?;
        if self
            .data
            .users
            .get(user)
            .is_none_or(|u| u.id != media.uploader)
        {
            return Err("Upload not found.".into());
        }
        let db = self.db.lock().map_err(|_| "Storage unavailable.")?;
        let changed = db
            .execute(
                "UPDATE media SET deleting=1 WHERE id=?1 AND published IS NULL AND deleted=0",
                [id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            tracing::debug!(
                media_id = id,
                "Abandon ignored for a published or deleted upload"
            );
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn expired_media(&self) -> Result<Vec<Media>, String> {
        self.expired_media_before(u64::MAX)
    }
    pub(crate) fn expired_media_before(&self, attempted_before: u64) -> Result<Vec<Media>, String> {
        let timestamp = now();
        let mut db = self.db.lock().map_err(|_| "Storage unavailable.")?;
        let tx = db.savepoint().map_err(|e| e.to_string())?;
        delete_tombstones(&tx, timestamp.saturating_sub(self.media_config.pending_ttl))?;
        // Sort at most 3 * batch candidates, rather than the entire live table.
        let result = {
            let mut query = tx.prepare("SELECT json,ready,expires,deleting,deleted FROM media WHERE id IN (
                SELECT id FROM (SELECT id FROM media WHERE deleted=0 AND deleting=1 AND cleanup_attempt<?4 ORDER BY cleanup_attempt,created LIMIT ?3)
                UNION SELECT id FROM (SELECT id FROM media WHERE deleted=0 AND deleting=0 AND expires<=?1 AND cleanup_attempt<?4 ORDER BY expires,created LIMIT ?3)
                UNION SELECT id FROM (SELECT id FROM media WHERE deleted=0 AND deleting=0 AND unreferenced IS NOT NULL AND (published IS NOT NULL OR unreferenced<=?2) AND NOT EXISTS(SELECT 1 FROM message_media WHERE media_id=media.id) AND cleanup_attempt<?4 ORDER BY unreferenced,created LIMIT ?3)
            ) ORDER BY cleanup_attempt,created LIMIT ?3").map_err(|e| e.to_string())?;
            query
                .query_map(
                    params![
                        timestamp,
                        timestamp.saturating_sub(self.media_config.pending_ttl),
                        self.media_config.cleanup_batch,
                        attempted_before.min(i64::MAX.unsigned_abs())
                    ],
                    media_row,
                )
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
        };
        for media in &result {
            tx.execute(
                "UPDATE media SET deleting=1,cleanup_attempt=?2 WHERE id=?1",
                params![media.id, timestamp],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(result
            .into_iter()
            .map(|mut media| {
                media.deleting = true;
                media
            })
            .collect())
    }
    pub(crate) fn media_staging_cleanup(
        &self,
        attempted_before: u64,
    ) -> Result<Vec<String>, String> {
        let timestamp = now();
        let mut db = self.db.lock().map_err(|_| "Storage unavailable.")?;
        let tx = db.savepoint().map_err(|e| e.to_string())?;
        let ids = {
            let mut query = tx.prepare("SELECT id FROM media WHERE staging_clean=0 AND created<=?1 AND staging_attempt<?3 ORDER BY staging_attempt,created LIMIT ?2").map_err(|e| e.to_string())?;
            query
                .query_map(
                    params![
                        timestamp.saturating_sub(self.media_config.pending_ttl),
                        self.media_config.cleanup_batch,
                        attempted_before
                    ],
                    |row| row.get(0),
                )
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<String>, _>>()
                .map_err(|e| e.to_string())?
        };
        for id in &ids {
            tx.execute(
                "UPDATE media SET staging_attempt=?2 WHERE id=?1",
                params![id, timestamp],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(ids)
    }
    pub(crate) fn media_staging_cleaned(&self, id: &str) -> Result<(), String> {
        self.db
            .lock()
            .map_err(|_| "Storage unavailable.")?
            .execute("UPDATE media SET staging_clean=1 WHERE id=?1", [id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub(crate) fn remove_media_record(&self, id: &str) -> Result<(), String> {
        let db = self.db.lock().map_err(|_| "Storage unavailable.")?;
        db.execute(
            "UPDATE media SET deleted=1,deleting=0,ready=0 WHERE id=?1 AND deleting=1",
            [id],
        )
        .map_err(|e| e.to_string())?;
        // Keep tombstones while retained messages reference expired attachments.
        // Unreferenced tombstones also preserve rate accounting and staging recovery.
        delete_tombstones(&db, now().saturating_sub(self.media_config.pending_ttl))?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    #[test]
    fn reconciling_media_free_message_does_not_write() {
        let mut engine = Engine::open(std::path::Path::new(":memory:")).unwrap();
        engine
            .provision("alice", "hash".into(), true, false)
            .unwrap();
        engine
            .execute(Some("alice"), None, "/new room files")
            .unwrap();
        engine
            .execute(Some("alice"), Some("files"), "plain text")
            .unwrap();
        let message = engine.history("alice", "files").unwrap().messages[0].clone();
        let conversation = engine.data.rooms["files"].id.clone();
        let db = engine.db.lock().unwrap();
        let changes = db.total_changes();
        reconcile(&db, &conversation, &message).unwrap();
        assert_eq!(db.total_changes(), changes);
    }
}
