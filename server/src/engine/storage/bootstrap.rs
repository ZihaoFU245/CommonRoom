use super::*;

impl Engine {
    pub(crate) fn history_store(&self) -> &std::sync::Arc<super::history::HistoryStore> {
        &self.store.history
    }

    pub fn open(path: &Path) -> Result<Self, EngineError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(EngineError::from)?;
        }
        let lock = folder_lock(path)?;
        let memory = path == Path::new(":memory:");
        let location = if memory {
            format!(
                "file:chat-{}?mode=memory&cache=shared",
                uuid::Uuid::new_v4()
            )
        } else {
            path.to_string_lossy().into_owned()
        };
        let db = Connection::open(&location).map_err(EngineError::from)?;
        let version: u32 = db
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(EngineError::from)?;
        if ![0, SCHEMA_VERSION].contains(&version) {
            return Err(EngineError::Invalid(format!(
                "Unsupported database schema; only current v{SCHEMA_VERSION} is supported."
            )));
        }
        if version == 0 {
            let existing: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%')", [], |r| r.get(0),
            ).map_err(EngineError::from)?;
            if existing {
                return Err("Unversioned existing databases are not supported.".into());
            }
        }
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .map_err(EngineError::from)?;
        if version == 0 {
            initialize(&db)?;
        }
        Self::from_database(db, lock, &location, memory)
    }

    pub(crate) fn open_postgres(url: &str) -> Result<Self, EngineError> {
        let db = Connection::postgres(url, false)?;
        postgres_owner(&db)?;
        let exists: bool =
            db.query_row("SELECT to_regclass('chat_schema') IS NOT NULL", [], |r| {
                r.get(0)
            })?;
        if exists {
            let version: u32 =
                db.query_row("SELECT version FROM chat_schema WHERE id=1", [], |r| {
                    r.get(0)
                })?;
            if version != SCHEMA_VERSION {
                return Err(EngineError::Invalid(format!(
                    "Unsupported PostgreSQL schema; only current v{SCHEMA_VERSION} is supported."
                )));
            }
        } else {
            require_empty_postgres(&db)?;
            let tx = db.unchecked_transaction()?;
            tx.execute_batch(include_str!("postgres.sql"))?;
            let mut data = Data::default();
            data.policy.assignments.push(authorization::Assignment {
                account_id: "console".into(),
                group: authorization::Group::Su,
                scope: authorization::Scope::Server,
            });
            write_state(&tx, &data)?;
            tx.execute("INSERT INTO counters VALUES(1,$1)", [data.next_sequence])?;
            tx.commit()?;
        }
        Self::from_database(db, None, url, false)
    }

    pub(crate) fn open_postgres_in_folder(url: &str, path: &Path) -> Result<Self, EngineError> {
        let lock = folder_lock(path)?;
        let mut engine = Self::open_postgres(url)?;
        engine.store._lock = lock;
        Ok(engine)
    }

    fn from_database(
        db: Connection,
        lock: Option<std::fs::File>,
        location: &str,
        memory: bool,
    ) -> Result<Self, EngineError> {
        let mut data = load_data(&db)?;
        crate::bots::models::validate(&data).map_err(EngineError::unavailable)?;
        let authorization = super::authorization::CompiledPolicy::compile(&data)
            .map_err(EngineError::unavailable)?;
        let read_positions = load_read_positions(&db)?;
        data.sessions.retain(|_, session| session.expires > now());
        let mut engine = Self {
            working: None,
            color_revision: 0,
            committed_colors: Default::default(),
            bot_index: crate::bots::models::Index::build(&data),
            authorization,
            presence: Default::default(),
            bot_budgets: BTreeMap::new(),
            data,
            revision: 0,
            recovery_failed: false,
            metadata_dirty: false,
            read_model: Default::default(),
            read_clock: 0,
            instance: uuid::Uuid::new_v4().to_string(),
            read_revisions: BTreeMap::new(),
            changed_conversation: None,
            max_users: 64,
            max_rooms: 64,
            max_messages: 1000,
            media: Default::default(),
            store: Store {
                history: std::sync::Arc::new(
                    super::history::HistoryStore::open_backend(location, memory, db.is_postgres())
                        .map_err(EngineError::unavailable)?,
                ),
                db: std::sync::Arc::new(std::sync::Mutex::new(db)),
                read_positions,
                pending_deletes: Vec::new(),
                pending_cleanups: Vec::new(),
                _lock: lock,
            },
        };
        engine.committed_colors = engine.color_directory();
        Ok(engine)
    }
}

fn folder_lock(path: &Path) -> Result<Option<std::fs::File>, EngineError> {
    if path == Path::new(":memory:") {
        return Ok(None);
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("lock"))?;
    file.try_lock().map_err(|_| {
        EngineError::Conflict("Another server is already using this data folder.".into())
    })?;
    Ok(Some(file))
}

pub(super) fn postgres_owner(db: &Connection) -> Result<(), EngineError> {
    // Advisory locks are scoped to this database and last for the writer session.
    // No automatic reconnect: losing the session invalidates the engine owner.
    let acquired: bool =
        db.query_row("SELECT pg_try_advisory_lock(1128808788::bigint)", [], |r| {
            r.get(0)
        })?;
    if !acquired {
        return Err(EngineError::Conflict(
            "Another server is already using this PostgreSQL database.".into(),
        ));
    }
    Ok(())
}

pub(super) fn require_empty_postgres(db: &Connection) -> Result<(), EngineError> {
    let occupied: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=current_schema() AND c.relkind IN ('r','p','v','m','S','f'))", [], |r| r.get(0))?;
    if occupied {
        return Err(EngineError::Invalid("PostgreSQL target schema must be empty; existing unversioned objects are not supported.".into()));
    }
    Ok(())
}

fn initialize(db: &Connection) -> Result<(), EngineError> {
    let mut data = Data::default();
    data.policy.assignments.push(authorization::Assignment {
        account_id: "console".into(),
        group: authorization::Group::Su,
        scope: authorization::Scope::Server,
    });
    let tx = db.unchecked_transaction()?;
    for schema in [
        SCHEMA,
        RECEIPT_SCHEMA,
        command_receipts::SCHEMA,
        AUDIT_SCHEMA,
        INDEXES,
        ACCOUNT_REFERENCE_SCHEMA,
        media::SCHEMA,
        media::INDEXES,
        retention::SCHEMA,
        auth::SCHEMA,
    ] {
        tx.execute_batch(schema)?;
    }
    write_state(&tx, &data)?;
    tx.execute(
        sql(
            "INSERT INTO counters VALUES(1,?1)",
            "INSERT INTO counters VALUES(1,$1)",
        ),
        [data.next_sequence],
    )?;
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(())
}
