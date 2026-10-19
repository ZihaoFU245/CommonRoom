use super::*;
use crate::engine::media::{Media, media_references};
use rusqlite::OptionalExtension;

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS media(id TEXT PRIMARY KEY, uploader TEXT NOT NULL, conversation TEXT NOT NULL, size INTEGER NOT NULL, created INTEGER NOT NULL, ready INTEGER NOT NULL, unreferenced INTEGER, staging_clean INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL, expires INTEGER, published INTEGER, deleting INTEGER NOT NULL DEFAULT 0, deleted INTEGER NOT NULL DEFAULT 0, cleanup_attempt INTEGER NOT NULL DEFAULT 0);
CREATE INDEX IF NOT EXISTS media_expiry ON media(deleted,deleting,expires,unreferenced);
CREATE INDEX IF NOT EXISTS media_uploader ON media(uploader,created);
CREATE TABLE IF NOT EXISTS message_media(conversation TEXT NOT NULL, sequence INTEGER NOT NULL, media_id TEXT NOT NULL REFERENCES media(id), PRIMARY KEY(conversation,sequence,media_id), FOREIGN KEY(conversation,sequence) REFERENCES messages(conversation,sequence) ON DELETE CASCADE) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS message_media_id ON message_media(media_id);
";

pub(super) fn reconcile(
    db: &Connection,
    conversation: &str,
    message: &Message,
) -> Result<(), String> {
    let ids = media_references(&message.text);
    // Validate before replacing references, allowing an editor to preserve another
    // author's attachment that is already published in this conversation.
    for id in &ids {
        let existing: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM message_media WHERE conversation=?1 AND sequence=?2 AND media_id=?3)", params![conversation,message.sequence,id], |r| r.get(0)).map_err(|e| e.to_string())?;
        if existing {
            continue;
        }
        let valid: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM media WHERE id=?1 AND conversation=?2 AND ready=1 AND deleting=0 AND deleted=0 AND (expires IS NULL OR expires>?4) AND (uploader=?3 OR EXISTS(SELECT 1 FROM message_media WHERE media_id=?1 AND conversation=?2)))", params![id,conversation,message.author_id,now()], |r| r.get(0)).map_err(|e| e.to_string())?;
        if !valid {
            return Err(
                "Attachment is unavailable or belongs to another conversation/user.".into(),
            );
        }
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
    pub(crate) fn store_media_allocation(&self, media: &Media) -> Result<(), String> {
        let mut db = self.db.lock().map_err(|_| "Storage unavailable.")?;
        let tx = db.savepoint().map_err(|e| e.to_string())?;
        let (bytes, pending, recent): (u64,u64,u64) = tx.query_row("SELECT COALESCE(SUM(CASE WHEN deleted=0 THEN size ELSE 0 END),0),COALESCE(SUM(deleted=0 AND published IS NULL),0),COALESCE(SUM(created>?2),0) FROM media WHERE uploader=?1", params![media.uploader,now().saturating_sub(60)], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|e| e.to_string())?;
        if bytes.saturating_add(media.size) > self.media_config.user_quota
            || pending >= self.media_config.pending_limit
            || recent >= self.media_config.allocations_per_minute
        {
            return Err("Upload quota reached; retry later or remove attachments.".into());
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
    pub(crate) fn stored_media(&self, id: &str) -> Result<Media, String> {
        let db = self.db.lock().map_err(|_| "Storage unavailable.")?;
        let json: Option<String> = db
            .query_row("SELECT json FROM media WHERE id=?1", [id], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())?;
        serde_json::from_str(&json.ok_or("Upload not found.")?).map_err(|e| e.to_string())
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
        db.execute("UPDATE media SET deleting=1,json=json_set(json,'$.deleting',json('true')) WHERE id=?1 AND published IS NULL AND deleted=0", [id]).map_err(|e| e.to_string())?;
        Ok(())
    }
    pub(crate) fn expired_media(&self) -> Result<Vec<Media>, String> {
        let mut db = self.db.lock().map_err(|_| "Storage unavailable.")?;
        let tx = db.savepoint().map_err(|e| e.to_string())?;
        tx.execute("UPDATE media SET unreferenced=?1 WHERE unreferenced IS NULL AND NOT EXISTS(SELECT 1 FROM message_media WHERE media_id=media.id)", [now()]).map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM media WHERE deleted=1 AND staging_clean=1 AND created<?1 AND NOT EXISTS(SELECT 1 FROM message_media WHERE media_id=media.id)", [now().saturating_sub(self.media_config.pending_ttl)]).map_err(|e| e.to_string())?;
        // Oldest attempt first prevents a permanently failing object starving the queue.
        let mut result: Vec<Media> = {
            let mut q = tx.prepare("SELECT json FROM media WHERE deleted=0 AND (deleting=1 OR expires<=?1 OR unreferenced<=?2) ORDER BY cleanup_attempt,created LIMIT ?3").map_err(|e| e.to_string())?;
            let rows = q
                .query_map(
                    params![
                        now(),
                        now().saturating_sub(self.media_config.pending_ttl),
                        self.media_config.cleanup_batch
                    ],
                    |r| r.get::<_, String>(0),
                )
                .map_err(|e| e.to_string())?;
            rows.map(|row| {
                serde_json::from_str(&row.map_err(|e| e.to_string())?).map_err(|e| e.to_string())
            })
            .collect::<Result<_, String>>()?
        };
        for media in &mut result {
            media.deleting = true;
            tx.execute("UPDATE media SET deleting=1,cleanup_attempt=?2,json=json_set(json,'$.deleting',json('true')) WHERE id=?1", params![media.id, now()]).map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(result)
    }
    pub(crate) fn media_staging_cleanup(&self) -> Result<Vec<String>, String> {
        let db = self.db.lock().map_err(|_| "Storage unavailable.")?;
        let mut q = db.prepare("SELECT id FROM media WHERE staging_clean=0 AND created<=?1 ORDER BY created LIMIT ?2").map_err(|e| e.to_string())?;
        q.query_map(
            params![
                now().saturating_sub(self.media_config.pending_ttl),
                self.media_config.cleanup_batch
            ],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
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
        db.execute("UPDATE media SET deleted=1,deleting=0,ready=0,json=json_set(json,'$.deleted',json('true'),'$.deleting',json('false'),'$.ready',json('false')) WHERE id=?1 AND deleting=1", [id]).map_err(|e| e.to_string())?;
        // Keep tombstones while retained messages reference expired attachments.
        // Unreferenced tombstones also preserve rate accounting and staging recovery.
        db.execute("DELETE FROM media WHERE deleted=1 AND staging_clean=1 AND created<?1 AND NOT EXISTS(SELECT 1 FROM message_media WHERE media_id=media.id)", [now().saturating_sub(self.media_config.pending_ttl)]).map_err(|e| e.to_string())?;
        Ok(())
    }
}
