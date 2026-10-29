//! Database connections backed by SQLx. Backend SQL is always explicit.
//! Live operations use the typed async dispatcher. Synchronous connection utilities
//! serve bootstrap, offline transfer and test fixtures; backend SQL is independent.
mod transfer;
pub(crate) use transfer::{MigrationReport, transfer_sqlite};
mod bootstrap;
pub(crate) use bootstrap::{POSTGRES_SCHEMA, POSTGRES_SCHEMA_VERSION, SQLITE_SCHEMA_VERSION};
mod command;
pub(crate) use command::{CommandCommit, CommandEffect, CommandWrite};
mod receipts;
pub(crate) use receipts::ReceiptRead;
mod media;
mod profile;
pub(crate) use media::{MediaAllocation, MediaWrite, StickerCopy};
pub(crate) use profile::ProfileWrite;
mod metadata;
pub(crate) use metadata::{CursorRows, EntityRow, MetadataOp, MetadataSnapshot, MetadataWrite};
mod history;
mod transaction;
pub(crate) use history::wire_size as history_wire_size;
pub(crate) use history::{PageRead, UnreadHint, UnreadRead};
#[cfg(test)]
pub(crate) use runtime::block_on as block_on_storage;
#[cfg(test)]
pub(crate) use sqlite::history::{CLEANUP_COUNTS_SQL, PAGE_ASC_SQL};
#[cfg(test)]
pub(crate) use sqlite::media::{CLAIM_CLEANUP_SQL, STARTUP_CLEANUP_SQL, TOMBSTONES_SQL};
mod cursor;
pub(crate) use cursor::CursorAdvance;
mod context;
pub(crate) use context::Handle;
mod dispatcher;
pub(crate) use dispatcher::Database;
#[cfg(test)]
pub(crate) use dispatcher::Lease as ReaderLease;
mod native;
use native::Native;
mod message;
pub(crate) use message::{MessageCommit, MessageMutation, MessageReceipt, MessageWrite};
mod postgres;
mod projection;
mod runtime;
mod sqlite;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub(crate) mod tests;
mod value;
use futures_util::TryStreamExt;
pub(crate) use projection::{entity_projection, message_projection};
use sqlx::Executor as _;
#[cfg(test)]
use sqlx::Statement as _;
use std::{
    cell::{Cell, RefCell},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
#[cfg(test)]
pub(crate) use value::{Encode, params};
pub(crate) use value::{Params, Row, Value};

#[derive(Debug)]
pub(crate) enum Error {
    Driver(sqlx::Error),
    Conversion(&'static str),
    NoRows,
    MissingMedia,
    RevisionConflict,
    ReceiptConflict,
    ReceiptCapacity,
    ReceiptPayload,
    RestartRequired,
    InvalidRequest(&'static str),
    InvalidData(String),
    Conflict(&'static str),
}
pub(crate) type Result<T> = std::result::Result<T, Error>;
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Neither SQL text nor driver details may expose private row values.
            Self::Driver(sqlx::Error::Database(error)) => write!(
                f,
                "Database operation failed ({})",
                error.code().as_deref().unwrap_or("database")
            ),
            Self::Driver(_) => f.write_str("Database connection or operation failed"),
            Self::Conversion(message) => f.write_str(message),
            Self::InvalidData(message) => f.write_str(message),
            Self::NoRows => f.write_str("Database row not found"),
            Self::MissingMedia => f.write_str("Upload not found."),
            Self::ReceiptConflict => f.write_str("Key already belongs to another message request."),
            Self::ReceiptCapacity => f.write_str("Wait for message receipts to expire."),
            Self::ReceiptPayload => f.write_str("Command receipt is too large."),
            Self::InvalidRequest(message) | Self::Conflict(message) => f.write_str(message),
            Self::RestartRequired => f.write_str("Database unavailable; restart the service"),
            Self::RevisionConflict => {
                f.write_str("Conversation or sequence changed; retry the operation.")
            }
        }
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Driver(sqlx::Error::Io(error))
    }
}
impl From<sqlx::Error> for Error {
    fn from(error: sqlx::Error) -> Self {
        Self::Driver(error)
    }
}
#[cfg(test)]
pub(crate) trait OptionalExtension<T> {
    fn optional(self) -> Result<Option<T>>;
}
#[cfg(test)]
impl<T> OptionalExtension<T> for Result<T> {
    fn optional(self) -> Result<Option<T>> {
        match self {
            Err(Error::NoRows) | Err(Error::Driver(sqlx::Error::RowNotFound)) => Ok(None),
            result => result.map(Some),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum SqliteAccess {
    ReadWrite,
    ReadOnly,
}
#[cfg(test)]
pub(crate) enum TransactionBehavior {
    Immediate,
}

#[derive(Clone, Default)]
struct TransactionState(Arc<AtomicBool>);
impl TransactionState {
    fn get(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    fn set(&self, value: bool) {
        self.0.store(value, Ordering::Release);
    }
}

#[derive(Clone, Default)]
struct ChangeCount(Arc<AtomicU64>);
impl ChangeCount {
    #[cfg(test)]
    fn get(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }
    fn set(&self, value: u64) {
        self.0.store(value, Ordering::Release);
    }
    fn add(&self, value: u64) {
        self.0.fetch_add(value, Ordering::AcqRel);
    }
}

pub(crate) struct Connection {
    backend: RefCell<Backend>,
    readonly: bool,
    changes: ChangeCount,
    transaction: TransactionState,
    #[cfg(test)]
    command_root: Cell<bool>,
    #[cfg(test)]
    serial: Cell<u64>,
    failed: Cell<bool>,
}
enum Backend {
    Closed,
    Sqlite(Native<sqlx::Sqlite>),
    Postgres(Native<sqlx::Postgres>),
}
impl Connection {
    fn new(backend: Backend, readonly: bool) -> Self {
        Self {
            backend: RefCell::new(backend),
            readonly,
            changes: ChangeCount::default(),
            transaction: TransactionState::default(),
            #[cfg(test)]
            command_root: Cell::new(false),
            #[cfg(test)]
            serial: Cell::new(0),
            failed: Cell::new(false),
        }
    }
    pub(crate) fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_access(path, SqliteAccess::ReadWrite)
    }
    pub(crate) fn open_with_access(path: impl AsRef<Path>, access: SqliteAccess) -> Result<Self> {
        let readonly = matches!(access, SqliteAccess::ReadOnly);
        let db = Native::Owned(runtime::run(sqlite::open(path.as_ref(), readonly))?);
        runtime::run(Self::sqlite_session(db, readonly, ChangeCount::default()))
    }
    async fn sqlite_session(
        mut db: Native<sqlx::Sqlite>,
        readonly: bool,
        changes: ChangeCount,
    ) -> Result<Self> {
        let transaction = TransactionState::default();
        let committed = transaction.clone();
        let rolled_back = transaction.clone();
        {
            let mut handle = db.lock_handle().await?;
            handle.set_commit_hook(move || {
                committed.set(false);
                true
            });
            handle.set_rollback_hook(move || rolled_back.set(false));
        }
        let mut connection = Self::new(Backend::Sqlite(db), readonly);
        connection.transaction = transaction;
        connection.changes = changes;
        Ok(connection)
    }
    pub(crate) fn postgres(url: &str, readonly: bool) -> Result<Self> {
        Ok(Self::new(
            Backend::Postgres(Native::Owned(runtime::run(postgres::open(url, readonly))?)),
            readonly,
        ))
    }
    #[cfg(test)]
    async fn execute_batch_async(&mut self, query: &str) -> Result<()> {
        self.ensure_available()?;
        let result = match self.backend.get_mut() {
            Backend::Sqlite(db) => {
                async {
                    (&mut **db).execute(query).await?;
                    let total: i64 = sqlx::query_scalar("SELECT total_changes()")
                        .fetch_one(&mut **db)
                        .await?;
                    Ok(Some(u64::try_from(total).map_err(|_| {
                        Error::Conversion("Invalid SQLite change count")
                    })?))
                }
                .await
            }
            Backend::Postgres(db) => {
                async {
                    (&mut **db).execute(query).await?;
                    Ok(None)
                }
                .await
            }
            Backend::Closed => Err(Error::Conversion("Database closed")),
        };
        if let Some(total) = self.checked(result)? {
            self.changes.set(total);
        }
        Ok(())
    }
    pub(crate) fn is_postgres(&self) -> bool {
        matches!(*self.backend.borrow(), Backend::Postgres(_))
    }
    pub(crate) fn ensure_available(&self) -> Result<()> {
        if self.failed.get() || matches!(*self.backend.borrow(), Backend::Closed) {
            Err(Error::RestartRequired)
        } else {
            Ok(())
        }
    }
    fn checked<T>(&self, result: Result<T>) -> Result<T> {
        if let Err(error) = &result {
            self.observe_error(error);
        }
        result
    }
    fn observe_error(&self, error: &Error) {
        if let Error::Driver(error) = error {
            let fatal = match error {
                sqlx::Error::Database(db) => db
                    .try_downcast_ref::<sqlx::postgres::PgDatabaseError>()
                    .is_some_and(|pg| {
                        matches!(
                            pg.severity(),
                            sqlx::postgres::PgSeverity::Fatal | sqlx::postgres::PgSeverity::Panic
                        )
                    }),
                sqlx::Error::Io(_)
                | sqlx::Error::Tls(_)
                | sqlx::Error::Protocol(_)
                | sqlx::Error::WorkerCrashed => true,
                _ => false,
            };
            if fatal {
                self.failed.set(true);
            }
        }
    }
    pub(crate) fn prepare<'c>(&'c self, query: impl AsRef<str>) -> Result<Statement<'c>> {
        self.ensure_available()?;
        let query = query.as_ref().to_owned();
        #[cfg(test)]
        let parameters = {
            let mut backend = self.backend.borrow_mut();
            let parameters = match &mut *backend {
                Backend::Sqlite(db) => runtime::run(async {
                    Ok(db
                        .prepare(&query)
                        .await?
                        .parameters()
                        .map(|p| p.map_left(|p| p.len()).into_inner())
                        .unwrap_or(0))
                }),
                Backend::Postgres(db) => runtime::run(async {
                    Ok(db
                        .prepare(&query)
                        .await?
                        .parameters()
                        .map(|p| p.map_left(|p| p.len()).into_inner())
                        .unwrap_or(0))
                }),
                Backend::Closed => Err(Error::Conversion("Database closed")),
            };
            self.checked(parameters)?
        };
        Ok(Statement {
            connection: self,
            query,
            #[cfg(test)]
            parameters,
        })
    }
    #[cfg(test)]
    pub(crate) fn prepare_cached<'c>(&'c self, query: impl AsRef<str>) -> Result<Statement<'c>> {
        self.prepare(query)
    }
    #[cfg(test)]
    pub(crate) fn execute(&self, query: impl AsRef<str>, params: impl Params) -> Result<usize> {
        self.prepare(query)?.execute(params)
    }
    pub(crate) fn query_row<T, P: Params, F: FnOnce(&Row<'_>) -> Result<T>>(
        &self,
        query: impl AsRef<str>,
        params: P,
        f: F,
    ) -> Result<T> {
        self.prepare(query)?.query_row(params, f)
    }
    pub(crate) fn execute_batch(&self, query: &str) -> Result<()> {
        self.ensure_available()?;
        let mut backend = self.backend.borrow_mut();
        let result = match &mut *backend {
            Backend::Sqlite(db) => runtime::run(async {
                (&mut **db).execute(query).await?;
                let count: i64 = sqlx::query_scalar("SELECT total_changes()")
                    .fetch_one(&mut **db)
                    .await?;
                Ok(Some(u64::try_from(count).map_err(|_| {
                    Error::Conversion("Invalid SQLite change count")
                })?))
            }),
            Backend::Postgres(db) => runtime::run(async {
                (&mut **db).execute(query).await?;
                Ok(None)
            }),
            Backend::Closed => Err(Error::Conversion("Database closed")),
        };
        if let Some(count) = self.checked(result)? {
            self.changes.set(count);
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn total_changes(&self) -> u64 {
        self.changes.get()
    }
    pub(crate) fn is_autocommit(&self) -> bool {
        !self.transaction.get()
    }
    pub(crate) fn busy_timeout(&self, timeout: std::time::Duration) -> Result<()> {
        if self.is_postgres() {
            return Err(Error::Conversion("busy_timeout applies only to SQLite"));
        }
        self.execute_batch(&format!("PRAGMA busy_timeout={}", timeout.as_millis()))
    }
    pub(crate) fn visit_rows(
        &self,
        query: &str,
        mut visit: impl FnMut(&Row<'_>) -> Result<()>,
    ) -> Result<()> {
        self.ensure_available()?;
        let mut backend = self.backend.borrow_mut();
        match &mut *backend {
            Backend::Sqlite(db) => {
                let mut rows = sqlx::query(query).fetch(&mut **db);
                while let Some(row) =
                    self.checked(runtime::run(async { Ok(rows.try_next().await?) }))?
                {
                    visit(&Row {
                        inner: value::RowData::Sqlite(&row),
                    })?;
                }
            }
            Backend::Postgres(db) => {
                let mut rows = sqlx::query(query).fetch(&mut **db);
                while let Some(row) =
                    self.checked(runtime::run(async { Ok(rows.try_next().await?) }))?
                {
                    visit(&Row {
                        inner: value::RowData::Postgres(&row),
                    })?;
                }
            }
            Backend::Closed => return Err(Error::Conversion("Database closed")),
        }
        Ok(())
    }
    pub(crate) fn copy_from(
        &self,
        table: &str,
        columns: &[&str],
        produce: impl FnOnce(&mut dyn FnMut(Vec<Value>) -> Result<()>) -> Result<()>,
    ) -> Result<u64> {
        self.ensure_available()?;
        let mut backend = self.backend.borrow_mut();
        let Backend::Postgres(db) = &mut *backend else {
            return Err(Error::Conversion("COPY requires PostgreSQL"));
        };
        let result = postgres::copy_from(db, table, columns, produce);
        let count = self.checked(result)?;
        self.changes.add(count);
        Ok(count)
    }
    pub(crate) fn unchecked_transaction(&self) -> Result<Transaction<'_>> {
        if !self.is_autocommit() {
            return Err(Error::Conversion("Transaction already active"));
        }
        self.execute_batch(if self.is_postgres() && self.readonly {
            "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY"
        } else {
            "BEGIN"
        })?;
        self.transaction.set(true);
        Ok(Transaction {
            connection: self,
            name: None,
            root: true,
            finished: false,
        })
    }
    #[cfg(test)]
    pub(crate) fn begin_command(&self) -> Result<()> {
        let root = self.is_autocommit();
        self.command_root.set(root);
        if self.is_postgres() && root {
            self.execute_batch("BEGIN")?;
        }
        self.transaction.set(true);
        if let Err(error) = self.execute_batch("SAVEPOINT bot_command_receipt") {
            self.rollback_root();
            return Err(error);
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn rollback_command_effect(&self) -> Result<()> {
        self.execute_batch("ROLLBACK TO bot_command_receipt")
    }
    #[cfg(test)]
    pub(crate) fn finish_command(&self) -> Result<()> {
        self.execute_batch("RELEASE bot_command_receipt")?;
        if self.is_postgres() && self.command_root.get() {
            self.commit_root()?;
        } else {
            self.transaction.set(!self.command_root.get());
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn abort_command(&self) -> Result<()> {
        let result = self.execute_batch(if self.is_postgres() && self.command_root.get() {
            "ROLLBACK"
        } else {
            "ROLLBACK TO bot_command_receipt; RELEASE bot_command_receipt"
        });
        if result.is_err() {
            self.failed.set(true);
        }
        self.transaction.set(!self.command_root.get());
        result
    }
    fn commit_root(&self) -> Result<()> {
        if let Err(error) = self.execute_batch("COMMIT") {
            // A lost commit acknowledgement has an uncertain outcome. Never reuse
            // this connection or keep serving with potentially stale engine state.
            self.failed.set(true);
            return Err(error);
        }
        self.transaction.set(false);
        Ok(())
    }
    fn rollback_root(&self) {
        if self.execute_batch("ROLLBACK").is_err() {
            self.failed.set(true);
        }
        self.transaction.set(false);
    }
    #[cfg(test)]
    pub(crate) fn transaction(&mut self) -> Result<Transaction<'_>> {
        self.unchecked_transaction()
    }
    #[cfg(test)]
    pub(crate) fn transaction_with_behavior(
        &mut self,
        behavior: TransactionBehavior,
    ) -> Result<Transaction<'_>> {
        let begin = match behavior {
            TransactionBehavior::Immediate => "BEGIN IMMEDIATE",
        };
        self.execute_batch(begin)?;
        self.transaction.set(true);
        Ok(Transaction {
            connection: self,
            name: None,
            root: true,
            finished: false,
        })
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        if self.transaction.get() && !self.failed.get() {
            self.rollback_root();
        }
        let backend = std::mem::replace(self.backend.get_mut(), Backend::Closed);
        let result = match backend {
            Backend::Sqlite(db) => db.release(self.failed.get()),
            Backend::Postgres(db) => db.release(self.failed.get()),
            Backend::Closed => Ok(()),
        };
        if result.is_err() {
            tracing::error!("Database shutdown failed");
        }
    }
}

pub(crate) struct Transaction<'a> {
    connection: &'a Connection,
    name: Option<String>,
    root: bool,
    finished: bool,
}
impl std::ops::Deref for Transaction<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.connection
    }
}
impl Transaction<'_> {
    #[cfg(test)]
    pub(crate) fn rollback(mut self) -> Result<()> {
        self.connection.execute_batch("ROLLBACK")?;
        self.connection.transaction.set(false);
        self.finished = true;
        Ok(())
    }
    pub(crate) fn commit(mut self) -> Result<()> {
        if let Some(name) = &self.name {
            self.connection.execute_batch(&format!("RELEASE {name}"))?;
        }
        if self.name.is_none() || self.root && self.connection.is_postgres() {
            self.connection.commit_root()?;
        } else if self.root {
            self.connection.transaction.set(false);
        }
        self.finished = true;
        Ok(())
    }
}
impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if self.finished || self.connection.is_autocommit() {
            return;
        }
        if let Some(name) = &self.name {
            if self
                .connection
                .execute_batch(&format!("ROLLBACK TO {name}; RELEASE {name}"))
                .is_err()
            {
                self.connection.failed.set(true);
            }
            if self.root && self.connection.is_postgres() {
                self.connection.rollback_root();
            } else if self.root {
                self.connection.transaction.set(false);
            }
        } else {
            self.connection.rollback_root();
        }
    }
}

