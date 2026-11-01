//! Account sticker libraries. A sticker object is uploaded once and shared by
//! reference: library entries and message attachments each keep it alive, and
//! cleanup removes it once neither remains. Quota stays with the uploader.
use super::*;
use authorization::Scope;
use database::StickerReach;
use media::{Media, MediaAccess};

pub(crate) const STICKERS: &str = "@sticker";
const TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

fn filename(content_type: &str) -> &'static str {
    match content_type {
        "image/gif" => "sticker.gif",
        "image/png" => "sticker.png",
        "image/jpeg" => "sticker.jpg",
        _ => "sticker.webp",
    }
}

/// Classification comes from verified bytes, never from the client.
pub(crate) fn check_type(media: &mut Media) -> Result<(), EngineError> {
    if !TYPES.contains(&media.content_type.as_str()) {
        return Err("Stickers must be PNG, JPEG, GIF or WebP images.".into());
    }
    media.filename = filename(&media.content_type).into();
    Ok(())
}

pub(crate) struct PendingSticker {
    store: MediaStore,
    media: Media,
    quota: u64,
}

impl Engine {
    fn media_quota(&self, user: &str) -> u64 {
        self.metadata_value(user, &Scope::Server, "media.user_quota")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(self.media.config.user_quota)
            .min(self.media.config.user_quota)
    }
    pub(crate) fn prepare_sticker_allocation(
        &self,
        user: &str,
        size: u64,
    ) -> Result<PendingSticker, EngineError> {
        let uploader = self
            .principal_id(Some(user))
            .ok_or_else(|| EngineError::Forbidden("Account unavailable.".into()))?;
        if size == 0 || size > self.media.config.sticker_size_limit() {
            return Err(EngineError::Invalid(format!(
                "Stickers must be between 1 byte and {} bytes.",
                self.media.config.sticker_size_limit()
            )));
        }
        Ok(PendingSticker {
            store: self.media_store(),
            quota: self.media_quota(user),
            media: Media {
                id: uuid::Uuid::new_v4().to_string(),
                uploader: uploader.into(),
                conversation: STICKERS.into(),
                filename: "sticker".into(),
                hints: BTreeMap::new(),
                size,
                content_type: "application/octet-stream".into(),
                object_key: String::new(),
                ready: false,
                created: now(),
                max_age: -1,
                expires: None,
                deleting: false,
                deleted: false,
                content_digest: None,
            },
        })
    }
    /// Unfinished uploads must complete within the pending period.
    pub(crate) fn authorize_sticker_upload(
        &self,
        user: &str,
        media: &Media,
    ) -> Result<(), EngineError> {
        if !self.active(user)
            || self.principal_id(Some(user)) != Some(media.uploader.as_str())
            || media.conversation != STICKERS
            || media.deleting
            || media.deleted
            || (!media.ready
                && now().saturating_sub(media.created) >= self.media.config.pending_ttl)
        {
            return Err(EngineError::NotFound("Sticker not found.".into()));
        }
        Ok(())
    }
    /// A sticker is readable from its owner's library or from any readable
    /// conversation whose messages reference it.
    pub(crate) fn sticker_readable(&self, user: &str, reach: &StickerReach) -> bool {
        self.active(user)
            && (reach.owned
                || reach
                    .conversations
                    .iter()
                    .any(|id| self.media_scope(user, id, MediaAccess::Read).is_ok()))
    }
    pub(crate) fn authorize_sticker_collect(
        &self,
        user: &str,
        media: &Media,
        reach: &StickerReach,
    ) -> Result<(), EngineError> {
        if media.conversation != STICKERS
            || !media.ready
            || media.deleting
            || media.deleted
            || !self.sticker_readable(user, reach)
        {
            return Err(EngineError::NotFound("Sticker not found.".into()));
        }
        Ok(())
    }
}

impl PendingSticker {
    pub(crate) fn media(&self) -> &Media {
        &self.media
    }
    pub(crate) async fn save(&self) -> Result<(), EngineError> {
        self.store
            .database
            .media_write(&database::MediaWrite::StickerAllocate {
                allocation: database::MediaAllocation {
                    media: &self.media,
                    quota: self.quota,
                    pending_limit: self.store.config.pending_limit,
                    rate_limit: self.store.config.allocations_per_minute,
                    at: now(),
                    private: None,
                },
                limit: self.store.config.sticker_limit,
            })
            .await?;
        Ok(())
    }
}

