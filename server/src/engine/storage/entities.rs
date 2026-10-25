use super::*;

pub(super) fn put_entity(
    db: &Connection,
    kind: &str,
    key: &str,
    value: &impl serde::Serialize,
) -> Result<(), EngineError> {
    let json = serde_json::to_string(value).map_err(EngineError::from)?;
    if db.is_postgres() {
        let projection = database::entity_projection(&json)?;
        db.execute("INSERT INTO entities(kind,key,json,projection) VALUES($1,$2,$3,$4::text::jsonb) ON CONFLICT(kind,key) DO UPDATE SET json=excluded.json,projection=excluded.projection WHERE entities.json!=excluded.json",params![kind,key,json,projection])?;
    } else {
        db.execute("INSERT INTO entities(kind,key,json) VALUES(?1,?2,?3) ON CONFLICT(kind,key) DO UPDATE SET json=excluded.json WHERE json!=excluded.json",params![kind,key,json])?;
    }
    Ok(())
}
// Keep the existing JSON representation while borrowing metadata directly.
#[derive(serde::Serialize)]
pub(super) struct RoomMetadata<'a> {
    id: &'a str,
    owner_id: &'a str,
    members: &'a BTreeSet<String>,
    messages: &'static [Message],
    revision: u64,
}
#[derive(serde::Serialize)]
pub(super) struct PrivateMetadata<'a> {
    id: &'a str,
    messages: &'static [Message],
    revision: u64,
}
pub(super) fn room_metadata(room: &Room) -> RoomMetadata<'_> {
    RoomMetadata {
        id: &room.id,
        owner_id: &room.owner_id,
        members: &room.members,
        messages: &[],
        revision: 0,
    }
}
pub(super) fn private_metadata(chat: &PrivateChat) -> PrivateMetadata<'_> {
    PrivateMetadata {
        id: &chat.id,
        messages: &[],
        revision: 0,
    }
}
#[derive(serde::Serialize)]
struct PolicyMetadata<'a> {
    revision: u64,
    assignments: &'a [authorization::Assignment],
    grants: &'a [authorization::Grant],
    media_revocations: &'a BTreeMap<String, BTreeSet<String>>,
    audit: &'static [authorization::Audit],
}
#[derive(serde::Serialize)]
struct StateMetadata<'a> {
    bots: &'a BTreeMap<String, crate::bots::models::Registration>,
    users: BTreeMap<String, ()>,
    rooms: BTreeMap<String, ()>,
    private: BTreeMap<String, ()>,
    next_sequence: u64,
    sessions: BTreeMap<String, ()>,
    policy: PolicyMetadata<'a>,
}
pub(super) fn write_state(db: &Connection, data: &Data) -> Result<(), EngineError> {
    let metadata = StateMetadata {
        bots: &data.bots,
        users: BTreeMap::new(),
        rooms: BTreeMap::new(),
        private: BTreeMap::new(),
        next_sequence: 0,
        sessions: BTreeMap::new(),
        policy: PolicyMetadata {
            revision: data.policy.revision,
            assignments: &data.policy.assignments,
            grants: &data.policy.grants,
            media_revocations: &data.policy.media_revocations,
            audit: &[],
        },
    };
    let json = serde_json::to_string(&metadata)?;
    db.execute(sql("INSERT INTO state VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET json=excluded.json WHERE json!=excluded.json", "INSERT INTO state VALUES(1,$1) ON CONFLICT(id) DO UPDATE SET json=excluded.json WHERE state.json!=excluded.json"), params![json])?;
    Ok(())
}
pub(in crate::engine) fn load_entities(
    db: &Connection,
    data: &mut Data,
) -> Result<(), EngineError> {
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
    Ok(())
}

