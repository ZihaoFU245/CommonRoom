use super::*;
mod command_receipts;
mod media;
mod metadata;

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
        let memory = path == Path::new(":memory:");
        let location = if memory {
            format!(
                "file:chat-{}?mode=memory&cache=shared",
                uuid::Uuid::new_v4()
            )
        } else {
            path.to_string_lossy().into_owned()
        };
        let db = Connection::open(&location).map_err(|e| e.to_string())?;
        let version: u32 = db
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if ![0, 16, 17].contains(&version) {
            return Err("Unsupported database schema; only HEAD v16 and v17 are supported.".into());
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
            transaction
                .execute_batch(RECEIPT_SCHEMA)
                .map_err(|e| e.to_string())?;
            transaction
                .execute_batch(command_receipts::SCHEMA)
                .map_err(|e| e.to_string())?;
            transaction
                .execute_batch(AUDIT_SCHEMA)
                .map_err(|e| e.to_string())?;
            write_data(&transaction, &data)?;
            transaction
                .execute_batch(INDEXES)
                .map_err(|e| e.to_string())?;
            transaction
                .execute_batch(ACCOUNT_REFERENCE_SCHEMA)
                .map_err(|e| e.to_string())?;
            transaction
                .execute_batch(media::SCHEMA)
                .map_err(|e| e.to_string())?;
            transaction
                .execute_batch(media::INDEXES)
                .map_err(|e| e.to_string())?;
            transaction
                .execute_batch("PRAGMA user_version=17;")
                .map_err(|e| e.to_string())?;
            transaction.commit().map_err(|e| e.to_string())?;
        }
        let raw: String = db
            .query_row("SELECT json FROM state WHERE id=1", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        let mut data: Data = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        let old_state: serde_json::Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        drop(raw);
        load_entities(&db, &mut data)?;
        crate::bots::models::validate(&data)?;
        let mut authorization = super::authorization::CompiledPolicy::compile(&data)?;
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
        if version == 16 {
            let migration = db.unchecked_transaction().map_err(|e| e.to_string())?;
            let before = data.clone();
            metadata::migrate_cleanup_limits(&mut data, &old_state)?;
            authorization = super::authorization::CompiledPolicy::compile(&data)?;
            write_map_delta(&migration, "account", &data.users, &before.users)?;
            write_state(&migration, &data)?;
            migration
                .execute_batch("PRAGMA user_version=17;")
                .map_err(|e| e.to_string())?;
            migration.commit().map_err(|e| e.to_string())?;
        }
        data.sessions.retain(|_, session| session.expires > now());
        for room in data.rooms.values_mut() {
            room.messages.clear();
        }
        for chat in data.private.values_mut() {
            chat.messages.clear();
        }
        Ok(Self {
            bot_index: crate::bots::models::Index::build(&data),
            authorization,
            online_connections: BTreeMap::new(),
            bot_budgets: BTreeMap::new(),
            data,
            read_positions,
            revision: 0,
            recovery_failed: false,
            presence_revision: 0,
            read_clock: 0,
            instance: uuid::Uuid::new_v4().to_string(),
            read_revisions: BTreeMap::new(),
            changed_conversation: None,
            max_users: 64,
            max_rooms: 64,
            max_messages: 1000,
            media_config: crate::config::MediaConfig::default(),
            history_store: std::sync::Arc::new(super::history::HistoryStore::open(
                &location, memory,
            )?),
            db: std::sync::Arc::new(std::sync::Mutex::new(db)),
            pending_deletes: Vec::new(),
            pending_cleanups: Vec::new(),
            _lock: lock,
        })
    }
    pub fn set_limits(
        &mut self,
        max_users: usize,
        max_rooms: usize,
        max_messages: usize,
    ) -> Result<(), String> {
        crate::config::validate_limits(max_users, max_rooms, max_messages)?;
        let writer = self.db.clone();
        let mut db = writer.lock().map_err(|_| "Storage unavailable.")?;
        let tx = db.savepoint().map_err(|e| e.to_string())?;
        let mut changed = Vec::new();
        for (kind, key, id, revision) in self
            .data
            .rooms
            .iter()
            .map(|(key, r)| ("room", key, &r.id, r.revision))
            .chain(
                self.data
                    .private
                    .iter()
                    .map(|(key, c)| ("private", key, &c.id, c.revision)),
            )
        {
            let deleted=tx.execute("DELETE FROM messages WHERE conversation=?1 AND sequence <= (SELECT sequence FROM messages WHERE conversation=?1 ORDER BY sequence DESC LIMIT 1 OFFSET ?2)",params![id,max_messages]).map_err(|e|e.to_string())?;
            if deleted > 0 {
                let revision = revision.wrapping_add(1);
                tx.execute(
                    "UPDATE entities SET revision=?3 WHERE kind=?1 AND key=?2",
                    params![kind, key, revision],
                )
                .map_err(|e| e.to_string())?;
                changed.push((kind, key.clone(), revision));
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        for (kind, key, revision) in changed {
            if kind == "room" {
                if let Some(r) = self.data.rooms.get_mut(&key) {
                    r.revision = revision;
                }
            } else if let Some(c) = self.data.private.get_mut(&key) {
                c.revision = revision;
            }
        }
        self.max_users = max_users;
        self.max_rooms = max_rooms;
        self.max_messages = max_messages;

        Ok(())
    }
    #[cfg(test)]
    pub(super) fn save(&mut self) -> Result<(), String> {
        self.save_with_removed_rooms(&[], None)
    }
    pub(super) fn save_delta(&mut self, before: &Data) -> Result<(), String> {
        self.save_with_removed_rooms(&[], Some(before))
    }
    pub(super) fn save_with_removed_rooms(
        &mut self,
        rooms: &[String],
        before: Option<&Data>,
    ) -> Result<(), String> {
        let authorization = if self.authorization.revision != self.data.policy.revision {
            Some(super::authorization::CompiledPolicy::compile(&self.data)?)
        } else {
            None
        };
        let writer = self.db.clone();
        let mut db = writer.lock().map_err(|_| "Storage unavailable.")?;
        let tx = db.savepoint().map_err(|e| e.to_string())?;
        if let Some(before) = before {
            write_delta(&tx, &self.data, before).map_err(storage_error)?;
        } else {
            write_data(&tx, &self.data).map_err(storage_error)?;
        }
        for room in rooms {
            tx.execute(
                "DELETE FROM read_positions WHERE conversation=?1",
                params![format!("room:{room}")],
            )
            .map_err(|e| storage_error(e.to_string()))?;
        }
        write_history_deletions(&tx, &self.pending_deletes, &self.pending_cleanups)?;
        tx.commit().map_err(|e| storage_error(e.to_string()))?;
        for room in rooms {
            let key = format!("room:{room}");
            for positions in self.read_positions.values_mut() {
                positions.remove(&key);
            }
        }
        if let Some(authorization) = authorization {
            self.authorization = authorization;
        }
        self.bot_index = crate::bots::models::Index::build(&self.data);
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }
    pub fn checkpoint(&self) -> Result<(), String> {
        self.db
            .lock()
            .map_err(|_| "Storage unavailable.")?
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
        self.db.lock().map_err(|_| "Storage unavailable.")?.execute("INSERT INTO read_positions VALUES(?1,?2,?3) ON CONFLICT(username,conversation) DO UPDATE SET sequence=MAX(sequence,excluded.sequence)",
            params![user,key,through]).map_err(|e| { tracing::error!(error=%e,"Read position write failed"); "Storage unavailable; read position was not applied.".to_string() })?;
        Ok(())
    }
    /// Room metadata and name-based read cursors move together; history and
    /// grants remain attached to the unchanged room ID.
    pub(super) fn store_room_rename(
        &mut self,
        name: &str,
        new_name: &str,
        before: &Data,
    ) -> Result<(), String> {
        let writer = self.db.clone();
        let mut db = writer.lock().map_err(|_| "Storage unavailable.")?;
        let transaction = db.savepoint().map_err(|e| e.to_string())?;
        // A reused room name must not inherit another room's orphaned cursors.
        transaction
            .execute(
                "DELETE FROM read_positions WHERE conversation=?1",
                params![format!("room:{new_name}")],
            )
            .map_err(|e| e.to_string())?;
        transaction
            .execute(
                "UPDATE read_positions SET conversation=?1 WHERE conversation=?2",
                params![format!("room:{new_name}"), format!("room:{name}")],
            )
            .map_err(|e| e.to_string())?;
        write_delta(&transaction, &self.data, before)?;
        transaction.commit().map_err(|e| e.to_string())
    }
    /// Rename account state and every affected read cursor in one transaction.
    pub(super) fn store_account_rename(
        &mut self,
        name: &str,
        new_name: &str,
        conversations: &BTreeMap<String, String>,
        before: &Data,
    ) -> Result<(), String> {
        let writer = self.db.clone();
        let mut db = writer.lock().map_err(|_| "Storage unavailable.")?;
        let transaction = db.savepoint().map_err(|e| e.to_string())?;
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
        let affected = account_conversations(&transaction, name)?;
        rewrite_account_messages(&transaction, name, Some(new_name))?;
        for (id, revision) in self
            .data
            .rooms
            .values_mut()
            .map(|r| (&r.id, &mut r.revision))
            .chain(
                self.data
                    .private
                    .values_mut()
                    .map(|c| (&c.id, &mut c.revision)),
            )
        {
            if affected.contains(id) {
                *revision = revision.wrapping_add(1);
            }
        }
        write_delta(&transaction, &self.data, before)?;
        transaction.commit().map_err(|e| e.to_string())
    }
    /// Account deletion and its cursor cleanup must commit atomically.
    pub(super) fn store_account_deletion(
        &mut self,
        name: &str,
        removed: &BTreeSet<String>,
        before: &Data,
    ) -> Result<(), String> {
        let writer = self.db.clone();
        let mut db = writer.lock().map_err(|_| "Storage unavailable.")?;
        let transaction = db.savepoint().map_err(|e| e.to_string())?;
        rewrite_account_messages(&transaction, name, None)?;
        write_delta(&transaction, &self.data, before)?;
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

const RECEIPT_SCHEMA: &str = "CREATE TABLE bot_receipts(account_id TEXT NOT NULL,key TEXT NOT NULL,digest TEXT NOT NULL,conversation TEXT NOT NULL,sequence INTEGER NOT NULL,created INTEGER NOT NULL,PRIMARY KEY(account_id,key)) WITHOUT ROWID;
CREATE INDEX bot_receipts_created ON bot_receipts(created);";

const INDEXES: &str =
    "CREATE INDEX messages_id ON messages(json_extract(json,'$.id'),conversation);
CREATE INDEX messages_author ON messages(conversation,json_extract(json,'$.from'),sequence);
CREATE INDEX messages_time ON messages(conversation,json_extract(json,'$.time'));
CREATE INDEX messages_sequence ON messages(sequence);
CREATE INDEX entities_id ON entities(json_extract(json,'$.id')) WHERE kind IN ('room','private');";

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
    markdown: bool,
    edited: bool,
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
        markdown: message.markdown,
        edited: message.edited,
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
    media::reconcile(db, key, message)?;
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
        policy: authorization::Policy {
            revision: data.policy.revision,
            assignments: data.policy.assignments.clone(),
            grants: data.policy.grants.clone(),
            media_revocations: data.policy.media_revocations.clone(),
            ..Default::default()
        },
        bots: data.bots.clone(),
        ..Data::default()
    };
    let mut metadata = metadata;
    metadata.policy.audit.clear();
    let json = serde_json::to_string(&metadata).map_err(|e| e.to_string())?;
    db.execute(
        "INSERT INTO state VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET json=excluded.json WHERE json!=excluded.json",
        params![json],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
// Administrative mutations may affect many records. Reconcile inside the caller's
// transaction, writing only changed rows, without retaining another history copy.
fn write_data(db: &Connection, data: &Data) -> Result<(), String> {
    db.execute_batch("CREATE TEMP TABLE IF NOT EXISTS live_entities(kind TEXT,key TEXT,PRIMARY KEY(kind,key)) WITHOUT ROWID; DELETE FROM live_entities;").map_err(|e| e.to_string())?;
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
        }
    }
    db.execute_batch("DELETE FROM messages WHERE conversation IN (SELECT json_extract(e.json,'$.id') FROM entities e WHERE e.kind IN ('room','private') AND NOT EXISTS(SELECT 1 FROM entities current JOIN live_entities l ON l.kind=current.kind AND l.key=current.key WHERE current.kind=e.kind AND json_extract(current.json,'$.id')=json_extract(e.json,'$.id'))); DELETE FROM entities WHERE NOT EXISTS(SELECT 1 FROM live_entities l WHERE l.kind=entities.kind AND l.key=entities.key); DELETE FROM live_entities;").map_err(|e| e.to_string())?;
    db.execute("INSERT INTO counters VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET sequence=excluded.sequence", params![data.next_sequence]).map_err(|e| e.to_string())?;
    // Account deletion must also remove its machine request receipts. Renames
    // preserve them because they use stable account IDs.
    db.execute("DELETE FROM bot_receipts WHERE account_id NOT IN (SELECT json_extract(json,'$.id') FROM entities WHERE kind='account')", [])
        .map_err(|e| e.to_string())?;
    db.execute("DELETE FROM bot_command_receipts WHERE account_id NOT IN (SELECT json_extract(json,'$.id') FROM entities WHERE kind='account')", []).map_err(|e| e.to_string())?;
    write_audit(db, data)?;
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

    let has_audit: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='audit_entries' AND type='table')", [], |r|r.get(0)).map_err(|e|e.to_string())?;
    if has_audit {
        let mut query = db
            .prepare("SELECT json FROM audit_entries ORDER BY sequence")
            .map_err(|e| e.to_string())?;
        let rows = query
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        data.policy.audit = rows
            .map(|row| {
                serde_json::from_str(&row.map_err(|e| e.to_string())?).map_err(|e| e.to_string())
            })
            .collect::<Result<_, String>>()?;
    }
    Ok(())
}

