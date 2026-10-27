//! Schema/session setup dispatched to native backend implementations.
use super::{Backend, Connection, Error, Handle, Result, postgres, runtime, sqlite};
use sqlx::Connection as _;

pub(crate) const SCHEMA_VERSION: u32 = 19;
pub(crate) const POSTGRES_SCHEMA: &str = include_str!("postgres/schema.sql");

impl Connection {
    pub(crate) fn initialize(&self, state: &str, sequence: u64) -> Result<()> {
        let result = match &mut *self.backend.borrow_mut() {
            Backend::Sqlite(db) => runtime::run(sqlite::bootstrap::initialize(db, state, sequence)),
            Backend::Postgres(db) => {
                runtime::run(postgres::bootstrap::initialize(db, state, sequence))
            }
            Backend::Closed => Err(Error::RestartRequired),
        };
        self.checked(result)
    }
    pub(crate) fn acquire_postgres_owner(&self) -> Result<()> {
        let result = match &mut *self.backend.borrow_mut() {
            Backend::Postgres(db) => runtime::run(postgres::bootstrap::acquire_owner(db)),
            _ => Err(Error::Conversion(
                "PostgreSQL ownership requires PostgreSQL",
            )),
        };
        self.checked(result)
    }
    pub(crate) fn require_empty_postgres(&self) -> Result<()> {
        let result = match &mut *self.backend.borrow_mut() {
            Backend::Postgres(db) => runtime::run(postgres::bootstrap::require_empty(db)),
            _ => Err(Error::Conversion("PostgreSQL target requires PostgreSQL")),
        };
        self.checked(result)
    }
    pub(crate) async fn checkpoint_async(&mut self) -> Result<()> {
        let result = async {
            match self.backend.get_mut() {
                Backend::Sqlite(db) => {
                    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
                        .execute(&mut **db)
                        .await?;
                    Ok(())
                }
                Backend::Postgres(db) => Ok(db.ping().await?),
                Backend::Closed => Err(Error::RestartRequired),
            }
        }
        .await;
        self.checked(result)
    }
}

impl Handle {
    pub(crate) async fn checkpoint(&self) -> Result<()> {
        self.ensure_available_async().await?;
        self.acquire().await?.checkpoint_async().await
    }
}