pub(super) fn write_audit(db: &Connection, data: &Data) -> Result<(), EngineError> {
    use crate::engine::database::OptionalExtension;
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
                sql("INSERT OR IGNORE INTO audit_entries(entry_id,json) VALUES(?1,?2)", "INSERT INTO audit_entries(entry_id,json) VALUES($1,$2) ON CONFLICT(entry_id) DO NOTHING"),
                params![entry.id, raw],
            )
            .map_err(EngineError::from)?;
    }
    // The first retained ID already defines the exact boundary. Seek its unique
    // index only after append; ordinary commits perform no pruning statement.
    if inserted > 0
        && let Some(first) = data.policy.audit.front()
    {
        db.execute(sql("DELETE FROM audit_entries WHERE sequence < (SELECT sequence FROM audit_entries WHERE entry_id=?1)", "DELETE FROM audit_entries WHERE sequence < (SELECT sequence FROM audit_entries WHERE entry_id=$1)"), [&first.id]).map_err(EngineError::from)?;
    }
    Ok(())
}

pub(super) fn write_map_delta<T: serde::Serialize + PartialEq>(
    db: &Connection,
    kind: &str,
    values: &BTreeMap<String, T>,
    before: Option<&BTreeMap<String, T>>,
) -> Result<(), EngineError> {
    write_map_keys(db, kind, values, before, None)
}
pub(super) fn write_map_keys<T: serde::Serialize + PartialEq>(
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
                        sql(
                            "DELETE FROM entities WHERE kind=?1 AND key=?2",
                            "DELETE FROM entities WHERE kind=$1 AND key=$2",
                        ),
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
            sql(
                "DELETE FROM entities WHERE kind=?1 AND key=?2",
                "DELETE FROM entities WHERE kind=$1 AND key=$2",
            ),
            params![kind, key],
        )
        .map_err(EngineError::from)?;
    }
    Ok(())
}
pub(super) fn write_delta_view(
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
                    sql(
                        "UPDATE entities SET revision=?3 WHERE kind=?1 AND key=?2 AND revision!=?3",
                        "UPDATE entities SET revision=$3 WHERE kind=$1 AND key=$2 AND revision!=$3",
                    ),
                    params![kind, key, revision],
                )
                .map_err(EngineError::from)?;
            } else if let Some((id, _)) = previous {
                if !current_ids.contains(id) {
                    db.execute(
                        sql(
                            "DELETE FROM messages WHERE conversation=?1",
                            "DELETE FROM messages WHERE conversation=$1",
                        ),
                        [id],
                    )
                    .map_err(EngineError::from)?;
                }
                db.execute(
                    sql(
                        "DELETE FROM entities WHERE kind=?1 AND key=?2",
                        "DELETE FROM entities WHERE kind=$1 AND key=$2",
                    ),
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
                sql(
                    "DELETE FROM bot_receipts WHERE account_id=?1",
                    "DELETE FROM bot_receipts WHERE account_id=$1",
                ),
                [&account.id],
            )
            .map_err(EngineError::from)?;
            db.execute(
                sql(
                    "DELETE FROM bot_command_receipts WHERE account_id=?1",
                    "DELETE FROM bot_command_receipts WHERE account_id=$1",
                ),
                [&account.id],
            )
            .map_err(EngineError::from)?;
        }
    }
    if data.next_sequence != before.next_sequence {
        db.execute(
            sql(
                "UPDATE counters SET sequence=?1 WHERE id=1",
                "UPDATE counters SET sequence=$1 WHERE id=1",
            ),
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

pub(super) type ReadPositions = BTreeMap<String, BTreeMap<String, u64>>;

pub(super) fn load_data(db: &Connection) -> Result<Data, EngineError> {
    let raw: String = db.query_row("SELECT json FROM state WHERE id=1", [], |r| r.get(0))?;
    let mut data: Data = serde_json::from_str(&raw)?;
    load_entities(db, &mut data)?;
    Ok(data)
}

pub(super) fn load_read_positions(db: &Connection) -> Result<ReadPositions, EngineError> {
    let mut positions = BTreeMap::<String, BTreeMap<String, u64>>::new();
    let mut query = db.prepare("SELECT username,conversation,sequence FROM read_positions")?;
    let rows = query.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, u64>(2)?,
        ))
    })?;
    for row in rows {
        let (user, view, through) = row?;
        positions.entry(user).or_default().insert(view, through);
    }
    Ok(positions)
}
