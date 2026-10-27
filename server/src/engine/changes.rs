//! Resolved-key administrative rollback. Message bodies never enter snapshots.
use super::*;

#[derive(Clone, Default)]
pub(super) struct DirtyKeys {
    pub(super) users: BTreeSet<String>,
    pub(super) sessions: BTreeSet<String>,
    pub(super) rooms: BTreeSet<String>,
    pub(super) private: BTreeSet<String>,
    pub(super) bots: BTreeSet<String>,
}
#[derive(Clone)]
pub(super) struct MetadataChange {
    keys: Option<DirtyKeys>,
    users: Option<BTreeMap<String, Account>>,
    rooms: Option<BTreeMap<String, Room>>,
    private: Option<BTreeMap<String, PrivateChat>>,
    sessions: Option<BTreeMap<String, Session>>,
    bots: Option<BTreeMap<String, crate::bots::models::Registration>>,
    pub(super) policy: authorization::Policy,
    next_sequence: u64,
}
// Explicit optional families determine persistence work; no pointer identity is used.
pub(super) struct DataView<'a> {
    pub(super) keys: Option<&'a DirtyKeys>,
    pub(super) users: Option<&'a BTreeMap<String, Account>>,
    pub(super) rooms: Option<&'a BTreeMap<String, Room>>,
    pub(super) private: Option<&'a BTreeMap<String, PrivateChat>>,
    pub(super) sessions: Option<&'a BTreeMap<String, Session>>,
    pub(super) bots: Option<&'a BTreeMap<String, crate::bots::models::Registration>>,
    pub(super) policy: &'a authorization::Policy,
}
impl<'a> From<&'a Data> for DataView<'a> {
    fn from(data: &'a Data) -> Self {
        Self {
            keys: None,
            users: Some(&data.users),
            rooms: Some(&data.rooms),
            private: Some(&data.private),
            sessions: Some(&data.sessions),
            bots: Some(&data.bots),
            policy: &data.policy,
        }
    }
}
impl MetadataChange {
    pub(super) fn capture_current(&self, data: &Data) -> Result<Self, EngineError> {
        let keys = self
            .keys
            .clone()
            .ok_or("Metadata targets are unavailable.")?;
        Ok(Self::keyed(data, keys))
    }
    // The caller has already resolved and authorized the complete target set.
    // Missing selected names are retained as keys so inserts roll back as well.
    pub(super) fn accounts(
        data: &Data,
        names: impl IntoIterator<Item = String>,
        credentials: bool,
    ) -> Self {
        let users: BTreeSet<_> = names.into_iter().collect();
        let sessions = if credentials {
            data.sessions
                .iter()
                .filter(|(_, session)| users.contains(&session.username))
                .map(|(key, _)| key.clone())
                .collect()
        } else {
            BTreeSet::new()
        };
        Self::keyed(
            data,
            DirtyKeys {
                users,
                sessions,
                ..Default::default()
            },
        )
    }
    pub(super) fn keyed(data: &Data, keys: DirtyKeys) -> Self {
        let users = (!keys.users.is_empty()).then(|| {
            keys.users
                .iter()
                .filter_map(|key| {
                    data.users
                        .get(key)
                        .map(|value| (key.clone(), value.clone()))
                })
                .collect()
        });
        let sessions = (!keys.sessions.is_empty()).then(|| {
            keys.sessions
                .iter()
                .filter_map(|key| {
                    data.sessions
                        .get(key)
                        .map(|value| (key.clone(), value.clone()))
                })
                .collect()
        });
        let rooms = (!keys.rooms.is_empty()).then(|| {
            keys.rooms
                .iter()
                .filter_map(|key| data.rooms.get(key).map(|room| (key.clone(), room.clone())))
                .collect()
        });
        let private = (!keys.private.is_empty()).then(|| {
            keys.private
                .iter()
                .filter_map(|key| {
                    data.private
                        .get(key)
                        .map(|chat| (key.clone(), chat.clone()))
                })
                .collect()
        });
        let bots = (!keys.bots.is_empty()).then(|| {
            keys.bots
                .iter()
                .filter_map(|key| data.bots.get(key).map(|value| (key.clone(), value.clone())))
                .collect()
        });
        Self {
            keys: Some(keys),
            users,
            sessions,
            rooms,
            private,
            bots,
            policy: data.policy.clone(),
            next_sequence: data.next_sequence,
        }
    }
    pub(super) fn include_new_bot(&mut self, id: String) {
        if let Some(keys) = &mut self.keys {
            keys.bots.insert(id);
            self.bots.get_or_insert_with(BTreeMap::new);
        }
    }
    pub(super) fn unchanged(&self, data: &Data) -> bool {
        let Some(keys) = &self.keys else {
            return false;
        };
        keys.users
            .iter()
            .all(|key| data.users.get(key) == self.users.as_ref().and_then(|map| map.get(key)))
            && keys.sessions.iter().all(|key| {
                data.sessions.get(key) == self.sessions.as_ref().and_then(|map| map.get(key))
            })
            && keys.rooms.iter().all(|key| {
                data.rooms
                    .get(key)
                    .map(|room| (&room.id, &room.owner_id, &room.members, room.revision))
                    == self
                        .rooms
                        .as_ref()
                        .and_then(|map| map.get(key))
                        .map(|room| (&room.id, &room.owner_id, &room.members, room.revision))
            })
            && keys.private.iter().all(|key| {
                data.private.get(key).map(|chat| (&chat.id, chat.revision))
                    == self
                        .private
                        .as_ref()
                        .and_then(|map| map.get(key))
                        .map(|chat| (&chat.id, chat.revision))
            })
            && keys
                .bots
                .iter()
                .all(|key| data.bots.get(key) == self.bots.as_ref().and_then(|map| map.get(key)))
            && data.policy == self.policy
            && data.next_sequence == self.next_sequence
    }
    pub(super) fn view(&self) -> DataView<'_> {
        DataView {
            keys: self.keys.as_ref(),
            users: self.users.as_ref(),
            rooms: self.rooms.as_ref(),
            private: self.private.as_ref(),
            sessions: self.sessions.as_ref(),
            bots: self.bots.as_ref(),
            policy: &self.policy,
        }
    }
    pub(super) fn removed_rooms(&self, current: &Data) -> Vec<String> {
        self.rooms
            .iter()
            .flat_map(|rooms| rooms.keys())
            .filter(|name| !current.rooms.contains_key(*name))
            .cloned()
            .collect()
    }
    pub(super) fn restore(self, data: &mut Data) {
        restore_map(
            &mut data.users,
            self.users,
            self.keys.as_ref().map(|keys| &keys.users),
        );
        restore_map(
            &mut data.sessions,
            self.sessions,
            self.keys.as_ref().map(|keys| &keys.sessions),
        );
        restore_map(
            &mut data.rooms,
            self.rooms,
            self.keys.as_ref().map(|keys| &keys.rooms),
        );
        restore_map(
            &mut data.private,
            self.private,
            self.keys.as_ref().map(|keys| &keys.private),
        );
        restore_map(
            &mut data.bots,
            self.bots,
            self.keys.as_ref().map(|keys| &keys.bots),
        );
        data.policy = self.policy;
        data.next_sequence = self.next_sequence;
    }
}

