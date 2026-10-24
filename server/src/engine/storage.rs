use super::*;
mod color_grants;
mod command_receipts;
mod media;
mod revision;

/// Owns the database lifetime, including readers and the exclusive folder lock.
/// The lock is dropped last, after every connection owned by the engine.
pub(super) struct Store {
    history: std::sync::Arc<super::history::HistoryStore>,
    pub(super) db: std::sync::Arc<std::sync::Mutex<Connection>>,
    pub(super) read_positions: BTreeMap<String, BTreeMap<String, u64>>,
    pub(super) pending_deletes: Vec<(String, u64)>,
    pub(super) pending_cleanups: Vec<(String, u64)>,
    _lock: Option<std::fs::File>,
}

impl Store {
    fn checkpoint(&self) -> Result<(), EngineError> {
        self.db
            .lock()
            .map_err(EngineError::unavailable)?
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(EngineError::from)
    }
}

impl Engine {
    pub(crate) fn history_store(&self) -> &std::sync::Arc<super::history::HistoryStore> {
        &self.store.history
    }

    pub fn open(path: &Path) -> Result<Self, EngineError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(EngineError::from)?;
        }
        let lock = if path == Path::new(":memory:") {
            None
        } else {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(path.with_extension("lock"))
                .map_err(EngineError::from)?;
            file.try_lock().map_err(|_| {
                EngineError::Conflict("Another server is already using this data folder.".into())
            })?;
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
        let db = Connection::open(&location).map_err(EngineError::from)?;
        let version: u32 = db
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(EngineError::from)?;
        if ![0, 17, 18, 19].contains(&version) {
            return Err(
                "Unsupported database schema; only v17, v18 and current v19 are supported.".into(),
            );
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
            let mut data = Data::default();
            data.policy
                .assignments
                .push(super::authorization::Assignment {
                    account_id: "console".into(),
                    group: super::authorization::Group::Su,
                    scope: super::authorization::Scope::Server,
                });
            let transaction = db.unchecked_transaction().map_err(EngineError::from)?;
            transaction.execute_batch("CREATE TABLE state (id INTEGER PRIMARY KEY CHECK(id=1), json TEXT NOT NULL); CREATE TABLE read_positions (username TEXT NOT NULL, conversation TEXT NOT NULL, sequence INTEGER NOT NULL, PRIMARY KEY(username,conversation));").map_err(EngineError::from)?;
            transaction
                .execute_batch(SCHEMA)
                .map_err(EngineError::from)?;
            transaction
                .execute_batch(RECEIPT_SCHEMA)
                .map_err(EngineError::from)?;
            transaction
                .execute_batch(command_receipts::SCHEMA)
                .map_err(EngineError::from)?;
            transaction
                .execute_batch(AUDIT_SCHEMA)
                .map_err(EngineError::from)?;
            write_state(&transaction, &data)?;
            transaction
                .execute("INSERT INTO counters VALUES(1,?1)", [data.next_sequence])
                .map_err(EngineError::from)?;
            transaction
                .execute_batch(INDEXES)
                .map_err(EngineError::from)?;
            transaction
                .execute_batch(ACCOUNT_REFERENCE_SCHEMA)
                .map_err(EngineError::from)?;
            transaction
                .execute_batch(media::SCHEMA)
                .map_err(EngineError::from)?;
            transaction
                .execute_batch(media::INDEXES)
                .map_err(EngineError::from)?;
            transaction
                .execute_batch(revision::SCHEMA)
                .map_err(EngineError::from)?;
            transaction
                .execute_batch("PRAGMA user_version=19;")
                .map_err(EngineError::from)?;
            transaction.commit().map_err(EngineError::from)?;
        }
        let raw: String = db
            .query_row("SELECT json FROM state WHERE id=1", [], |r| r.get(0))
            .map_err(EngineError::from)?;
        let mut data: Data = serde_json::from_str(&raw).map_err(EngineError::from)?;
        drop(raw);
        load_entities(&db, &mut data)?;
        crate::bots::models::validate(&data).map_err(EngineError::unavailable)?;
        if matches!(version, 17 | 18) {
            color_grants::upgrade_policy(&mut data);
        }
        let authorization = super::authorization::CompiledPolicy::compile(&data)
            .map_err(EngineError::unavailable)?;
        let mut read_positions: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
        {
            let mut query = db
                .prepare("SELECT username,conversation,sequence FROM read_positions")
                .map_err(EngineError::from)?;
            let rows = query
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, u64>(2)?,
                    ))
                })
                .map_err(EngineError::from)?;
            for row in rows {
                let (user, view, sequence) = row.map_err(EngineError::from)?;
                read_positions
                    .entry(user)
                    .or_default()
                    .insert(view, sequence);
            }
        }
        if matches!(version, 17 | 18) {
            let tx = db.unchecked_transaction()?;
            if version == 17 {
                revision::migrate(&tx, &mut data)?;
            }
            write_state(&tx, &data)?;
            tx.pragma_update(None, "user_version", 19)?;
            tx.commit()?;
        }
        data.sessions.retain(|_, session| session.expires > now());
        for room in data.rooms.values_mut() {
            room.messages.clear();
        }
        for chat in data.private.values_mut() {
            chat.messages.clear();
        }
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
                    super::history::HistoryStore::open(&location, memory)
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
    pub fn set_limits(
        &mut self,
        max_users: usize,
        max_rooms: usize,
        max_messages: usize,
    ) -> Result<(), EngineError> {
        crate::config::validate_limits(max_users, max_rooms, max_messages)
            .map_err(EngineError::Invalid)?;
        let writer = self.store.db.clone();
        let mut db = writer.lock().map_err(EngineError::unavailable)?;
        let tx = db.savepoint().map_err(EngineError::from)?;
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
            let deleted = revision::prune(&tx, id, max_messages)?;
            if deleted > 0 {
                let revision = revision.wrapping_add(1);
                tx.execute(
                    "UPDATE entities SET revision=?3 WHERE kind=?1 AND key=?2",
                    params![kind, key, revision],
                )
                .map_err(EngineError::from)?;
                changed.push((kind, key.clone(), revision));
            }
        }
        tx.commit().map_err(EngineError::from)?;
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
    pub(super) fn save_delta(&mut self, before: &Data) -> Result<(), EngineError> {
        self.save_metadata(&[], changes::Baseline::Data(before))
    }
    pub(super) fn save_metadata(
        &mut self,
        rooms: &[String],
        baseline: changes::Baseline<'_>,
    ) -> Result<(), EngineError> {
        let authorization = if self.authorization.revision != self.data.policy.revision {
            Some(
                super::authorization::CompiledPolicy::compile(&self.data)
                    .map_err(EngineError::unavailable)?,
            )
        } else {
            None
        };
        let notice = MetadataNotice::between(
            &self.data,
            baseline.view(),
            &self.authorization,
            authorization.as_ref().unwrap_or(&self.authorization),
        );
        let account_keys = baseline.view().keys;
        let previous_users = baseline.view().users;
        let writer = self.store.db.clone();
        let mut db = writer.lock().map_err(EngineError::unavailable)?;
        let tx = db.savepoint().map_err(EngineError::from)?;
        write_delta_view(&tx, &self.data, baseline.view())?;
        for room in rooms {
            tx.execute(
                "DELETE FROM read_positions WHERE conversation=?1",
                params![format!("room:{room}")],
            )
            .map_err(EngineError::from)?;
        }
        write_history_deletions(
            &tx,
            &self.store.pending_deletes,
            &self.store.pending_cleanups,
        )?;
        tx.commit().map_err(EngineError::from)?;
        for room in rooms {
            let key = format!("room:{room}");
            for positions in self.store.read_positions.values_mut() {
                positions.remove(&key);
            }
        }
        if let Some(authorization) = authorization {
            self.authorization = authorization;
        }
        if let Some(keys) = account_keys
            .filter(|keys| keys.rooms.is_empty() && keys.private.is_empty() && keys.bots.is_empty())
        {
            for name in &keys.users {
                if let Some(previous) = previous_users.and_then(|users| users.get(name)) {
                    self.bot_index.accounts.remove(&previous.id);
                }
                if let Some(account) = self.data.users.get(name) {
                    self.bot_index
                        .accounts
                        .insert(account.id.clone(), name.clone());
                }
            }
        } else {
            self.bot_index = crate::bots::models::Index::build(&self.data);
        }
        self.advance_colors();
        self.read_model.advance();
        self.revision = self.revision.wrapping_add(1);
        self.record_metadata(notice);
        Ok(())
    }
    pub fn checkpoint(&self) -> Result<(), EngineError> {
        self.store.checkpoint()
    }
    /// Persist a cursor separately from message state. Caller updates its cache
    /// only after this succeeds; SQL keeps the cursor monotonic across devices.
    pub(super) fn store_read_position(
        &self,
        user: &str,
        key: &str,
        through: u64,
    ) -> Result<(), EngineError> {
        self.store.db.lock().map_err(EngineError::unavailable)?.execute("INSERT INTO read_positions VALUES(?1,?2,?3) ON CONFLICT(username,conversation) DO UPDATE SET sequence=MAX(sequence,excluded.sequence)",
            params![user,key,through])?;
        Ok(())
    }
    /// Room metadata and name-based read cursors move together; history and
    /// grants remain attached to the unchanged room ID.
    pub(super) fn store_room_rename(
        &mut self,
        name: &str,
        new_name: &str,
        before: &MetadataChange,
    ) -> Result<(), EngineError> {
        let writer = self.store.db.clone();
        let mut db = writer.lock().map_err(EngineError::unavailable)?;
        let transaction = db.savepoint().map_err(EngineError::from)?;
        // A reused room name must not inherit another room's orphaned cursors.
        transaction
            .execute(
                "DELETE FROM read_positions WHERE conversation=?1",
                params![format!("room:{new_name}")],
            )
            .map_err(EngineError::from)?;
        transaction
            .execute(
                "UPDATE read_positions SET conversation=?1 WHERE conversation=?2",
                params![format!("room:{new_name}"), format!("room:{name}")],
            )
            .map_err(EngineError::from)?;
        write_delta_view(&transaction, &self.data, before.view())?;
        transaction.commit().map_err(EngineError::from)
    }
    pub(super) fn account_reference_ids(
        &self,
        name: &str,
    ) -> Result<BTreeSet<String>, EngineError> {
        let db = self.store.db.lock().map_err(EngineError::unavailable)?;
        account_conversations(&db, name)
    }
    /// Rename account state and every affected read cursor in one transaction.
    pub(super) fn store_account_rename(
        &mut self,
        name: &str,
        new_name: &str,
        conversations: &BTreeMap<String, String>,
        affected: &BTreeSet<String>,
        before: &MetadataChange,
    ) -> Result<(), EngineError> {
        let writer = self.store.db.clone();
        let mut db = writer.lock().map_err(EngineError::unavailable)?;
        let transaction = db.savepoint().map_err(EngineError::from)?;
        // A reused name must never inherit orphaned cursors from an old account.
        transaction
            .execute(
                "DELETE FROM read_positions WHERE username=?1",
                params![new_name],
            )
            .map_err(EngineError::from)?;
        transaction
            .execute(
                "UPDATE read_positions SET username=?1 WHERE username=?2",
                params![new_name, name],
            )
            .map_err(EngineError::from)?;
        for (old, new) in conversations {
            transaction
                .execute(
                    "DELETE FROM read_positions WHERE conversation=?1",
                    params![new],
                )
                .map_err(EngineError::from)?;
            transaction
                .execute(
                    "UPDATE read_positions SET conversation=?1 WHERE conversation=?2",
                    params![new, old],
                )
                .map_err(EngineError::from)?;
        }
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
        write_delta_view(&transaction, &self.data, before.view())?;
        transaction.commit().map_err(EngineError::from)
    }
    /// Account deletion and its cursor cleanup must commit atomically.
    pub(super) fn store_account_deletion(
        &mut self,
        name: &str,
        removed: &BTreeSet<String>,
        before: &MetadataChange,
    ) -> Result<(), EngineError> {
        let writer = self.store.db.clone();
        let mut db = writer.lock().map_err(EngineError::unavailable)?;
        let transaction = db.savepoint().map_err(EngineError::from)?;
        rewrite_account_messages(&transaction, name, None)?;
        write_delta_view(&transaction, &self.data, before.view())?;
        transaction
            .execute(
                "DELETE FROM read_positions WHERE username=?1",
                params![name],
            )
            .map_err(EngineError::from)?;
        for key in removed {
            transaction
                .execute(
                    "DELETE FROM read_positions WHERE conversation=?1",
                    params![key],
                )
                .map_err(EngineError::from)?;
        }
        transaction.commit().map_err(EngineError::from)
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

fn put_entity(
    db: &Connection,
    kind: &str,
    key: &str,
    value: &impl serde::Serialize,
) -> Result<(), EngineError> {
    let json = serde_json::to_string(value).map_err(EngineError::from)?;
    db.execute("INSERT INTO entities(kind,key,json) VALUES(?1,?2,?3) ON CONFLICT(kind,key) DO UPDATE SET json=excluded.json WHERE json!=excluded.json", params![kind,key,json]).map_err(EngineError::from)?;
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
fn put_message(db: &Connection, key: &str, message: &Message) -> Result<(), EngineError> {
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
    let json = serde_json::to_string(&record).map_err(EngineError::from)?;
    db.execute("INSERT INTO messages VALUES(?1,?2,?3) ON CONFLICT(conversation,sequence) DO UPDATE SET json=excluded.json WHERE json!=excluded.json", params![key,message.sequence,json]).map_err(EngineError::from)?;
    media::reconcile(db, key, message)?;
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
fn write_state(db: &Connection, data: &Data) -> Result<(), EngineError> {
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
    let json = serde_json::to_string(&metadata).map_err(EngineError::from)?;
    db.execute(
        "INSERT INTO state VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET json=excluded.json WHERE json!=excluded.json",
        params![json],
    )
    .map_err(EngineError::from)?;
    Ok(())
}
pub(super) fn load_entities(db: &Connection, data: &mut Data) -> Result<(), EngineError> {
    data.next_sequence = db
        .query_row("SELECT sequence FROM counters WHERE id=1", [], |r| r.get(0))
        .map_err(EngineError::from)?;
    let mut query = db
        .prepare("SELECT kind,key,json,revision FROM entities")
        .map_err(EngineError::from)?;
    let rows = query
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, u64>(3)?,
            ))
        })
        .map_err(EngineError::from)?;
    for row in rows {
        let (kind, key, json, revision) = row.map_err(EngineError::from)?;
        match kind.as_str() {
            "account" => {
                data.users.insert(
                    key.clone(),
                    serde_json::from_str(&json).map_err(EngineError::from)?,
                );
            }
            "session" => {
                data.sessions.insert(
                    key.clone(),
                    serde_json::from_str(&json).map_err(EngineError::from)?,
                );
            }
            "room" => {
                data.rooms.insert(
                    key.clone(),
                    serde_json::from_str(&json).map_err(EngineError::from)?,
                );
            }
            "private" => {
                data.private.insert(
                    key.clone(),
                    serde_json::from_str(&json).map_err(EngineError::from)?,
                );
            }
            _ => return Err(EngineError::unavailable("Unknown stored entity kind.")),
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

    let has_audit: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='audit_entries' AND type='table')", [], |r|r.get(0)).map_err(EngineError::from)?;
    if has_audit {
        let mut query = db
            .prepare("SELECT json FROM audit_entries ORDER BY sequence")
            .map_err(EngineError::from)?;
        let rows = query
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(EngineError::from)?;
        data.policy.audit = rows
            .map(|row| {
                serde_json::from_str(&row.map_err(EngineError::from)?).map_err(EngineError::from)
            })
            .collect::<Result<_, EngineError>>()?;
    }
    Ok(())
}

