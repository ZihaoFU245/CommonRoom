//! SQLite-specific SQLx execution. No PostgreSQL SQL is translated here.
use super::Error;
use super::{Result, Value};
use sqlx::{
    Connection as _, Sqlite, SqliteConnection,
    sqlite::{SqliteConnectOptions, SqliteRow},
};
use std::path::Path;

pub(super) fn options(path: &Path, readonly: bool) -> Result<SqliteConnectOptions> {
    let location = path.to_string_lossy();
    let options = if let Some(uri) = location.strip_prefix("file:") {
        let options = uri.parse::<SqliteConnectOptions>()?;
        options.filename(location.split('?').next().unwrap_or(location.as_ref()))
    } else {
        SqliteConnectOptions::new().filename(path)
    };
    Ok(options
        .read_only(readonly)
        .create_if_missing(!readonly)
        .foreign_keys(false)
        .busy_timeout(std::time::Duration::from_secs(5)))
}
pub(super) async fn open(path: &Path, readonly: bool) -> Result<SqliteConnection> {
    Ok(SqliteConnection::connect_with(&options(path, readonly)?).await?)
}
fn query<'a>(
    sql: &'a str,
    values: Vec<Value>,
) -> sqlx::query::Query<'a, Sqlite, sqlx::sqlite::SqliteArguments<'a>> {
    let mut query = sqlx::query(sql);
    for value in values {
        query = match value {
            Value::Null => query.bind(None::<String>),
            Value::Integer(n) => query.bind(n),
            Value::Real(n) => query.bind(n),
            Value::Text(s) => query.bind(s),
            Value::Blob(b) => query.bind(b),
        };
    }
    query
}
pub(super) async fn execute(
    db: &mut SqliteConnection,
    sql: &str,
    values: Vec<Value>,
) -> Result<(u64, Option<u64>)> {
    let affected = query(sql, values).execute(&mut *db).await?.rows_affected();
    let total: i64 = sqlx::query_scalar("SELECT total_changes()")
        .fetch_one(db)
        .await?;
    Ok((
        affected,
        Some(u64::try_from(total).map_err(|_| Error::Conversion("Invalid SQLite change count"))?),
    ))
}
pub(super) async fn fetch_optional(
    db: &mut SqliteConnection,
    sql: &str,
    values: Vec<Value>,
) -> Result<Option<SqliteRow>> {
    Ok(query(sql, values).fetch_optional(db).await?)
}
pub(super) async fn fetch_all(
    db: &mut SqliteConnection,
    sql: &str,
    values: Vec<Value>,
) -> Result<Vec<SqliteRow>> {
    Ok(query(sql, values).fetch_all(db).await?)
}
