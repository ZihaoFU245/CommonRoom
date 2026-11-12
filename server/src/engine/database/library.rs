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

pub(super) struct LibraryPageItems {
    pub(super) items: Vec<LibraryItem>,
    pub(super) next_cursor: Option<String>,
}

impl LibraryPageItems {
    pub(super) fn from_rows(rows: Vec<LibraryRow>, limit: u64, recent: bool) -> Result<Self> {
        let limit =
            usize::try_from(limit).map_err(|_| Error::Conversion("Invalid library page size"))?;
        let next_cursor = if !recent && rows.len() > limit {
            rows.get(limit.saturating_sub(1))
                .map(|row| row.cursor.clone())
        } else {
            None
        };
        let items = rows
            .into_iter()
            .take(limit)
            .map(LibraryRow::into_item)
            .collect::<Result<_>>()?;
        Ok(Self { items, next_cursor })
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LibraryAction {
    Save,
    Acquire,
    Remove,
    Release,
    ReleaseAndRemove,
    Delete,
    Move,
}
#[derive(Clone, Copy)]
pub(crate) struct LibraryWrite<'a> {
    pub(crate) account: &'a str,
    pub(crate) id: &'a str,
    pub(crate) source: Option<&'a str>,
    pub(crate) before: Option<&'a str>,
    pub(crate) after: Option<&'a str>,
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
impl LibraryList<'_> {
    pub(super) fn page_bounds(&self, config: &crate::config::MediaConfig) -> Result<(u64, &str)> {
        if !(1..=100).contains(&self.limit) {
            return Err(Error::InvalidRequest("Library page size must be 1..100."));
        }
        if self.recent {
            return Ok((config.library_recent_limit, ""));
        }
        if self.owned {
            return Ok((self.limit, self.after));
        }
        let after = if self.after.is_empty() {
            "0"
        } else {
            self.after
        };
        if !after.parse::<i64>().is_ok_and(|position| position >= 0) {
            return Err(Error::InvalidRequest("Invalid collection cursor."));
        }
        Ok((self.limit, after))
    }
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
    pub(crate) async fn media_usage(&self, account: &str) -> Result<u64> {
        let mut session = self.acquire_read().await?;
        let result = match session.backend.get_mut() {
            Backend::Sqlite(db) => super::sqlite::library::usage(db, account).await,
            Backend::Postgres(db) => super::postgres::library::usage(db, account).await,
            Backend::Closed => Err(Error::RestartRequired),
        };
        session.checked(result)
    }

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
    pub(crate) async fn library_pages(
        &self,
        request: &LibraryList<'_>,
        config: &crate::config::MediaConfig,
        sections: bool,
    ) -> Result<(LibraryPage, Option<LibraryPage>)> {
        use sqlx::Connection as _;

        macro_rules! read_pages {
            ($library:ident, $transaction:expr) => {{
                async {
                    use super::$library::library::{backlog, page_items, usage};
                    let mut tx = $transaction.await?;
                    let now = super::message::number(crate::engine::media::timestamp())?;
                    let first = page_items(&mut tx, request, config, now).await?;
                    let recent = if sections {
                        Some(
                            page_items(
                                &mut tx,
                                &LibraryList {
                                    recent: true,
                                    ..*request
                                },
                                config,
                                now,
                            )
                            .await?,
                        )
                    } else {
                        None
                    };
                    let usage = usage(&mut tx, request.account).await?;
                    let (bytes, objects, oldest) = backlog(&mut tx).await?;
                    let cleanup_bytes = super::history::unsigned(bytes)?;
                    let cleanup_objects = super::history::unsigned(objects)?;
                    let first = LibraryPage {
                        items: first.items,
                        next_cursor: first.next_cursor,
                        usage,
                        quota: request.quota,
                        cleanup_bytes,
                        cleanup_objects,
                        oldest_cleanup: oldest.map(super::history::unsigned).transpose()?,
                        uploads_paused: cleanup_bytes >= config.library_cleanup_bytes
                            || cleanup_objects >= config.library_cleanup_objects,
                    };
                    let recent = recent.map(|page| LibraryPage {
                        items: page.items,
                        next_cursor: page.next_cursor,
                        ..first
                    });
                    tx.commit().await?;
                    Ok((first, recent))
                }
                .await
            }};
        }
        let mut session = self.acquire_read().await?;
        let result = match session.backend.get_mut() {
            Backend::Sqlite(db) => read_pages!(sqlite, db.begin()),
            Backend::Postgres(db) => read_pages!(
                postgres,
                db.begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY")
            ),
            Backend::Closed => Err(Error::RestartRequired),
        };
        session.checked(result)
    }
}
