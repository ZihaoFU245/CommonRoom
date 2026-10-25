//! Synchronous database boundary. SQL dialects are explicit at the call site;
//! neither backend uses a shadow database or runtime SQL translation.
mod projection;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub(crate) mod tests;
mod value;
pub(crate) use projection::{entity_projection, message_projection};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::Path,
};
pub(crate) use value::{Encode, Params, Row, Value, params};

#[derive(Debug)]
pub(crate) enum Error {
    Sqlite(rusqlite::Error),
    Postgres(postgres::Error),
    Conversion(&'static str),
    NoRows,
}
pub(crate) type Result<T> = std::result::Result<T, Error>;
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sqlite(error) => write!(f, "SQLite operation failed: {error}"),
            // PostgreSQL detail/context can contain private row values.
            Self::Postgres(error) => write!(
                f,
                "PostgreSQL operation failed ({})",
                error.code().map_or("connection", |code| code.code())
            ),
            Self::Conversion(message) => f.write_str(message),
            Self::NoRows => f.write_str("Database row not found"),
        }
    }
}
impl std::error::Error for Error {}
impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}
impl From<postgres::Error> for Error {
    fn from(error: postgres::Error) -> Self {
        Self::Postgres(error)
    }
}
pub(crate) trait OptionalExtension<T> {
    fn optional(self) -> Result<Option<T>>;
}
impl<T> OptionalExtension<T> for Result<T> {
    fn optional(self) -> Result<Option<T>> {
        match self {
            Err(Error::NoRows) | Err(Error::Sqlite(rusqlite::Error::QueryReturnedNoRows)) => {
                Ok(None)
            }
            result => result.map(Some),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Sql<'a> {
    sqlite: &'a str,
    postgres: &'a str,
}
pub(crate) const fn sql<'a>(sqlite: &'a str, postgres: &'a str) -> Sql<'a> {
    Sql { sqlite, postgres }
}
#[cfg(test)]
impl std::fmt::Display for Sql<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.sqlite)
    }
}
impl<'a> From<&'a str> for Sql<'a> {
    fn from(query: &'a str) -> Self {
        sql(query, query)
    }
}
impl<'a> From<&'a String> for Sql<'a> {
    fn from(query: &'a String) -> Self {
        query.as_str().into()
    }
}

pub(crate) struct Connection {
    backend: Backend,
    readonly: bool,
    changes: Cell<u64>,
    transaction: Cell<bool>,
    command_root: Cell<bool>,
    serial: Cell<u64>,
    failed: Cell<bool>,
}
enum Backend {
    Closed,
    Sqlite(rusqlite::Connection),
    Postgres {
        client: RefCell<Box<postgres::Client>>,
        statements: RefCell<HashMap<String, postgres::Statement>>,
    },
}
impl Connection {
    /// Stream an offline transfer without retaining the source table in memory.
    /// Identifiers are supplied only by the migration's static table manifest.
    pub(crate) fn copy_from(
        &self,
        table: &str,
        columns: &[&str],
        produce: impl FnOnce(&mut dyn FnMut(Vec<Value>) -> Result<()>) -> Result<()>,
    ) -> Result<u64> {
        self.ensure_available()?;
        let Backend::Postgres { client, .. } = &self.backend else {
            return Err(Error::Conversion("COPY requires PostgreSQL"));
        };
        let names = columns.join(",");
        let placeholders = (1..=columns.len())
            .map(|i| format!("${i}"))
            .collect::<Vec<_>>()
            .join(",");
        let mut client = client.borrow_mut();
        let insert = client
            .prepare(&format!(
                "INSERT INTO {table}({names}) VALUES({placeholders})"
            ))
            .map_err(|error| self.postgres_error(error))?;
        let types = insert.params();
        let sink = client
            .copy_in(&format!("COPY {table}({names}) FROM STDIN BINARY"))
            .map_err(|error| self.postgres_error(error))?;
        let mut writer = postgres::binary_copy::BinaryCopyInWriter::new(sink, types);
        produce(&mut |values| {
            let values = value::postgres_values(values, types)?;
            let refs: Vec<_> = values.iter().map(|v| v.as_ref()).collect();
            writer
                .write(&refs)
                .map_err(|error| self.postgres_error(error))?;
            Ok(())
        })?;
        let count = writer
            .finish()
            .map_err(|error| self.postgres_error(error))?;
        self.changes.set(self.changes.get().wrapping_add(count));
        Ok(count)
    }

