//! Family-scoped administrative rollback. Message bodies never enter snapshots.
use super::*;

pub(super) enum ChangeFamily {
    Metadata,
    Accounts,
    Credentials,
    Policy,
    Rooms,
    RoomRename,
    AccountRename,
    DeleteAccount,
}
pub(super) struct MetadataChange {
    users: Option<BTreeMap<String, Account>>,
    rooms: Option<BTreeMap<String, Room>>,
    private: Option<BTreeMap<String, PrivateChat>>,
    sessions: Option<BTreeMap<String, Session>>,
    bots: Option<BTreeMap<String, crate::bots::models::Registration>>,
    pub(super) policy: authorization::Policy,
    next_sequence: u64,
}
// A borrowed projection lets the delta writer compare untouched maps to themselves.
// It avoids materializing a whole Data value just to pass the persistence boundary.
pub(super) struct DataView<'a> {
    pub(super) users: &'a BTreeMap<String, Account>,
    pub(super) rooms: &'a BTreeMap<String, Room>,
    pub(super) private: &'a BTreeMap<String, PrivateChat>,
    pub(super) sessions: &'a BTreeMap<String, Session>,
    pub(super) bots: &'a BTreeMap<String, crate::bots::models::Registration>,
    pub(super) policy: &'a authorization::Policy,
    pub(super) next_sequence: u64,
}
impl<'a> From<&'a Data> for DataView<'a> {
    fn from(data: &'a Data) -> Self {
        Self {
            users: &data.users,
            rooms: &data.rooms,
            private: &data.private,
            sessions: &data.sessions,
            bots: &data.bots,
            policy: &data.policy,
            next_sequence: data.next_sequence,
        }
    }
}
impl MetadataChange {
    pub(super) fn capture(data: &Data, family: ChangeFamily) -> Self {
        use ChangeFamily::*;
        Self {
            users: matches!(
                family,
                Metadata | Accounts | Credentials | Rooms | AccountRename | DeleteAccount
            )
            .then(|| data.users.clone()),
            rooms: matches!(
                family,
                Accounts | Policy | Rooms | RoomRename | AccountRename | DeleteAccount
            )
            .then(|| {
                data.rooms
                    .iter()
                    .map(|(key, room)| {
                        (
                            key.clone(),
                            Room {
                                id: room.id.clone(),
                                owner_id: room.owner_id.clone(),
                                members: room.members.clone(),
                                revision: room.revision,
                                ..Default::default()
                            },
                        )
                    })
                    .collect()
            }),
            private: matches!(family, AccountRename | DeleteAccount).then(|| {
                data.private
                    .iter()
                    .map(|(key, chat)| {
                        (
                            key.clone(),
                            PrivateChat {
                                id: chat.id.clone(),
                                revision: chat.revision,
                                ..Default::default()
                            },
                        )
                    })
                    .collect()
            }),
            sessions: matches!(
                family,
                Accounts | Credentials | AccountRename | DeleteAccount
            )
            .then(|| data.sessions.clone()),
            bots: matches!(family, DeleteAccount).then(|| data.bots.clone()),
            policy: data.policy.clone(),
            next_sequence: data.next_sequence,
        }
    }
    pub(super) fn view<'a>(&'a self, current: &'a Data) -> DataView<'a> {
        DataView {
            users: self.users.as_ref().unwrap_or(&current.users),
            rooms: self.rooms.as_ref().unwrap_or(&current.rooms),
            private: self.private.as_ref().unwrap_or(&current.private),
            sessions: self.sessions.as_ref().unwrap_or(&current.sessions),
            bots: self.bots.as_ref().unwrap_or(&current.bots),
            policy: &self.policy,
            next_sequence: self.next_sequence,
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
        if let Some(users) = self.users {
            data.users = users;
        }
        if let Some(rooms) = self.rooms {
            data.rooms = rooms;
        }
        if let Some(private) = self.private {
            data.private = private;
        }
        if let Some(sessions) = self.sessions {
            data.sessions = sessions;
        }
        if let Some(bots) = self.bots {
            data.bots = bots;
        }
        data.policy = self.policy;
        data.next_sequence = self.next_sequence;
    }
}
