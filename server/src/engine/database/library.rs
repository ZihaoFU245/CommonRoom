//! Typed library operations. Native transactions own membership, quota and retirement.
use super::{Backend, Error, Result};
use serde::{Deserialize, Serialize};
pub(super) struct LibraryRow {
    pub(super) cursor: String,
    json: String,
    ready: i64,
    expires: Option<i64>,
    deleting: i64,
    deleted: i64,
    owned: bool,
    saved: bool,
    owners: i64,
}

macro_rules! library_row {
    ($row:ty) => {
        impl<'r> sqlx::FromRow<'r, $row> for LibraryRow {
            fn from_row(row: &'r $row) -> std::result::Result<Self, sqlx::Error> {
                use sqlx::Row as _;
                Ok(Self {
                    cursor: row.try_get("cursor")?,
                    json: row.try_get("json")?,
                    ready: row.try_get("ready")?,
                    expires: row.try_get("expires")?,
                    deleting: row.try_get("deleting")?,
                    deleted: row.try_get("deleted")?,
                    owned: row.try_get("owned")?,
                    saved: row.try_get("saved")?,
                    owners: row.try_get("owners")?,
                })
            }
        }
    };
}
library_row!(sqlx::sqlite::SqliteRow);
library_row!(sqlx::postgres::PgRow);

impl LibraryRow {
    pub(super) fn into_item(self) -> Result<LibraryItem> {
        let media = super::media::decode((
            self.json,
            self.ready,
            self.expires,
            self.deleting,
            self.deleted,
        ))?;
        let available = media.available_at(crate::engine::media::timestamp());
        Ok(LibraryItem {
            media_id: media.id,
            filename: media.filename,
            content_type: media.content_type,
            size: media.size,
            available,
            owned: self.owned && available,
            saved: self.saved,
            owner_count: if available {
                super::history::unsigned(self.owners)?
            } else {
                0
            },
        })
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LibraryAction {
    Save,
    Acquire,
    Remove,
    Release,
    Delete,
}
#[derive(Clone, Copy)]
pub(crate) struct LibraryWrite<'a> {
    pub(crate) account: &'a str,
    pub(crate) id: &'a str,
    pub(crate) source: Option<&'a str>,
    pub(crate) action: LibraryAction,
    pub(crate) confirm_last: bool,
    pub(crate) quota: u64,
    pub(crate) config: &'a crate::config::MediaConfig,
}
pub(crate) struct LibraryList<'a> {
    pub(crate) account: &'a str,
    pub(crate) owned: bool,
    pub(crate) recent: bool,
    pub(crate) after: &'a str,
    pub(crate) limit: u64,
    pub(crate) quota: u64,
    pub(crate) conversations: &'a [String],
}
#[derive(Serialize)]
pub(crate) struct LibraryItem {
    pub(crate) media_id: String,
    pub(crate) filename: String,
    pub(crate) content_type: String,
    pub(crate) size: u64,
    pub(crate) available: bool,
    pub(crate) owned: bool,
    pub(crate) saved: bool,
    pub(crate) owner_count: u64,
}
#[derive(Serialize)]
pub(crate) struct LibraryPage {
    pub(crate) items: Vec<LibraryItem>,
    pub(crate) next_cursor: Option<String>,
    pub(crate) usage: u64,
    pub(crate) quota: u64,
    pub(crate) cleanup_bytes: u64,
    pub(crate) cleanup_objects: u64,
    pub(crate) oldest_cleanup: Option<u64>,
    pub(crate) uploads_paused: bool,
}
impl super::Handle {
    pub(crate) async fn library_source(
        &self,
        id: &str,
        conversations: &[String],
    ) -> Result<Option<String>> {
        let mut session = self.acquire_read().await?;
        let result = match session.backend.get_mut() {
            Backend::Sqlite(db) => super::sqlite::library::source(db, id, conversations).await,
            Backend::Postgres(db) => super::postgres::library::source(db, id, conversations).await,
            Backend::Closed => Err(Error::RestartRequired),
        };
        session.checked(result)
    }
    pub(crate) async fn library_access(
        &self,
        id: &str,
        account: &str,
        conversations: &[String],
    ) -> Result<bool> {
        let mut session = self.acquire_read().await?;
        let result = match session.backend.get_mut() {
            Backend::Sqlite(db) => {
                super::sqlite::library::access(db, id, account, conversations).await
            }
            Backend::Postgres(db) => {
                super::postgres::library::access(db, id, account, conversations).await
            }
            Backend::Closed => Err(Error::RestartRequired),
        };
        session.checked(result)
    }
    pub(crate) async fn library_page(
        &self,
        request: &LibraryList<'_>,
        config: &crate::config::MediaConfig,
    ) -> Result<LibraryPage> {
        let mut session = self.acquire_read().await?;
        let result = match session.backend.get_mut() {
            Backend::Sqlite(db) => super::sqlite::library::page(db, request, config).await,
            Backend::Postgres(db) => super::postgres::library::page(db, request, config).await,
            Backend::Closed => Err(Error::RestartRequired),
        };
        session.checked(result)
    }
}
