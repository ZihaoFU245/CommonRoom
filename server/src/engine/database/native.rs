//! A transaction session can own a standalone connection or lease a native pool.
use super::{Error, Result};
use sqlx::{Connection as _, Database, pool::PoolConnection};

pub(super) enum Native<D: Database> {
    Owned(D::Connection),
    #[allow(dead_code)] // Activated by the next dispatcher checkpoint.
    Pooled(PoolConnection<D>),
}
impl<D: Database> std::ops::Deref for Native<D> {
    type Target = D::Connection;
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Owned(db) => db,
            Self::Pooled(db) => db,
        }
    }
}
impl<D: Database> std::ops::DerefMut for Native<D> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Owned(db) => db,
            Self::Pooled(db) => db,
        }
    }
}
impl<D: Database> Native<D> {
    pub(super) fn release(self, failed: bool) -> Result<()> {
        match self {
            Self::Owned(db) => super::runtime::run(async { db.close().await.map_err(Error::from) }),
            Self::Pooled(mut db) => super::runtime::with_context(move || {
                if failed {
                    db.close_on_drop();
                }
                // SQLx returns the lease (and rolls back dropped transactions)
                // asynchronously. Native async operations must never block an
                // executor thread while returning a connection to its pool.
                drop(db);
            }),
        }
    }
}