    pub(crate) fn visit_rows(
        &self,
        query: &str,
        mut visit: impl FnMut(&Row<'_>) -> Result<()>,
    ) -> Result<()> {
        self.ensure_available()?;
        match &self.backend {
            Backend::Closed => return Err(Error::Conversion("Database closed")),
            Backend::Sqlite(db) => {
                let mut statement = db.prepare(query)?;
                let mut rows = statement.query([])?;
                while let Some(row) = rows.next()? {
                    visit(&Row {
                        inner: value::RowData::Sqlite(row),
                    })?;
                }
            }
            Backend::Postgres { client, .. } => {
                use postgres::fallible_iterator::FallibleIterator;
                let mut client = client.borrow_mut();
                let mut rows = client
                    .query_raw(
                        query,
                        std::iter::empty::<&(dyn postgres::types::ToSql + Sync)>(),
                    )
                    .map_err(|error| self.postgres_error(error))?;
                while let Some(row) = rows.next().map_err(|error| self.postgres_error(error))? {
                    visit(&Row {
                        inner: value::RowData::Postgres(&row),
                    })?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn open(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self::sqlite(rusqlite::Connection::open(path)?))
    }
    pub(crate) fn open_with_flags(
        path: impl AsRef<Path>,
        flags: rusqlite::OpenFlags,
    ) -> Result<Self> {
        Ok(Self::sqlite(rusqlite::Connection::open_with_flags(
            path, flags,
        )?))
    }
    fn sqlite(connection: rusqlite::Connection) -> Self {
        Self {
            backend: Backend::Sqlite(connection),
            readonly: false,
            changes: Cell::new(0),
            transaction: Cell::new(false),
            command_root: Cell::new(false),
            serial: Cell::new(0),
            failed: Cell::new(false),
        }
    }
    pub(crate) fn postgres(url: &str, readonly: bool) -> Result<Self> {
        let tls = native_tls::TlsConnector::builder()
            .build()
            .map_err(|_| Error::Conversion("Cannot initialize database TLS"))?;
        let mut config: postgres::Config = url.parse()?;
        config.connect_timeout(std::time::Duration::from_secs(5));
        let mut client = config.connect(postgres_native_tls::MakeTlsConnector::new(tls))?;
        client.batch_execute("SET statement_timeout='10s'; SET lock_timeout='2s'; SET idle_in_transaction_session_timeout='30s';")?;
        if readonly {
            client.batch_execute("SET default_transaction_read_only=on;")?;
        }
        Ok(Self {
            backend: Backend::Postgres {
                client: RefCell::new(Box::new(client)),
                statements: RefCell::new(HashMap::new()),
            },
            readonly,
            changes: Cell::new(0),
            transaction: Cell::new(false),
            command_root: Cell::new(false),
            serial: Cell::new(0),
            failed: Cell::new(false),
        })
    }
    pub(crate) fn is_postgres(&self) -> bool {
        matches!(self.backend, Backend::Postgres { .. })
    }
    pub(crate) fn ensure_available(&self) -> Result<()> {
        let closed = match &self.backend {
            Backend::Closed => true,
            Backend::Postgres { client, .. } => client.borrow().is_closed(),
            Backend::Sqlite(_) => false,
        };
        if self.failed.get() || closed {
            Err(Error::Conversion(
                "Database connection or transaction lost; restart the service",
            ))
        } else {
            Ok(())
        }
    }
    fn postgres_error(&self, error: postgres::Error) -> Error {
        // A synchronous driver's channel may not report `is_closed` until its
        // runtime is polled again. A FATAL response or transport failure already
        // proves that this engine's owning session cannot be reused.
        if error.code().is_none()
            || error
                .as_db_error()
                .is_some_and(|db| matches!(db.severity(), "FATAL" | "PANIC"))
        {
            self.failed.set(true);
        }
        Error::Postgres(error)
    }
    fn selected<'a>(&self, query: Sql<'a>) -> &'a str {
        if self.is_postgres() {
            query.postgres
        } else {
            query.sqlite
        }
    }
    pub(crate) fn prepare<'c, 'q>(&'c self, query: impl Into<Sql<'q>>) -> Result<Statement<'c>> {
        self.ensure_available()?;
        let query = self.selected(query.into());
        let inner = match &self.backend {
            Backend::Closed => return Err(Error::Conversion("Database closed")),
            Backend::Sqlite(db) => StatementData::Sqlite(db.prepare_cached(query)?),
            Backend::Postgres { client, statements } => {
                let cached = statements.borrow().get(query).cloned();
                let statement = if let Some(statement) = cached {
                    statement
                } else {
                    let statement = client
                        .borrow_mut()
                        .prepare(query)
                        .map_err(|error| self.postgres_error(error))?;
                    // Bound the cache; ad-hoc maintenance queries cannot grow it indefinitely.
                    let mut statements = statements.borrow_mut();
                    if statements.len() >= 256 {
                        statements.clear();
                    }
                    statements.insert(query.into(), statement.clone());
                    statement
                };
                StatementData::Postgres(statement)
            }
        };
        Ok(Statement {
            connection: self,
            inner,
        })
    }
    pub(crate) fn prepare_cached<'c, 'q>(
        &'c self,
        query: impl Into<Sql<'q>>,
    ) -> Result<Statement<'c>> {
        self.prepare(query)
    }
    pub(crate) fn execute<'q>(
        &self,
        query: impl Into<Sql<'q>>,
        params: impl Params,
    ) -> Result<usize> {
        self.prepare(query)?.execute(params)
    }
    pub(crate) fn query_row<'q, T, P: Params, F: FnOnce(&Row<'_>) -> Result<T>>(
        &self,
        query: impl Into<Sql<'q>>,
        params: P,
        f: F,
    ) -> Result<T> {
        self.prepare(query)?.query_row(params, f)
    }
    pub(crate) fn execute_batch(&self, query: &str) -> Result<()> {
        self.ensure_available()?;
        match &self.backend {
            Backend::Closed => Err(Error::Conversion("Database closed")),
            Backend::Sqlite(db) => db.execute_batch(query).map_err(Error::from),
            Backend::Postgres { client, .. } => client
                .borrow_mut()
                .batch_execute(query)
                .map_err(|error| self.postgres_error(error)),
        }
    }
    pub(crate) fn total_changes(&self) -> u64 {
        match &self.backend {
            Backend::Closed => 0,
            Backend::Sqlite(db) => db.total_changes(),
            Backend::Postgres { .. } => self.changes.get(),
        }
    }
    pub(crate) fn is_autocommit(&self) -> bool {
        match &self.backend {
            Backend::Closed => true,
            Backend::Sqlite(db) => db.is_autocommit(),
            Backend::Postgres { .. } => !self.transaction.get(),
        }
    }
    pub(crate) fn busy_timeout(&self, timeout: std::time::Duration) -> Result<()> {
        match &self.backend {
            Backend::Closed => Err(Error::Conversion("Database closed")),
            Backend::Sqlite(db) => db.busy_timeout(timeout).map_err(Error::from),
            Backend::Postgres { .. } => {
                Err(Error::Conversion("busy_timeout applies only to SQLite"))
            }
        }
    }
    pub(crate) fn pragma_update(&self, _: Option<&str>, name: &str, value: u32) -> Result<()> {
        match &self.backend {
            Backend::Closed => Err(Error::Conversion("Database closed")),
            Backend::Sqlite(db) => db.pragma_update(None, name, value).map_err(Error::from),
            Backend::Postgres { .. } => {
                Err(Error::Conversion("SQLite pragma requested on PostgreSQL"))
            }
        }
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
    pub(crate) fn savepoint(&mut self) -> Result<Transaction<'_>> {
        let root = self.is_autocommit();
        if root && self.is_postgres() {
            self.execute_batch("BEGIN")?;
        }
        self.transaction.set(true);
        let serial = self.serial.get().wrapping_add(1);
        self.serial.set(serial);
        let name = format!("chat_{serial}");
        if let Err(error) = self.execute_batch(&format!("SAVEPOINT {name}")) {
            if root && self.is_postgres() {
                self.rollback_root();
            }
            return Err(error);
        }
        Ok(Transaction {
            connection: self,
            name: Some(name),
            root,
            finished: false,
        })
    }
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
    pub(crate) fn finish_command(&self) -> Result<()> {
        self.execute_batch("RELEASE bot_command_receipt")?;
        if self.is_postgres() && self.command_root.get() {
            self.commit_root()?;
        } else {
            self.transaction.set(!self.command_root.get());
        }
        Ok(())
    }
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
        behavior: rusqlite::TransactionBehavior,
    ) -> Result<Transaction<'_>> {
        let begin = match behavior {
            rusqlite::TransactionBehavior::Immediate => "BEGIN IMMEDIATE",
            rusqlite::TransactionBehavior::Exclusive => "BEGIN EXCLUSIVE",
            _ => "BEGIN",
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
        let backend = std::mem::replace(&mut self.backend, Backend::Closed);
        // The last App/HistoryStore owner can be released by an async socket or
        // shutdown task. The synchronous driver's runtime must close off Tokio.
        if matches!(backend, Backend::Postgres { .. })
            && tokio::runtime::Handle::try_current().is_ok()
            && std::thread::spawn(move || drop(backend)).join().is_err()
        {
            tracing::error!("Database shutdown worker failed");
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
            }
        } else {
            self.connection.rollback_root();
        }
    }
}

