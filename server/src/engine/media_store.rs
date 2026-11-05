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
    pub(crate) async fn read_batch(
        &self,
        ids: &[String],
        account: &str,
        scopes: &[String],
    ) -> Result<Vec<database::MediaRead>, EngineError> {
        Ok(self.database.media_reads(ids, account, scopes).await?)
    }
    pub(crate) async fn library_source(
        &self,
        id: &str,
        conversations: &[String],
    ) -> Result<Option<String>, EngineError> {
        Ok(self.database.library_source(id, conversations).await?)
    }
    pub(crate) async fn library_page(
        &self,
        request: &database::LibraryList<'_>,
    ) -> Result<database::LibraryPage, EngineError> {
        Ok(self.database.library_page(request, &self.config).await?)
    }
    pub(crate) async fn library_action(
        &self,
        request: &database::LibraryWrite<'_>,
    ) -> Result<bool, EngineError> {
        let result = self
            .database
            .media_write(&MediaWrite::Library(*request))
            .await?;
        Ok(result.affected > 0)
    }
    pub(crate) async fn health(&self) -> Result<(), EngineError> {
        Ok(self.database.ensure_available_async().await?)
    }
    pub(crate) async fn load(&self, id: &str) -> Result<(Media, bool), EngineError> {
        Ok(self.database.stored_media(id).await?)
    }
    pub(crate) async fn ready(&self, media: &Media) -> Result<(), EngineError> {
        if media.conversation == "@library"
            && !matches!(
                media.content_type.as_str(),
                "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/avif"
            )
        {
            return Err("Library assets must be raster images or GIFs.".into());
        }
        let at = now();
        let media = media.completed_record(at)?;
        self.database
            .media_write(&MediaWrite::Ready { media: &media, at })
            .await?;
        Ok(())
    }
    pub(crate) async fn abandon(&self, id: &str, uploader: &str) -> Result<(), EngineError> {
        let result = self
            .database
            .media_write(&MediaWrite::Abandon { id, uploader })
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
    pub(crate) async fn cleanup_done(
        &self,
        records: &[Media],
        staging: &[String],
    ) -> Result<(), EngineError> {
        if records.is_empty() && staging.is_empty() {
            return Ok(());
        }
        let at = now();
        let records: Vec<_> = records
            .iter()
            .map(|media| {
                (
                    media.id.clone(),
                    media.created.saturating_add(self.config.pending_ttl) <= at,
                )
            })
            .collect();
        self.database
            .media_write(&MediaWrite::CleanupDone {
                records: &records,
                staging,
                tombstones_before: at.saturating_sub(self.config.pending_ttl),
            })
            .await?;
        Ok(())
    }
}
