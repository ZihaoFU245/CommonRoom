//! Explicit offline SQLite HEAD -> empty PostgreSQL transfer. Source is read-only;
//! target schema, data, derived indexes and version become visible in one commit.
use super::*;
use crate::engine::database::{self, Row, Value};
use blake2::{Blake2s256, Digest};

struct Table {
    name: &'static str,
    columns: &'static [&'static str],
    keys: &'static [&'static str],
    derived: bool,
}
macro_rules! table {
    ($name:literal, [$($column:literal),+], [$($key:literal),+]) => {
        Table { name: $name, columns: &[$($column),+], keys: &[$($key),+], derived: false }
    };
}
const TABLES: &[Table] = &[
    table!("state", ["id", "json"], ["id"]),
    table!("counters", ["id", "sequence"], ["id"]),
    table!(
        "entities",
        ["kind", "key", "json", "revision"],
        ["kind", "key"]
    ),
    table!(
        "media",
        [
            "id",
            "uploader",
            "conversation",
            "size",
            "created",
            "ready",
            "unreferenced",
            "staging_clean",
            "json",
            "expires",
            "published",
            "deleting",
            "deleted",
            "cleanup_attempt",
            "staging_attempt"
        ],
        ["id"]
    ),
    table!(
        "messages",
        ["conversation", "sequence", "json"],
        ["conversation", "sequence"]
    ),
    table!(
        "reactions",
        ["conversation", "sequence", "reaction", "username"],
        ["conversation", "sequence", "reaction", "username"]
    ),
    table!(
        "read_positions",
        ["username", "conversation", "sequence"],
        ["username", "conversation"]
    ),
    table!(
        "bot_receipts",
        [
            "account_id",
            "key",
            "digest",
            "conversation",
            "sequence",
            "created"
        ],
        ["account_id", "key"]
    ),
    table!(
        "bot_command_receipts",
        ["account_id", "key", "digest", "created", "outcome"],
        ["account_id", "key"]
    ),
    table!(
        "audit_entries",
        ["sequence", "entry_id", "json"],
        ["sequence"]
    ),
    table!(
        "auth_events",
        ["sequence", "at", "client", "username", "outcome"],
        ["sequence"]
    ),
    table!(
        "message_media",
        ["conversation", "sequence", "media_id"],
        ["conversation", "sequence", "media_id"]
    ),
    Table {
        derived: true,
        ..table!(
            "retention",
            ["conversation", "count", "oldest"],
            ["conversation"]
        )
    },
    Table {
        derived: true,
        ..table!(
            "message_accounts",
            ["name", "conversation", "sequence"],
            ["name", "sequence", "conversation"]
        )
    },
];

#[derive(Debug)]
pub(crate) struct MigrationReport {
    pub(crate) rows: BTreeMap<String, u64>,
}

impl Engine {
    pub(crate) fn migrate_sqlite(
        source: &Path,
        destination: &str,
    ) -> Result<MigrationReport, EngineError> {
        if !source.is_file() {
            return Err("Migration source must be an existing SQLite file.".into());
        }
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(source.with_extension("lock"))?;
        lock.try_lock().map_err(|_| {
            EngineError::Conflict("Stop the SQLite server before migrating its data.".into())
        })?;
        let source =
            Connection::open_with_flags(source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        source.busy_timeout(std::time::Duration::from_secs(2))?;
        let snapshot = source.unchecked_transaction()?;
        let version: u32 = snapshot.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version != SCHEMA_VERSION {
            return Err(EngineError::Invalid(format!(
                "Migration requires SQLite HEAD schema v{SCHEMA_VERSION}."
            )));
        }
        let integrity: String =
            snapshot.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        let foreign_keys: u64 =
            snapshot.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })?;
        if integrity != "ok" || foreign_keys != 0 {
            return Err("SQLite integrity or foreign-key validation failed.".into());
        }
        validate_metadata(&snapshot)?;

