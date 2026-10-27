//! Monotonic cursor operations, independent of backend SQL.
use super::{Backend, Error, Result};

pub(crate) struct CursorAdvance<'a> {
    pub(crate) user: &'a str,
    pub(crate) conversation: &'a str,
    pub(crate) through: u64,
}
impl super::Connection {
    pub(super) async fn advance_cursor_async(&mut self, request: &CursorAdvance<'_>) -> Result<()> {
        self.ensure_available()?;
        let result = match self.backend.get_mut() {
            Backend::Sqlite(db) => {
                async {
                    let changed = super::sqlite::cursors::advance(db, request).await?;
                    let total: i64 = sqlx::query_scalar("SELECT total_changes()")
                        .fetch_one(&mut **db)
                        .await?;
                    Ok((
                        changed,
                        Some(
                            u64::try_from(total)
                                .map_err(|_| Error::Conversion("Invalid SQLite change count"))?,
                        ),
                    ))
                }
                .await
            }
            Backend::Postgres(db) => super::postgres::cursors::advance(db, request)
                .await
                .map(|changed| (changed, None)),
            Backend::Closed => Err(Error::Conversion("Database closed")),
        };
        let (changed, total) = self.checked(result)?;
        if let Some(total) = total {
            self.changes.set(total);
        } else {
            self.changes.add(changed);
        }
        Ok(())
    }
}