pub(crate) struct Statement<'a> {
    connection: &'a Connection,
    inner: StatementData<'a>,
}
enum StatementData<'a> {
    Sqlite(rusqlite::CachedStatement<'a>),
    Postgres(postgres::Statement),
}
impl Statement<'_> {
    #[cfg(test)]
    pub(crate) fn parameter_count(&self) -> usize {
        match &self.inner {
            StatementData::Sqlite(statement) => statement.parameter_count(),
            StatementData::Postgres(statement) => statement.params().len(),
        }
    }
    #[cfg(test)]
    pub(crate) fn exists(&mut self, params: impl Params) -> Result<bool> {
        self.query_row(params, |_| Ok(()))
            .optional()
            .map(|row| row.is_some())
    }
    pub(crate) fn execute(&mut self, params: impl Params) -> Result<usize> {
        let values = params.values()?;
        match &mut self.inner {
            StatementData::Sqlite(statement) => {
                Ok(statement.execute(rusqlite::params_from_iter(value::sqlite_values(values)))?)
            }
            StatementData::Postgres(statement) => {
                let values = value::postgres_values(values, statement.params())?;
                let refs: Vec<_> = values.iter().map(|v| v.as_ref()).collect();
                let Backend::Postgres { client, .. } = &self.connection.backend else {
                    return Err(Error::Conversion("Statement backend mismatch"));
                };
                let count = client
                    .borrow_mut()
                    .execute(statement, &refs)
                    .map_err(|error| self.connection.postgres_error(error))?;
                self.connection
                    .changes
                    .set(self.connection.changes.get().wrapping_add(count));
                usize::try_from(count)
                    .map_err(|_| Error::Conversion("Affected row count exceeds platform range"))
            }
        }
    }
    pub(crate) fn query_row<T, P: Params, F: FnOnce(&Row<'_>) -> Result<T>>(
        &mut self,
        params: P,
        f: F,
    ) -> Result<T> {
        let values = params.values()?;
        match &mut self.inner {
            StatementData::Sqlite(statement) => {
                let mut rows =
                    statement.query(rusqlite::params_from_iter(value::sqlite_values(values)))?;
                let row = rows.next()?.ok_or(Error::NoRows)?;
                f(&Row {
                    inner: value::RowData::Sqlite(row),
                })
            }
            StatementData::Postgres(statement) => {
                let values = value::postgres_values(values, statement.params())?;
                let refs: Vec<_> = values.iter().map(|v| v.as_ref()).collect();
                let Backend::Postgres { client, .. } = &self.connection.backend else {
                    return Err(Error::Conversion("Statement backend mismatch"));
                };
                let row = client
                    .borrow_mut()
                    .query_opt(statement, &refs)
                    .map_err(|error| self.connection.postgres_error(error))?
                    .ok_or(Error::NoRows)?;
                f(&Row {
                    inner: value::RowData::Postgres(&row),
                })
            }
        }
    }
    pub(crate) fn query_map<T, P: Params, F: FnMut(&Row<'_>) -> Result<T>>(
        &mut self,
        params: P,
        mut f: F,
    ) -> Result<std::vec::IntoIter<Result<T>>> {
        let values = params.values()?;
        let mut result = Vec::new();
        match &mut self.inner {
            StatementData::Sqlite(statement) => {
                let mut rows =
                    statement.query(rusqlite::params_from_iter(value::sqlite_values(values)))?;
                while let Some(row) = rows.next()? {
                    result.push(f(&Row {
                        inner: value::RowData::Sqlite(row),
                    }));
                }
            }
            StatementData::Postgres(statement) => {
                let values = value::postgres_values(values, statement.params())?;
                let refs: Vec<_> = values.iter().map(|v| v.as_ref()).collect();
                let Backend::Postgres { client, .. } = &self.connection.backend else {
                    return Err(Error::Conversion("Statement backend mismatch"));
                };
                for row in client
                    .borrow_mut()
                    .query(statement, &refs)
                    .map_err(|error| self.connection.postgres_error(error))?
                {
                    result.push(f(&Row {
                        inner: value::RowData::Postgres(&row),
                    }));
                }
            }
        }
        Ok(result.into_iter())
    }
}