impl MediaStore {
    pub(crate) async fn stickers(&self, owner: &str) -> Result<Vec<Media>, EngineError> {
        Ok(self.database.stickers(owner).await?)
    }
    pub(crate) async fn sticker_digest_taken(
        &self,
        owner: &str,
        digest: &str,
    ) -> Result<bool, EngineError> {
        Ok(self.database.sticker_digest_taken(owner, digest).await?)
    }
    pub(crate) async fn sticker_reach(
        &self,
        id: &str,
        owner: &str,
    ) -> Result<StickerReach, EngineError> {
        Ok(self.database.sticker_reach(id, owner).await?)
    }
    pub(crate) async fn delete_sticker(&self, id: &str, owner: &str) -> Result<(), EngineError> {
        let result = self
            .database
            .media_write(&database::MediaWrite::DeleteSticker {
                id,
                owner,
                at: now(),
            })
            .await?;
        if result.affected == 0 {
            return Err(EngineError::NotFound("Sticker not found.".into()));
        }
        Ok(())
    }
    pub(crate) async fn collect_sticker(&self, id: &str, owner: &str) -> Result<(), EngineError> {
        self.database
            .media_write(&database::MediaWrite::CollectSticker {
                id,
                owner,
                at: now(),
                limit: self.config.sticker_limit,
            })
            .await?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    fn run<T: Send>(future: impl std::future::Future<Output = Result<T, EngineError>> + Send) -> T {
        database::block_on_storage(future).unwrap().unwrap()
    }
    fn sticker(e: &Engine, user: &str, content_type: &str) -> Media {
        let pending = e.prepare_sticker_allocation(user, 64).unwrap();
        run(pending.save());
        let mut media = pending.media().clone();
        media.ready = true;
        media.object_key = media.final_key();
        media.content_type = content_type.into();
        check_type(&mut media).unwrap();
        run(e.media_store().ready(&media));
        media
    }
    fn account(e: &Engine, user: &str) -> String {
        e.active_account_id(user).unwrap().to_owned()
    }
    fn readable(e: &Engine, user: &str, id: &str) -> bool {
        let store = e.media_store();
        let (media, _) = run(store.load(id));
        let reach = run(store.sticker_reach(id, &account(e, user)));
        let attached = e.sticker_readable(user, &reach);
        e.authorize_media_read(user, &media, attached, false)
            .is_ok()
    }
    fn send(e: &mut Engine, user: &str, room: &str, id: &str) -> Result<String, EngineError> {
        e.execute(
            Some(user),
            Some(room),
            &format!("![sticker](/api/media/{id}/file \"media:sticker\")"),
        )
    }
    fn claimable(e: &Engine, id: &str) -> bool {
        e.prepare_media_cleanup().unwrap();
        e.expired_media()
            .unwrap()
            .iter()
            .any(|media| media.id == id)
    }

    #[test]
    fn library_entries_are_private_until_sent_and_cleaned_after_deletion() {
        let e = super::super::tests::engine();
        let alice = account(&e, "alice");
        let media = sticker(&e, "alice", "image/gif");
        assert!(media.object_key.starts_with(&format!("stickers/{alice}/")));
        assert_eq!(media.filename, "sticker.gif");
        let store = e.media_store();
        let listed = run(store.stickers(&alice));
        assert_eq!(listed.len(), 1);
        assert!(listed[0].expires.is_none());
        assert!(readable(&e, "alice", &media.id));
        assert!(!readable(&e, "bob", &media.id));
        assert!(!claimable(&e, &media.id));
        let bob = account(&e, "bob");
        assert!(
            database::block_on_storage(store.delete_sticker(&media.id, &bob))
                .unwrap()
                .is_err()
        );
        run(store.delete_sticker(&media.id, &alice));
        assert!(run(store.stickers(&alice)).is_empty());
        assert!(!readable(&e, "alice", &media.id));
        assert!(claimable(&e, &media.id));
    }

    #[test]
    fn duplicate_content_digest_is_rejected() {
        let e = super::super::tests::engine();
        let digest = "a".repeat(64);
        let first = e.prepare_sticker_allocation("alice", 64).unwrap();
        run(first.save());
        let mut media = first.media().clone();
        media.ready = true;
        media.object_key = media.final_key();
        media.content_type = "image/png".into();
        media.content_digest = Some(digest.clone());
        check_type(&mut media).unwrap();
        run(e.media_store().ready(&media));
        let pending = e.prepare_sticker_allocation("alice", 64).unwrap();
        run(pending.save());
        let mut duplicate = pending.media().clone();
        duplicate.ready = true;
        duplicate.object_key = duplicate.final_key();
        duplicate.content_type = "image/png".into();
        duplicate.content_digest = Some(digest);
        check_type(&mut duplicate).unwrap();
        let err = database::block_on_storage(e.media_store().ready(&duplicate))
            .unwrap()
            .unwrap_err();
        assert!(matches!(err, EngineError::Conflict(_)));
        assert_eq!(run(e.media_store().stickers(&account(&e, "alice"))).len(), 1);
    }

    #[test]
    fn size_type_and_count_limits_are_enforced() {
        let mut e = super::super::tests::engine();
        assert!(e.prepare_sticker_allocation("alice", 0).is_err());
        let max = e.media.config.sticker_size_limit();
        assert!(e.prepare_sticker_allocation("alice", max + 1).is_err());
        let mut svg = e
            .prepare_sticker_allocation("alice", 8)
            .unwrap()
            .media()
            .clone();
        svg.content_type = "image/svg+xml".into();
        assert!(check_type(&mut svg).is_err());
        e.media.config.sticker_limit = 1;
        let first = sticker(&e, "alice", "image/webp");
        let pending = e.prepare_sticker_allocation("alice", 8).unwrap();
        assert!(database::block_on_storage(pending.save()).unwrap().is_err());
        let store = e.media_store();
        let bob = account(&e, "bob");
        let own = sticker(&e, "bob", "image/png");
        assert!(
            database::block_on_storage(store.collect_sticker(&first.id, &bob))
                .unwrap()
                .is_err()
        );
        run(store.delete_sticker(&own.id, &bob));
        run(store.collect_sticker(&first.id, &bob));
        run(store.collect_sticker(&first.id, &bob));
        assert_eq!(run(store.stickers(&bob)).len(), 1);
    }

    #[test]
    fn sending_references_the_library_object_and_keeps_it_alive() {
        let mut e = super::super::tests::engine();
        let alice = account(&e, "alice");
        let media = sticker(&e, "alice", "image/webp");
        assert!(send(&mut e, "bob", "lobby", &media.id).is_err());
        send(&mut e, "alice", "lobby", &media.id).unwrap();
        assert!(readable(&e, "bob", &media.id));
        // Anyone may repeat a sticker already shown in the conversation.
        send(&mut e, "bob", "lobby", &media.id).unwrap();
        run(e.media_store().delete_sticker(&media.id, &alice));
        assert!(readable(&e, "bob", &media.id));
        assert!(!claimable(&e, &media.id));
        e.execute(Some("alice"), None, "/new room hidden").unwrap();
        assert!(send(&mut e, "alice", "hidden", &media.id).is_err());
        e.execute(Some("alice"), None, "/room delete #lobby")
            .unwrap();
        assert!(!readable(&e, "bob", &media.id));
        assert!(claimable(&e, &media.id));
    }

    #[test]
    fn sending_needs_upload_permission() {
        let mut e = super::super::tests::engine();
        let media = sticker(&e, "bob", "image/png");
        e.execute(None, None, "/revoke bob #lobby w:media.upload")
            .unwrap();
        assert!(send(&mut e, "bob", "lobby", &media.id).is_err());
    }

    #[test]
    fn collecting_adds_a_reference_that_outlives_the_uploader_entry() {
        let mut e = super::super::tests::engine();
        let alice = account(&e, "alice");
        let bob = account(&e, "bob");
        let store = e.media_store();
        let mut plain = e
            .allocate_media("alice", &e.data.rooms["lobby"].id, "photo.png", 8)
            .unwrap();
        plain.ready = true;
        plain.object_key = plain.final_key();
        plain.content_type = "image/png".into();
        e.finish_media("alice", &plain).unwrap();
        let reach = run(store.sticker_reach(&plain.id, &alice));
        assert!(
            e.authorize_sticker_collect("alice", &plain, &reach)
                .is_err()
        );
        let media = sticker(&e, "alice", "image/gif");
        let (current, _) = run(store.load(&media.id));
        let reach = run(store.sticker_reach(&media.id, &bob));
        assert!(
            e.authorize_sticker_collect("bob", &current, &reach)
                .is_err()
        );
        send(&mut e, "alice", "lobby", &media.id).unwrap();
        let reach = run(store.sticker_reach(&media.id, &bob));
        e.authorize_sticker_collect("bob", &current, &reach)
            .unwrap();
        run(store.collect_sticker(&media.id, &bob));
        assert_eq!(run(store.stickers(&bob))[0].id, media.id);
        run(store.delete_sticker(&media.id, &alice));
        e.execute(Some("alice"), None, "/room delete #lobby")
            .unwrap();
        assert!(readable(&e, "bob", &media.id));
        assert!(!claimable(&e, &media.id));
        e.execute(Some("alice"), None, "/new room kept").unwrap();
        e.execute(Some("alice"), None, "/add bob #kept").unwrap();
        send(&mut e, "bob", "kept", &media.id).unwrap();
        e.execute(None, None, "/user delete bob").unwrap();
        assert!(run(store.stickers(&bob)).is_empty());
        assert!(!claimable(&e, &media.id));
        e.execute(Some("alice"), None, "/room delete #kept")
            .unwrap();
        assert!(claimable(&e, &media.id));
    }
}
