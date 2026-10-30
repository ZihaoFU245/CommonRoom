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
        initialize(&db)?;
        Self::from_database(db, lock, &location, memory, Default::default())
    }

    #[cfg(test)]
    pub(crate) fn open_postgres(url: &str) -> Result<Self, EngineError> {
        Self::open_postgres_with_pool(url, Default::default())
    }
    fn open_postgres_with_pool(
        url: &str,
        limits: crate::config::PostgresPoolConfig,
    ) -> Result<Self, EngineError> {
        let db = Connection::postgres(url, false)?;
        initialize(&db)?;
        Self::from_database(db, None, url, false, limits)
    }

    pub(crate) fn open_postgres_in_folder_with_pool(
        url: &str,
        path: &Path,
        limits: crate::config::PostgresPoolConfig,
    ) -> Result<Self, EngineError> {
        let lock = folder_lock(path)?;
        let engine = Self::open_postgres_with_pool(url, limits)?;
        engine.store.db.attach_folder_lock(lock)?;
        Ok(engine)
    }

    fn from_database(
        db: Connection,
        lock: Option<std::fs::File>,
        location: &str,
        _memory: bool,
        limits: crate::config::PostgresPoolConfig,
    ) -> Result<Self, EngineError> {
        let mut data = load_data(&db)?;
        crate::bots::models::validate(&data).map_err(EngineError::unavailable)?;
        let authorization = super::authorization::CompiledPolicy::compile(&data)
            .map_err(EngineError::unavailable)?;
        let read_positions = load_read_positions(&db)?;
        data.sessions.retain(|_, session| session.expires > now());
        let db = std::sync::Arc::new(database::Database::from_connection(
            db, location, lock, limits,
        )?);
        let mut engine = Self {
            color_revision: 0,
            committed_colors: Default::default(),
            bot_index: crate::bots::models::Index::build(&data),
            authorization,
            presence: Default::default(),
            bot_budgets: BTreeMap::new(),
            data,
            revision: 0,
            recovery_failed: false,
            recovery_attempted: false,
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
                history: std::sync::Arc::new(super::history::HistoryStore::new(db.clone())),
                db: database::Handle::new(db),
                read_positions,
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
    file.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => {
            EngineError::Conflict("Another server is already using this data folder.".into())
        }
        std::fs::TryLockError::Error(error) => EngineError::from(error),
    })?;
    Ok(Some(file))
}

fn initialize(db: &Connection) -> Result<(), EngineError> {
    let mut data = Data::default();
    data.policy.assignments.push(authorization::Assignment {
        account_id: "console".into(),
        group: authorization::Group::Su,
        scope: authorization::Scope::Server,
    });
    db.initialize(&state_json(&data)?, data.next_sequence)?;
    Ok(())
}
