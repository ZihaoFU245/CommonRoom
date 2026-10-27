//! Backend resource ownership and async dispatch. PostgreSQL write leases are
//! independent; SQLite has exactly one writer lease. Command sessions are
//! supplied explicitly by operation handles, never pinned on the dispatcher.
use super::{Backend, ChangeCount, Connection, Error, MessageWrite, Native, Result, runtime};
use sqlx::{Executor as _, PgPool, SqlitePool};
use std::{
    ops::{Deref, DerefMut},
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

enum Pool {
    Sqlite(SqlitePool),
    Postgres(PgPool),
}
pub(crate) struct Database {
    pool: Pool,
    readers: Pool,
    owner: Option<tokio::sync::Mutex<Connection>>,
    changes: ChangeCount,
    failed: AtomicBool,
    folder_lock: Mutex<Option<std::fs::File>>,
}
impl Database {
    pub(crate) fn from_connection(
        connection: Connection,
        location: &str,
        lock: Option<std::fs::File>,
    ) -> Result<Self> {
        runtime::run(Self::open(connection, location, lock))
    }
    async fn open(
        connection: Connection,
        location: &str,
        lock: Option<std::fs::File>,
    ) -> Result<Self> {
        let (pool, owner) = if connection.is_postgres() {
            let options = location.parse::<sqlx::postgres::PgConnectOptions>()?;
            let pool = sqlx::postgres::PgPoolOptions::new()
                .min_connections(1).max_connections(8)
                .acquire_timeout(std::time::Duration::from_secs(5))
                .after_connect(|db, _| Box::pin(async move {
                    db.execute("SET statement_timeout='10s'; SET lock_timeout='2s'; SET idle_in_transaction_session_timeout='30s';").await?;
                    Ok(())
                })).connect_with(options).await?;
            (
                Pool::Postgres(pool),
                Some(tokio::sync::Mutex::new(connection)),
            )
        } else {
            let options = super::sqlite::options(Path::new(location), false)?.foreign_keys(true);
            // Keep this connection for the database lifetime, including named
            // in-memory databases and connection-local regression fixtures.
            let pool = sqlx::sqlite::SqlitePoolOptions::new()
                .min_connections(1)
                .max_connections(1)
                .idle_timeout(None)
                .max_lifetime(None)
                .acquire_timeout(std::time::Duration::from_secs(5))
                .connect_with(options)
                .await?;
            drop(connection);
            (Pool::Sqlite(pool), None)
        };
        let readers = match &pool {
            Pool::Postgres(_) => {
                let options = location.parse::<sqlx::postgres::PgConnectOptions>()?;
                Pool::Postgres(sqlx::postgres::PgPoolOptions::new()
                    .min_connections(4).max_connections(4)
                    .acquire_timeout(std::time::Duration::from_secs(5))
                    .after_connect(|db, _| Box::pin(async move {
                        db.execute("SET statement_timeout='10s'; SET lock_timeout='2s'; SET idle_in_transaction_session_timeout='30s'; SET default_transaction_read_only=on;").await?;
                        Ok(())
                    })).connect_with(options).await?)
            }
            Pool::Sqlite(_) => {
                let memory = location.starts_with("file:") && location.contains("mode=memory");
                let options = super::sqlite::options(Path::new(location), !memory)?
                    .pragma("query_only", "ON")
                    .pragma("cache_size", "-2048")
                    .busy_timeout(std::time::Duration::from_secs(2));
                Pool::Sqlite(
                    sqlx::sqlite::SqlitePoolOptions::new()
                        .min_connections(4)
                        .max_connections(4)
                        .idle_timeout(None)
                        .max_lifetime(None)
                        .acquire_timeout(std::time::Duration::from_secs(5))
                        .connect_with(options)
                        .await?,
                )
            }
        };
        Ok(Self {
            pool,
            readers,
            owner,
            changes: ChangeCount::default(),
            failed: AtomicBool::new(false),
            folder_lock: Mutex::new(lock),
        })
    }
    pub(crate) fn attach_folder_lock(&self, lock: Option<std::fs::File>) -> Result<()> {
        let mut folder = self
            .folder_lock
            .lock()
            .map_err(|_| Error::Conversion("Database ownership state poisoned"))?;
        if folder.is_some() {
            return Err(Error::Conversion("Database folder already owned"));
        }
        *folder = lock;
        Ok(())
    }
    pub(super) async fn acquire_reader(&self) -> Result<Lease<'_>> {
        self.ensure_state()?;
        self.check_owner().await?;
        let connection = match &self.readers {
            Pool::Sqlite(pool) => {
                Connection::sqlite_session(
                    Native::Pooled(pool.acquire().await?),
                    true,
                    ChangeCount::default(),
                )
                .await?
            }
            Pool::Postgres(pool) => Connection::new(
                Backend::Postgres(Native::Pooled(pool.acquire().await?)),
                true,
            ),
        };
        Ok(Lease {
            database: self,
            connection,
        })
    }
    #[cfg(test)]
    pub(crate) fn reader_lease(&self) -> Result<Lease<'_>> {
        runtime::run(self.acquire_reader())
    }
    pub(crate) fn ensure_state(&self) -> Result<()> {
        if self.failed.load(Ordering::Acquire) {
            Err(Error::RestartRequired)
        } else {
            Ok(())
        }
    }
    pub(crate) fn ensure_available(&self) -> Result<()> {
        runtime::run(self.ensure_available_async())
    }
    pub(crate) async fn ensure_available_async(&self) -> Result<()> {
        self.ensure_state()?;
        self.check_owner().await
    }
    pub(super) async fn check_owner(&self) -> Result<()> {
        // The session advisory lock is held on this dedicated connection, never
        // on a pooled transaction session. A lost owner invalidates all writes.
        if let Some(owner) = &self.owner {
            let mut owner = owner.lock().await;
            let health =
                tokio::time::timeout(std::time::Duration::from_secs(5), owner.ping()).await;
            if !matches!(health, Ok(Ok(()))) {
                self.failed.store(true, Ordering::Release);
                return Err(Error::RestartRequired);
            }
        }
        self.ensure_state()
    }
    #[cfg(test)]
    pub(super) fn ownership_pid(&self) -> Result<i32> {
        let owner = self
            .owner
            .as_ref()
            .ok_or(Error::Conversion("No PostgreSQL owner"))?;
        runtime::run(async {
            let owner = owner.lock().await;
            owner.query_row("SELECT pg_backend_pid()", [], |row| row.get(0))
        })
    }
    pub(super) async fn acquire(&self) -> Result<Lease<'_>> {
        self.ensure_state()?;
        self.check_owner().await?;
        let connection = match &self.pool {
            Pool::Sqlite(pool) => {
                Connection::sqlite_session(
                    Native::Pooled(pool.acquire().await?),
                    false,
                    self.changes.clone(),
                )
                .await?
            }
            Pool::Postgres(pool) => {
                let mut connection = Connection::new(
                    Backend::Postgres(Native::Pooled(pool.acquire().await?)),
                    false,
                );
                connection.changes = self.changes.clone();
                connection
            }
        };
        Ok(Lease {
            database: self,
            connection,
        })
    }
}
impl super::Handle {
    #[cfg(test)]
    pub(crate) fn advance_cursor_blocking(&self, request: &super::CursorAdvance<'_>) -> Result<()> {
        runtime::run(self.advance_cursor(request))
    }
    pub(crate) async fn advance_cursor(&self, request: &super::CursorAdvance<'_>) -> Result<()> {
        let mut session = self.acquire().await?;
        session.advance_cursor_async(request).await
    }
    pub(crate) fn save_message_blocking(
        &self,
        request: &MessageWrite<'_>,
    ) -> Result<super::MessageCommit> {
        runtime::run(self.save_message(request))
    }
    pub(crate) async fn save_message(
        &self,
        request: &MessageWrite<'_>,
    ) -> Result<super::MessageCommit> {
        let mut session = self.acquire().await?;
        session
            .save_write_async(&super::transaction::WriteRequest::Message(request), self)
            .await?
            .message
            .ok_or(Error::Conversion("Missing committed message result"))
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        let result = runtime::run(async {
            match &self.pool {
                Pool::Sqlite(pool) => close_pool(pool).await,
                Pool::Postgres(pool) => close_pool(pool).await,
            }
            match &self.readers {
                Pool::Sqlite(pool) => close_pool(pool).await,
                Pool::Postgres(pool) => close_pool(pool).await,
            }
            Ok(())
        });
        if result.is_err() {
            tracing::error!("Database pool shutdown failed");
        }
        // Field destruction closes the owning session after every pool lease.
    }
}

pub(crate) struct Lease<'a> {
    database: &'a Database,
    connection: Connection,
}
impl Lease<'_> {
    pub(super) fn into_connection(mut self) -> Connection {
        std::mem::replace(
            &mut self.connection,
            Connection::new(Backend::Closed, false),
        )
    }
}
impl Database {
    pub(super) fn invalidate(&self) {
        self.failed.store(true, Ordering::Release);
    }
}
impl Deref for Lease<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.connection
    }
}
impl DerefMut for Lease<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.connection
    }
}
impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.connection.failed.set(true);
        }
        if self.connection.failed.get() {
            self.database.failed.store(true, Ordering::Release);
        }
    }
}

#[cfg(test)]
#[path = "dispatcher_tests.rs"]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests;

// A lease can pass SQLx's on-release health check concurrently with pool.close.
// In that case it can enter the idle queue after close has scanned it. Drain
// until the actual connection count reaches zero before releasing ownership.
async fn close_pool<D: sqlx::Database>(pool: &sqlx::Pool<D>) {
    loop {
        pool.close().await;
        if pool.size() == 0 {
            return;
        }
        tokio::task::yield_now().await;
    }
}
