//! Owned media storage context. Authorization uses memory; these operations await
//! the typed dispatcher without borrowing an engine guard.
use super::*;
use database::MediaWrite;
use media::Media;

#[derive(Clone)]
pub(crate) struct MediaStore {
    pub(super) database: database::Handle,
    pub(super) config: crate::config::MediaConfig,
}
impl Engine {
    pub(crate) fn media_store(&self) -> MediaStore {
        MediaStore {
            database: self.store.db.clone(),
            config: self.media.config.clone(),
        }
    }
}
impl MediaStore {
    pub(crate) async fn health(&self) -> Result<(), EngineError> {
        Ok(self.database.ensure_available_async().await?)
    }
    pub(crate) async fn load(&self, id: &str) -> Result<(Media, bool), EngineError> {
        Ok(self.database.stored_media(id).await?)
    }
    pub(crate) async fn ready(&self, media: &Media) -> Result<(), EngineError> {
        let at = now();
        let media = media.completed_record(at)?;
        self.database
            .media_write(&MediaWrite::Ready { media: &media, at })
            .await?;
        Ok(())
    }
    pub(crate) async fn abandon(&self, id: &str) -> Result<(), EngineError> {
        let result = self
            .database
            .media_write(&MediaWrite::Abandon { id })
            .await?;
        if result.affected == 0 {
            tracing::debug!(
                media_id = id,
                "Abandon ignored for a published or deleted upload"
            );
        }
        Ok(())
    }
    pub(crate) async fn expired(&self, attempted_before: u64) -> Result<Vec<Media>, EngineError> {
        let at = now();
        Ok(self
            .database
            .media_write(&MediaWrite::ClaimCleanup {
                at,
                pending_before: at.saturating_sub(self.config.pending_ttl),
                attempted_before,
                limit: self.config.cleanup_batch,
            })
            .await?
            .media)
    }
    pub(crate) async fn staging(&self, attempted_before: u64) -> Result<Vec<String>, EngineError> {
        let at = now();
        Ok(self
            .database
            .media_write(&MediaWrite::ClaimStaging {
                at,
                pending_before: at.saturating_sub(self.config.pending_ttl),
                attempted_before,
                limit: self.config.cleanup_batch,
            })
            .await?
            .staging)
    }
    pub(crate) async fn staging_cleaned(&self, id: &str) -> Result<(), EngineError> {
        self.database
            .media_write(&MediaWrite::StagingCleaned { id })
            .await?;
        Ok(())
    }
    pub(crate) async fn cleanup_done(&self, media: &Media) -> Result<(), EngineError> {
        let at = now();
        self.database
            .media_write(&MediaWrite::RemoveRecord {
                id: &media.id,
                tombstones_before: at.saturating_sub(self.config.pending_ttl),
                staging_clean: media.created.saturating_add(self.config.pending_ttl) <= at,
            })
            .await?;
        Ok(())
    }
}
