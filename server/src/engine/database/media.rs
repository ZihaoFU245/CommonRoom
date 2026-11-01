//! Typed upload lifecycle requests and authoritative mutable column decoding.
use super::{Backend, Error, Result, metadata::WriteResult};
use crate::engine::media::Media;

pub(crate) struct MediaAllocation<'a> {
    pub(crate) media: &'a Media,
    pub(crate) quota: u64,
    pub(crate) pending_limit: u64,
    pub(crate) rate_limit: u64,
    pub(crate) at: u64,
    pub(crate) private: Option<(&'a str, String)>,
}
pub(crate) enum MediaWrite<'a> {
    Allocate(MediaAllocation<'a>),
    AvatarAllocate {
        allocation: MediaAllocation<'a>,
        pending_ttl: u64,
    },
    /// Counts library entries and pending sticker uploads under the uploader lock.
    StickerAllocate {
        allocation: MediaAllocation<'a>,
        limit: u64,
    },
    /// Removes one library entry; the object remains while anything references it.
    DeleteSticker {
        id: &'a str,
        owner: &'a str,
        at: u64,
    },
    /// Adds a library entry for an existing sticker object; repeats are no-ops.
    CollectSticker {
        id: &'a str,
        owner: &'a str,
        at: u64,
        limit: u64,
    },
    Ready {
        media: &'a Media,
        at: u64,
    },
    PrepareCleanup {
        pending_before: u64,
    },
    Abandon {
        id: &'a str,
        uploader: &'a str,
    },
    ClaimCleanup {
        at: u64,
        pending_before: u64,
        attempted_before: u64,
        limit: usize,
    },
    ClaimStaging {
        at: u64,
        pending_before: u64,
        attempted_before: u64,
        limit: usize,
    },
    CleanupDone {
        records: &'a [(String, bool)],
        staging: &'a [String],
        tombstones_before: u64,
    },
    #[cfg(test)]
    RemoveRecord {
        id: &'a str,
        tombstones_before: u64,
        staging_clean: bool,
    },
}
pub(crate) struct MediaCommit {
    pub(crate) affected: u64,
    pub(crate) media: Vec<Media>,
    pub(crate) staging: Vec<String>,
}
pub(super) fn avatar_replay(
    media: Media,
    request: &MediaAllocation<'_>,
    pending_ttl: u64,
) -> Result<Media> {
    if media.uploader != request.media.uploader
        || media.conversation != "@avatar"
        || media.deleting
        || media.deleted
        || request.at.saturating_sub(media.created) >= pending_ttl
    {
        return Err(Error::InvalidRequest("Avatar upload unavailable."));
    }
    if media.size != request.media.size || media.ready {
        return Err(Error::InvalidRequest("Avatar upload changed."));
    }
    Ok(media)
}
pub(super) type MediaColumns = (String, i64, Option<i64>, i64, i64);
pub(super) fn decode(columns: MediaColumns) -> Result<Media> {
    let (json, ready, expires, deleting, deleted) = columns;
    let mut media: Media =
        serde_json::from_str(&json).map_err(|_| Error::Conversion("Invalid stored media JSON"))?;
    media.ready = flag(ready)?;
    media.expires = expires.map(super::history::unsigned).transpose()?;
    media.deleting = flag(deleting)?;
    media.deleted = flag(deleted)?;
    Ok(media)
}
fn flag(value: i64) -> Result<bool> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(Error::Conversion("Invalid stored media flag")),
    }
}
impl super::Connection {
    pub(super) async fn stored_media_async(&mut self, id: &str) -> Result<Option<(Media, bool)>> {
        self.ensure_available()?;
        let result = match self.backend.get_mut() {
            Backend::Sqlite(db) => super::sqlite::media::load(db, id).await,
            Backend::Postgres(db) => super::postgres::media::load(db, id).await,
            Backend::Closed => Err(Error::Conversion("Database closed")),
        };
        self.checked(result)
    }
    async fn stickers_async(&mut self, owner: &str) -> Result<Vec<Media>> {
        self.ensure_available()?;
        let result = match self.backend.get_mut() {
            Backend::Sqlite(db) => super::sqlite::media::stickers(db, owner).await,
            Backend::Postgres(db) => super::postgres::media::stickers(db, owner).await,
            Backend::Closed => Err(Error::Conversion("Database closed")),
        };
        self.checked(result)
    }
    async fn sticker_reach_async(&mut self, id: &str, owner: &str) -> Result<StickerReach> {
        self.ensure_available()?;
        let result = match self.backend.get_mut() {
            Backend::Sqlite(db) => super::sqlite::media::sticker_reach(db, id, owner).await,
            Backend::Postgres(db) => super::postgres::media::sticker_reach(db, id, owner).await,
            Backend::Closed => Err(Error::Conversion("Database closed")),
        };
        self.checked(result)
    }
    async fn sticker_digest_taken_async(&mut self, owner: &str, digest: &str) -> Result<bool> {
        self.ensure_available()?;
        let result = match self.backend.get_mut() {
            Backend::Sqlite(db) => {
                super::sqlite::media::sticker_digest_taken(db, owner, digest).await
            }
            Backend::Postgres(db) => {
                super::postgres::media::sticker_digest_taken(db, owner, digest).await
            }
            Backend::Closed => Err(Error::Conversion("Database closed")),
        };
        self.checked(result)
    }
}
/// Paths through which one account may reach a sticker object.
#[derive(Default)]
pub(crate) struct StickerReach {
    pub(crate) owned: bool,
    /// Conversations with a message that references the sticker.
    pub(crate) conversations: Vec<String>,
}
impl super::Handle {
    pub(crate) async fn stickers(&self, owner: &str) -> Result<Vec<Media>> {
        let mut session = self.acquire_read().await?;
        session.stickers_async(owner).await
    }
    pub(crate) async fn sticker_reach(&self, id: &str, owner: &str) -> Result<StickerReach> {
        let mut session = self.acquire_read().await?;
        session.sticker_reach_async(id, owner).await
    }
    pub(crate) async fn sticker_digest_taken(&self, owner: &str, digest: &str) -> Result<bool> {
        let mut session = self.acquire_read().await?;
        session.sticker_digest_taken_async(owner, digest).await
    }
    #[cfg(test)]
    pub(crate) fn media_write_blocking(&self, request: &MediaWrite<'_>) -> Result<MediaCommit> {
        super::runtime::run(self.media_write(request))
    }
    pub(crate) async fn media_write(&self, request: &MediaWrite<'_>) -> Result<MediaCommit> {
        let mut session = self.acquire().await?;
        let WriteResult {
            changed,
            media,
            staging,
            ..
        } = session
            .save_write_async(&super::transaction::WriteRequest::Media(request), self)
            .await?;
        Ok(MediaCommit {
            affected: changed,
            media,
            staging,
        })
    }
    #[cfg(test)]
    pub(crate) fn stored_media_blocking(&self, id: &str) -> Result<(Media, bool)> {
        super::runtime::run(self.stored_media(id))
    }
    pub(crate) async fn stored_media(&self, id: &str) -> Result<(Media, bool)> {
        let mut session = self.acquire_read().await?;
        session
            .stored_media_async(id)
            .await?
            .ok_or(Error::MissingMedia)
    }
}
