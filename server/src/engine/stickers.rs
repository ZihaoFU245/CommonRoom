//! Account-owned sticker library. Library objects are permanent and readable only
//! by their owner. Sending copies one into a conversation as an ordinary upload,
//! so retention, quota and read access follow that conversation's media rules.
use super::media_allocation::PendingMedia;
use super::*;
use authorization::Scope;
use media::Media;

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
    /// `source` names the published conversation copy a sticker is collected from.
    pub(crate) fn prepare_sticker_allocation(
        &self,
        user: &str,
        size: u64,
        source: Option<String>,
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
                source,
            },
        })
    }
    /// Unfinished entries must complete within the pending period; using an
    /// entry requires a verified one.
    pub(crate) fn authorize_sticker_owner(
        &self,
        user: &str,
        media: &Media,
        completing: bool,
    ) -> Result<(), EngineError> {
        if !self.active(user)
            || self.principal_id(Some(user)) != Some(media.uploader.as_str())
            || media.conversation != STICKERS
            || media.deleting
            || media.deleted
            || (completing
                && !media.ready
                && now().saturating_sub(media.created) >= self.media.config.pending_ttl)
            || (!completing && !media.ready)
        {
            return Err(EngineError::NotFound("Sticker not found.".into()));
        }
        Ok(())
    }
    /// The copy is an ordinary conversation upload and needs upload permission there.
    pub(crate) fn prepare_sticker_send(
        &self,
        user: &str,
        sticker: &Media,
        target: &str,
    ) -> Result<PendingMedia, EngineError> {
        self.authorize_sticker_owner(user, sticker, false)?;
        let mut pending = self.prepare_media_allocation(
            user,
            target,
            filename(&sticker.content_type),
            sticker.size,
            BTreeMap::new(),
        )?;
        pending.media.source = Some(sticker.id.clone());
        Ok(pending)
    }
    /// Only readable conversation copies of stickers can be collected.
    pub(crate) fn authorize_sticker_collect(
        &self,
        user: &str,
        media: &Media,
        attached: bool,
    ) -> Result<(), EngineError> {
        self.authorize_media_read(user, media, attached, false)?;
        if media.source.is_none()
            || !TYPES.contains(&media.content_type.as_str())
            || media.size > self.media.config.sticker_size_limit()
        {
            return Err("Only stickers can be added to your stickers.".into());
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
    pub(crate) async fn stickers(&self, uploader: &str) -> Result<Vec<Media>, EngineError> {
        Ok(self.database.stickers(uploader).await?)
    }
    pub(crate) async fn sticker_copy(
        &self,
        conversation: &str,
        uploader: &str,
        source: &str,
    ) -> Result<Option<Media>, EngineError> {
        let at = now();
        Ok(self
            .database
            .sticker_copy(&database::StickerCopy {
                conversation,
                uploader,
                source,
                at,
                pending_after: at.saturating_sub(self.config.pending_ttl / 2),
            })
            .await?)
    }
    pub(crate) async fn delete_sticker(&self, id: &str, uploader: &str) -> Result<(), EngineError> {
        let result = self
            .database
            .media_write(&database::MediaWrite::DeleteSticker { id, uploader })
            .await?;
        if result.affected == 0 {
            return Err(EngineError::NotFound("Sticker not found.".into()));
        }
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
        let pending = e.prepare_sticker_allocation(user, 64, None).unwrap();
        run(pending.save());
        let mut media = pending.media().clone();
        media.ready = true;
        media.object_key = media.final_key();
        media.content_type = content_type.into();
        check_type(&mut media).unwrap();
        run(e.media_store().ready(&media));
        media
    }
    fn send(e: &mut Engine, user: &str, sticker: &Media, target: &str) -> Media {
        let pending = e.prepare_sticker_send(user, sticker, target).unwrap();
        run(pending.save());
        let mut media = e.publish_media_allocation(pending).unwrap();
        media.ready = true;
        media.object_key = media.final_key();
        media.content_type.clone_from(&sticker.content_type);
        run(e.media_store().ready(&media));
        media
    }

    #[test]
    fn library_entries_are_owned_permanent_and_survive_cleanup_until_deleted() {
        let e = super::super::tests::engine();
        let alice = e.active_account_id("alice").unwrap().to_owned();
        let media = sticker(&e, "alice", "image/gif");
        assert!(media.object_key.starts_with(&format!("stickers/{alice}/")));
        assert_eq!(media.filename, "sticker.gif");
        let store = e.media_store();
        let listed = run(store.stickers(&alice));
        assert_eq!(listed.len(), 1);
        assert!(listed[0].expires.is_none());
        e.prepare_media_cleanup().unwrap();
        assert!(e.expired_media().unwrap().is_empty());
        assert!(e.authorize_sticker_owner("bob", &listed[0], false).is_err());
        let bob = e.active_account_id("bob").unwrap().to_owned();
        assert!(
            database::block_on_storage(store.delete_sticker(&media.id, &bob))
                .unwrap()
                .is_err()
        );
        run(store.delete_sticker(&media.id, &alice));
        assert!(run(store.stickers(&alice)).is_empty());
        assert_eq!(e.expired_media().unwrap().len(), 1);
    }

    #[test]
    fn size_type_and_count_limits_are_enforced() {
        let mut e = super::super::tests::engine();
        assert!(e.prepare_sticker_allocation("alice", 0, None).is_err());
        let max = e.media.config.sticker_size_limit();
        assert!(
            e.prepare_sticker_allocation("alice", max + 1, None)
                .is_err()
        );
        let mut svg = e
            .prepare_sticker_allocation("alice", 8, None)
            .unwrap()
            .media()
            .clone();
        svg.content_type = "image/svg+xml".into();
        assert!(check_type(&mut svg).is_err());
        e.media.config.sticker_limit = 1;
        sticker(&e, "alice", "image/webp");
        let pending = e.prepare_sticker_allocation("alice", 8, None).unwrap();
        assert!(database::block_on_storage(pending.save()).unwrap().is_err());
        let pending = e.prepare_sticker_allocation("bob", 8, None).unwrap();
        run(pending.save());
    }

    #[test]
    fn sending_needs_upload_permission_and_reuses_one_live_copy() {
        let mut e = super::super::tests::engine();
        let room = e.data.rooms["lobby"].id.clone();
        let alice = e.active_account_id("alice").unwrap().to_owned();
        let library = sticker(&e, "bob", "image/webp");
        assert!(e.prepare_sticker_send("alice", &library, &room).is_err());
        let library = sticker(&e, "alice", "image/webp");
        let copy = send(&mut e, "alice", &library, &room);
        assert_eq!(copy.source.as_deref(), Some(library.id.as_str()));
        assert!(copy.object_key.starts_with("media/"));
        let store = e.media_store();
        let reused = run(store.sticker_copy(&room, &alice, &library.id)).unwrap();
        assert_eq!(reused.id, copy.id);
        assert!(run(store.sticker_copy(&room, &alice, "other")).is_none());
        e.execute(None, None, "/revoke bob #lobby w:media.upload")
            .unwrap();
        let bob_sticker = sticker(&e, "bob", "image/png");
        assert!(e.prepare_sticker_send("bob", &bob_sticker, &room).is_err());
        run(store.delete_sticker(&library.id, &alice));
        assert!(e.readable_media("alice", &copy.id).is_err());
        e.execute(
            Some("alice"),
            Some("lobby"),
            &format!("![sticker](/api/media/{}/file \"media:sticker\")", copy.id),
        )
        .unwrap();
        assert!(e.readable_media("bob", &copy.id).is_ok());
    }

    #[test]
    fn only_readable_sticker_copies_can_be_collected() {
        let mut e = super::super::tests::engine();
        let room = e.data.rooms["lobby"].id.clone();
        let library = sticker(&e, "alice", "image/gif");
        let copy = send(&mut e, "alice", &library, &room);
        let (current, attached) = run(e.media_store().load(&copy.id));
        assert!(
            e.authorize_sticker_collect("bob", &current, attached)
                .is_err()
        );
        e.execute(
            Some("alice"),
            Some("lobby"),
            &format!("![sticker](/api/media/{}/file \"media:sticker\")", copy.id),
        )
        .unwrap();
        let (current, attached) = run(e.media_store().load(&copy.id));
        e.authorize_sticker_collect("bob", &current, attached)
            .unwrap();
        e.execute(Some("alice"), None, "/new room hidden").unwrap();
        e.execute(Some("alice"), None, "/kick bob #lobby").unwrap();
        assert!(
            e.authorize_sticker_collect("bob", &current, attached)
                .is_err()
        );
        let mut plain = e.allocate_media("alice", &room, "photo.png", 8).unwrap();
        plain.ready = true;
        plain.object_key = plain.final_key();
        plain.content_type = "image/png".into();
        e.finish_media("alice", &plain).unwrap();
        e.execute(
            Some("alice"),
            Some("lobby"),
            &format!("![photo](/api/media/{}/file)", plain.id),
        )
        .unwrap();
        let (plain, attached) = run(e.media_store().load(&plain.id));
        assert!(
            e.authorize_sticker_collect("alice", &plain, attached)
                .is_err()
        );
    }
}
