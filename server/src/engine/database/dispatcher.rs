//! Backend resource ownership and async dispatch. PostgreSQL write leases are
//! independent; SQLite has exactly one writer lease. Production handles never
//! pin a command session; explicit legacy scopes exist only in test fixtures.
use super::{Backend, ChangeCount, Connection, Error, MessageWrite, Native, Result, runtime};
use sqlx::{Executor as _, PgPool, SqlitePool};
use std::{
    ops::{Deref, DerefMut},
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

enum Pool {
    Sqlite(SqlitePool),
    Postgres(PgPool),
}
#[derive(Default)]
struct PoolMetrics {
    acquisitions: AtomicU64,
    errors: AtomicU64,
    timeouts: AtomicU64,
    total_us: AtomicU64,
    max_us: AtomicU64,
}
impl PoolMetrics {
    fn stats(&self, pool: &Pool) -> serde_json::Value {
        let (open, idle, min, max) = match pool {
            Pool::Sqlite(pool) => (
                pool.size(),
                pool.num_idle(),
                pool.options().get_min_connections(),
                pool.options().get_max_connections(),
            ),
            Pool::Postgres(pool) => (
                pool.size(),
                pool.num_idle(),
                pool.options().get_min_connections(),
                pool.options().get_max_connections(),
            ),
        };
        serde_json::json!({"open":open,"idle":idle,"min":min,"max":max,
            "acquisitions":self.acquisitions.load(Ordering::Relaxed),
            "acquire_errors":self.errors.load(Ordering::Relaxed),
            "acquire_timeouts":self.timeouts.load(Ordering::Relaxed),
            "acquire_total_us":self.total_us.load(Ordering::Relaxed),
            "acquire_max_us":self.max_us.load(Ordering::Relaxed)})
    }
}
async fn acquire_pool<D: sqlx::Database>(
    pool: &sqlx::Pool<D>,
    metrics: &PoolMetrics,
) -> Result<sqlx::pool::PoolConnection<D>> {
    let start = std::time::Instant::now();
    let result = pool.acquire().await;
    let elapsed = u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX);
    metrics.acquisitions.fetch_add(1, Ordering::Relaxed);
    metrics.total_us.fetch_add(elapsed, Ordering::Relaxed);
    metrics.max_us.fetch_max(elapsed, Ordering::Relaxed);
    if result.is_err() {
        metrics.errors.fetch_add(1, Ordering::Relaxed);
    }
    if matches!(&result, Err(sqlx::Error::PoolTimedOut)) {
        metrics.timeouts.fetch_add(1, Ordering::Relaxed);
    }
    Ok(result?)
}
pub(crate) struct Database {
    pool: Pool,
    readers: Pool,
    writer_metrics: PoolMetrics,
    reader_metrics: PoolMetrics,
    history_results: ResultMetrics,
    transaction_results: ResultMetrics,
    cursor_results: ResultMetrics,
    owner: Option<tokio::sync::Mutex<Connection>>,
    #[cfg(test)]
    forced_owner_probes: AtomicU64,
    owner_checked: AtomicU64,
    owner_epoch: std::time::Instant,
    changes: ChangeCount,
    failed: AtomicBool,
    folder_lock: Mutex<Option<std::fs::File>>,
}
#[derive(Default)]
struct ResultMetrics {
    succeeded: AtomicU64,
    failed: AtomicU64,
}
impl ResultMetrics {
    fn record(&self, succeeded: bool) {
        if succeeded {
            &self.succeeded
        } else {
            &self.failed
        }
        .fetch_add(1, Ordering::Relaxed);
    }
    fn stats(&self) -> serde_json::Value {
        serde_json::json!({"succeeded":self.succeeded.load(Ordering::Relaxed),"failed":self.failed.load(Ordering::Relaxed)})
    }
}
impl Database {
    pub(crate) fn from_connection(
        connection: Connection,
        location: &str,
        lock: Option<std::fs::File>,
        limits: crate::config::PostgresPoolConfig,
    ) -> Result<Self> {
        runtime::run(Self::open(connection, location, lock, limits))
    }
    async fn open(
        connection: Connection,
        location: &str,
        lock: Option<std::fs::File>,
        limits: crate::config::PostgresPoolConfig,
    ) -> Result<Self> {
        let (pool, owner) = if connection.is_postgres() {
            let options = location.parse::<sqlx::postgres::PgConnectOptions>()?;
            let pool = sqlx::postgres::PgPoolOptions::new()
                .min_connections(1).max_connections(limits.max_writers)
                .idle_timeout(std::time::Duration::from_secs(limits.idle_timeout_seconds))
                .max_lifetime(std::time::Duration::from_secs(30 * 60))
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
                    .min_connections(limits.min_readers).max_connections(limits.max_readers)
                    .idle_timeout(std::time::Duration::from_secs(limits.idle_timeout_seconds))
                    .max_lifetime(std::time::Duration::from_secs(30 * 60))
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
                        .min_connections(2)
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
            writer_metrics: PoolMetrics::default(),
            reader_metrics: PoolMetrics::default(),
            history_results: ResultMetrics::default(),
            transaction_results: ResultMetrics::default(),
            cursor_results: ResultMetrics::default(),
            owner,
            #[cfg(test)]
            forced_owner_probes: AtomicU64::new(0),
            owner_checked: AtomicU64::new(0),
            owner_epoch: std::time::Instant::now(),
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
    pub(crate) fn pool_stats(&self) -> serde_json::Value {
        serde_json::json!({"readers":self.reader_metrics.stats(&self.readers),
            "writers":self.writer_metrics.stats(&self.pool),"dedicated_owner":self.owner.is_some(),
            "operations":{"history_reads":self.history_results.stats(),"transactions":self.transaction_results.stats(),"cursor_advances":self.cursor_results.stats()}})
    }
    pub(super) fn observe_history<T>(&self, result: Result<T>) -> Result<T> {
        self.history_results.record(result.is_ok());
        result
    }
    pub(super) fn observe_transaction(&self, succeeded: bool) {
        self.transaction_results.record(succeeded);
    }
    pub(super) async fn acquire_reader(&self) -> Result<Lease<'_>> {
        self.ensure_available_async().await?;
        let connection = match &self.readers {
            Pool::Sqlite(pool) => {
                Connection::sqlite_session(
                    Native::Pooled(acquire_pool(pool, &self.reader_metrics).await?),
                    true,
                    ChangeCount::default(),
                )
                .await?
            }
            Pool::Postgres(pool) => Connection::new(
                Backend::Postgres(Native::Pooled(
                    acquire_pool(pool, &self.reader_metrics).await?,
                )),
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
    #[cfg(test)]
    pub(crate) fn ensure_available(&self) -> Result<()> {
        runtime::run(self.ensure_available_async())
    }
    pub(crate) async fn ensure_available_async(&self) -> Result<()> {
        self.ensure_state()?;
        self.validate_owner(false).await
    }
    pub(super) async fn check_owner(&self) -> Result<()> {
        self.validate_owner(true).await
    }
    fn recent_owner_check(&self) -> bool {
        let checked = self.owner_checked.load(Ordering::Acquire);
        let elapsed = u64::try_from(self.owner_epoch.elapsed().as_millis()).unwrap_or(u64::MAX);
        checked > 0 && elapsed.saturating_sub(checked - 1) < 100
    }
    async fn validate_owner(&self, force: bool) -> Result<()> {
        // The session advisory lock is held on this dedicated connection, never
        // on a pooled transaction session. A lost owner invalidates all writes.
        if let Some(owner) = &self.owner {
            if !force && self.recent_owner_check() {
                return self.ensure_state();
            }
            let mut owner = owner.lock().await;
            // Share one bounded read-health probe across cache hits and waiters.
            // Writers validate ownership immediately before committing.
            if !force && self.recent_owner_check() {
                return self.ensure_state();
            }
            #[cfg(test)]
            if force {
                self.forced_owner_probes.fetch_add(1, Ordering::Relaxed);
            }
            let health = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                owner.validate_owner_async(),
            )
            .await;
            match health {
                Ok(Ok(())) => {
                    let elapsed = u64::try_from(self.owner_epoch.elapsed().as_millis())
                        .unwrap_or(u64::MAX - 1);
                    self.owner_checked
                        .store(elapsed.saturating_add(1), Ordering::Release);
                }
                Err(_) => return Err(sqlx::Error::PoolTimedOut.into()),
                Ok(Err(error)) => {
                    owner.observe_error(&error);
                    // Never reconnect an owning session: another process could
                    // have acquired ownership and invalidated our memory state.
                    if owner.ensure_available().is_err() {
                        self.failed.store(true, Ordering::Release);
                    }
                    return Err(error);
                }
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
        self.ensure_available_async().await?;
        let connection = match &self.pool {
            Pool::Sqlite(pool) => {
                Connection::sqlite_session(
                    Native::Pooled(acquire_pool(pool, &self.writer_metrics).await?),
                    false,
                    self.changes.clone(),
                )
                .await?
            }
            Pool::Postgres(pool) => {
                let mut connection = Connection::new(
                    Backend::Postgres(Native::Pooled(
                        acquire_pool(pool, &self.writer_metrics).await?,
                    )),
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
impl Connection {
    async fn validate_owner_async(&mut self) -> Result<()> {
        self.ensure_available()?;
        let owned: bool = match self.backend.get_mut() {
            Backend::Postgres(db) => sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE locktype='advisory' AND pid=pg_backend_pid() AND classid=0 AND objid=1128808788 AND objsubid=1 AND granted)"
            ).fetch_one(&mut **db).await?,
            _ => return Err(Error::Conversion("No PostgreSQL ownership session")),
        };
        if !owned {
            self.failed.set(true);
            return Err(Error::RestartRequired);
        }
        Ok(())
    }
}

impl super::Handle {
    #[cfg(test)]
    pub(crate) fn advance_cursor_blocking(&self, request: &super::CursorAdvance<'_>) -> Result<()> {
        runtime::run(self.advance_cursor(request))
    }
    pub(crate) async fn advance_cursor(&self, request: &super::CursorAdvance<'_>) -> Result<()> {
        let mut session = self.acquire().await?;
        // Cursor advancement is one autocommit statement rather than a root
        // transaction, so its forced ownership check belongs directly here.
        self.check_owner().await?;
        let result = session.advance_cursor_async(request).await;
        self.cursor_results.record(result.is_ok());
        result
    }
    #[cfg(test)]
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
    #[cfg(test)]
    pub(super) fn into_connection(mut self) -> Connection {
        std::mem::replace(
            &mut self.connection,
            Connection::new(Backend::Closed, false),
        )
    }
}
impl Database {
    #[cfg(test)]
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
