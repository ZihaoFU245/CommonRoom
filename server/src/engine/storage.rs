use super::*;

impl Engine {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let lock = if path == Path::new(":memory:") {
            None
        } else {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(path.with_extension("lock"))
                .map_err(|e| e.to_string())?;
            file.try_lock()
                .map_err(|_| "Another server is already using this data folder.".to_string())?;
            Some(file)
        };
        let db = Connection::open(path).map_err(|e| e.to_string())?;
        let version: u32 = db
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if version != 0 && version != 5 {
            return Err("Unsupported database schema; only current schema v5 is supported.".into());
        }
        if version == 0 {
            let existing: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%')", [], |r| r.get(0),
            ).map_err(|e| e.to_string())?;
            if existing {
                return Err("Unversioned existing databases are not supported.".into());
            }
        }
        db.execute_batch("PRAGMA journal_mode=WAL;")
            .map_err(|e| e.to_string())?;
        if version == 0 {
            let mut data = Data::default();
            data.policy
                .assignments
                .push(super::authorization::Assignment {
                    account_id: "console".into(),
                    group: super::authorization::Group::Su,
                    scope: super::authorization::Scope::Server,
                });
            let json = serde_json::to_string(&data).map_err(|e| e.to_string())?;
            let transaction = db.unchecked_transaction().map_err(|e| e.to_string())?;
            transaction.execute_batch("CREATE TABLE state (id INTEGER PRIMARY KEY CHECK(id=1), json TEXT NOT NULL); CREATE TABLE read_positions (username TEXT NOT NULL, conversation TEXT NOT NULL, sequence INTEGER NOT NULL, PRIMARY KEY(username,conversation));").map_err(|e| e.to_string())?;
            transaction
                .execute("INSERT INTO state VALUES(1,?1)", params![json])
                .map_err(|e| e.to_string())?;
            transaction
                .execute_batch("PRAGMA user_version=5;")
                .map_err(|e| e.to_string())?;
            transaction.commit().map_err(|e| e.to_string())?;
        }
        let raw: String = db
            .query_row("SELECT json FROM state WHERE id=1", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        let data: Data = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        let mut read_positions: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
        {
            let mut query = db
                .prepare("SELECT username,conversation,sequence FROM read_positions")
                .map_err(|e| e.to_string())?;
            let rows = query
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, u64>(2)?,
                    ))
                })
                .map_err(|e| e.to_string())?;
            for row in rows {
                let (user, view, sequence) = row.map_err(|e| e.to_string())?;
                read_positions
                    .entry(user)
                    .or_default()
                    .insert(view, sequence);
            }
        }
        let authorization = super::authorization::CompiledPolicy::compile(&data)?;
        Ok(Self {
            authorization,
            online_connections: BTreeMap::new(),
            data,
            read_positions,
            revision: 0,
            max_users: 64,
            max_rooms: 64,
            max_messages: 1000,
            agent_pending: BTreeSet::new(),
            agent_queue: Vec::new(),
            db,
            _lock: lock,
        })
    }
    pub fn set_limits(
        &mut self,
        max_users: usize,
        max_rooms: usize,
        max_messages: usize,
    ) -> Result<(), String> {
        if max_users == 0 || max_rooms == 0 || max_messages == 0 {
            return Err("max_users, max_rooms and max_messages must be positive integers.".into());
        }
        if self
            .data
            .rooms
            .values()
            .any(|room| room.messages.len() > max_messages)
            || self
                .data
                .private
                .values()
                .any(|chat| chat.messages.len() > max_messages)
        {
            let previous = self.data.clone();
            for room in self.data.rooms.values_mut() {
                if room.messages.len() > max_messages {
                    room.messages.drain(..room.messages.len() - max_messages);
                    room.revision = room.revision.wrapping_add(1);
                }
            }
            for chat in self.data.private.values_mut() {
                if chat.messages.len() > max_messages {
                    chat.messages.drain(..chat.messages.len() - max_messages);
                    chat.revision = chat.revision.wrapping_add(1);
                }
            }
            if let Err(error) = self.save() {
                self.data = previous;
                return Err(error);
            }
        }
        self.max_users = max_users;
        self.max_rooms = max_rooms;
        self.max_messages = max_messages;
        Ok(())
    }
    pub(super) fn save(&mut self) -> Result<(), String> {
        let authorization = if self.authorization.revision != self.data.policy.revision {
            Some(super::authorization::CompiledPolicy::compile(&self.data)?)
        } else {
            None
        };
        let json = serde_json::to_string(&self.data).map_err(|e| e.to_string())?;
        self.db.execute("INSERT INTO state(id,json) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET json=excluded.json", params![json]).map_err(|e| { tracing::error!(error = %e, "Storage write failed"); "Storage unavailable; change was not applied.".to_string() })?;
        if let Some(authorization) = authorization {
            self.authorization = authorization;
        }
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }
    pub fn checkpoint(&self) -> Result<(), String> {
        self.db
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(|e| e.to_string())
    }
    /// Persist a cursor separately from message state. Caller updates its cache
    /// only after this succeeds; SQL keeps the cursor monotonic across devices.
    pub(super) fn store_read_position(
        &self,
        user: &str,
        key: &str,
        through: u64,
    ) -> Result<(), String> {
        self.db.execute("INSERT INTO read_positions VALUES(?1,?2,?3) ON CONFLICT(username,conversation) DO UPDATE SET sequence=MAX(sequence,excluded.sequence)",
            params![user,key,through]).map_err(|e| { tracing::error!(error=%e,"Read position write failed"); "Storage unavailable; read position was not applied.".to_string() })?;
        Ok(())
    }
    /// Rename account state and every affected read cursor in one transaction.
    pub(super) fn store_account_rename(
        &mut self,
        name: &str,
        new_name: &str,
        conversations: &BTreeMap<String, String>,
    ) -> Result<(), String> {
        let json = serde_json::to_string(&self.data).map_err(|e| e.to_string())?;
        let transaction = self.db.transaction().map_err(|e| e.to_string())?;
        // A reused name must never inherit orphaned cursors from an old account.
        transaction
            .execute(
                "DELETE FROM read_positions WHERE username=?1",
                params![new_name],
            )
            .map_err(|e| e.to_string())?;
        transaction
            .execute(
                "UPDATE read_positions SET username=?1 WHERE username=?2",
                params![new_name, name],
            )
            .map_err(|e| e.to_string())?;
        for (old, new) in conversations {
            transaction
                .execute(
                    "DELETE FROM read_positions WHERE conversation=?1",
                    params![new],
                )
                .map_err(|e| e.to_string())?;
            transaction
                .execute(
                    "UPDATE read_positions SET conversation=?1 WHERE conversation=?2",
                    params![new, old],
                )
                .map_err(|e| e.to_string())?;
        }
        transaction.execute("INSERT INTO state(id,json) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET json=excluded.json", params![json]).map_err(|e| e.to_string())?;
        transaction.commit().map_err(|e| e.to_string())
    }
    /// Account deletion and its cursor cleanup must commit atomically.
    pub(super) fn store_account_deletion(
        &mut self,
        name: &str,
        removed: &BTreeSet<String>,
    ) -> Result<(), String> {
        let json = serde_json::to_string(&self.data).map_err(|e| e.to_string())?;
        let transaction = self.db.transaction().map_err(|e| e.to_string())?;
        transaction.execute("INSERT INTO state(id,json) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET json=excluded.json", params![json]).map_err(|e| e.to_string())?;
        transaction
            .execute(
                "DELETE FROM read_positions WHERE username=?1",
                params![name],
            )
            .map_err(|e| e.to_string())?;
        if !removed.is_empty() {
            let placeholders = vec!["?"; removed.len()].join(",");
            transaction
                .execute(
                    &format!("DELETE FROM read_positions WHERE conversation IN ({placeholders})"),
                    rusqlite::params_from_iter(removed),
                )
                .map_err(|e| e.to_string())?;
        }
        transaction.commit().map_err(|e| e.to_string())
    }
}