impl Engine {
    pub(super) fn command_metadata_change(
        &self,
        view: Option<&str>,
        parts: &[&str],
    ) -> MetadataChange {
        let mut keys = DirtyKeys::default();
        let mut room = None;
        match parts {
            ["/new", "room", name, ..] => room = Some(*name),
            ["/rm", "room", name, ..] => {
                room = Some(*name);
                if let Some(target) = self.data.rooms.get(*name) {
                    keys.users.extend(
                        self.data
                            .users
                            .iter()
                            .filter(|(_, user)| user.metadata.contains_key(&target.id))
                            .map(|(name, _)| name.clone()),
                    );
                }
            }
            ["/add" | "/kick" | "/owner", _, rest @ ..] => room = rest.first().copied().or(view),
            ["/join" | "/leave", rest @ ..] => room = rest.first().copied().or(view),
            [command @ ("/enable" | "/disable"), name, ..] => {
                keys.users.insert((*name).into());
                if *command == "/disable" {
                    keys.sessions.extend(
                        self.data
                            .sessions
                            .iter()
                            .filter(|(_, session)| session.username == *name)
                            .map(|(key, _)| key.clone()),
                    );
                    if let Some(account) = self.data.users.get(*name) {
                        keys.rooms.extend(
                            self.data
                                .rooms
                                .iter()
                                .filter(|(_, room)| {
                                    room.owner_id == account.id || room.members.contains(*name)
                                })
                                .map(|(name, _)| name.clone()),
                        );
                    }
                }
            }
            ["/grant" | "/revoke", _, target, rest @ ..] => {
                room = if authorization::Group::parse(target).is_ok() {
                    rest.first().copied()
                } else {
                    Some(*target)
                };
            }
            ["/rm", "group", _, _, rest @ ..] => room = rest.first().copied(),
            _ => {}
        }
        if let Some(room) = room.filter(|room| !room.starts_with('@')) {
            keys.rooms.insert(room.trim_start_matches('#').into());
        }
        MetadataChange::keyed(&self.data, keys)
    }
}

fn restore_map<T>(
    data: &mut BTreeMap<String, T>,
    previous: Option<BTreeMap<String, T>>,
    keys: Option<&BTreeSet<String>>,
) {
    if let Some(previous) = previous {
        if let Some(keys) = keys {
            for key in keys {
                data.remove(key);
            }
            data.extend(previous);
        } else {
            *data = previous;
        }
    }
}

