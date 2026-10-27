//! Domain metadata projection and resolved-key changes. No backend SQL lives here.
use super::*;
use database::{MetadataOp, MetadataSnapshot, MetadataWrite};

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
pub(super) fn state_json(data: &Data) -> Result<String, EngineError> {
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
    Ok(serde_json::to_string(&metadata)?)
}
pub(super) fn push_state(batch: &mut MetadataWrite<'_>, data: &Data) -> Result<(), EngineError> {
    batch.operations.push(MetadataOp::State {
        json: state_json(data)?,
    });
    Ok(())
}
fn apply_entities(snapshot: MetadataSnapshot, data: &mut Data) -> Result<(), EngineError> {
    data.next_sequence = snapshot.sequence;
    for row in snapshot.entities {
        let database::EntityRow {
            kind,
            key,
            json,
            revision,
        } = row;
        match kind.as_str() {
            "account" => {
                data.users.insert(key.clone(), serde_json::from_str(&json)?);
            }
            "session" => {
                data.sessions
                    .insert(key.clone(), serde_json::from_str(&json)?);
            }
            "room" => {
                let mut room: Room = serde_json::from_str(&json)?;
                room.revision = revision;
                data.rooms.insert(key.clone(), room);
            }
            "private" => {
                let mut chat: PrivateChat = serde_json::from_str(&json)?;
                chat.revision = revision;
                data.private.insert(key.clone(), chat);
            }
            _ => return Err(EngineError::unavailable("Unknown stored entity kind.")),
        }
    }
    data.policy.audit = snapshot
        .audit
        .iter()
        .map(|raw| serde_json::from_str(raw).map_err(EngineError::from))
        .collect::<Result<_, _>>()?;
    Ok(())
}
#[cfg(test)]
pub(in crate::engine) fn load_entities(
    db: &Connection,
    data: &mut Data,
) -> Result<(), EngineError> {
    apply_entities(db.metadata_snapshot()?, data)
}
pub(super) fn write_audit<'a>(batch: &mut MetadataWrite<'a>, data: &'a Data) {
    batch.operations.push(MetadataOp::Audit(&data.policy.audit));
}
pub(super) fn write_map_delta<T: serde::Serialize + PartialEq>(
    batch: &mut MetadataWrite<'_>,
    kind: &'static str,
    values: &BTreeMap<String, T>,
    before: Option<&BTreeMap<String, T>>,
) -> Result<(), EngineError> {
    write_map_keys(batch, kind, values, before, None)
}
fn write_map_keys<T: serde::Serialize + PartialEq>(
    batch: &mut MetadataWrite<'_>,
    kind: &'static str,
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
                (Some(value), old) if Some(value) != old => batch.entity(kind, key, value)?,
                (None, Some(_)) => batch.operations.push(MetadataOp::RemoveEntity {
                    kind,
                    key: key.clone(),
                }),
                _ => {}
            }
        }
        return Ok(());
    }
    for (key, value) in values {
        if before.get(key) != Some(value) {
            batch.entity(kind, key, value)?;
        }
    }
    for key in before.keys().filter(|key| !values.contains_key(*key)) {
        batch.operations.push(MetadataOp::RemoveEntity {
            kind,
            key: key.clone(),
        });
    }
    Ok(())
}
pub(super) fn write_delta_view<'a>(
    batch: &mut MetadataWrite<'a>,
    data: &'a Data,
    before: changes::DataView<'_>,
) -> Result<(), EngineError> {
    write_delta_entities(batch, data, before)?;
    write_audit(batch, data);
    Ok(())
}
pub(super) fn write_delta_entities(
    batch: &mut MetadataWrite<'_>,
    data: &Data,
    before: changes::DataView<'_>,
) -> Result<(), EngineError> {
    write_map_keys(
        batch,
        "account",
        &data.users,
        before.users,
        before.keys.map(|keys| &keys.users),
    )?;
    write_map_keys(
        batch,
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
                        batch.entity(kind, key, &room_metadata(room))?;
                    }
                } else if let Some(chat) = data.private.get(key) {
                    batch.entity(kind, key, &private_metadata(chat))?;
                }
                batch.operations.push(MetadataOp::Revision {
                    kind,
                    key: key.clone(),
                    revision,
                });
            } else if let Some((id, _)) = previous {
                if !current_ids.contains(id) {
                    batch.operations.push(MetadataOp::RemoveMessages {
                        conversation: id.into(),
                    });
                }
                batch.operations.push(MetadataOp::RemoveEntity {
                    kind,
                    key: key.clone(),
                });
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
            batch.operations.push(MetadataOp::RemoveAccountReceipts {
                account: account.id.clone(),
            });
        }
    }
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
        push_state(batch, data)?;
    }
    Ok(())
}
pub(super) type ReadPositions = BTreeMap<String, BTreeMap<String, u64>>;
pub(super) fn load_data(db: &Connection) -> Result<Data, EngineError> {
    let snapshot = db.metadata_snapshot()?;
    let mut data: Data = serde_json::from_str(&snapshot.state)?;
    apply_entities(snapshot, &mut data)?;
    Ok(data)
}
pub(super) fn load_read_positions(db: &Connection) -> Result<ReadPositions, EngineError> {
    let mut positions = ReadPositions::new();
    for (user, view, through) in db.cursor_rows()? {
        positions.entry(user).or_default().insert(view, through);
    }
    Ok(positions)
}
