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
    Ready {
        media: &'a Media,
        at: u64,
    },
    PrepareCleanup,
    Abandon {
        id: &'a str,
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
    StagingCleaned {
        id: &'a str,
    },
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
}
impl super::Handle {
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
