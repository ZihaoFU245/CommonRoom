//! Owned replay-safe avatar allocation; SQL and retry-slot locks stay in backends.
use super::*;
use media::Media;

pub(crate) struct PendingAvatar {
    store: MediaStore,
    media: Media,
    quota: u64,
}
impl Engine {
    pub(crate) fn prepare_avatar_allocation(
        &self,
        user: &str,
        size: u64,
        id: String,
    ) -> Result<PendingAvatar, EngineError> {
        let uploader = self
            .principal_id(Some(user))
            .ok_or_else(|| EngineError::Forbidden("Account unavailable.".into()))?;
        if !uuid::Uuid::parse_str(&id).is_ok_and(|uuid| uuid.to_string() == id) {
            return Err("Invalid avatar upload ID.".into());
        }
        if size == 0 || size > avatars::MAX_AVATAR_SIZE {
            return Err("Avatar must be between 1 byte and 64 KiB.".into());
        }
        let quota = self
            .metadata_value(user, &authorization::Scope::Server, "media.user_quota")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(self.media.config.user_quota)
            .min(self.media.config.user_quota);
        Ok(PendingAvatar {
            store: self.media_store(),
            quota,
            media: Media {
                id,
                uploader: uploader.into(),
                conversation: "@avatar".into(),
                filename: "avatar.png".into(),
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
            },
        })
    }
}
impl PendingAvatar {
    pub(crate) async fn save(&self) -> Result<Media, EngineError> {
        let result = self
            .store
            .database
            .media_write(&database::MediaWrite::AvatarAllocate {
                allocation: database::MediaAllocation {
                    media: &self.media,
                    quota: self.quota,
                    pending_limit: self.store.config.pending_limit,
                    rate_limit: self.store.config.allocations_per_minute,
                    at: now(),
                    private: None,
                },
                pending_ttl: self.store.config.pending_ttl,
            })
            .await?;
        result
            .media
            .into_iter()
            .next()
            .ok_or_else(|| EngineError::Internal("Missing avatar reservation result.".into()))
    }
}