impl Engine {
    pub(super) fn save_message_change_with_receipt(
        &mut self,
        change: &super::dispatch::MessageChange,
        receipt: Option<&crate::bots::models::Receipt>,
    ) -> Result<(), EngineError> {
        let (id, messages) = if change.private {
            let chat =
                self.data.private.get(&change.key).ok_or_else(|| {
                    EngineError::NotFound("Private conversation not found.".into())
                })?;
            (&chat.id, &chat.messages)
        } else {
            let room = self
                .data
                .rooms
                .get(&change.key)
                .ok_or_else(|| EngineError::NotFound("Room not found.".into()))?;
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
        let writer = self.store.db.clone();
        let mut db = writer.lock().map_err(EngineError::unavailable)?;
        let tx = db.savepoint().map_err(EngineError::from)?;
        if !change.reaction
            && let Some(index) = change.index
        {
            put_message(
                &tx,
                id,
                messages
                    .get(index)
                    .ok_or_else(|| EngineError::NotFound("Message not found.".into()))?,
            )?;
        } else if change.reaction {
            if let Some(index) = change.index {
                let current = messages
                    .get(index)
                    .ok_or_else(|| EngineError::NotFound("Message not found.".into()))?;
                let previous = change
                    .previous
                    .as_ref()
                    .ok_or_else(|| EngineError::Internal("Missing reaction rollback.".into()))?;
                for (reaction, users) in &previous.reactions {
                    for user in users {
                        if !current
                            .reactions
                            .get(reaction)
                            .is_some_and(|users| users.contains(user))
                        {
                            tx.execute("DELETE FROM reactions WHERE conversation=?1 AND sequence=?2 AND reaction=?3 AND username=?4", params![id,current.sequence,reaction,user]).map_err(EngineError::from)?;
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
                            .map_err(EngineError::from)?;
                        }
                    }
                }
            }
        } else if let Some(message) = messages.back().filter(|m| m.sequence > change.sequence) {
            put_message(&tx, id, message)?;
            revision::prune(&tx, id, self.max_messages)?;
            tx.execute(
                "UPDATE counters SET sequence=?1 WHERE id=1",
                params![self.data.next_sequence],
            )
            .map_err(EngineError::from)?;
        }
        let (kind, revision) = if change.private {
            let chat =
                self.data.private.get(&change.key).ok_or_else(|| {
                    EngineError::NotFound("Private conversation not found.".into())
                })?;
            if change.new_conversation() {
                put_entity(&tx, "private", &change.key, &private_metadata(chat))?;
            }
            ("private", chat.revision)
        } else {
            (
                "room",
                self.data
                    .rooms
                    .get(&change.key)
                    .ok_or_else(|| EngineError::NotFound("Room not found.".into()))?
                    .revision,
            )
        };
        tx.execute(
            "UPDATE entities SET revision=?3 WHERE kind=?1 AND key=?2",
            params![kind, change.key, revision],
        )
        .map_err(EngineError::from)?;
        if let Some(receipt) = receipt {
            tx.execute(
                "DELETE FROM bot_receipts WHERE created<=?1",
                [receipt
                    .created
                    .saturating_sub(crate::bots::models::RECEIPT_TTL)],
            )
            .map_err(EngineError::from)?;
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
            .map_err(EngineError::from)?;
        }
        tx.commit().map_err(EngineError::from)?;
        self.changed_conversation = (!change.new_conversation()).then(|| id.clone());
        if change.new_conversation() {
            self.read_model.advance();
            self.bot_index
                .conversations
                .insert(id.clone(), format!("@private:{}", change.key));
        }
        self.revision = self.revision.wrapping_add(1);
        if change.new_conversation() {
            self.record_metadata(
                self.scope_notice([authorization::Scope::Private(id.clone())].into()),
            );
        }
        Ok(())
    }

    pub(super) fn bot_receipt(
        &self,
        account: &str,
        key: &str,
    ) -> Result<Option<crate::bots::models::Receipt>, EngineError> {
        use rusqlite::OptionalExtension;
        let db = self.store.db.lock().map_err(EngineError::unavailable)?;
        db.query_row("SELECT digest,conversation,sequence,created FROM bot_receipts WHERE account_id=?1 AND key=?2 AND created>?3", params![account, key, now().saturating_sub(crate::bots::models::RECEIPT_TTL)], |r| Ok(crate::bots::models::Receipt {
            account_id: account.into(), key: key.into(), digest: r.get(0)?, conversation: r.get(1)?, sequence: r.get(2)?, created: r.get(3)?,
        })).optional().map_err(EngineError::from)
    }

    pub(super) fn admit_bot_receipt(&self, account: &str) -> Result<(), crate::bots::error::Error> {
        use crate::bots::error::{Code, Error};
        let db = self.store.db.lock().map_err(Error::storage)?;
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
) -> Result<(), EngineError> {
    let replacement = new_name
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{name} (deleted)"));
    let mut after = 0u64;
    loop {
        let mut query=db.prepare("SELECT m.conversation,m.json FROM message_accounts a JOIN messages m ON m.conversation=a.conversation AND m.sequence=a.sequence WHERE a.name=?2 AND a.sequence>?1 ORDER BY a.sequence LIMIT 100").map_err(EngineError::from)?;
        let rows = query
            .query_map(params![after, name], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(EngineError::from)?;
        let batch = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(EngineError::from)?;
        if batch.is_empty() {
            break;
        }
        for (id, json) in batch {
            let mut m: Message = serde_json::from_str(&json).map_err(EngineError::from)?;
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
        .map_err(EngineError::from)?;
    }
    db.execute("DELETE FROM reactions WHERE username=?1", [name])
        .map_err(EngineError::from)?;
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
fn account_conversations(db: &Connection, name: &str) -> Result<BTreeSet<String>, EngineError> {
    let mut query = db.prepare("SELECT conversation FROM message_accounts WHERE name=?1 UNION SELECT conversation FROM reactions WHERE username=?1").map_err(EngineError::from)?;
    query
        .query_map([name], |r| r.get(0))
        .map_err(EngineError::from)?
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(EngineError::from)
}

const AUDIT_SCHEMA: &str = "CREATE TABLE audit_entries(sequence INTEGER PRIMARY KEY AUTOINCREMENT,entry_id TEXT NOT NULL UNIQUE,json TEXT NOT NULL); CREATE INDEX messages_reply ON messages(conversation,json_extract(json,'$.reply.id'));";
fn write_audit(db: &Connection, data: &Data) -> Result<(), EngineError> {
    use rusqlite::OptionalExtension;
    let last: Option<String> = db
        .query_row(
            "SELECT entry_id FROM audit_entries ORDER BY sequence DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(EngineError::from)?;
    let start = last
        .and_then(|id| data.policy.audit.iter().rposition(|entry| entry.id == id))
        .map_or(0, |index| index + 1);
    let mut inserted = 0;
    for entry in data.policy.audit.iter().skip(start) {
        let raw = serde_json::to_string(entry).map_err(EngineError::from)?;
        inserted += db
            .execute(
                "INSERT OR IGNORE INTO audit_entries(entry_id,json) VALUES(?1,?2)",
                params![entry.id, raw],
            )
            .map_err(EngineError::from)?;
    }
    // The first retained ID already defines the exact boundary. Seek its unique
    // index only after append; ordinary commits perform no pruning statement.
    if inserted > 0
        && let Some(first) = data.policy.audit.front()
    {
        db.execute("DELETE FROM audit_entries WHERE sequence < (SELECT sequence FROM audit_entries WHERE entry_id=?1)", [&first.id]).map_err(EngineError::from)?;
    }
    Ok(())
}

fn write_map_delta<T: serde::Serialize + PartialEq>(
    db: &Connection,
    kind: &str,
    values: &BTreeMap<String, T>,
    before: Option<&BTreeMap<String, T>>,
) -> Result<(), EngineError> {
    write_map_keys(db, kind, values, before, None)
}
fn write_map_keys<T: serde::Serialize + PartialEq>(
    db: &Connection,
    kind: &str,
    values: &BTreeMap<String, T>,
    before: Option<&BTreeMap<String, T>>,
    keys: Option<&BTreeSet<String>>,
) -> Result<(), EngineError> {
    let Some(before) = before else {
        return Ok(());
    };
    if let Some(keys) = keys {
        for key in keys {
            match (values.get(key), before.get(key)) {
                (Some(value), old) if Some(value) != old => put_entity(db, kind, key, value)?,
                (None, Some(_)) => {
                    db.execute(
                        "DELETE FROM entities WHERE kind=?1 AND key=?2",
                        params![kind, key],
                    )
                    .map_err(EngineError::from)?;
                }
                _ => {}
            }
        }
        return Ok(());
    }
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
        .map_err(EngineError::from)?;
    }
    Ok(())
}
fn write_delta_view(
    db: &Connection,
    data: &Data,
    before: changes::DataView<'_>,
) -> Result<(), EngineError> {
    write_map_keys(
        db,
        "account",
        &data.users,
        before.users,
        before.keys.map(|keys| &keys.users),
    )?;
    write_map_keys(
        db,
        "session",
        &data.sessions,
        before.sessions,
        before.keys.map(|keys| &keys.sessions),
    )?;
    let previous_rooms = before.rooms.unwrap_or(&data.rooms);
    let previous_private = before.private.unwrap_or(&data.private);
    for (kind, selected) in [
        ("room", before.rooms.is_some()),
        ("private", before.private.is_some()),
    ] {
        if !selected {
            continue;
        }
        let keys: BTreeSet<&String> = if let Some(keys) = before.keys {
            if kind == "room" {
                keys.rooms.iter().collect()
            } else {
                keys.private.iter().collect()
            }
        } else if kind == "room" {
            data.rooms.keys().chain(previous_rooms.keys()).collect()
        } else {
            data.private.keys().chain(previous_private.keys()).collect()
        };
        let current_ids: BTreeSet<&str> = keys
            .iter()
            .filter_map(|key| {
                if kind == "room" {
                    data.rooms.get(*key).map(|room| room.id.as_str())
                } else {
                    data.private.get(*key).map(|chat| chat.id.as_str())
                }
            })
            .collect();
        for key in keys {
            let (current, previous) = if kind == "room" {
                (
                    data.rooms
                        .get(key)
                        .map(|room| (room.id.as_str(), room.revision)),
                    previous_rooms
                        .get(key)
                        .map(|room| (room.id.as_str(), room.revision)),
                )
            } else {
                (
                    data.private
                        .get(key)
                        .map(|chat| (chat.id.as_str(), chat.revision)),
                    previous_private
                        .get(key)
                        .map(|chat| (chat.id.as_str(), chat.revision)),
                )
            };
            let changed = if kind == "room" {
                data.rooms
                    .get(key)
                    .map(|room| (&room.id, &room.owner_id, &room.members, room.revision))
                    != previous_rooms
                        .get(key)
                        .map(|room| (&room.id, &room.owner_id, &room.members, room.revision))
            } else {
                current != previous
            };
            if !changed {
                continue;
            }
            if let Some((_, revision)) = current {
                if kind == "room" {
                    if let Some(room) = data.rooms.get(key) {
                        put_entity(db, kind, key, &room_metadata(room))?;
                    }
                } else if let Some(chat) = data.private.get(key) {
                    put_entity(db, kind, key, &private_metadata(chat))?;
                }
                db.execute(
                    "UPDATE entities SET revision=?3 WHERE kind=?1 AND key=?2 AND revision!=?3",
                    params![kind, key, revision],
                )
                .map_err(EngineError::from)?;
            } else if let Some((id, _)) = previous {
                if !current_ids.contains(id) {
                    db.execute("DELETE FROM messages WHERE conversation=?1", [id])
                        .map_err(EngineError::from)?;
                }
                db.execute(
                    "DELETE FROM entities WHERE kind=?1 AND key=?2",
                    params![kind, key],
                )
                .map_err(EngineError::from)?;
            }
        }
    }
    if let Some(previous_users) = before.users {
        for (_, account) in previous_users.iter().filter(|(name, old)| {
            data.users
                .get(*name)
                .is_none_or(|current| current.id != old.id)
                && !data.users.values().any(|current| current.id == old.id)
        }) {
            db.execute(
                "DELETE FROM bot_receipts WHERE account_id=?1",
                [&account.id],
            )
            .map_err(EngineError::from)?;
            db.execute(
                "DELETE FROM bot_command_receipts WHERE account_id=?1",
                [&account.id],
            )
            .map_err(EngineError::from)?;
        }
    }
    if data.next_sequence != before.next_sequence {
        db.execute(
            "UPDATE counters SET sequence=?1 WHERE id=1",
            [data.next_sequence],
        )
        .map_err(EngineError::from)?;
    }
    write_audit(db, data)?;
    if data.policy.revision != before.policy.revision
        || data.policy.assignments != before.policy.assignments
        || data.policy.grants != before.policy.grants
        || data.policy.media_revocations != before.policy.media_revocations
        || before.bots.is_some_and(|previous| {
            before.keys.map_or_else(
                || &data.bots != previous,
                |keys| {
                    keys.bots
                        .iter()
                        .any(|key| data.bots.get(key) != previous.get(key))
                },
            )
        })
    {
        write_state(db, data)?;
    }
    Ok(())
}

fn write_history_deletions(
    tx: &Connection,
    deletes: &[(String, u64)],
    cleanups: &[(String, u64)],
) -> Result<(), EngineError> {
    for (id, sequence) in deletes {
        let message_id:String=tx.query_row("SELECT json_extract(json,'$.id') FROM messages WHERE conversation=?1 AND sequence=?2",params![id,sequence],|r|r.get(0)).map_err(EngineError::from)?;
        tx.execute("UPDATE messages SET json=json_set(json,'$.reply',NULL) WHERE conversation=?1 AND json_extract(json,'$.reply.id')=?2",params![id,message_id]).map_err(EngineError::from)?;
        tx.execute(
            "DELETE FROM messages WHERE conversation=?1 AND sequence=?2",
            params![id, sequence],
        )
        .map_err(EngineError::from)?;
    }
    for (id, cutoff) in cleanups {
        tx.execute(
            "DELETE FROM messages WHERE conversation=?1 AND json_extract(json,'$.time')<?2",
            params![id, cutoff],
        )
        .map_err(EngineError::from)?;
    }
    Ok(())
}
impl Engine {
    pub(super) fn save_sessions(
        &mut self,
        before: &BTreeMap<String, Session>,
        event: Option<&AuthEvent>,
    ) -> Result<(), EngineError> {
        let writer = self.store.db.clone();
        let mut db = writer.lock().map_err(EngineError::unavailable)?;
        let tx = db.savepoint().map_err(EngineError::from)?;
        write_map_delta(&tx, "session", &self.data.sessions, Some(before))?;
        tx.execute(
            "DELETE FROM entities WHERE kind='session' AND json_extract(json,'$.expires')<=?1",
            [now()],
        )
        .map_err(EngineError::from)?;
        if let Some(event) = event {
            revision::write_auth_event(&tx, event)?;
        }
        tx.commit().map_err(EngineError::from)?;
        self.revision = self.revision.wrapping_add(1);
        Ok(())
    }
    /// One indexed range per target, batched on a single reader connection.
    /// The existing messages_time expression index covers the cutoff predicate.
    pub(super) fn cleanup_counts(
        &self,
        ids: &[String],
        cutoff: u64,
    ) -> Result<BTreeMap<String, (usize, usize)>, EngineError> {
        self.history_store().read(|db| {
            let mut totals = BTreeMap::new();
            let mut query = db.prepare("SELECT targets.value, count(DISTINCT m.sequence), count(refs.media_id) FROM json_each(?1) AS targets LEFT JOIN messages AS m ON m.conversation=targets.value AND json_extract(m.json,'$.time')<?2 LEFT JOIN message_media AS refs ON refs.conversation=m.conversation AND refs.sequence=m.sequence GROUP BY targets.value").map_err(EngineError::unavailable)?;
            for batch in ids.chunks(256) {
                let json = serde_json::to_string(batch).map_err(EngineError::unavailable)?;
                let rows = query.query_map(params![json, cutoff], |row| Ok((row.get::<_, String>(0)?, (row.get::<_, usize>(1)?, row.get::<_, usize>(2)?)))).map_err(EngineError::unavailable)?;
                for row in rows {
                    let (id, counts) = row.map_err(EngineError::unavailable)?;
                    totals.insert(id, counts);
                }
            }
            Ok(totals)
        })
    }
    pub(super) fn save_history_maintenance(&mut self) -> Result<(), EngineError> {
        if self.store.pending_deletes.is_empty() && self.store.pending_cleanups.is_empty() {
            return Ok(());
        }
        let ids: BTreeSet<_> = self
            .store
            .pending_deletes
            .iter()
            .chain(self.store.pending_cleanups.iter())
            .map(|(id, _)| id.clone())
            .collect();
        let scopes = ids
            .iter()
            .filter_map(|id| {
                self.bot_index.conversations.get(id).map(|view| {
                    if view.starts_with("@private:") {
                        authorization::Scope::Private(id.clone())
                    } else {
                        authorization::Scope::Room(id.clone())
                    }
                })
            })
            .collect();
        let notice = self.scope_notice(scopes);
        let writer = self.store.db.clone();
        let mut db = writer.lock().map_err(EngineError::unavailable)?;
        let tx = db.savepoint().map_err(EngineError::from)?;
        write_history_deletions(
            &tx,
            &self.store.pending_deletes,
            &self.store.pending_cleanups,
        )?;
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
                .map_err(EngineError::from)?;
            }
        }
        write_audit(&tx, &self.data)?;
        tx.commit().map_err(EngineError::from)?;
        self.revision = self.revision.wrapping_add(1);
        self.record_metadata(notice);
        if ids.len() == 1 {
            self.changed_conversation = ids.into_iter().next();
        }
        Ok(())
    }
}

impl Engine {
    pub(super) fn save_bot_metadata(&mut self, before: &MetadataChange) -> Result<(), EngineError> {
        let notice = MetadataNotice::between(
            &self.data,
            before.view(),
            &self.authorization,
            &self.authorization,
        );
        let writer = self.store.db.clone();
        let mut db = writer.lock().map_err(EngineError::unavailable)?;
        let tx = db.savepoint().map_err(EngineError::from)?;
        write_state(&tx, &self.data)?;
        tx.commit().map_err(EngineError::from)?;
        self.bot_index = crate::bots::models::Index::build(&self.data);
        self.read_model.advance();
        self.revision = self.revision.wrapping_add(1);
        self.record_metadata(notice);
        Ok(())
    }

    pub(super) fn save_empty_private(&mut self, key: &str) -> Result<(), EngineError> {
        let chat = self
            .data
            .private
            .get(key)
            .ok_or_else(|| EngineError::NotFound("Private conversation not found.".into()))?;
        let writer = self.store.db.clone();
        let mut db = writer.lock().map_err(EngineError::unavailable)?;
        let tx = db.savepoint().map_err(EngineError::from)?;
        put_entity(&tx, "private", key, &private_metadata(chat))?;
        tx.commit().map_err(EngineError::from)?;
        self.bot_index
            .conversations
            .insert(chat.id.clone(), format!("@private:{key}"));
        self.read_model.advance();
        self.revision = self.revision.wrapping_add(1);
        self.record_metadata(
            self.scope_notice(
                [authorization::Scope::Private(
                    self.data.private[key].id.clone(),
                )]
                .into(),
            ),
        );
        Ok(())
    }
}