impl Engine {
    pub(super) fn save_message_change_with_receipt(
        &mut self,
        change: &super::dispatch::MessageChange,
        receipt: Option<&crate::bots::models::Receipt>,
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
        let revision = if change.private {
            self.data.private.get(&change.key).map(|chat| chat.revision)
        } else {
            self.data.rooms.get(&change.key).map(|room| room.revision)
        };
        if change.index.is_some() && revision == change.revision {
            return Ok(());
        }
        let writer = self.db.clone();
        let mut db = writer.lock().map_err(|_| "Storage unavailable.")?;
        let tx = db.savepoint().map_err(|e| e.to_string())?;
        if !change.reaction
            && let Some(index) = change.index
        {
            put_message(&tx, id, messages.get(index).ok_or("Message not found.")?)
                .map_err(storage_error)?;
        } else if change.reaction {
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
            tx.execute("DELETE FROM messages WHERE conversation=?1 AND sequence <= (SELECT sequence FROM messages WHERE conversation=?1 ORDER BY sequence DESC LIMIT 1 OFFSET ?2)",params![id,self.max_messages]).map_err(|e|storage_error(e.to_string()))?;
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
        if let Some(receipt) = receipt {
            tx.execute(
                "DELETE FROM bot_receipts WHERE created<=?1",
                [receipt
                    .created
                    .saturating_sub(crate::bots::models::RECEIPT_TTL)],
            )
            .map_err(|e| storage_error(e.to_string()))?;
            tx.execute(
                "INSERT INTO bot_receipts VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    receipt.account_id,
                    receipt.key,
                    receipt.digest,
                    receipt.conversation,
                    receipt.sequence,
                    receipt.created
                ],
            )
            .map_err(|e| storage_error(e.to_string()))?;
        }
        tx.commit().map_err(|e| storage_error(e.to_string()))?;
        self.changed_conversation = (!change.new_conversation()).then(|| id.clone());
        if change.new_conversation() {
            self.bot_index
                .conversations
                .insert(id.clone(), format!("@private:{}", change.key));
        }
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }

