//! Committed metadata audiences use stable principals and old/new scoped access.
use super::*;
use authorization::{Action, CompiledPolicy, Scope};
use changes::DataView;

#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub(crate) struct MetadataNotice {
    all: bool,
    accounts: BTreeSet<String>,
    catalogues: BTreeSet<String>,
}
impl MetadataNotice {
    pub(crate) fn all() -> Self {
        Self {
            all: true,
            ..Default::default()
        }
    }
    pub(crate) fn targets(&self, account: &str) -> bool {
        self.all || self.accounts.contains(account)
    }
    pub(crate) fn catalogues(&self) -> impl Iterator<Item = &String> {
        self.catalogues.iter()
    }
    pub(crate) fn has_catalogues(&self) -> bool {
        !self.catalogues.is_empty()
    }
    pub(crate) fn merge(&mut self, other: &Self) {
        self.all |= other.all;
        self.accounts.extend(other.accounts.iter().cloned());
        self.catalogues.extend(other.catalogues.iter().cloned());
    }
    pub(crate) fn catalogues_only(&self) -> Self {
        Self {
            catalogues: self.catalogues.clone(),
            ..Default::default()
        }
    }
    pub(super) fn scopes(data: &Data, policy: &CompiledPolicy, scopes: &BTreeSet<Scope>) -> Self {
        let mut notice = Self::default();
        for account in data.users.values().filter(|account| !account.disabled) {
            if scopes
                .iter()
                .any(|scope| watches(policy, &account.id, scope))
            {
                notice.accounts.insert(account.id.clone());
            }
        }
        notice
    }
    pub(super) fn between(
        data: &Data,
        before: DataView<'_>,
        old: &CompiledPolicy,
        new: &CompiledPolicy,
    ) -> Self {
        let mut notice = Self {
            accounts: old.changed_profiles(new),
            ..Default::default()
        };
        let mut scopes = BTreeSet::new();
        for key in keys(
            before.keys.map(|keys| &keys.rooms),
            before.rooms,
            &data.rooms,
        ) {
            let previous = before.rooms.and_then(|rooms| rooms.get(&key));
            let current = data.rooms.get(&key);
            let changed = match (previous, current) {
                (Some(a), Some(b)) => {
                    a.id != b.id
                        || a.owner_id != b.owner_id
                        || a.members != b.members
                        || a.revision != b.revision
                }
                (None, None) => false,
                _ => true,
            };
            if changed {
                scopes.extend(
                    previous
                        .into_iter()
                        .chain(current)
                        .map(|room| Scope::Room(room.id.clone())),
                );
            }
        }
        for key in keys(
            before.keys.map(|keys| &keys.private),
            before.private,
            &data.private,
        ) {
            let previous = before.private.and_then(|chats| chats.get(&key));
            let current = data.private.get(&key);
            if previous.map(|chat| (&chat.id, chat.revision))
                != current.map(|chat| (&chat.id, chat.revision))
            {
                scopes.extend(
                    previous
                        .into_iter()
                        .chain(current)
                        .map(|chat| Scope::Private(chat.id.clone())),
                );
            }
        }
        let mut directory = false;
        let mut credentials = BTreeSet::new();
        for key in keys(
            before.keys.map(|keys| &keys.users),
            before.users,
            &data.users,
        ) {
            let previous = before.users.and_then(|users| users.get(&key));
            let current = data.users.get(&key);
            if previous == current {
                continue;
            }
            notice.accounts.extend(
                previous
                    .into_iter()
                    .chain(current)
                    .map(|account| account.id.clone()),
            );
            let identity_changed = previous.map(|account| (&account.id, account.disabled))
                != current.map(|account| (&account.id, account.disabled));
            directory |= identity_changed;
            if identity_changed {
                credentials.extend(
                    previous
                        .into_iter()
                        .chain(current)
                        .filter(|account| {
                            data.bots.contains_key(&account.id)
                                || before
                                    .bots
                                    .is_some_and(|bots| bots.contains_key(&account.id))
                        })
                        .map(|account| account.id.clone()),
                );
            }
            let avatar = |account: &Account| {
                account
                    .metadata
                    .get("@global")
                    .and_then(|metadata| metadata.get("avatar"))
                    .cloned()
            };
            fn colors(account: &Account) -> Vec<(&String, &String)> {
                account
                    .metadata
                    .iter()
                    .filter_map(|(scope, entries)| Some((scope, entries.get(colors::COLOR_KEY)?)))
                    .collect()
            }
            // Generic su metadata edits may affect historical-author profiles.
            // The public avatar API emits its narrower profile event instead.
            fn display_names(account: &Account) -> Vec<(&String, &String)> {
                account
                    .metadata
                    .iter()
                    .filter_map(|(scope, entries)| Some((scope, entries.get("displayname")?)))
                    .collect()
            }
            notice.all |= previous.map(display_names).unwrap_or_default()
                != current.map(display_names).unwrap_or_default();
            notice.all |= previous.and_then(avatar) != current.and_then(avatar)
                || previous.map(colors).unwrap_or_default()
                    != current.map(colors).unwrap_or_default();
        }
        for id in keys(before.keys.map(|keys| &keys.bots), before.bots, &data.bots) {
            let previous = before.bots.and_then(|bots| bots.get(&id));
            let current = data.bots.get(&id);
            if previous == current {
                continue;
            }
            credentials.insert(id.clone());
            notice.accounts.insert(id.clone());
            notice.accounts.extend(
                previous
                    .into_iter()
                    .chain(current)
                    .filter_map(|bot| bot.owner_id.clone()),
            );
            let names: BTreeSet<_> = data
                .users
                .iter()
                .chain(before.users.into_iter().flat_map(|users| users.iter()))
                .filter(|(_, account)| account.id == id)
                .map(|(name, _)| name.as_str())
                .collect();
            for room in data
                .rooms
                .values()
                .chain(before.rooms.into_iter().flat_map(|rooms| rooms.values()))
            {
                if names.iter().any(|name| room.members.contains(*name)) {
                    notice.catalogues.insert(room.id.clone());
                }
            }
            for (key, chat) in data
                .private
                .iter()
                .chain(before.private.into_iter().flat_map(|chats| chats.iter()))
            {
                if key.split(':').any(|name| names.contains(name)) {
                    notice.catalogues.insert(chat.id.clone());
                }
            }
        }
        let ids: BTreeSet<_> = data
            .users
            .values()
            .chain(before.users.into_iter().flat_map(|users| users.values()))
            .map(|account| account.id.clone())
            .collect();
        for id in ids {
            if credentials.iter().any(|target| {
                manages_credentials(old, &id, target) || manages_credentials(new, &id, target)
            }) || directory
                && (old
                    .effective_for(&id, &Scope::Server)
                    .has(Action::Directory)
                    || new
                        .effective_for(&id, &Scope::Server)
                        .has(Action::Directory))
                || scopes
                    .iter()
                    .any(|scope| watches(old, &id, scope) || watches(new, &id, scope))
            {
                notice.accounts.insert(id);
            }
        }
        notice
    }
}
fn manages_credentials(policy: &CompiledPolicy, actor: &str, target: &str) -> bool {
    use authorization::Group;
    let groups = policy.groups.get(actor);
    let su = groups.is_some_and(|groups| groups.contains(&Group::Su));
    (su || groups.is_some_and(|groups| groups.contains(&Group::Admin)))
        && policy.effective_for(actor, &Scope::Server).command("/bot")
        && policy
            .effective_for(actor, &Scope::Account(target.into()))
            .has(Action::BotCredentials)
        && (su
            || !policy
                .groups
                .get(target)
                .is_some_and(|groups| groups.contains(&Group::Su)))
}
fn watches(policy: &CompiledPolicy, id: &str, scope: &Scope) -> bool {
    let rights = policy.effective_for(id, scope);
    [
        Action::Read,
        Action::Discover,
        Action::Members,
        Action::PolicyRead,
    ]
    .iter()
    .any(|action| rights.has(*action))
}
fn keys<T>(
    selected: Option<&BTreeSet<String>>,
    before: Option<&BTreeMap<String, T>>,
    current: &BTreeMap<String, T>,
) -> BTreeSet<String> {
    if let Some(selected) = selected {
        return selected.clone();
    }
    before
        .map(|before| before.keys().chain(current.keys()).cloned().collect())
        .unwrap_or_default()
}
impl Engine {
    pub(crate) fn metadata_change(&self) -> std::sync::Arc<MetadataNotice> {
        self.read_model.notice(self.revision)
    }
    pub(super) fn record_metadata(&mut self, notice: MetadataNotice) {
        self.read_model.record_notice(self.revision, notice);
    }
    pub(super) fn scope_notice(&self, scopes: BTreeSet<Scope>) -> MetadataNotice {
        MetadataNotice::scopes(&self.data, &self.authorization, &scopes)
    }
}

