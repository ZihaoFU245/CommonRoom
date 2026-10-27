//! Production operation handles always acquire independent reader/writer leases.
//! Test compatibility can explicitly attach a legacy outer-command session.
use super::{Connection, Database, Result, dispatcher::Lease};
#[cfg(test)]
use super::{Error, runtime};
use std::{
    ops::{Deref, DerefMut},
    sync::Arc,
};

#[derive(Clone)]
pub(crate) struct Handle {
    database: Arc<Database>,
    #[cfg(test)]
    command: Option<Arc<Command>>,
}
#[cfg(test)]
struct Command {
    connection: tokio::sync::Mutex<Connection>,
    database: Arc<Database>,
}
impl Handle {
    pub(crate) fn new(database: Arc<Database>) -> Self {
        Self {
            database,
            #[cfg(test)]
            command: None,
        }
    }
    #[cfg(test)]
    pub(crate) fn command_scope_blocking(&self) -> Result<Self> {
        runtime::run(self.command_scope())
    }
    #[cfg(test)]
    pub(crate) async fn command_scope(&self) -> Result<Self> {
        if self.command.is_some() {
            return Err(Error::Conversion("Nested command scope is not supported"));
        }
        let connection = self.database.acquire().await?.into_connection();
        Ok(Self {
            database: self.database.clone(),
            command: Some(Arc::new(Command {
                connection: tokio::sync::Mutex::new(connection),
                database: self.database.clone(),
            })),
        })
    }
    #[cfg(test)]
    pub(crate) fn lock(&self) -> Result<Session<'_>> {
        runtime::run(self.acquire())
    }
    pub(super) async fn acquire_read(&self) -> Result<Session<'_>> {
        #[cfg(test)]
        if self.command.is_some() {
            return self.acquire().await;
        }
        Ok(Session::Pooled(self.database.acquire_reader().await?))
    }
    pub(super) async fn acquire(&self) -> Result<Session<'_>> {
        #[cfg(test)]
        if let Some(command) = &self.command {
            self.database.ensure_available_async().await?;
            return Ok(Session::Command {
                database: &self.database,
                connection: command.connection.lock().await,
            });
        }
        Ok(Session::Pooled(self.database.acquire().await?))
    }
}
impl Deref for Handle {
    type Target = Database;
    fn deref(&self) -> &Database {
        &self.database
    }
}
pub(crate) enum Session<'a> {
    Pooled(Lease<'a>),
    #[cfg(test)]
    Command {
        database: &'a Database,
        connection: tokio::sync::MutexGuard<'a, Connection>,
    },
}
impl Deref for Session<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        match self {
            Self::Pooled(lease) => lease,
            #[cfg(test)]
            Self::Command { connection, .. } => connection,
        }
    }
}
impl DerefMut for Session<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        match self {
            Self::Pooled(lease) => lease,
            #[cfg(test)]
            Self::Command { connection, .. } => connection,
        }
    }
}
#[cfg(test)]
impl Drop for Session<'_> {
    fn drop(&mut self) {
        if let Self::Command {
            database,
            connection,
        } = self
        {
            if std::thread::panicking() {
                connection.failed.set(true);
            }
            if connection.failed.get() {
                database.invalidate();
            }
        }
    }
}
#[cfg(test)]
impl Drop for Command {
    fn drop(&mut self) {
        let connection = self.connection.get_mut();
        if connection.transaction.get() && !connection.failed.get() {
            connection.rollback_root();
        }
        if connection.failed.get() {
            self.database.invalidate();
        }
    }
}
