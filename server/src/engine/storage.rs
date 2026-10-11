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
        let mut version: u32 = db
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if version != 0 && version != 5 && version != 6 {
            return Err("Unsupported database schema; schemas v5 and v6 are supported.".into());
        }
        if version == 0 {
            let existing: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%')", [], |r| r.get(0),
            ).map_err(|e| e.to_string())?;
            if existing {
                return Err("Unversioned existing databases are not supported.".into());
            }
        }
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
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
            let transaction = db.unchecked_transaction().map_err(|e| e.to_string())?;
            transaction.execute_batch("CREATE TABLE state (id INTEGER PRIMARY KEY CHECK(id=1), json TEXT NOT NULL); CREATE TABLE read_positions (username TEXT NOT NULL, conversation TEXT NOT NULL, sequence INTEGER NOT NULL, PRIMARY KEY(username,conversation));").map_err(|e| e.to_string())?;
            transaction
                .execute_batch(SCHEMA)
                .map_err(|e| e.to_string())?;
            write_data(&transaction, &data)?;
            transaction
                .execute_batch("PRAGMA user_version=6;")
                .map_err(|e| e.to_string())?;
            transaction.commit().map_err(|e| e.to_string())?;
            version = 6;
        }
        let raw: String = db
            .query_row("SELECT json FROM state WHERE id=1", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        let mut data: Data = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        drop(raw);
        if version == 6 {
            load_entities(&db, &mut data)?;
        }
        // Upgrade persisted command grants once; dispatch accepts only the new syntax.
        // This changes names within v5, not the schema or action permissions.
        let mut upgraded = false;
        for grant in &mut data.policy.grants {
            for (old, new) in [
                ("/delete", "/rm"),
                ("/deleteuser", "/rm"),
                ("/user", "/new"),
                ("/rooms", "/ls"),
                ("/users", "/ls"),
            ] {
                if grant.permissions.remove(old) {
                    grant.permissions.insert(new.into());
                    upgraded = true;
                }
            }
        }
        if upgraded {
            data.policy.revision = data.policy.revision.wrapping_add(1);
        }
        // Validate before changing a legacy database; DDL and data move atomically.
        let authorization = super::authorization::CompiledPolicy::compile(&data)?;
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
        if version != 6 {
            let tx = db.unchecked_transaction().map_err(|e| e.to_string())?;
            tx.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
            write_data(&tx, &data)?;
            tx.execute_batch("PRAGMA user_version=6;")
                .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
            upgraded = false;
        }
        let mut engine = Self {
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
        };
        // Validate the complete policy before writing. A failed atomic state write
        // leaves the original database intact and prevents startup.
        if upgraded {
            engine.save()?;
        }
        Ok(engine)
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
        let tx = self.db.transaction().map_err(|e| e.to_string())?;
        write_data(&tx, &self.data).map_err(storage_error)?;
        tx.commit().map_err(|e| storage_error(e.to_string()))?;
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
        write_data(&transaction, &self.data)?;
        transaction.commit().map_err(|e| e.to_string())
    }
    /// Account deletion and its cursor cleanup must commit atomically.
    pub(super) fn store_account_deletion(
        &mut self,
        name: &str,
        removed: &BTreeSet<String>,
    ) -> Result<(), String> {
        let transaction = self.db.transaction().map_err(|e| e.to_string())?;
        write_data(&transaction, &self.data)?;
        transaction
            .execute(
                "DELETE FROM read_positions WHERE username=?1",
                params![name],
            )
            .map_err(|e| e.to_string())?;
        for key in removed {
            transaction
                .execute(
                    "DELETE FROM read_positions WHERE conversation=?1",
                    params![key],
                )
                .map_err(|e| e.to_string())?;
        }
        transaction.commit().map_err(|e| e.to_string())
    }
}

const SCHEMA: &str = "CREATE TABLE entities(kind TEXT NOT NULL,key TEXT NOT NULL,json TEXT NOT NULL,revision INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(kind,key)) WITHOUT ROWID;
CREATE TABLE messages(conversation TEXT NOT NULL,sequence INTEGER NOT NULL,json TEXT NOT NULL,PRIMARY KEY(conversation,sequence)) WITHOUT ROWID;
CREATE TABLE reactions(conversation TEXT NOT NULL,sequence INTEGER NOT NULL,reaction TEXT NOT NULL,username TEXT NOT NULL,PRIMARY KEY(conversation,sequence,reaction,username),FOREIGN KEY(conversation,sequence) REFERENCES messages(conversation,sequence) ON DELETE CASCADE) WITHOUT ROWID;
CREATE TABLE counters(id INTEGER PRIMARY KEY CHECK(id=1),sequence INTEGER NOT NULL);
CREATE INDEX read_positions_conversation ON read_positions(conversation);";