        let destination = Connection::postgres(destination, false)?;
        bootstrap::postgres_owner(&destination)?;
        bootstrap::require_empty_postgres(&destination)?;
        // A large offline COPY/verification may exceed normal request deadlines.
        destination
            .execute_batch("SET statement_timeout=0; SET idle_in_transaction_session_timeout=0;")?;
        let transaction = destination.unchecked_transaction()?;
        transaction.execute_batch(include_str!("postgres.sql"))?;
        let mut report = MigrationReport {
            rows: BTreeMap::new(),
        };
        for table in TABLES {
            let select = select_table(table, false);
            if !table.derived {
                let mut columns = table.columns.to_vec();
                let projected = matches!(table.name, "entities" | "messages");
                if projected {
                    columns.push("projection");
                }
                transaction.copy_from(table.name, &columns, |sink| {
                    snapshot.visit_rows(&select, |row| {
                        let mut record = values(row, table.columns.len())?;
                        if projected {
                            let raw: String = row.get(2)?;
                            record.push(Value::Text(if table.name == "entities" {
                                database::entity_projection(&raw)?
                            } else {
                                database::message_projection(&raw)?
                            }));
                        }
                        sink(record)
                    })
                })?;
            }
            let expected = fingerprint(&snapshot, &select, table.columns.len())?;
            let actual = fingerprint(
                &transaction,
                &select_table(table, true),
                table.columns.len(),
            )?;
            if expected != actual {
                return Err(EngineError::Invalid(format!(
                    "Migration verification failed for {}; target transaction rolled back.",
                    table.name
                )));
            }
            report.rows.insert(table.name.into(), actual.0);
        }
        // Explicit imported identity values do not advance PostgreSQL sequences.
        for table in ["audit_entries", "auth_events"] {
            transaction.query_row(&format!("SELECT setval(pg_get_serial_sequence('{table}','sequence'),coalesce(max(sequence),1),count(*)>0) FROM {table}"), [], |row| row.get::<_,i64>(0))?;
        }
        validate_metadata(&transaction)?;
        transaction.commit()?;
        // Read-only source transaction and lock are released after target commit.
        drop(snapshot);
        drop(lock);
        Ok(report)
    }
}

fn validate_metadata(db: &Connection) -> Result<(), EngineError> {
    let data = load_data(db)?;
    crate::bots::models::validate(&data).map_err(EngineError::unavailable)?;
    authorization::CompiledPolicy::compile(&data).map_err(EngineError::unavailable)?;
    load_read_positions(db)?;
    let valid: bool = db.query_row("SELECT (SELECT sequence FROM counters WHERE id=1)>=coalesce(max(sequence),0) FROM messages", [], |row| row.get(0))?;
    if !valid {
        return Err("Stored global sequence precedes retained messages.".into());
    }
    Ok(())
}

fn select_table(table: &Table, postgres: bool) -> String {
    let order = table
        .keys
        .iter()
        .map(|key| {
            let number =
                *key == "sequence" || *key == "id" && matches!(table.name, "state" | "counters");
            if number {
                (*key).into()
            } else {
                format!(
                    "{key} COLLATE {}",
                    if postgres { "\"C\"" } else { "BINARY" }
                )
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "SELECT {} FROM {} ORDER BY {order}",
        table.columns.join(","),
        table.name
    )
}

fn values(row: &Row<'_>, columns: usize) -> database::Result<Vec<Value>> {
    (0..columns).map(|column| row.get(column)).collect()
}

fn fingerprint(db: &Connection, query: &str, columns: usize) -> database::Result<(u64, [u8; 32])> {
    let mut hash = Blake2s256::new();
    let mut count = 0u64;
    db.visit_rows(query, |row| {
        hash.update([255]);
        for value in values(row, columns)? {
            let (tag, bytes) = match value {
                Value::Null => (0, Vec::new()),
                Value::Integer(n) => (1, n.to_le_bytes().to_vec()),
                Value::Real(n) => (2, n.to_bits().to_le_bytes().to_vec()),
                Value::Text(s) => (3, s.into_bytes()),
                Value::Blob(b) => (4, b),
            };
            hash.update([tag]);
            hash.update(
                u64::try_from(bytes.len())
                    .map_err(|_| database::Error::Conversion("Row exceeds platform range"))?
                    .to_le_bytes(),
            );
            hash.update(bytes);
        }
        count = count
            .checked_add(1)
            .ok_or(database::Error::Conversion("Too many database rows"))?;
        Ok(())
    })?;
    Ok((count, hash.finalize().into()))
}
