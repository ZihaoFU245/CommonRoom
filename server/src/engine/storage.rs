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
        if version > 5 {
            return Err("This data folder was written by a newer, incompatible server.".into());
        }
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS state (id INTEGER PRIMARY KEY CHECK(id=1), json TEXT NOT NULL);").map_err(|e| e.to_string())?;
        let raw = db.query_row("SELECT json FROM state WHERE id=1", [], |r| {
            r.get::<_, String>(0)
        });
        let mut data: Data = match raw {
            Ok(raw) => serde_json::from_str(&raw).map_err(|e| e.to_string())?,
            Err(rusqlite::Error::QueryReturnedNoRows) => Data::default(),
            Err(e) => return Err(e.to_string()),
        };
        db.execute_batch("CREATE TABLE IF NOT EXISTS read_positions (username TEXT NOT NULL, conversation TEXT NOT NULL, sequence INTEGER NOT NULL, PRIMARY KEY(username,conversation));").map_err(|e| e.to_string())?;
        for message in data.direct.drain(..) {
            let peer = message
                .to
                .as_deref()
                .ok_or("Invalid legacy private message.")?;
            data.private
                .entry(private_key(&message.from, peer))
                .or_default()
                .messages
                .push_back(message);
        }
        let mut messages: Vec<_> = data
            .rooms
            .values_mut()
            .flat_map(|r| r.messages.iter_mut())
            .chain(
                data.private
                    .values_mut()
                    .flat_map(|r| r.messages.iter_mut()),
            )
            .collect();
        messages.sort_by_key(|m| m.time);
        data.next_sequence = data
            .next_sequence
            .max(messages.iter().map(|m| m.sequence).max().unwrap_or(0));
        for message in messages {
            if message.sequence == 0 {
                data.next_sequence = data
                    .next_sequence
                    .checked_add(1)
                    .filter(|n| *n <= i64::MAX as u64)
                    .ok_or("Message sequence exhausted.")?;
                message.sequence = data.next_sequence;
            }
        }
        if version < 3 {
            for room in data.rooms.values_mut() {
                room.messages.make_contiguous().sort_by_key(|m| m.sequence);
            }
            for chat in data.private.values_mut() {
                chat.messages.make_contiguous().sort_by_key(|m| m.sequence);
            }
        }
        // Migration and its read baselines are committed together. Existing
        // history starts read; future messages receive monotonically larger IDs.
        if version < 5 {
            if version < 4 {
                super::authorization::migrate(&mut data);
            }
            super::authorization::migrate_command_grants(&mut data);
            let transaction = db.unchecked_transaction().map_err(|e| e.to_string())?;
            if version < 3 {
                for (name, room) in &data.rooms {
                    if let Some(last) = room.messages.back() {
                        for user in &room.members {
                            transaction
                                .execute(
                                    "INSERT OR IGNORE INTO read_positions VALUES(?1,?2,?3)",
                                    params![user, format!("room:{name}"), last.sequence],
                                )
                                .map_err(|e| e.to_string())?;
                        }
                    }
                }
                for (key, chat) in &data.private {
                    let (a, b) = key.split_once(':').ok_or("Invalid private conversation.")?;
                    if let Some(last) = chat.messages.back() {
                        for user in [a, b] {
                            transaction
                                .execute(
                                    "INSERT OR IGNORE INTO read_positions VALUES(?1,?2,?3)",
                                    params![user, format!("dm:{key}"), last.sequence],
                                )
                                .map_err(|e| e.to_string())?;
                        }
                    }
                }
            }
            let json = serde_json::to_string(&data).map_err(|e| e.to_string())?;
            transaction.execute("INSERT INTO state(id,json) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET json=excluded.json", params![json]).map_err(|e| e.to_string())?;
            transaction
                .execute_batch("PRAGMA user_version=5;")
                .map_err(|e| e.to_string())?;
            transaction.commit().map_err(|e| e.to_string())?;
        }
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