impl Engine {
    /// Profile values change the delivery epoch but leave compiled visibility
    /// and command directories valid. Patch bounded cached avatar values once.
    pub(super) fn finish_profile_commit(&mut self, notice: MetadataNotice, names: Vec<String>) {
        let profiles = names
            .into_iter()
            .map(|name| {
                let value = self
                    .avatar_directory_for(&[name.as_str()].into())
                    .remove(&name);
                (name, value)
            })
            .collect();
        self.read_model.refresh_profiles(&profiles);
        self.revision = self.revision.wrapping_add(1);
        self.record_metadata(notice);
    }
    /// Publish metadata invalidation only after the owning savepoint succeeds.
    /// An outer command receipt can still roll it back and reload authoritative state.
    pub(super) fn finish_metadata_commit(&mut self, notice: MetadataNotice) {
        self.read_model.advance();
        self.revision = self.revision.wrapping_add(1);
        self.record_metadata(notice);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    fn snapshots(engine: &Engine) -> BTreeMap<String, serde_json::Value> {
        engine
            .data
            .users
            .iter()
            .map(|(name, account)| {
                let mut value = engine
                    .snapshot(name)
                    .map(|snapshot| serde_json::to_value(snapshot).unwrap())
                    .unwrap_or(serde_json::Value::Null);
                if let Some(object) = value.as_object_mut() {
                    object.remove("sync");
                    object.remove("policy_revision");
                    // Synchronization stamps may advance without changing this viewer's content.
                    object.remove("color_version");
                }
                (account.id.clone(), value)
            })
            .collect()
    }
    #[test]
    fn committed_audiences_cover_cold_snapshot_differences_across_lifecycle_and_policy_changes() {
        let mut engine = super::super::tests::engine();
        engine
            .provision("outside", "hash".into(), false, false)
            .unwrap();
        for command in [
            "/room new hidden",
            "/room add bob #hidden",
            "/room rename #hidden renamed",
            "/grant bob #renamed /debug",
            "/revoke bob user #renamed",
            "/room add bob #renamed",
            "/user rename bob robert",
            "/user disable robert",
            "/user enable robert",
            "/room delete #renamed",
            "/tell eve private history",
            "/user delete robert",
        ] {
            let before = snapshots(&engine);
            let actor = matches!(command, "/room new hidden" | "/tell eve private history")
                .then_some("alice");
            engine.execute(actor, None, command).unwrap();
            let after = snapshots(&engine);
            let notice = engine.metadata_change();
            for id in before.keys().chain(after.keys()) {
                if before.get(id) != after.get(id) {
                    assert!(notice.targets(id), "Missing changed viewer for {command}");
                }
            }
        }
    }
    #[test]
    fn hidden_room_and_bot_metadata_do_not_invalidate_unrelated_viewers() {
        let mut engine = super::super::tests::engine();
        let alice = engine.account_id("alice").unwrap().to_owned();
        let bob = engine.account_id("bob").unwrap().to_owned();
        let eve = engine.account_id("eve").unwrap().to_owned();
        engine
            .execute(Some("alice"), None, "/room new hidden")
            .unwrap();
        let notice = engine.metadata_change();
        assert!(notice.targets(&alice));
        assert!(!notice.targets(&bob));
        assert!(!notice.targets(&eve));
        engine.execute(None, None, "/room add bob #hidden").unwrap();
        let notice = engine.metadata_change();
        assert!(notice.targets(&alice) && notice.targets(&bob));
        assert!(!notice.targets(&eve));
        engine
            .execute(None, None, "/room kick bob #hidden")
            .unwrap();
        let notice = engine.metadata_change();
        assert!(notice.targets(&alice) && notice.targets(&bob));
        assert!(!notice.targets(&eve));
        engine
            .execute(Some("alice"), Some("hidden"), "/bot new helper")
            .unwrap();
        engine
            .execute(Some("alice"), None, "/bot token new helper observer-test")
            .unwrap();
        let notice = engine.metadata_change();
        assert!(notice.targets(engine.account_id("helper").unwrap()));
        assert!(notice.targets(&alice));
        assert!(!notice.targets(&eve));
        assert!(
            notice
                .catalogues()
                .any(|id| id == &engine.data.rooms["hidden"].id)
        );
    }
}
