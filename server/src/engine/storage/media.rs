use super::*;
use crate::engine::database::OptionalExtension;
use crate::engine::media::{Media, media_references};

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS media(id TEXT PRIMARY KEY, uploader TEXT NOT NULL, conversation TEXT NOT NULL, size INTEGER NOT NULL, created INTEGER NOT NULL, ready INTEGER NOT NULL, unreferenced INTEGER, staging_clean INTEGER NOT NULL DEFAULT 0, json TEXT NOT NULL, expires INTEGER, published INTEGER, deleting INTEGER NOT NULL DEFAULT 0, deleted INTEGER NOT NULL DEFAULT 0, cleanup_attempt INTEGER NOT NULL DEFAULT 0, staging_attempt INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS message_media(conversation TEXT NOT NULL, sequence INTEGER NOT NULL, media_id TEXT NOT NULL REFERENCES media(id), PRIMARY KEY(conversation,sequence,media_id), FOREIGN KEY(conversation,sequence) REFERENCES messages(conversation,sequence) ON DELETE CASCADE) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS message_media_id ON message_media(media_id);
";

pub(super) const INDEXES: &str = "
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

const TOMBSTONES_SQL: crate::engine::database::Sql<'static> = sql(
    "DELETE FROM media WHERE deleted=1 AND staging_clean=1 AND created<?1 AND NOT EXISTS(SELECT 1 FROM message_media WHERE media_id=media.id) AND ('/api/avatars/'||media.id||'/file') NOT IN (SELECT (json_extract(json,'$.metadata') -> '@global' ->> 'avatar') FROM entities WHERE kind='account' AND (json_extract(json,'$.metadata') -> '@global' ->> 'avatar') IS NOT NULL)",
    "DELETE FROM media WHERE deleted=1 AND staging_clean=1 AND created<$1 AND NOT EXISTS(SELECT 1 FROM message_media WHERE media_id=media.id) AND ('/api/avatars/'||media.id||'/file') NOT IN (SELECT ((projection #> '{metadata}') -> '@global' ->> 'avatar') FROM entities WHERE kind='account' AND ((projection #> '{metadata}') -> '@global' ->> 'avatar') IS NOT NULL)",
);
const STARTUP_CLEANUP_SQL: crate::engine::database::Sql<'static> = sql(
    "UPDATE media SET deleting=1,cleanup_attempt=0 WHERE deleted=0 AND NOT EXISTS(SELECT 1 FROM message_media WHERE media_id=media.id) AND ('/api/avatars/'||media.id||'/file') NOT IN (SELECT (json_extract(json,'$.metadata') -> '@global' ->> 'avatar') FROM entities WHERE kind='account' AND (json_extract(json,'$.metadata') -> '@global' ->> 'avatar') IS NOT NULL)",
    "UPDATE media SET deleting=1,cleanup_attempt=0 WHERE deleted=0 AND NOT EXISTS(SELECT 1 FROM message_media WHERE media_id=media.id) AND ('/api/avatars/'||media.id||'/file') NOT IN (SELECT ((projection #> '{metadata}') -> '@global' ->> 'avatar') FROM entities WHERE kind='account' AND ((projection #> '{metadata}') -> '@global' ->> 'avatar') IS NOT NULL)",
);
const CLAIM_CLEANUP_SQL: crate::engine::database::Sql<'static> = sql("SELECT json,ready,expires,deleting,deleted FROM media WHERE id IN (
                SELECT id FROM (SELECT id FROM media WHERE deleted=0 AND deleting=1 AND cleanup_attempt<?4 ORDER BY cleanup_attempt,created LIMIT ?3)
                UNION SELECT id FROM (SELECT id FROM media WHERE deleted=0 AND deleting=0 AND expires<=?1 AND cleanup_attempt<?4 ORDER BY expires,created LIMIT ?3)
                UNION SELECT id FROM (SELECT id FROM media WHERE deleted=0 AND deleting=0 AND unreferenced IS NOT NULL AND (published IS NOT NULL OR unreferenced<=?2) AND NOT EXISTS(SELECT 1 FROM message_media WHERE media_id=media.id) AND ('/api/avatars/'||media.id||'/file') NOT IN (SELECT (json_extract(json,'$.metadata') -> '@global' ->> 'avatar') FROM entities WHERE kind='account' AND (json_extract(json,'$.metadata') -> '@global' ->> 'avatar') IS NOT NULL) AND cleanup_attempt<?4 ORDER BY unreferenced,created LIMIT ?3)
            ) ORDER BY cleanup_attempt,created LIMIT ?3", "SELECT json,ready,expires,deleting,deleted FROM media WHERE id IN (\n                SELECT id FROM (SELECT id FROM media WHERE deleted=0 AND deleting=1 AND cleanup_attempt<$4 ORDER BY cleanup_attempt,created LIMIT $3)\n                UNION SELECT id FROM (SELECT id FROM media WHERE deleted=0 AND deleting=0 AND expires<=$1 AND cleanup_attempt<$4 ORDER BY expires,created LIMIT $3)\n                UNION SELECT id FROM (SELECT id FROM media WHERE deleted=0 AND deleting=0 AND unreferenced IS NOT NULL AND (published IS NOT NULL OR unreferenced<=$2) AND NOT EXISTS(SELECT 1 FROM message_media WHERE media_id=media.id) AND ('/api/avatars/'||media.id||'/file') NOT IN (SELECT ((projection #> '{metadata}') -> '@global' ->> 'avatar') FROM entities WHERE kind='account' AND ((projection #> '{metadata}') -> '@global' ->> 'avatar') IS NOT NULL) AND cleanup_attempt<$4 ORDER BY unreferenced,created LIMIT $3)\n            ) ORDER BY cleanup_attempt,created LIMIT $3");

// Disabled accounts intentionally retain their avatar reference for re-enabling.
// Uncorrelated NOT IN builds one transient lookup per statement; account JSON
// is not reparsed for each candidate. Exclude NULLs so accounts without avatars
// cannot suppress cleanup.
fn delete_tombstones(db: &Connection, cutoff: u64) -> Result<(), EngineError> {
    db.execute(TOMBSTONES_SQL, [cutoff])
        .map_err(EngineError::unavailable)?;
    Ok(())
}
// Columns are authoritative for mutable state; legacy JSON copies are ignored.
fn media_row(row: &crate::engine::database::Row<'_>) -> crate::engine::database::Result<Media> {
    let json: String = row.get(0)?;
    let mut media: Media = serde_json::from_str(&json)
        .map_err(|_| crate::engine::database::Error::Conversion("Invalid stored media JSON"))?;
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
) -> Result<(), EngineError> {
    let ids = media_references(&message.text);
    let previous: BTreeSet<String> = {
        let mut query = db
            .prepare(sql(
                "SELECT media_id FROM message_media WHERE conversation=?1 AND sequence=?2",
                "SELECT media_id FROM message_media WHERE conversation=$1 AND sequence=$2",
            ))
            .map_err(EngineError::unavailable)?;
        query
            .query_map(params![conversation, message.sequence], |row| row.get(0))
            .map_err(EngineError::unavailable)?
            .collect::<Result<_, _>>()
            .map_err(EngineError::unavailable)?
    };
    reconcile_references(db, conversation, message, ids, previous)
}

pub(super) fn reconcile_insert(
    db: &Connection,
    conversation: &str,
    message: &Message,
) -> Result<(), EngineError> {
    let ids = media_references(&message.text);
    if ids.is_empty() {
        return Ok(());
    }
    reconcile_references(db, conversation, message, ids, BTreeSet::new())
}

fn reconcile_references(
    db: &Connection,
    conversation: &str,
    message: &Message,
    ids: BTreeSet<String>,
    previous: BTreeSet<String>,
) -> Result<(), EngineError> {
    // Validate before replacing references, allowing an editor to preserve another
    // author's attachment that is already published in this conversation.
    for id in &ids {
        if previous.contains(id) {
            continue;
        }
        let valid: bool = db.query_row(sql("SELECT EXISTS(SELECT 1 FROM media WHERE id=?1 AND conversation=?2 AND ready=1 AND deleting=0 AND deleted=0 AND (expires IS NULL OR expires>?4) AND (uploader=?3 OR EXISTS(SELECT 1 FROM message_media WHERE media_id=?1 AND conversation=?2)))", "SELECT EXISTS(SELECT 1 FROM media WHERE id=$1 AND conversation=$2 AND ready=1 AND deleting=0 AND deleted=0 AND (expires IS NULL OR expires>$4) AND (uploader=$3 OR EXISTS(SELECT 1 FROM message_media WHERE media_id=$1 AND conversation=$2)))"), params![id,conversation,message.author_id,now()], |r| r.get(0)).map_err(EngineError::unavailable)?;
        if !valid {
            return Err(EngineError::NotFound("Upload not found.".into()));
        }
    }
    if ids == previous {
        return Ok(());
    }
    for id in previous.difference(&ids) {
        db.execute(
            sql(
                "DELETE FROM message_media WHERE conversation=?1 AND sequence=?2 AND media_id=?3",
                "DELETE FROM message_media WHERE conversation=$1 AND sequence=$2 AND media_id=$3",
            ),
            params![conversation, message.sequence, id],
        )?;
    }
    for id in ids.difference(&previous) {
        db.execute(
            sql(
                "INSERT INTO message_media VALUES(?1,?2,?3)",
                "INSERT INTO message_media VALUES($1,$2,$3)",
            ),
            params![conversation, message.sequence, id],
        )
        .map_err(EngineError::unavailable)?;
        db.execute(
            sql(
                "UPDATE media SET unreferenced=NULL,published=COALESCE(published,?2) WHERE id=?1",
                "UPDATE media SET unreferenced=NULL,published=COALESCE(published,$2) WHERE id=$1",
            ),
            params![id, now()],
        )
        .map_err(EngineError::unavailable)?;
    }
    Ok(())
}

impl Engine {
    pub(crate) fn media_read_version(&self) -> Result<(u64, u64), EngineError> {
        Ok((
            self.revision,
            self.store
                .db
                .lock()
                .map_err(EngineError::unavailable)?
                .total_changes(),
        ))
    }

    // Called before accepting uploads on restart. SQLite references are authoritative;
    // recent uploads from a previous process no longer have a live reservation.
    pub(crate) fn prepare_media_cleanup(&self) -> Result<(), EngineError> {
        self.store
            .db
            .lock()
            .map_err(EngineError::unavailable)?
            .execute(STARTUP_CLEANUP_SQL, [])
            .map_err(EngineError::unavailable)?;
        Ok(())
    }
    pub(crate) fn store_media_allocation(&self, media: &Media) -> Result<(), EngineError> {
        let mut db = self.store.db.lock().map_err(EngineError::unavailable)?;
        let tx = db.savepoint().map_err(EngineError::unavailable)?;
        let (bytes, pending, recent): (u64,u64,u64) = tx.query_row(sql("SELECT COALESCE(SUM(CASE WHEN deleted=0 THEN size ELSE 0 END),0),COALESCE(SUM(deleted=0 AND published IS NULL),0),COALESCE(SUM(created>?2),0) FROM media WHERE uploader=?1", "SELECT COALESCE(SUM(CASE WHEN deleted=0 THEN size ELSE 0 END)::bigint,0),COALESCE(count(*) FILTER (WHERE deleted=0 AND published IS NULL),0),COALESCE(count(*) FILTER (WHERE created>$2),0) FROM media WHERE uploader=$1"), params![media.uploader,now().saturating_sub(60)], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(EngineError::unavailable)?;
        let quota = self
            .data
            .users
            .iter()
            .find(|(_, u)| u.id == media.uploader)
            .and_then(|(name, _)| {
                self.metadata_value(name, &authorization::Scope::Server, "media.user_quota")
            })
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(self.media.config.user_quota)
            .min(self.media.config.user_quota);
        if bytes.saturating_add(media.size) > quota {
            return Err(
                "Upload byte quota reached; remove attachments or wait for cleanup.".into(),
            );
        }
        if pending >= self.media.config.pending_limit {
            return Err(
                "Pending upload limit reached; send or remove attachments and wait for cleanup."
                    .into(),
            );
        }
        if recent >= self.media.config.allocations_per_minute {
            return Err("Upload rate limit reached; retry in a minute.".into());
        }
        if let Some((key, chat)) = self
            .data
            .private
            .iter()
            .find(|(_, chat)| chat.id == media.conversation)
        {
            let exists: bool = tx
                .query_row(
                    sql(
                        "SELECT EXISTS(SELECT 1 FROM entities WHERE kind='private' AND key=?1)",
                        "SELECT EXISTS(SELECT 1 FROM entities WHERE kind='private' AND key=$1)",
                    ),
                    [key],
                    |row| row.get(0),
                )
                .map_err(EngineError::unavailable)?;
            if !exists {
                put_entity(&tx, "private", key, &private_metadata(chat))?;
            }
        }
        let json = serde_json::to_string(media).map_err(EngineError::unavailable)?;
        tx.execute(
            sql("INSERT INTO media(id,uploader,conversation,size,created,ready,unreferenced,staging_clean,json) VALUES(?1,?2,?3,?4,?5,0,?5,0,?6)", "INSERT INTO media(id,uploader,conversation,size,created,ready,unreferenced,staging_clean,json) VALUES($1,$2,$3,$4,$5,0,$5,0,$6)"),
            params![
                media.id,
                media.uploader,
                media.conversation,
                media.size,
                media.created,
                json
            ],
        )
        .map_err(EngineError::unavailable)?;
        tx.commit().map_err(EngineError::unavailable)
    }
    pub(crate) fn stored_media_with_reference(
        &self,
        id: &str,
    ) -> Result<(Media, bool), EngineError> {
        let db = self.store.db.lock().map_err(EngineError::unavailable)?;
        db.query_row(sql("SELECT json,ready,expires,deleting,deleted,EXISTS(SELECT 1 FROM message_media WHERE media_id=media.id) FROM media WHERE id=?1", "SELECT json,ready,expires,deleting,deleted,EXISTS(SELECT 1 FROM message_media WHERE media_id=media.id) FROM media WHERE id=$1"), [id], |row| Ok((media_row(row)?, row.get(5)?)))
            .optional().map_err(EngineError::unavailable)?.ok_or_else(|| EngineError::NotFound("Upload not found.".into()))
    }
    pub(crate) fn stored_media(&self, id: &str) -> Result<Media, EngineError> {
        self.stored_media_with_reference(id).map(|(media, _)| media)
    }
    pub(crate) fn store_media_ready(&self, media: &Media) -> Result<(), EngineError> {
        let mut media = media.clone();
        media.expires =
            if media.max_age == -1 {
                None
            } else {
                Some(now().saturating_add(
                    u64::try_from(media.max_age).map_err(|_| "Invalid media age.")?,
                ))
            };
        let json = serde_json::to_string(&media).map_err(EngineError::unavailable)?;
        let db = self.store.db.lock().map_err(EngineError::unavailable)?;
        let changed = db
            .execute(
                sql("UPDATE media SET ready=1,json=?2,unreferenced=?3,expires=?4,published=CASE WHEN conversation='@avatar' THEN ?3 ELSE published END WHERE id=?1 AND ready=0 AND deleting=0 AND deleted=0", "UPDATE media SET ready=1,json=$2,unreferenced=$3,expires=$4,published=CASE WHEN conversation='@avatar' THEN $3 ELSE published END WHERE id=$1 AND ready=0 AND deleting=0 AND deleted=0"),
                params![media.id, json, now(), media.expires],
            )
            .map_err(EngineError::unavailable)?;
        if changed != 1 {
            return Err(EngineError::Conflict(
                "Upload changed; retry completion.".into(),
            ));
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn media_attached(&self, id: &str) -> Result<bool, EngineError> {
        self.store
            .db
            .lock()
            .map_err(EngineError::unavailable)?
            .query_row(
                sql(
                    "SELECT EXISTS(SELECT 1 FROM message_media WHERE media_id=?1)",
                    "SELECT EXISTS(SELECT 1 FROM message_media WHERE media_id=$1)",
                ),
                [id],
                |r| r.get(0),
            )
            .map_err(EngineError::unavailable)
    }
    pub(crate) fn abandon_media(&self, user: &str, id: &str) -> Result<(), EngineError> {
        let media = self.stored_media(id)?;
        if self
            .data
            .users
            .get(user)
            .is_none_or(|u| u.id != media.uploader)
        {
            return Err(EngineError::NotFound("Upload not found.".into()));
        }
        let db = self.store.db.lock().map_err(EngineError::unavailable)?;
        let changed = db
            .execute(
                sql(
                    "UPDATE media SET deleting=1 WHERE id=?1 AND published IS NULL AND deleted=0",
                    "UPDATE media SET deleting=1 WHERE id=$1 AND published IS NULL AND deleted=0",
                ),
                [id],
            )
            .map_err(EngineError::unavailable)?;
        if changed == 0 {
            tracing::debug!(
                media_id = id,
                "Abandon ignored for a published or deleted upload"
            );
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn expired_media(&self) -> Result<Vec<Media>, EngineError> {
        self.expired_media_before(u64::MAX)
    }
    pub(crate) fn expired_media_before(
        &self,
        attempted_before: u64,
    ) -> Result<Vec<Media>, EngineError> {
        let timestamp = now();
        let mut db = self.store.db.lock().map_err(EngineError::unavailable)?;
        let tx = db.savepoint().map_err(EngineError::unavailable)?;
        delete_tombstones(&tx, timestamp.saturating_sub(self.media.config.pending_ttl))?;
        // Sort at most 3 * batch candidates, rather than the entire live table.
        let result = {
            let mut query = tx
                .prepare(CLAIM_CLEANUP_SQL)
                .map_err(EngineError::unavailable)?;
            query
                .query_map(
                    params![
                        timestamp,
                        timestamp.saturating_sub(self.media.config.pending_ttl),
                        self.media.config.cleanup_batch,
                        attempted_before.min(i64::MAX.unsigned_abs())
                    ],
                    media_row,
                )
                .map_err(EngineError::unavailable)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(EngineError::unavailable)?
        };
        for media in &result {
            tx.execute(
                sql(
                    "UPDATE media SET deleting=1,cleanup_attempt=?2 WHERE id=?1",
                    "UPDATE media SET deleting=1,cleanup_attempt=$2 WHERE id=$1",
                ),
                params![media.id, timestamp],
            )
            .map_err(EngineError::unavailable)?;
        }
        tx.commit().map_err(EngineError::unavailable)?;
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
    ) -> Result<Vec<String>, EngineError> {
        let timestamp = now();
        let mut db = self.store.db.lock().map_err(EngineError::unavailable)?;
        let tx = db.savepoint().map_err(EngineError::unavailable)?;
        let ids = {
            let mut query = tx.prepare(sql("SELECT id FROM media WHERE staging_clean=0 AND created<=?1 AND staging_attempt<?3 ORDER BY staging_attempt,created LIMIT ?2", "SELECT id FROM media WHERE staging_clean=0 AND created<=$1 AND staging_attempt<$3 ORDER BY staging_attempt,created LIMIT $2")).map_err(EngineError::unavailable)?;
            query
                .query_map(
                    params![
                        timestamp.saturating_sub(self.media.config.pending_ttl),
                        self.media.config.cleanup_batch,
                        attempted_before
                    ],
                    |row| row.get(0),
                )
                .map_err(EngineError::unavailable)?
                .collect::<Result<Vec<String>, _>>()
                .map_err(EngineError::unavailable)?
        };
        for id in &ids {
            tx.execute(
                sql(
                    "UPDATE media SET staging_attempt=?2 WHERE id=?1",
                    "UPDATE media SET staging_attempt=$2 WHERE id=$1",
                ),
                params![id, timestamp],
            )
            .map_err(EngineError::unavailable)?;
        }
        tx.commit().map_err(EngineError::unavailable)?;
        Ok(ids)
    }
    pub(crate) fn media_staging_cleaned(&self, id: &str) -> Result<(), EngineError> {
        self.store
            .db
            .lock()
            .map_err(EngineError::unavailable)?
            .execute(
                sql(
                    "UPDATE media SET staging_clean=1 WHERE id=?1",
                    "UPDATE media SET staging_clean=1 WHERE id=$1",
                ),
                [id],
            )
            .map_err(EngineError::unavailable)?;
        Ok(())
    }
    pub(crate) fn remove_media_record(&self, id: &str) -> Result<(), EngineError> {
        let db = self.store.db.lock().map_err(EngineError::unavailable)?;
        db.execute(
            sql(
                "UPDATE media SET deleted=1,deleting=0,ready=0 WHERE id=?1 AND deleting=1",
                "UPDATE media SET deleted=1,deleting=0,ready=0 WHERE id=$1 AND deleting=1",
            ),
            [id],
        )
        .map_err(EngineError::unavailable)?;
        // Keep tombstones while retained messages reference expired attachments.
        // Unreferenced tombstones also preserve rate accounting and staging recovery.
        delete_tombstones(&db, now().saturating_sub(self.media.config.pending_ttl))?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    #[test]
    fn avatar_cleanup_builds_one_reference_list_instead_of_a_correlated_account_scan() {
        let engine = Engine::open(std::path::Path::new(":memory:")).unwrap();
        let db = engine.store.db.lock().unwrap();
        for sql in [TOMBSTONES_SQL, STARTUP_CLEANUP_SQL, CLAIM_CLEANUP_SQL] {
            let mut query = db.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
            let bindings = vec![1_i64; query.parameter_count()];
            let plan: Vec<(i64, i64, String)> = query
                .query_map(bindings, |row| Ok((row.get(0)?, row.get(1)?, row.get(3)?)))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            assert!(
                plan.iter()
                    .any(|(_, _, detail)| detail.contains("LIST SUBQUERY")),
                "{plan:?}"
            );
            for (_, parent, detail) in &plan {
                if !detail.contains("entities") {
                    continue;
                }
                let mut ancestor = *parent;
                while let Some((_, parent, detail)) = plan.iter().find(|(id, _, _)| *id == ancestor)
                {
                    assert!(
                        !detail.contains("CORRELATED"),
                        "Account scan must not be correlated: {plan:?}"
                    );
                    ancestor = *parent;
                }
            }
        }
    }
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
        let db = engine.store.db.lock().unwrap();
        let changes = db.total_changes();
        reconcile(&db, &conversation, &message).unwrap();
        assert_eq!(db.total_changes(), changes);
    }
    #[test]
    fn attachment_edits_preserve_shared_refs_and_rollback_partial_deltas() {
        let mut e = crate::engine::tests::engine();
        let room = e.data.rooms["lobby"].id.clone();
        let mut uploads = Vec::new();
        for _ in 0..3 {
            let mut upload = e.allocate_media("alice", &room, "file.png", 8).unwrap();
            upload.ready = true;
            upload.object_key = upload.final_key();
            upload.content_type = "image/png".into();
            e.finish_media("alice", &upload).unwrap();
            uploads.push(upload.id);
        }
        let link = |id: &str| format!("![file](https://example.com/api/media/{id}/file)");
        e.execute(
            Some("alice"),
            Some("lobby"),
            &format!("{} {}", link(&uploads[0]), link(&uploads[1])),
        )
        .unwrap();
        let message = crate::engine::tests::room_messages(&e, "lobby")[0]
            .id
            .clone();
        e.store.db.lock().unwrap().execute_batch(&format!("CREATE TEMP TRIGGER keep_shared_reference BEFORE DELETE ON message_media WHEN OLD.media_id='{}' BEGIN SELECT RAISE(ABORT,'unchanged reference churn'); END;", uploads[0])).unwrap();
        e.execute(
            Some("alice"),
            Some("lobby"),
            &format!(
                "/edit {message} {} {}",
                link(&uploads[0]),
                link(&uploads[2])
            ),
        )
        .unwrap();
        let refs = || {
            e.store
                .db
                .lock()
                .unwrap()
                .prepare("SELECT media_id FROM message_media WHERE conversation=?1")
                .unwrap()
                .query_map([&room], |r| r.get::<_, String>(0))
                .unwrap()
                .collect::<Result<BTreeSet<_>, _>>()
                .unwrap()
        };
        assert_eq!(refs(), [uploads[0].clone(), uploads[2].clone()].into());
        let before = crate::engine::tests::room_messages(&e, "lobby");
        let revision = e.data.rooms["lobby"].revision;
        e.store.db.lock().unwrap().execute_batch("CREATE TEMP TRIGGER reject_reference BEFORE INSERT ON message_media BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(
            e.execute(
                Some("alice"),
                Some("lobby"),
                &format!(
                    "/edit {message} {} {}",
                    link(&uploads[0]),
                    link(&uploads[1])
                )
            )
            .is_err()
        );
        assert!(crate::engine::tests::room_messages(&e, "lobby") == before);
        assert_eq!(e.data.rooms["lobby"].revision, revision);
        let db = e.store.db.lock().unwrap();
        let refs: BTreeSet<String> = db
            .prepare("SELECT media_id FROM message_media WHERE conversation=?1")
            .unwrap()
            .query_map([&room], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(refs, [uploads[0].clone(), uploads[2].clone()].into());
        assert!(
            db.query_row::<bool, _, _>(
                "SELECT unreferenced IS NULL FROM media WHERE id=?1",
                [&uploads[2]],
                |r| r.get(0)
            )
            .unwrap()
        );
    }
}