    pub(super) fn bot_receipt(
        &self,
        account: &str,
        key: &str,
    ) -> Result<Option<crate::bots::models::Receipt>, String> {
        use rusqlite::OptionalExtension;
        let db = self.db.lock().map_err(|_| "Storage unavailable.")?;
        db.query_row("SELECT digest,conversation,sequence,created FROM bot_receipts WHERE account_id=?1 AND key=?2 AND created>?3", params![account, key, now().saturating_sub(crate::bots::models::RECEIPT_TTL)], |r| Ok(crate::bots::models::Receipt {
            account_id: account.into(), key: key.into(), digest: r.get(0)?, conversation: r.get(1)?, sequence: r.get(2)?, created: r.get(3)?,
        })).optional().map_err(|_| "Storage unavailable.".into())
    }

    pub(super) fn admit_bot_receipt(&self, account: &str) -> Result<(), crate::bots::error::Error> {
        use crate::bots::error::{Code, Error};
        let db = self.db.lock().map_err(Error::storage)?;
        let count: usize = db
            .query_row(
                "SELECT count(*) FROM bot_receipts WHERE account_id=?1 AND created>?2",
                params![
                    account,
                    now().saturating_sub(crate::bots::models::RECEIPT_TTL)
                ],
                |r| r.get(0),
            )
            .map_err(Error::storage)?;
        if count >= crate::bots::models::RECEIPT_LIMIT {
            return Err(Error::new(
                Code::IdempotencyCapacity,
                "Wait for receipts to expire.",
            ));
        }
        Ok(())
    }
}

