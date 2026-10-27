//! SQLite schema validation happens before persistent journal settings change.
use super::super::{Error, Result, SCHEMA_VERSION, message::number};
use sqlx::{Connection as _, Executor as _, SqliteConnection};

pub(in crate::engine::database) async fn initialize(
    db: &mut SqliteConnection,
    state: &str,
    sequence: u64,
) -> Result<()> {
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut *db)
        .await?;
    if version != 0 && version != i64::from(SCHEMA_VERSION) {
        return Err(Error::InvalidRequest(
            "Unsupported database schema; only current v19 is supported.",
        ));
    }
    if version == 0 {
        let existing: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%')")
            .fetch_one(&mut *db).await?;
        if existing {
            return Err(Error::InvalidRequest(
                "Unversioned existing databases are not supported.",
            ));
        }
    }
    (&mut *db)
        .execute("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
        .await?;
    if version == 0 {
        let mut tx = db.begin().await?;
        (&mut *tx).execute(include_str!("schema.sql")).await?;
        sqlx::query("INSERT INTO state(id,json) VALUES(1,?1)")
            .bind(state)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO counters VALUES(1,?1)")
            .bind(number(sequence)?)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
    }
    Ok(())
}
