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
    Sqlite(&'a rusqlite::Row<'a>),
    Postgres(&'a postgres::Row),
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
        let index = index.index();
        let value = match self.inner {
            RowData::Sqlite(row) => match row.get_ref(index)? {
                rusqlite::types::ValueRef::Null => Value::Null,
                rusqlite::types::ValueRef::Integer(n) => Value::Integer(n),
                rusqlite::types::ValueRef::Real(n) => Value::Real(n),
                rusqlite::types::ValueRef::Text(s) => Value::Text(
                    std::str::from_utf8(s)
                        .map_err(|_| Error::Conversion("Invalid UTF-8"))?
                        .into(),
                ),
                rusqlite::types::ValueRef::Blob(b) => Value::Blob(b.into()),
            },
            RowData::Postgres(row) => {
                use postgres::types::Type;
                let ty = row
                    .columns()
                    .get(index)
                    .ok_or(Error::Conversion("Missing column"))?
                    .type_();
                match *ty {
                    Type::INT8 => row.try_get::<_, Option<i64>>(index)?.map(Value::Integer),
                    Type::INT4 => row
                        .try_get::<_, Option<i32>>(index)?
                        .map(|n| Value::Integer(i64::from(n))),
                    Type::INT2 => row
                        .try_get::<_, Option<i16>>(index)?
                        .map(|n| Value::Integer(i64::from(n))),
                    Type::BOOL => row
                        .try_get::<_, Option<bool>>(index)?
                        .map(|n| Value::Integer(i64::from(n))),
                    Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME => {
                        row.try_get::<_, Option<String>>(index)?.map(Value::Text)
                    }
                    Type::BYTEA => row.try_get::<_, Option<Vec<u8>>>(index)?.map(Value::Blob),
                    Type::FLOAT8 => row.try_get::<_, Option<f64>>(index)?.map(Value::Real),
                    _ => return Err(Error::Conversion("Unsupported PostgreSQL column type")),
                }
                .unwrap_or(Value::Null)
            }
        };
        T::decode(value)
    }
}

pub(super) fn sqlite_values(values: Vec<Value>) -> Vec<rusqlite::types::Value> {
    values
        .into_iter()
        .map(|value| match value {
            Value::Null => rusqlite::types::Value::Null,
            Value::Integer(n) => rusqlite::types::Value::Integer(n),
            Value::Real(n) => rusqlite::types::Value::Real(n),
            Value::Text(s) => rusqlite::types::Value::Text(s),
            Value::Blob(b) => rusqlite::types::Value::Blob(b),
        })
        .collect()
}

pub(super) fn postgres_values(
    values: Vec<Value>,
    types: &[postgres::types::Type],
) -> Result<Vec<Box<dyn postgres::types::ToSql + Sync>>> {
    use postgres::types::Type;
    if values.len() != types.len() {
        return Err(Error::Conversion("Incorrect parameter count"));
    }
    values
        .into_iter()
        .zip(types)
        .map(|(value, ty)| {
            let result: Box<dyn postgres::types::ToSql + Sync> = match (value, ty) {
                (Value::Null, &Type::INT8) => Box::new(None::<i64>),
                (Value::Null, &Type::INT4) => Box::new(None::<i32>),
                (Value::Null, &Type::INT2) => Box::new(None::<i16>),
                (Value::Null, &Type::BOOL) => Box::new(None::<bool>),
                (Value::Null, &Type::BYTEA) => Box::new(None::<Vec<u8>>),
                (Value::Null, _) => Box::new(None::<String>),
                (Value::Integer(n), &Type::INT4) => Box::new(
                    i32::try_from(n).map_err(|_| Error::Conversion("Integer out of range"))?,
                ),
                (Value::Integer(n), &Type::INT2) => Box::new(
                    i16::try_from(n).map_err(|_| Error::Conversion("Integer out of range"))?,
                ),
                (Value::Integer(n), &Type::BOOL) if n == 0 || n == 1 => Box::new(n == 1),
                (Value::Integer(n), _) => Box::new(n),
                (Value::Text(s), &Type::JSONB) => Box::new(
                    serde_json::from_str::<serde_json::Value>(&s)
                        .map_err(|_| Error::Conversion("Invalid JSONB projection"))?,
                ),
                (Value::Text(s), _) => Box::new(s),
                (Value::Real(n), _) => Box::new(n),
                (Value::Blob(b), _) => Box::new(b),
            };
            Ok(result)
        })
        .collect()
}