pub(crate) struct Statement<'a> {
    connection: &'a Connection,
    query: String,
    #[cfg(test)]
    parameters: usize,
}
impl Statement<'_> {
    #[cfg(test)]
    pub(crate) fn parameter_count(&self) -> usize {
        self.parameters
    }
    #[cfg(test)]
    pub(crate) fn exists(&mut self, params: impl Params) -> Result<bool> {
        self.query_row(params, |_| Ok(()))
            .optional()
            .map(|row| row.is_some())
    }
    #[cfg(test)]
    pub(crate) fn execute(&mut self, params: impl Params) -> Result<usize> {
        self.connection.ensure_available()?;
        let values = params.values()?;
        if values.len() != self.parameters {
            return Err(Error::Conversion("Incorrect parameter count"));
        }
        let mut backend = self.connection.backend.borrow_mut();
        let result = match &mut *backend {
            Backend::Sqlite(db) => runtime::run(sqlite::execute(db, &self.query, values)),
            Backend::Postgres(db) => runtime::run(postgres::execute(db, &self.query, values)),
            Backend::Closed => Err(Error::Conversion("Database closed")),
        };
        let (affected, total) = self.connection.checked(result)?;
        if let Some(total) = total {
            self.connection.changes.set(total);
        } else {
            self.connection.changes.add(affected);
        }
        usize::try_from(affected)
            .map_err(|_| Error::Conversion("Affected row count exceeds platform range"))
    }
    pub(crate) fn query_row<T, P: Params, F: FnOnce(&Row<'_>) -> Result<T>>(
        &mut self,
        params: P,
        f: F,
    ) -> Result<T> {
        let values = params.values()?;
        let mut backend = self.connection.backend.borrow_mut();
        match &mut *backend {
            Backend::Sqlite(db) => {
                let row = self
                    .connection
                    .checked(runtime::run(sqlite::fetch_optional(
                        db,
                        &self.query,
                        values,
                    )))?
                    .ok_or(Error::NoRows)?;
                f(&Row {
                    inner: value::RowData::Sqlite(&row),
                })
            }
            Backend::Postgres(db) => {
                let row = self
                    .connection
                    .checked(runtime::run(postgres::fetch_optional(
                        db,
                        &self.query,
                        values,
                    )))?
                    .ok_or(Error::NoRows)?;
                f(&Row {
                    inner: value::RowData::Postgres(&row),
                })
            }
            Backend::Closed => Err(Error::Conversion("Database closed")),
        }
    }
    #[cfg(test)]
    pub(crate) fn query_map<T, P: Params, F: FnMut(&Row<'_>) -> Result<T>>(
        &mut self,
        params: P,
        mut f: F,
    ) -> Result<std::vec::IntoIter<Result<T>>> {
        let values = params.values()?;
        let mut backend = self.connection.backend.borrow_mut();
        let result = match &mut *backend {
            Backend::Sqlite(db) => self
                .connection
                .checked(runtime::run(sqlite::fetch_all(db, &self.query, values)))?
                .iter()
                .map(|row| {
                    f(&Row {
                        inner: value::RowData::Sqlite(row),
                    })
                })
                .collect::<Vec<_>>(),
            Backend::Postgres(db) => self
                .connection
                .checked(runtime::run(postgres::fetch_all(db, &self.query, values)))?
                .iter()
                .map(|row| {
                    f(&Row {
                        inner: value::RowData::Postgres(row),
                    })
                })
                .collect::<Vec<_>>(),
            Backend::Closed => return Err(Error::Conversion("Database closed")),
        };
        Ok(result.into_iter())
    }
}