// Bounded 100-row maintenance batches; the caller's transaction covers all batches.
fn rewrite_account_messages(
    db: &Connection,
    name: &str,
    new_name: Option<&str>,
) -> Result<(), String> {
    let replacement = new_name
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{name} (deleted)"));
    let mut after = 0u64;
    loop {
        let mut query=db.prepare("SELECT m.conversation,m.json FROM message_accounts a JOIN messages m ON m.conversation=a.conversation AND m.sequence=a.sequence WHERE a.name=?2 AND a.sequence>?1 ORDER BY a.sequence LIMIT 100").map_err(|e|e.to_string())?;
        let rows = query
            .query_map(params![after, name], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?;
        let batch = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        if batch.is_empty() {
            break;
        }
        for (id, json) in batch {
            let mut m: Message = serde_json::from_str(&json).map_err(|e| e.to_string())?;
            after = m.sequence;
            let mut changed = false;
            if m.from == name {
                m.from = replacement.clone();
                changed = true;
            }
            if new_name.is_some() && m.to.as_deref() == Some(name) {
                m.to = new_name.map(str::to_owned);
                changed = true;
            }
            if let Some(reply) = &mut m.reply
                && reply.from == name
            {
                reply.from = replacement.clone();
                changed = true;
            }
            if m.mentions.remove(name) {
                if let Some(new) = new_name {
                    m.mentions.insert(new.into());
                }
                changed = true;
            }
            if changed {
                put_message(db, &id, &m)?;
            }
        }
    }
    if let Some(new) = new_name {
        db.execute(
            "UPDATE OR IGNORE reactions SET username=?1 WHERE username=?2",
            params![new, name],
        )
        .map_err(|e| e.to_string())?;
    }
    db.execute("DELETE FROM reactions WHERE username=?1", [name])
        .map_err(|e| e.to_string())?;
    Ok(())
}

// This reverse index covers author, recipient, reply author and mentions. SQL
// triggers keep it correct for all mutation paths, including reply scrubbing.
const ACCOUNT_REFERENCE_SCHEMA: &str = "
CREATE TABLE message_accounts(name TEXT NOT NULL,conversation TEXT NOT NULL,sequence INTEGER NOT NULL,
 PRIMARY KEY(name,sequence,conversation), FOREIGN KEY(conversation,sequence) REFERENCES messages(conversation,sequence) ON DELETE CASCADE ON UPDATE CASCADE) WITHOUT ROWID;
CREATE INDEX message_accounts_message ON message_accounts(conversation,sequence);
CREATE INDEX reactions_username ON reactions(username,conversation);
CREATE TRIGGER message_accounts_insert AFTER INSERT ON messages BEGIN
 INSERT OR IGNORE INTO message_accounts SELECT value,NEW.conversation,NEW.sequence FROM
 (SELECT json_extract(NEW.json,'$.from') AS value UNION SELECT json_extract(NEW.json,'$.to') UNION SELECT json_extract(NEW.json,'$.reply.from') UNION SELECT value FROM json_each(NEW.json,'$.mentions')) WHERE value IS NOT NULL;
END;
CREATE TRIGGER message_accounts_update AFTER UPDATE OF json ON messages BEGIN
 DELETE FROM message_accounts WHERE conversation=OLD.conversation AND sequence=OLD.sequence;
 INSERT OR IGNORE INTO message_accounts SELECT value,NEW.conversation,NEW.sequence FROM
 (SELECT json_extract(NEW.json,'$.from') AS value UNION SELECT json_extract(NEW.json,'$.to') UNION SELECT json_extract(NEW.json,'$.reply.from') UNION SELECT value FROM json_each(NEW.json,'$.mentions')) WHERE value IS NOT NULL;
END;
INSERT OR IGNORE INTO message_accounts SELECT json_extract(json,'$.from'),conversation,sequence FROM messages WHERE json_extract(json,'$.from') IS NOT NULL;
INSERT OR IGNORE INTO message_accounts SELECT json_extract(json,'$.to'),conversation,sequence FROM messages WHERE json_extract(json,'$.to') IS NOT NULL;
INSERT OR IGNORE INTO message_accounts SELECT json_extract(json,'$.reply.from'),conversation,sequence FROM messages WHERE json_extract(json,'$.reply.from') IS NOT NULL;
INSERT OR IGNORE INTO message_accounts SELECT j.value,m.conversation,m.sequence FROM messages m,json_each(m.json,'$.mentions') j;
";
fn account_conversations(db: &Connection, name: &str) -> Result<BTreeSet<String>, String> {
    let mut query = db.prepare("SELECT conversation FROM message_accounts WHERE name=?1 UNION SELECT conversation FROM reactions WHERE username=?1").map_err(|e| e.to_string())?;
    query
        .query_map([name], |r| r.get(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(|e| e.to_string())
}

const AUDIT_SCHEMA: &str = "CREATE TABLE audit_entries(sequence INTEGER PRIMARY KEY AUTOINCREMENT,entry_id TEXT NOT NULL UNIQUE,json TEXT NOT NULL); CREATE INDEX messages_reply ON messages(conversation,json_extract(json,'$.reply.id'));";
fn write_audit(db: &Connection, data: &Data) -> Result<(), String> {
    use rusqlite::OptionalExtension;
    let last: Option<String> = db
        .query_row(
            "SELECT entry_id FROM audit_entries ORDER BY sequence DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let start = last
        .and_then(|id| data.policy.audit.iter().rposition(|entry| entry.id == id))
        .map_or(0, |index| index + 1);
    for entry in data.policy.audit.iter().skip(start) {
        let raw = serde_json::to_string(entry).map_err(|e| e.to_string())?;
        db.execute(
            "INSERT OR IGNORE INTO audit_entries(entry_id,json) VALUES(?1,?2)",
            params![entry.id, raw],
        )
        .map_err(|e| e.to_string())?;
    }
    // Append only until the bounded audit retention is reached; old records
    // are pruned without rewriting any retained audit payload.
    db.execute("DELETE FROM audit_entries WHERE sequence <= (SELECT sequence FROM audit_entries ORDER BY sequence DESC LIMIT 1 OFFSET 1000)", []).map_err(|e|e.to_string())?;
    Ok(())
}

fn write_map_delta<T: serde::Serialize + PartialEq>(
    db: &Connection,
    kind: &str,
    values: &BTreeMap<String, T>,
    before: &BTreeMap<String, T>,
) -> Result<(), String> {
    for (key, value) in values {
        if before.get(key) != Some(value) {
            put_entity(db, kind, key, value)?;
        }
    }
    for key in before.keys().filter(|key| !values.contains_key(*key)) {
        db.execute(
            "DELETE FROM entities WHERE kind=?1 AND key=?2",
            params![kind, key],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn write_delta(db: &Connection, data: &Data, before: &Data) -> Result<(), String> {
    write_map_delta(db, "account", &data.users, &before.users)?;
    write_map_delta(db, "session", &data.sessions, &before.sessions)?;
    for (kind, current, previous) in [
        (
            "room",
            data.rooms
                .iter()
                .map(|(key, r)| (key, &r.id, r.revision))
                .collect::<Vec<_>>(),
            before
                .rooms
                .iter()
                .map(|(key, r)| (key, &r.id, r.revision))
                .collect::<Vec<_>>(),
        ),
        (
            "private",
            data.private
                .iter()
                .map(|(key, c)| (key, &c.id, c.revision))
                .collect(),
            before
                .private
                .iter()
                .map(|(key, c)| (key, &c.id, c.revision))
                .collect(),
        ),
    ] {
        for (key, _, revision) in &current {
            let changed = if kind == "room" {
                data.rooms.get(*key) != before.rooms.get(*key)
            } else {
                data.private.get(*key) != before.private.get(*key)
            };
            if changed {
                if kind == "room" {
                    if let Some(room) = data.rooms.get(*key) {
                        put_entity(db, kind, key, &room_metadata(room))?;
                    }
                } else if let Some(chat) = data.private.get(*key) {
                    put_entity(db, kind, key, &private_metadata(chat))?;
                }
                db.execute(
                    "UPDATE entities SET revision=?3 WHERE kind=?1 AND key=?2 AND revision!=?3",
                    params![kind, key, revision],
                )
                .map_err(|e| e.to_string())?;
            }
        }
        let keys: BTreeSet<_> = current.iter().map(|(key, _, _)| *key).collect();
        let ids: BTreeSet<_> = current.iter().map(|(_, id, _)| *id).collect();
        for (key, id, _) in previous.iter().filter(|(key, _, _)| !keys.contains(key)) {
            if !ids.contains(id) {
                db.execute("DELETE FROM messages WHERE conversation=?1", [id])
                    .map_err(|e| e.to_string())?;
            }
            db.execute(
                "DELETE FROM entities WHERE kind=?1 AND key=?2",
                params![kind, key],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    for account in before
        .users
        .values()
        .filter(|old| !data.users.values().any(|new| new.id == old.id))
    {
        db.execute(
            "DELETE FROM bot_receipts WHERE account_id=?1",
            [&account.id],
        )
        .map_err(|e| e.to_string())?;
        db.execute(
            "DELETE FROM bot_command_receipts WHERE account_id=?1",
            [&account.id],
        )
        .map_err(|e| e.to_string())?;
    }
    if data.next_sequence != before.next_sequence {
        db.execute(
            "UPDATE counters SET sequence=?1 WHERE id=1",
            [data.next_sequence],
        )
        .map_err(|e| e.to_string())?;
    }
    write_audit(db, data)?;
    if data.policy.revision != before.policy.revision
        || data.policy.assignments != before.policy.assignments
        || data.policy.grants != before.policy.grants
        || data.policy.media_revocations != before.policy.media_revocations
        || data.bots != before.bots
    {
        write_state(db, data)?;
    }
    Ok(())
}

fn write_history_deletions(
    tx: &Connection,
    deletes: &[(String, u64)],
    cleanups: &[(String, u64)],
) -> Result<(), String> {
    for (id, sequence) in deletes {
        let message_id:String=tx.query_row("SELECT json_extract(json,'$.id') FROM messages WHERE conversation=?1 AND sequence=?2",params![id,sequence],|r|r.get(0)).map_err(|e|storage_error(e.to_string()))?;
        tx.execute("UPDATE messages SET json=json_set(json,'$.reply',NULL) WHERE conversation=?1 AND json_extract(json,'$.reply.id')=?2",params![id,message_id]).map_err(|e|storage_error(e.to_string()))?;
        tx.execute(
            "DELETE FROM messages WHERE conversation=?1 AND sequence=?2",
            params![id, sequence],
        )
        .map_err(|e| storage_error(e.to_string()))?;
    }
    for (id, cutoff) in cleanups {
        tx.execute(
            "DELETE FROM messages WHERE conversation=?1 AND json_extract(json,'$.time')<?2",
            params![id, cutoff],
        )
        .map_err(|e| storage_error(e.to_string()))?;
    }
    Ok(())
}
impl Engine {
    pub(super) fn save_sessions(
        &mut self,
        before: &BTreeMap<String, Session>,
    ) -> Result<(), String> {
        let writer = self.db.clone();
        let mut db = writer.lock().map_err(|_| "Storage unavailable.")?;
        let tx = db.savepoint().map_err(|e| e.to_string())?;
        write_map_delta(&tx, "session", &self.data.sessions, before).map_err(storage_error)?;
        tx.execute(
            "DELETE FROM entities WHERE kind='session' AND json_extract(json,'$.expires')<=?1",
            [now()],
        )
        .map_err(|e| storage_error(e.to_string()))?;
        tx.commit().map_err(|e| storage_error(e.to_string()))?;
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }
    pub(super) fn save_history_maintenance(&mut self) -> Result<(), String> {
        let ids: BTreeSet<_> = self
            .pending_deletes
            .iter()
            .chain(self.pending_cleanups.iter())
            .map(|(id, _)| id.clone())
            .collect();
        let writer = self.db.clone();
        let mut db = writer.lock().map_err(|_| "Storage unavailable.")?;
        let tx = db.savepoint().map_err(|e| e.to_string())?;
        write_history_deletions(&tx, &self.pending_deletes, &self.pending_cleanups)?;
        for (kind, key, id, revision) in self
            .data
            .rooms
            .iter()
            .map(|(key, r)| ("room", key, &r.id, r.revision))
            .chain(
                self.data
                    .private
                    .iter()
                    .map(|(key, c)| ("private", key, &c.id, c.revision)),
            )
        {
            if ids.contains(id) {
                tx.execute(
                    "UPDATE entities SET revision=?3 WHERE kind=?1 AND key=?2",
                    params![kind, key, revision],
                )
                .map_err(|e| storage_error(e.to_string()))?;
            }
        }
        write_audit(&tx, &self.data).map_err(storage_error)?;
        tx.commit().map_err(|e| storage_error(e.to_string()))?;
        self.revision = self.revision.wrapping_add(1);
        if ids.len() == 1 {
            self.changed_conversation = ids.into_iter().next();
        }
        Ok(())
    }
}

impl Engine {
    pub(super) fn save_bot_metadata(&mut self) -> Result<(), String> {
        let writer = self.db.clone();
        let mut db = writer.lock().map_err(|_| "Storage unavailable.")?;
        let tx = db.savepoint().map_err(|e| e.to_string())?;
        write_state(&tx, &self.data).map_err(storage_error)?;
        tx.commit().map_err(|e| storage_error(e.to_string()))?;
        self.bot_index = crate::bots::models::Index::build(&self.data);
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }
    pub(super) fn save_empty_private(&mut self, key: &str) -> Result<(), String> {
        let chat = self
            .data
            .private
            .get(key)
            .ok_or("Private conversation not found.")?;
        let writer = self.db.clone();
        let mut db = writer.lock().map_err(|_| "Storage unavailable.")?;
        let tx = db.savepoint().map_err(|e| e.to_string())?;
        put_entity(&tx, "private", key, &private_metadata(chat)).map_err(storage_error)?;
        tx.commit().map_err(|e| storage_error(e.to_string()))?;
        self.bot_index
            .conversations
            .insert(chat.id.clone(), format!("@private:{key}"));
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }
}
