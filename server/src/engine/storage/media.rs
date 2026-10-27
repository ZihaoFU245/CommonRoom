//! Media authorization/configuration coordination above typed lifecycle storage.
use super::*;
#[cfg(test)]
use crate::engine::media::Media;
#[cfg(test)]
use database::{CLAIM_CLEANUP_SQL, STARTUP_CLEANUP_SQL, TOMBSTONES_SQL};

impl Engine {
    #[cfg(test)]
    pub(crate) fn prepare_media_cleanup(&self) -> Result<(), EngineError> {
        database::block_on_storage(self.prepare_media_cleanup_async())?
    }
    pub(crate) async fn prepare_media_cleanup_async(&self) -> Result<(), EngineError> {
        self.store
            .db
            .media_write(&database::MediaWrite::PrepareCleanup {
                pending_before: now().saturating_sub(self.media.config.pending_ttl),
            })
            .await?;
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn store_media_allocation(&self, media: &Media) -> Result<(), EngineError> {
        let quota = self
            .data
            .users
            .iter()
            .find(|(_, user)| user.id == media.uploader)
            .and_then(|(name, _)| {
                self.metadata_value(name, &authorization::Scope::Server, "media.user_quota")
            })
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(self.media.config.user_quota)
            .min(self.media.config.user_quota);
        let private = self
            .data
            .private
            .iter()
            .find(|(_, chat)| chat.id == media.conversation)
            .map(|(key, chat)| Ok::<_, EngineError>((key.as_str(), private_json(chat)?)))
            .transpose()?;
        self.store
            .db
            .media_write_blocking(&database::MediaWrite::Allocate(database::MediaAllocation {
                media,
                quota,
                pending_limit: self.media.config.pending_limit,
                rate_limit: self.media.config.allocations_per_minute,
                at: now(),
                private,
            }))?;
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn stored_media_with_reference(
        &self,
        id: &str,
    ) -> Result<(Media, bool), EngineError> {
        Ok(self.store.db.stored_media_blocking(id)?)
    }
    #[cfg(test)]
    pub(crate) fn stored_media(&self, id: &str) -> Result<Media, EngineError> {
        self.stored_media_with_reference(id).map(|(media, _)| media)
    }
    #[cfg(test)]
    pub(crate) fn store_media_ready(&self, media: &Media) -> Result<(), EngineError> {
        let at = now();
        let media = media.completed_record(at)?;
        self.store
            .db
            .media_write_blocking(&database::MediaWrite::Ready { media: &media, at })?;
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn media_attached(&self, id: &str) -> Result<bool, EngineError> {
        match self.store.db.stored_media_blocking(id) {
            Ok((_, referenced)) => Ok(referenced),
            Err(database::Error::MissingMedia) => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
    #[cfg(test)]
    pub(crate) fn abandon_media(&self, user: &str, id: &str) -> Result<(), EngineError> {
        let media = self.stored_media(id)?;
        if self
            .data
            .users
            .get(user)
            .is_none_or(|user| user.id != media.uploader)
        {
            return Err(EngineError::NotFound("Upload not found.".into()));
        }
        let result = self
            .store
            .db
            .media_write_blocking(&database::MediaWrite::Abandon {
                id,
                uploader: &media.uploader,
            })?;
        if result.affected == 0 {
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
    #[cfg(test)]
    pub(crate) fn expired_media_before(
        &self,
        attempted_before: u64,
    ) -> Result<Vec<Media>, EngineError> {
        let at = now();
        Ok(self
            .store
            .db
            .media_write_blocking(&database::MediaWrite::ClaimCleanup {
                at,
                pending_before: at.saturating_sub(self.media.config.pending_ttl),
                attempted_before,
                limit: self.media.config.cleanup_batch,
            })?
            .media)
    }
    #[cfg(test)]
    pub(crate) fn media_staging_cleanup(
        &self,
        attempted_before: u64,
    ) -> Result<Vec<String>, EngineError> {
        let at = now();
        Ok(self
            .store
            .db
            .media_write_blocking(&database::MediaWrite::ClaimStaging {
                at,
                pending_before: at.saturating_sub(self.media.config.pending_ttl),
                attempted_before,
                limit: self.media.config.cleanup_batch,
            })?
            .staging)
    }
    #[cfg(test)]
    pub(crate) fn remove_media_record(&self, id: &str) -> Result<(), EngineError> {
        self.store
            .db
            .media_write_blocking(&database::MediaWrite::RemoveRecord {
                id,
                tombstones_before: now().saturating_sub(self.media.config.pending_ttl),
                staging_clean: false,
            })?;
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
            let mut query = db.prepare(format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
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
