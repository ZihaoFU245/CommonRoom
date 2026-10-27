//! PostgreSQL-specific SQLx execution, native types and streaming COPY.
use super::{Error, Result, Value, runtime};
use sqlx::{
    Connection as _, Executor as _, PgConnection, Postgres, Statement as _, TypeInfo as _,
    postgres::{PgArguments, PgConnectOptions, PgRow, PgTypeInfo},
};

pub(super) async fn open(url: &str, readonly: bool) -> Result<PgConnection> {
    let options: PgConnectOptions = url.parse()?;
    let mut db = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        PgConnection::connect_with(&options),
    )
    .await
    .map_err(|_| Error::Conversion("Database connection deadline exceeded"))??;
    db.execute("SET statement_timeout='10s'; SET lock_timeout='2s'; SET idle_in_transaction_session_timeout='30s';").await?;
    if readonly {
        db.execute("SET default_transaction_read_only=on").await?;
    }
    Ok(db)
}
async fn query<'a>(
    db: &mut PgConnection,
    sql: &'a str,
    values: Vec<Value>,
) -> Result<sqlx::query::Query<'a, Postgres, PgArguments>> {
    let statement = db.prepare(sql).await?;
    let types = match statement.parameters() {
        Some(sqlx::Either::Left(types)) => types,
        _ => return Err(Error::Conversion("Missing PostgreSQL parameter types")),
    };
    if types.len() != values.len() {
        return Err(Error::Conversion("Incorrect parameter count"));
    }
    let mut query = sqlx::query(sql);
    for (value, ty) in values.into_iter().zip(types) {
        query =
            match (value, ty.name()) {
                (Value::Null, "INT8") => query.bind(None::<i64>),
                (Value::Null, "INT4") => query.bind(None::<i32>),
                (Value::Null, "INT2") => query.bind(None::<i16>),
                (Value::Null, "BOOL") => query.bind(None::<bool>),
                (Value::Null, "BYTEA") => query.bind(None::<Vec<u8>>),
                (Value::Null, "FLOAT8") => query.bind(None::<f64>),
                (Value::Null, "JSONB") => query.bind(None::<sqlx::types::Json<serde_json::Value>>),
                (Value::Null, _) => query.bind(None::<String>),
                (Value::Integer(n), "INT4") => query
                    .bind(i32::try_from(n).map_err(|_| Error::Conversion("Integer out of range"))?),
                (Value::Integer(n), "INT2") => query
                    .bind(i16::try_from(n).map_err(|_| Error::Conversion("Integer out of range"))?),
                (Value::Integer(n), "BOOL") if n == 0 || n == 1 => query.bind(n == 1),
                (Value::Integer(n), _) => query.bind(n),
                (Value::Text(s), "JSONB") => query.bind(sqlx::types::Json(
                    serde_json::from_str::<serde_json::Value>(&s)
                        .map_err(|_| Error::Conversion("Invalid JSONB projection"))?,
                )),
                (Value::Text(s), _) => query.bind(s),
                (Value::Real(n), _) => query.bind(n),
                (Value::Blob(b), _) => query.bind(b),
            };
    }
    Ok(query)
}
pub(super) async fn execute(
    db: &mut PgConnection,
    sql: &str,
    values: Vec<Value>,
) -> Result<(u64, Option<u64>)> {
    let query = query(db, sql, values).await?;
    Ok((query.execute(db).await?.rows_affected(), None))
}
pub(super) async fn fetch_optional(
    db: &mut PgConnection,
    sql: &str,
    values: Vec<Value>,
) -> Result<Option<PgRow>> {
    let query = query(db, sql, values).await?;
    Ok(query.fetch_optional(db).await?)
}
pub(super) async fn fetch_all(
    db: &mut PgConnection,
    sql: &str,
    values: Vec<Value>,
) -> Result<Vec<PgRow>> {
    let query = query(db, sql, values).await?;
    Ok(query.fetch_all(db).await?)
}

// The offline transfer streams binary COPY one row at a time. This is wire
// encoding for COPY, not SQL translation; SQLx owns the protocol session.
fn copy_record(values: Vec<Value>, types: &[PgTypeInfo]) -> Result<Vec<u8>> {
    if values.len() != types.len() {
        return Err(Error::Conversion("Incorrect COPY column count"));
    }
    let columns =
        i16::try_from(values.len()).map_err(|_| Error::Conversion("Too many COPY columns"))?;
    let mut record = columns.to_be_bytes().to_vec();
    for (value, ty) in values.into_iter().zip(types) {
        let bytes = match (value, ty.name()) {
            (Value::Null, _) => {
                record.extend_from_slice(&(-1_i32).to_be_bytes());
                continue;
            }
            (Value::Integer(n), "INT8") => n.to_be_bytes().to_vec(),
            (Value::Integer(n), "INT4") => i32::try_from(n)
                .map_err(|_| Error::Conversion("Integer out of range"))?
                .to_be_bytes()
                .to_vec(),
            (Value::Integer(n), "INT2") => i16::try_from(n)
                .map_err(|_| Error::Conversion("Integer out of range"))?
                .to_be_bytes()
                .to_vec(),
            (Value::Integer(n), "BOOL") if n == 0 || n == 1 => vec![u8::from(n == 1)],
            (Value::Text(s), "JSONB") => {
                let mut b = vec![1];
                b.extend_from_slice(s.as_bytes());
                b
            }
            (Value::Text(s), "TEXT" | "VARCHAR" | "BPCHAR" | "NAME") => s.into_bytes(),
            (Value::Real(n), "FLOAT8") => n.to_be_bytes().to_vec(),
            (Value::Blob(b), "BYTEA") => b,
            _ => return Err(Error::Conversion("Unsupported COPY value type")),
        };
        let length =
            i32::try_from(bytes.len()).map_err(|_| Error::Conversion("COPY value too large"))?;
        record.extend_from_slice(&length.to_be_bytes());
        record.extend_from_slice(&bytes);
    }
    Ok(record)
}
pub(super) fn copy_from(
    db: &mut PgConnection,
    table: &str,
    columns: &[&str],
    produce: impl FnOnce(&mut dyn FnMut(Vec<Value>) -> Result<()>) -> Result<()>,
) -> Result<u64> {
    let names = columns.join(",");
    let placeholders = (1..=columns.len())
        .map(|i| format!("${i}"))
        .collect::<Vec<_>>()
        .join(",");
    let types = runtime::run(async {
        let insert = format!("INSERT INTO {table}({names}) VALUES({placeholders})");
        let statement = db.prepare(&insert).await?;
        match statement.parameters() {
            Some(sqlx::Either::Left(types)) => Ok(types.to_vec()),
            _ => Err(Error::Conversion("Missing COPY parameter types")),
        }
    })?;
    let command = format!("COPY {table}({names}) FROM STDIN BINARY");
    let mut copy = runtime::run(async { Ok(db.copy_in_raw(&command).await?) })?;
    runtime::run(async {
        copy.send(&b"PGCOPY\n\xff\r\n\0\0\0\0\0\0\0\0\0"[..])
            .await?;
        Ok(())
    })?;
    let produced = produce(&mut |values| {
        let bytes = copy_record(values, &types)?;
        runtime::run(async {
            copy.send(bytes).await?;
            Ok(())
        })
    });
    if let Err(error) = produced {
        runtime::run(async {
            copy.abort("Source transfer failed").await?;
            Ok(())
        })?;
        return Err(error);
    }
    runtime::run(async {
        copy.send(&(-1_i16).to_be_bytes()[..]).await?;
        Ok(copy.finish().await?)
    })
}
