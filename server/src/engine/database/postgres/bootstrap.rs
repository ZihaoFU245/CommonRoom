//! PostgreSQL owns advisory ownership and transactional schema initialization.
use super::super::{Error, POSTGRES_SCHEMA, POSTGRES_SCHEMA_VERSION, Result, message::number};
use sqlx::{Connection as _, Executor as _, PgConnection};

pub(in crate::engine::database) async fn acquire_owner(db: &mut PgConnection) -> Result<()> {
    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(1128808788::bigint)")
        .fetch_one(db)
        .await?;
    if !acquired {
        return Err(Error::Conflict(
            "Another server is already using this PostgreSQL database.",
        ));
    }
    Ok(())
}
pub(in crate::engine::database) async fn require_empty(db: &mut PgConnection) -> Result<()> {
    let occupied: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=current_schema() AND c.relkind IN ('r','p','v','m','S','f'))")
        .fetch_one(db).await?;
    if occupied {
        return Err(Error::InvalidRequest(
            "PostgreSQL target schema must be empty; existing unversioned objects are not supported.",
        ));
    }
    Ok(())
}
pub(in crate::engine::database) async fn initialize(
    db: &mut PgConnection,
    state: &str,
    sequence: u64,
) -> Result<()> {
    acquire_owner(db).await?;
    let exists: bool = sqlx::query_scalar("SELECT to_regclass('chat_schema') IS NOT NULL")
        .fetch_one(&mut *db)
        .await?;
    if exists {
        let version: i64 = sqlx::query_scalar("SELECT version FROM chat_schema WHERE id=1")
            .fetch_one(&mut *db)
            .await?;
        if version != i64::from(POSTGRES_SCHEMA_VERSION) {
            return Err(Error::InvalidRequest(
                "Unsupported PostgreSQL schema; only current v1 is supported.",
            ));
        }
        // A data-only restore with triggers disabled can omit derived rows.
        // Avoid scanning history on normal startup, where retention is present.
        let empty: bool = sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM retention)")
            .fetch_one(&mut *db)
            .await?;
        if empty {
            let mut tx = db.begin().await?;
            // Offline startup maintenance may scan the entire retained history.
            // Runtime statement deadlines must not make large restores unbootable.
            sqlx::query("SET LOCAL statement_timeout=0")
                .execute(&mut *tx)
                .await?;
            sqlx::query("INSERT INTO retention SELECT conversation,count(*),min(sequence) FROM messages GROUP BY conversation")
                .execute(&mut *tx).await?;
            tx.commit().await?;
        }
    } else {
        require_empty(db).await?;
        let mut tx = db.begin().await?;
        (&mut *tx).execute(POSTGRES_SCHEMA).await?;
        sqlx::query("INSERT INTO state(id,json) VALUES(1,$1)")
            .bind(state)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO counters VALUES(1,$1)")
            .bind(number(sequence)?)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
    }
    Ok(())
}
