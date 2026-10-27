//! Shared transaction lifetime; every operation's SQL stays in its backend.
use super::{
    Error, MessageWrite, Result,
    metadata::{MetadataWrite, WriteResult},
};
pub(super) enum WriteRequest<'a> {
    Command(&'a super::CommandWrite<'a>),
    Message(&'a MessageWrite<'a>),
    Metadata(&'a MetadataWrite<'a>),
    Media(&'a super::media::MediaWrite<'a>),
    Profile(&'a super::profile::ProfileWrite<'a>),
}
impl super::Connection {
    pub(super) async fn save_write_async(
        &mut self,
        request: &WriteRequest<'_>,
        database: &super::Database,
    ) -> Result<WriteResult> {
        #[cfg(not(test))]
        {
            if !self.is_autocommit() {
                return Err(Error::Conversion(
                    "Native write requires an independent transaction",
                ));
            }
            self.save_root_write_async(request, database).await
        }
        #[cfg(test)]
        {
            if self.is_autocommit() {
                return self.save_root_write_async(request, database).await;
            }
            let serial = self.serial.get().wrapping_add(1);
            self.serial.set(serial);
            let name = format!("write_{serial}");
            let mut transaction = WriteTransaction {
                session: self,
                name,
                created: false,
                finished: false,
            };
            transaction
                .session
                .execute_batch_async(&format!("SAVEPOINT {}", transaction.name))
                .await?;
            transaction.created = true;
            let result = transaction.session.apply_write_async(request).await?;
            database.check_owner().await?;
            transaction
                .session
                .execute_batch_async(&format!("RELEASE {}", transaction.name))
                .await?;
            transaction.finished = true;
            Ok(result)
        }
    }

    async fn save_root_write_async(
        &mut self,
        request: &WriteRequest<'_>,
        database: &super::Database,
    ) -> Result<WriteResult> {
        use sqlx::Connection as _;
        self.ensure_available()?;
        let backend = self.backend.get_mut();
        let failed = &mut self.failed;
        let result = async move {
            match backend {
                super::Backend::Sqlite(db) => {
                    let mut tx = db.begin().await?;
                    let applied = apply_sqlite(&mut tx, request).await?;
                    let total: i64 = sqlx::query_scalar("SELECT total_changes()")
                        .fetch_one(&mut *tx)
                        .await?;
                    database.check_owner().await?;
                    // Losing/cancelling a COMMIT acknowledgement has an unknown
                    // outcome on either backend. Do not reuse this dispatcher.
                    failed.set(true);
                    tx.commit().await?;
                    failed.set(false);
                    Ok((
                        applied,
                        Some(
                            u64::try_from(total)
                                .map_err(|_| Error::Conversion("Invalid SQLite change count"))?,
                        ),
                    ))
                }
                super::Backend::Postgres(db) => {
                    let mut tx = db.begin().await?;
                    let applied = apply_postgres(&mut tx, request).await?;
                    database.check_owner().await?;
                    failed.set(true);
                    tx.commit().await?;
                    failed.set(false);
                    Ok((applied, None))
                }
                super::Backend::Closed => Err(Error::RestartRequired),
            }
        }
        .await;
        let (applied, total) = self.checked(result)?;
        if let Some(total) = total {
            self.changes.set(total);
        } else {
            self.changes.add(applied.changed);
        }
        Ok(applied)
    }
    #[cfg(test)]
    async fn apply_write_async(&mut self, request: &WriteRequest<'_>) -> Result<WriteResult> {
        self.ensure_available()?;
        let result = match self.backend.get_mut() {
            super::Backend::Sqlite(db) => {
                async {
                    let applied = apply_sqlite(db, request).await?;
                    let total: i64 = sqlx::query_scalar("SELECT total_changes()")
                        .fetch_one(&mut **db)
                        .await?;
                    Ok((
                        applied,
                        Some(
                            u64::try_from(total)
                                .map_err(|_| Error::Conversion("Invalid SQLite change count"))?,
                        ),
                    ))
                }
                .await
            }
            super::Backend::Postgres(db) => {
                let applied = apply_postgres(db, request).await;
                applied.map(|applied| (applied, None))
            }
            super::Backend::Closed => Err(Error::Conversion("Database closed")),
        };
        let (applied, total) = self.checked(result)?;
        if let Some(total) = total {
            self.changes.set(total);
        } else {
            self.changes.add(applied.changed);
        }
        Ok(applied)
    }
}
#[cfg(test)]
struct WriteTransaction<'a> {
    session: &'a mut super::Connection,
    name: String,
    created: bool,
    finished: bool,
}
#[cfg(test)]
impl Drop for WriteTransaction<'_> {
    fn drop(&mut self) {
        if self.finished || self.session.is_autocommit() {
            return;
        }
        if self.created
            && self
                .session
                .execute_batch(&format!("ROLLBACK TO {}; RELEASE {}", self.name, self.name))
                .is_err()
        {
            self.session.failed.set(true);
        }
    }
}

async fn apply_sqlite(
    db: &mut sqlx::SqliteConnection,
    request: &WriteRequest<'_>,
) -> Result<WriteResult> {
    match request {
        WriteRequest::Command(request) => super::sqlite::receipts::apply_command(db, request).await,
        WriteRequest::Message(request) => super::sqlite::messages::apply_message(db, request).await,
        WriteRequest::Metadata(request) => super::sqlite::metadata::apply(db, request).await,
        WriteRequest::Media(request) => super::sqlite::media::apply(db, request).await,
        WriteRequest::Profile(request) => super::sqlite::profile::apply(db, request).await,
    }
}
async fn apply_postgres(
    db: &mut sqlx::PgConnection,
    request: &WriteRequest<'_>,
) -> Result<WriteResult> {
    match request {
        WriteRequest::Command(request) => {
            super::postgres::receipts::apply_command(db, request).await
        }
        WriteRequest::Message(request) => {
            super::postgres::messages::apply_message(db, request).await
        }
        WriteRequest::Metadata(request) => super::postgres::metadata::apply(db, request).await,
        WriteRequest::Media(request) => super::postgres::media::apply(db, request).await,
        WriteRequest::Profile(request) => super::postgres::profile::apply(db, request).await,
    }
}
