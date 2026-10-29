//! SQLite schema validation happens before persistent journal settings change.
use super::super::{Error, Result, SQLITE_SCHEMA_VERSION, message::number};
use sqlx::{Connection as _, Executor as _, SqliteConnection};

pub(in crate::engine::database) async fn initialize(
    db: &mut SqliteConnection,
    state: &str,
    sequence: u64,
) -> Result<()> {
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut *db)
        .await?;
    if version != 0 && version != 19 && version != i64::from(SQLITE_SCHEMA_VERSION) {
        return Err(Error::InvalidRequest(
            "Unsupported database schema; only v19 upgrades and current v20 are supported.",
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
    if version == 19 {
        let mut tx = db.begin().await?;
        let state: String = sqlx::query_scalar("SELECT json FROM state WHERE id=1")
            .fetch_one(&mut *tx)
            .await?;
        let state = upgrade_credentials(&state)?;
        let mut metadata: crate::engine::Data = serde_json::from_str(&state)
            .map_err(|_| Error::InvalidData("Invalid legacy state; upgrade cancelled.".into()))?;
        for (name, json) in sqlx::query_as::<_, (String, String)>(
            "SELECT key,json FROM entities WHERE kind='account'",
        )
        .fetch_all(&mut *tx)
        .await?
        {
            let account = serde_json::from_str(&json).map_err(|_| {
                Error::InvalidData("Invalid legacy account; upgrade cancelled.".into())
            })?;
            metadata.users.insert(name, account);
        }
        crate::bots::models::validate(&metadata).map_err(Error::InvalidData)?;
        sqlx::query("UPDATE state SET json=?1 WHERE id=1")
            .bind(state)
            .execute(&mut *tx)
            .await?;
        (&mut *tx).execute("PRAGMA user_version=20").await?;
        tx.commit().await?;
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
    } else {
        let empty: bool = sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM retention)")
            .fetch_one(&mut *db)
            .await?;
        if empty {
            sqlx::query("INSERT INTO retention SELECT conversation,count(*),min(sequence) FROM messages GROUP BY conversation")
                .execute(&mut *db).await?;
        }
    }
    Ok(())
}

fn upgrade_credentials(state: &str) -> Result<String> {
    use serde_json::Value;
    let invalid =
        || Error::InvalidData("Invalid legacy bot credential metadata; upgrade cancelled.".into());
    let mut state: Value = serde_json::from_str(state).map_err(|_| invalid())?;
    let object = state.as_object_mut().ok_or_else(invalid)?;
    if let Some(bots) = object.get_mut("bots") {
        for registration in bots.as_object_mut().ok_or_else(invalid)?.values_mut() {
            let tokens = registration
                .get_mut("tokens")
                .and_then(Value::as_object_mut)
                .ok_or_else(invalid)?;
            for token in tokens.values_mut() {
                let token = token.as_object_mut().ok_or_else(invalid)?;
                let worker = token
                    .get("operations")
                    .is_some_and(|value| !value.is_null());
                if worker {
                    let operations = token
                        .get("operations")
                        .and_then(Value::as_array)
                        .ok_or_else(invalid)?;
                    if operations.is_empty()
                        || operations.iter().any(|op| {
                            !op.as_str()
                                .is_some_and(|op| crate::bots::models::OPERATIONS.contains(&op))
                        })
                        || !token
                            .get("expires")
                            .is_some_and(|v| v.as_u64().is_some_and(|v| v <= 9007199254740991))
                        || !token.get("conversations").is_some_and(Value::is_array)
                    {
                        return Err(invalid());
                    }
                } else if token
                    .get("conversations")
                    .is_some_and(|value| !value.is_null())
                {
                    return Err(invalid());
                }
                if token.contains_key("kind") {
                    return Err(invalid());
                }
                token.insert(
                    "kind".into(),
                    Value::String(if worker { "worker" } else { "issuer" }.into()),
                );
            }
        }
    }
    serde_json::to_string(&state).map_err(|_| invalid())
}