fn storage_error(error: String) -> String {
    tracing::error!(error, "Storage write failed");
    "Storage unavailable; change was not applied.".into()
}
fn put_entity(
    db: &Connection,
    kind: &str,
    key: &str,
    value: &impl serde::Serialize,
) -> Result<(), String> {
    let json = serde_json::to_string(value).map_err(|e| e.to_string())?;
    db.execute("INSERT INTO entities(kind,key,json) VALUES(?1,?2,?3) ON CONFLICT(kind,key) DO UPDATE SET json=excluded.json WHERE json!=excluded.json", params![kind,key,json]).map_err(|e| e.to_string())?;
    Ok(())
}
// Borrow the payload during serialization; reactions are independent rows.
#[derive(serde::Serialize)]
struct MessageRecord<'a> {
    id: &'a str,
    from: &'a str,
    author_id: &'a str,
    private_id: &'a str,
    to: &'a Option<String>,
    text: &'a str,
    time: u64,
    sequence: u64,
    reply: &'a Option<Reply>,
    mentions: &'a BTreeSet<String>,
}
fn put_message(db: &Connection, key: &str, message: &Message) -> Result<(), String> {
    let record = MessageRecord {
        id: &message.id,
        from: &message.from,
        author_id: &message.author_id,
        private_id: &message.private_id,
        to: &message.to,
        text: &message.text,
        time: message.time,
        sequence: message.sequence,
        reply: &message.reply,
        mentions: &message.mentions,
    };
    let json = serde_json::to_string(&record).map_err(|e| e.to_string())?;
    db.execute("INSERT INTO messages VALUES(?1,?2,?3) ON CONFLICT(conversation,sequence) DO UPDATE SET json=excluded.json WHERE json!=excluded.json", params![key,message.sequence,json]).map_err(|e| e.to_string())?;
    Ok(())
}
fn reconcile_reactions(db: &Connection, key: &str, message: &Message) -> Result<(), String> {
    let mut query = db
        .prepare("SELECT reaction,username FROM reactions WHERE conversation=?1 AND sequence=?2")
        .map_err(|e| e.to_string())?;
    let rows = query
        .query_map(params![key, message.sequence], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?;
    let stored = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    for (reaction, user) in stored {
        if !message
            .reactions
            .get(&reaction)
            .is_some_and(|users| users.contains(&user))
        {
            db.execute("DELETE FROM reactions WHERE conversation=?1 AND sequence=?2 AND reaction=?3 AND username=?4", params![key,message.sequence,reaction,user]).map_err(|e| e.to_string())?;
        }
    }
    for (reaction, users) in &message.reactions {
        for user in users {
            db.execute(
                "INSERT OR IGNORE INTO reactions VALUES(?1,?2,?3,?4)",
                params![key, message.sequence, reaction, user],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
fn room_metadata(room: &Room) -> Room {
    Room {
        id: room.id.clone(),
        owner_id: room.owner_id.clone(),
        members: room.members.clone(),
        revision: 0,
        messages: VecDeque::new(),
    }
}
fn private_metadata(chat: &PrivateChat) -> PrivateChat {
    PrivateChat {
        id: chat.id.clone(),
        revision: 0,
        messages: VecDeque::new(),
    }
}
fn write_state(db: &Connection, data: &Data) -> Result<(), String> {
    let metadata = Data {
        policy: data.policy.clone(),
        ..Data::default()
    };
    let json = serde_json::to_string(&metadata).map_err(|e| e.to_string())?;
    db.execute(
        "INSERT INTO state VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET json=excluded.json",
        params![json],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
// Administrative mutations may affect many records. Reconcile inside the caller's
// transaction, writing only changed rows, without retaining another history copy.
fn write_data(db: &Connection, data: &Data) -> Result<(), String> {
    db.execute_batch("CREATE TEMP TABLE IF NOT EXISTS live_entities(kind TEXT,key TEXT,PRIMARY KEY(kind,key)) WITHOUT ROWID; CREATE TEMP TABLE IF NOT EXISTS live_messages(conversation TEXT,sequence INTEGER,PRIMARY KEY(conversation,sequence)) WITHOUT ROWID; DELETE FROM live_entities; DELETE FROM live_messages;").map_err(|e| e.to_string())?;
    for row in data
        .users
        .iter()
        .map(|(k, v)| Ok(("account", k, serde_json::to_string(v)?)))
        .chain(
            data.sessions
                .iter()
                .map(|(k, v)| Ok(("session", k, serde_json::to_string(v)?))),
        )
        .chain(
            data.rooms
                .iter()
                .map(|(k, v)| Ok(("room", k, serde_json::to_string(&room_metadata(v))?))),
        )
        .chain(
            data.private
                .iter()
                .map(|(k, v)| Ok(("private", k, serde_json::to_string(&private_metadata(v))?))),
        )
    {
        let (kind, key, json) = row.map_err(|e: serde_json::Error| e.to_string())?;
        db.execute("INSERT INTO entities(kind,key,json) VALUES(?1,?2,?3) ON CONFLICT(kind,key) DO UPDATE SET json=excluded.json WHERE json!=excluded.json", params![kind,key,json]).map_err(|e| e.to_string())?;
        db.execute(
            "INSERT INTO live_entities VALUES(?1,?2)",
            params![kind, key],
        )
        .map_err(|e| e.to_string())?;
    }
    for (kind, key, revision) in data
        .rooms
        .iter()
        .map(|(k, r)| ("room", k, r.revision))
        .chain(data.private.iter().map(|(k, c)| ("private", k, c.revision)))
    {
        db.execute(
            "UPDATE entities SET revision=?3 WHERE kind=?1 AND key=?2 AND revision!=?3",
            params![kind, key, revision],
        )
        .map_err(|e| e.to_string())?;
    }
    for (key, messages) in data
        .rooms
        .values()
        .map(|r| (r.id.as_str(), &r.messages))
        .chain(data.private.values().map(|c| (c.id.as_str(), &c.messages)))
    {
        for m in messages {
            put_message(db, key, m)?;
            reconcile_reactions(db, key, m)?;
            db.execute(
                "INSERT INTO live_messages VALUES(?1,?2)",
                params![key, m.sequence],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    db.execute_batch("DELETE FROM entities WHERE NOT EXISTS(SELECT 1 FROM live_entities l WHERE l.kind=entities.kind AND l.key=entities.key); DELETE FROM messages WHERE NOT EXISTS(SELECT 1 FROM live_messages l WHERE l.conversation=messages.conversation AND l.sequence=messages.sequence); DELETE FROM live_entities; DELETE FROM live_messages;").map_err(|e| e.to_string())?;
    db.execute("INSERT INTO counters VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET sequence=excluded.sequence", params![data.next_sequence]).map_err(|e| e.to_string())?;
    write_state(db, data)
}
pub(super) fn load_entities(db: &Connection, data: &mut Data) -> Result<(), String> {
    data.next_sequence = db
        .query_row("SELECT sequence FROM counters WHERE id=1", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let mut query = db
        .prepare("SELECT kind,key,json,revision FROM entities")
        .map_err(|e| e.to_string())?;
    let rows = query
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, u64>(3)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (kind, key, json, revision) = row.map_err(|e| e.to_string())?;
        match kind.as_str() {
            "account" => {
                data.users.insert(
                    key.clone(),
                    serde_json::from_str(&json).map_err(|e| e.to_string())?,
                );
            }
            "session" => {
                data.sessions.insert(
                    key.clone(),
                    serde_json::from_str(&json).map_err(|e| e.to_string())?,
                );
            }
            "room" => {
                data.rooms.insert(
                    key.clone(),
                    serde_json::from_str(&json).map_err(|e| e.to_string())?,
                );
            }
            "private" => {
                data.private.insert(
                    key.clone(),
                    serde_json::from_str(&json).map_err(|e| e.to_string())?,
                );
            }
            _ => return Err("Unknown stored entity kind.".into()),
        }
        if kind == "room"
            && let Some(room) = data.rooms.get_mut(&key)
        {
            room.revision = revision;
        }
        if kind == "private"
            && let Some(chat) = data.private.get_mut(&key)
        {
            chat.revision = revision;
        }
    }
    let mut query = db
        .prepare("SELECT conversation,json FROM messages ORDER BY conversation,sequence")
        .map_err(|e| e.to_string())?;
    let mut rooms: BTreeMap<_, _> = data
        .rooms
        .values_mut()
        .map(|r| (r.id.clone(), &mut r.messages))
        .chain(
            data.private
                .values_mut()
                .map(|c| (c.id.clone(), &mut c.messages)),
        )
        .collect();
    let rows = query
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (key, json) = row.map_err(|e| e.to_string())?;
        let mut message: Message = serde_json::from_str(&json).map_err(|e| e.to_string())?;
        let mut reactions = db
            .prepare_cached(
                "SELECT reaction,username FROM reactions WHERE conversation=?1 AND sequence=?2",
            )
            .map_err(|e| e.to_string())?;
        let entries = reactions
            .query_map(params![key, message.sequence], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?;
        for entry in entries {
            let (reaction, user) = entry.map_err(|e| e.to_string())?;
            message.reactions.entry(reaction).or_default().insert(user);
        }
        rooms
            .get_mut(&key)
            .ok_or("Orphaned stored message.")?
            .push_back(message);
    }
    Ok(())
}

impl Engine {
    pub(super) fn save_message_change(
        &mut self,
        change: &super::dispatch::MessageChange,
    ) -> Result<(), String> {
        let (id, messages) = if change.private {
            let chat = self
                .data
                .private
                .get(&change.key)
                .ok_or("Private conversation not found.")?;
            (&chat.id, &chat.messages)
        } else {
            let room = self.data.rooms.get(&change.key).ok_or("Room not found.")?;
            (&room.id, &room.messages)
        };
        let tx = self.db.transaction().map_err(|e| e.to_string())?;
        if change.reaction {
            if let Some(index) = change.index {
                let current = messages.get(index).ok_or("Message not found.")?;
                let previous = change
                    .previous
                    .as_ref()
                    .ok_or("Missing reaction rollback.")?;
                for (reaction, users) in &previous.reactions {
                    for user in users {
                        if !current
                            .reactions
                            .get(reaction)
                            .is_some_and(|users| users.contains(user))
                        {
                            tx.execute("DELETE FROM reactions WHERE conversation=?1 AND sequence=?2 AND reaction=?3 AND username=?4", params![id,current.sequence,reaction,user]).map_err(|e| storage_error(e.to_string()))?;
                        }
                    }
                }
                for (reaction, users) in &current.reactions {
                    for user in users {
                        if !previous
                            .reactions
                            .get(reaction)
                            .is_some_and(|users| users.contains(user))
                        {
                            tx.execute(
                                "INSERT INTO reactions VALUES(?1,?2,?3,?4)",
                                params![id, current.sequence, reaction, user],
                            )
                            .map_err(|e| storage_error(e.to_string()))?;
                        }
                    }
                }
            }
        } else if let Some(message) = messages.back().filter(|m| m.sequence > change.sequence) {
            put_message(&tx, id, message).map_err(storage_error)?;
            if let Some(evicted) = &change.previous {
                tx.execute(
                    "DELETE FROM messages WHERE conversation=?1 AND sequence=?2",
                    params![id, evicted.sequence],
                )
                .map_err(|e| storage_error(e.to_string()))?;
            }
            tx.execute(
                "UPDATE counters SET sequence=?1 WHERE id=1",
                params![self.data.next_sequence],
            )
            .map_err(|e| storage_error(e.to_string()))?;
        }
        let (kind, revision) = if change.private {
            let chat = self
                .data
                .private
                .get(&change.key)
                .ok_or("Private conversation not found.")?;
            if change.new_conversation() {
                put_entity(&tx, "private", &change.key, &private_metadata(chat))
                    .map_err(storage_error)?;
            }
            ("private", chat.revision)
        } else {
            (
                "room",
                self.data
                    .rooms
                    .get(&change.key)
                    .ok_or("Room not found.")?
                    .revision,
            )
        };
        tx.execute(
            "UPDATE entities SET revision=?3 WHERE kind=?1 AND key=?2",
            params![kind, change.key, revision],
        )
        .map_err(|e| storage_error(e.to_string()))?;
        tx.commit().map_err(|e| storage_error(e.to_string()))?;
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }
}
