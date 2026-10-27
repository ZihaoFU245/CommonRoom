//! Driver-independent parameters and row decoding.
use super::{Error, Result};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

pub(crate) trait Encode {
    fn encode(&self) -> Result<Value>;
}
impl<T: Encode + ?Sized> Encode for &T {
    fn encode(&self) -> Result<Value> {
        (*self).encode()
    }
}
impl<T: Encode> Encode for Option<T> {
    fn encode(&self) -> Result<Value> {
        self.as_ref().map_or(Ok(Value::Null), Encode::encode)
    }
}
impl Encode for str {
    fn encode(&self) -> Result<Value> {
        Ok(Value::Text(self.into()))
    }
}
impl Encode for String {
    fn encode(&self) -> Result<Value> {
        self.as_str().encode()
    }
}
impl Encode for bool {
    fn encode(&self) -> Result<Value> {
        Ok(Value::Integer(i64::from(*self)))
    }
}
impl Encode for Value {
    fn encode(&self) -> Result<Value> {
        Ok(self.clone())
    }
}
macro_rules! integers {
    ($($ty:ty),*) => {$ (
        impl Encode for $ty {
            fn encode(&self) -> Result<Value> {
                i64::try_from(*self).map(Value::Integer).map_err(|_| Error::Conversion("Integer exceeds database range"))
            }
        }
        impl Decode for $ty {
            fn decode(value: Value) -> Result<Self> {
                match value {
                    Value::Integer(n) => Self::try_from(n).map_err(|_| Error::Conversion("Invalid stored integer")),
                    _ => Err(Error::Conversion("Expected an integer")),
                }
            }
        }
    )*};
}
integers!(u64, u32, usize, i64, i32, i16, u16, u8);

pub(crate) trait Decode: Sized {
    fn decode(value: Value) -> Result<Self>;
}
impl Decode for Value {
    fn decode(value: Value) -> Result<Self> {
        Ok(value)
    }
}
impl Decode for String {
    fn decode(value: Value) -> Result<Self> {
        match value {
            Value::Text(text) => Ok(text),
            _ => Err(Error::Conversion("Expected text")),
        }
    }
}
impl Decode for bool {
    fn decode(value: Value) -> Result<Self> {
        match value {
            Value::Integer(0) => Ok(false),
            Value::Integer(1) => Ok(true),
            _ => Err(Error::Conversion("Expected a boolean")),
        }
    }
}
impl<T: Decode> Decode for Option<T> {
    fn decode(value: Value) -> Result<Self> {
        match value {
            Value::Null => Ok(None),
            value => T::decode(value).map(Some),
        }
    }
}

pub(crate) trait Params {
    fn values(self) -> Result<Vec<Value>>;
}
impl Params for [(); 0] {
    fn values(self) -> Result<Vec<Value>> {
        Ok(Vec::new())
    }
}
macro_rules! arrays {
    ($($len:literal),*) => {$ (
        impl<T: Encode> Params for [T; $len] {
            fn values(self) -> Result<Vec<Value>> { self.iter().map(Encode::encode).collect() }
        }
    )*};
}
arrays!(1, 2, 3, 4, 5, 6, 7, 8);
impl Params for &[&dyn Encode] {
    fn values(self) -> Result<Vec<Value>> {
        self.iter().map(|v| v.encode()).collect()
    }
}
impl<T: Encode> Params for Vec<T> {
    fn values(self) -> Result<Vec<Value>> {
        self.iter().map(Encode::encode).collect()
    }
}

macro_rules! params {
    ($($param:expr),* $(,)?) => {
        &[$(&$param as &dyn $crate::engine::database::Encode),*] as &[&dyn $crate::engine::database::Encode]
    };
}
pub(crate) use params;

pub(crate) struct Row<'a> {
    pub(super) inner: RowData<'a>,
}
pub(super) enum RowData<'a> {
    Sqlite(&'a sqlx::sqlite::SqliteRow),
    Postgres(&'a sqlx::postgres::PgRow),
}
pub(crate) trait ColumnIndex {
    fn index(self) -> usize;
}
impl ColumnIndex for usize {
    fn index(self) -> usize {
        self
    }
}
impl Row<'_> {
    pub(crate) fn get<I: ColumnIndex, T: Decode>(&self, index: I) -> Result<T> {
        use sqlx::{Row as _, TypeInfo as _, ValueRef as _};
        let index = index.index();
        let value = match self.inner {
            RowData::Sqlite(row) => {
                let raw = row.try_get_raw(index)?;
                if raw.is_null() {
                    Value::Null
                } else {
                    match raw.type_info().name() {
                        "INTEGER" => Value::Integer(row.try_get(index)?),
                        "REAL" => Value::Real(row.try_get(index)?),
                        "TEXT" => Value::Text(row.try_get(index)?),
                        "BLOB" => Value::Blob(row.try_get(index)?),
                        "BOOLEAN" => Value::Integer(i64::from(row.try_get::<bool, _>(index)?)),
                        _ => return Err(Error::Conversion("Unsupported SQLite column type")),
                    }
                }
            }
            RowData::Postgres(row) => {
                let raw = row.try_get_raw(index)?;
                if raw.is_null() {
                    Value::Null
                } else {
                    match raw.type_info().name() {
                        "INT8" => Value::Integer(row.try_get(index)?),
                        "INT4" => Value::Integer(i64::from(row.try_get::<i32, _>(index)?)),
                        "INT2" => Value::Integer(i64::from(row.try_get::<i16, _>(index)?)),
                        "BOOL" => Value::Integer(i64::from(row.try_get::<bool, _>(index)?)),
                        "TEXT" | "VARCHAR" | "BPCHAR" | "NAME" => Value::Text(row.try_get(index)?),
                        "BYTEA" => Value::Blob(row.try_get(index)?),
                        "FLOAT8" => Value::Real(row.try_get(index)?),
                        _ => return Err(Error::Conversion("Unsupported PostgreSQL column type")),
                    }
                }
            }
        };
        T::decode(value)
    }
}