#[cfg(test)]
pub(super) enum Baseline<'a> {
    #[cfg(test)]
    Data(&'a Data),
    Captured(&'a MetadataChange),
}
#[cfg(test)]
impl Baseline<'_> {
    pub(super) fn view(&self) -> DataView<'_> {
        match self {
            #[cfg(test)]
            Self::Data(data) => (*data).into(),
            Self::Captured(change) => change.view(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn command_plans_capture_only_selected_rooms_accounts_and_referenced_metadata() {
        let mut engine = super::super::tests::engine();
        engine.execute(None, None, "/room new unrelated").unwrap();
        let room_id = engine.data.rooms["lobby"].id.clone();
        engine
            .data
            .users
            .get_mut("bob")
            .unwrap()
            .metadata
            .insert(room_id, [("key".into(), "value".into())].into());
        let add = engine.command_metadata_change(None, &["/add", "eve", "lobby"]);
        assert_eq!(
            add.rooms.as_ref().unwrap().keys().collect::<Vec<_>>(),
            ["lobby"]
        );
        assert!(add.users.is_none() && add.private.is_none() && add.sessions.is_none());
        let policy = engine.command_metadata_change(None, &["/grant", "bob", "user", "@global"]);
        assert!(policy.users.is_none() && policy.rooms.is_none() && policy.private.is_none());
        let disabled = engine.command_metadata_change(None, &["/disable", "bob"]);
        assert_eq!(
            disabled.rooms.as_ref().unwrap().keys().collect::<Vec<_>>(),
            ["lobby"]
        );
        assert_eq!(
            disabled.users.as_ref().unwrap().keys().collect::<Vec<_>>(),
            ["bob"]
        );
        let deleted = engine.command_metadata_change(None, &["/rm", "room", "lobby"]);
        assert_eq!(
            deleted.rooms.as_ref().unwrap().keys().collect::<Vec<_>>(),
            ["lobby"]
        );
        assert_eq!(
            deleted.users.as_ref().unwrap().keys().collect::<Vec<_>>(),
            ["bob"]
        );
        assert!(deleted.sessions.is_none() && deleted.bots.is_none());
    }
    #[test]
    fn keyed_account_snapshots_capture_only_resolved_rows_and_restore_only_their_keys() {
        let mut engine = super::super::tests::engine();
        engine
            .login("alice-one".into(), "alice", "test-hash", None)
            .unwrap();
        engine
            .login("bob-one".into(), "bob", "test-hash", None)
            .unwrap();
        let before = MetadataChange::accounts(&engine.data, ["alice".into()], true);
        assert_eq!(
            before.users.as_ref().unwrap().keys().collect::<Vec<_>>(),
            ["alice"]
        );
        assert_eq!(before.sessions.as_ref().unwrap().len(), 1);
        assert!(before.rooms.is_none() && before.private.is_none() && before.bots.is_none());
        engine.data.users.get_mut("alice").unwrap().hash = "changed".into();
        engine.data.sessions.remove(&session_digest("alice-one"));
        // A keyed rollback has no ownership of other rows.
        engine.data.users.get_mut("bob").unwrap().hash = "unrelated".into();
        before.restore(&mut engine.data);
        assert_eq!(engine.data.users["alice"].hash, "test-hash");
        assert_eq!(engine.data.users["bob"].hash, "unrelated");
        assert_eq!(engine.session("alice-one").as_deref(), Some("alice"));
        assert_eq!(engine.session("bob-one").as_deref(), Some("bob"));
        let new = MetadataChange::accounts(&engine.data, ["new".into()], false);
        assert!(new.users.as_ref().unwrap().is_empty());
        engine
            .data
            .users
            .insert("new".into(), engine.data.users["bob"].clone());
        new.restore(&mut engine.data);
        assert!(!engine.data.users.contains_key("new"));
        assert!(engine.data.users.contains_key("bob"));
    }
    #[test]
    fn failed_keyed_password_and_provisioning_restore_sessions_and_identity_indexes() {
        let mut engine = super::super::tests::engine();
        for (token, user) in [("keep", "alice"), ("other", "alice"), ("bob", "bob")] {
            engine.login(token.into(), user, "test-hash", None).unwrap();
        }
        let revision = engine.revision;
        let metadata = engine.read_model.revision;
        engine.store.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_keyed BEFORE INSERT ON entities WHEN NEW.kind='account' AND NEW.key IN ('alice','new') BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(
            engine
                .change_password("alice", "test-hash", "new-hash".into(), "keep")
                .is_err()
        );
        assert!(
            engine
                .provision("new", "hash".into(), false, false)
                .is_err()
        );
        assert_eq!(engine.revision, revision);
        assert_eq!(engine.read_model.revision, metadata);
        assert_eq!(engine.data.users["alice"].hash, "test-hash");
        assert!(!engine.bot_index.accounts.values().any(|name| name == "new"));
        for (token, user) in [("keep", "alice"), ("other", "alice"), ("bob", "bob")] {
            assert_eq!(engine.session(token).as_deref(), Some(user));
        }
        engine
            .store
            .db
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER reject_keyed;")
            .unwrap();
        engine
            .change_password("alice", "test-hash", "new-hash".into(), "keep")
            .unwrap();
        assert!(engine.session("other").is_none());
        assert_eq!(engine.session("bob").as_deref(), Some("bob"));
        engine
            .provision("new", "hash".into(), false, false)
            .unwrap();
        assert_eq!(
            engine.bot_index.accounts[&engine.data.users["new"].id],
            "new"
        );
    }
}
