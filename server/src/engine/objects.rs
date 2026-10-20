//! Object-oriented commands and bounded, scoped account metadata.
use super::*;
use authorization::{Action, Scope};

impl Engine {
    pub(super) fn object_scope(
        &self,
        actor: Option<&str>,
        view: Option<&str>,
        value: Option<&str>,
    ) -> Result<Scope, String> {
        let scope = match value {
            Some(value) if value.starts_with('#') || value.starts_with('@') => {
                self.policy_scope(actor, value, false)
            }
            Some(_) => Err("Room scopes require #name.".into()),
            None => self.command_scope(actor, view),
        }?;
        match scope {
            Scope::Server | Scope::Room(_) => Ok(scope),
            _ => Err(
                "This command needs a room or @global; specify the scope outside a room.".into(),
            ),
        }
    }
    pub(super) fn scope_users(
        &self,
        actor: Option<&str>,
        scope: &Scope,
    ) -> Result<Vec<String>, String> {
        match scope {
            Scope::Server => {
                self.require(actor, scope, Action::Directory)?;
                Ok(self.data.users.keys().cloned().collect())
            }
            Scope::Room(id) => {
                self.require(actor, scope, Action::Members)?;
                Ok(self
                    .data
                    .rooms
                    .values()
                    .find(|r| &r.id == id)
                    .ok_or("Room not found.")?
                    .members
                    .iter()
                    .cloned()
                    .collect())
            }
            _ => Err("Select @global or a room scope.".into()),
        }
    }
    fn metadata_targets(
        &self,
        actor: Option<&str>,
        who: &str,
        scope: &Scope,
    ) -> Result<Vec<String>, String> {
        if who == "@all" {
            return self.scope_users(actor, scope);
        }
        let name = who.strip_prefix('@').unwrap_or(who);
        if !self.data.users.contains_key(name) {
            return Err("User not found.".into());
        }
        if let Scope::Room(id) = scope
            && !self
                .data
                .rooms
                .values()
                .any(|r| &r.id == id && r.members.contains(name))
        {
            return Err("User is not a member of this scope.".into());
        }
        Ok(vec![name.into()])
    }
    fn metadata_access(
        &self,
        actor: Option<&str>,
        name: &str,
        scope: &Scope,
        write: bool,
    ) -> Result<(), String> {
        let account = self.data.users.get(name).ok_or("User not found.")?;
        let target = if *scope == Scope::Server {
            Scope::Account(account.id.clone())
        } else {
            scope.clone()
        };
        self.require_target_command(actor, &target, "/user")?;
        if self.is_su(actor) {
            return Ok(());
        }
        if write {
            return Err("Only su can change user metadata.".into());
        }
        if actor == Some(name) {
            return Ok(());
        }
        let action = Action::AccountMetadataRead;
        if self.allows(actor, &Scope::Account(account.id.clone()), action) {
            return Ok(());
        }
        if matches!(scope, Scope::Room(_)) {
            self.require(actor, scope, Action::PolicyWrite)
        } else {
            self.require(actor, scope, action)
        }
    }
    pub(super) fn metadata_scope_key(scope: &Scope) -> Result<&str, String> {
        match scope {
            Scope::Server => Ok("@global"),
            Scope::Room(id) | Scope::Private(id) => Ok(id),
            _ => Err("Metadata scope must be @global or a room.".into()),
        }
    }
    pub(super) fn metadata_value(&self, user: &str, scope: &Scope, key: &str) -> Option<&str> {
        let metadata = &self.data.users.get(user)?.metadata;
        metadata
            .get(Self::metadata_scope_key(scope).ok()?)
            .and_then(|m| m.get(key))
            .or_else(|| metadata.get("@global").and_then(|m| m.get(key)))
            .map(String::as_str)
    }
    pub(super) fn query_objects(
        &self,
        context: &dispatch::CommandContext<'_>,
    ) -> Result<String, String> {
        let parts = context.parts;
        if parts == ["/room", "ls"] {
            return Ok(self
                .data
                .rooms
                .iter()
                .filter(|(_, r)| {
                    self.allows(context.actor, &Scope::Room(r.id.clone()), Action::Discover)
                        || self.allows(context.actor, &Scope::Room(r.id.clone()), Action::Read)
                })
                .map(|(n, _)| format!("#{n}"))
                .collect::<Vec<_>>()
                .join("\n"));
        }
        match parts {
            ["/user", "ls", rest @ ..] if rest.len() <= 1 => {
                let scope =
                    self.object_scope(context.actor, context.view, rest.first().copied())?;
                self.require_target_command(context.actor, &scope, "/user")?;
                Ok(self.scope_users(context.actor, &scope)?.join("\n"))
            }
            ["/user", "get", who, rest @ ..] if rest.len() <= 2 => {
                let (key, scope) = match rest {
                    [] => (None, None),
                    [value] if value.starts_with('#') || *value == "@global" => {
                        (None, Some(*value))
                    }
                    ["@all"] => (None, None),
                    [key] => (Some(*key), None),
                    ["@all", scope] => (None, Some(*scope)),
                    [key, scope] => (Some(*key), Some(*scope)),
                    _ => return Err("Usage: /user get who [key] [scope]".into()),
                };
                let scope = self.object_scope(context.actor, context.view, scope)?;
                let mut output = BTreeMap::new();
                for name in self.metadata_targets(context.actor, who, &scope)? {
                    self.metadata_access(context.actor, &name, &scope, false)?;
                    let mut values = self.data.users[&name]
                        .metadata
                        .get(Self::metadata_scope_key(&scope)?)
                        .cloned()
                        .unwrap_or_default();
                    for (key, ceiling) in [
                        ("media.max_size", self.media_config.max_size),
                        ("media.user_quota", self.media_config.user_quota),
                    ] {
                        let effective_scope = if key == "media.user_quota" {
                            &Scope::Server
                        } else {
                            &scope
                        };
                        let value = self
                            .metadata_value(&name, effective_scope, key)
                            .and_then(|value| value.parse::<u64>().ok())
                            .unwrap_or(ceiling)
                            .min(ceiling);
                        values.insert(key.into(), value.to_string());
                    }
                    values.entry("media.max_age".into()).or_insert_with(|| {
                        self.metadata_value(&name, &scope, "media.max_age")
                            .map(str::to_owned)
                            .unwrap_or_else(|| self.media_config.max_age.to_string())
                    });
                    let values = if let Some(key) = key {
                        values.into_iter().filter(|(k, _)| k == key).collect()
                    } else {
                        values
                    };
                    output.insert(name, values);
                }
                serde_json::to_string_pretty(&output).map_err(|e| e.to_string())
            }
            _ => Err("Usage: /user ls [scope] | /user get who [key] [scope] | /room ls".into()),
        }
    }
    pub(super) fn apply_objects(
        &mut self,
        context: &dispatch::CommandContext<'_>,
    ) -> Result<String, String> {
        if let ["/user", "set", who, key, value, rest @ ..] = context.parts {
            if !self.is_su(context.actor) {
                return Err("Only su can change user metadata.".into());
            }
            if rest.len() > 1 {
                return Err("Usage: /user set who key value [scope]".into());
            }
            let scope = self.object_scope(
                context.actor,
                context.view,
                rest.first()
                    .copied()
                    .or((*key == "media.user_quota").then_some("@global")),
            )?;
            let scope_key = Self::metadata_scope_key(&scope)?.to_owned();
            let value = self.validate_metadata(key, value, &scope)?;
            let selection = if *who == "@all" && *key == "media.user_quota" && rest.is_empty() {
                self.object_scope(context.actor, context.view, None)?
            } else {
                scope.clone()
            };
            let targets = self.metadata_targets(context.actor, who, &selection)?;
            for name in &targets {
                self.metadata_access(context.actor, name, &scope, true)?;
            }
            for name in &targets {
                let metadata = &mut self
                    .data
                    .users
                    .get_mut(name)
                    .ok_or("User not found.")?
                    .metadata;
                let entries = metadata.entry(scope_key.clone()).or_default();
                if value != "@unset" && entries.len() >= 32 && !entries.contains_key(*key) {
                    return Err("At most 32 metadata keys per scope.".into());
                }
                if value == "@unset" {
                    entries.remove(*key);
                } else {
                    entries.insert((*key).into(), value.clone());
                }
                metadata.retain(|_, values| !values.is_empty());
            }
            self.audit(
                context.actor,
                "/user set",
                &format!("{who} {key} {}", self.scope_label(&scope)),
            );
            return Ok(format!("Updated metadata for {} users.", targets.len()));
        }
        self.query_objects(context)
    }
    fn validate_metadata(&self, key: &str, value: &str, scope: &Scope) -> Result<String, String> {
        if key.is_empty()
            || key.len() > 64
            || !key
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(&c))
            || value.len() > 256
            || value.chars().any(char::is_control)
        {
            return Err("Metadata keys use 1–64 lowercase letters, digits, ., _ or -; values are at most 256 bytes without controls.".into());
        }
        if key.starts_with("media.")
            && !["media.max_size", "media.max_age", "media.user_quota"].contains(&key)
        {
            return Err("Unknown media metadata key.".into());
        }
        if key == "media.user_quota" && *scope != Scope::Server {
            return Err("media.user_quota is account-wide; use @global.".into());
        }
        if value == "@unset" {
            return Ok(value.into());
        }
        match key {
            "media.max_size" | "media.user_quota" => {
                let n = crate::config::parse_byte_size(value)?;
                let ceiling = if key == "media.max_size" {
                    self.media_config.max_size
                } else {
                    self.media_config.user_quota
                };
                if n == 0 || n > ceiling {
                    return Err("Media limit exceeds the server ceiling or is zero.".into());
                }
                if key == "media.user_quota" && *scope != Scope::Server {
                    return Err("media.user_quota is account-wide; use @global.".into());
                }
                Ok(n.to_string())
            }
            "media.max_age" => {
                if value == "-1" {
                    return Ok(value.into());
                }
                let n = parse_age(value)?;
                if !(1..=315360000).contains(&n) {
                    return Err("Invalid media retention.".into());
                }
                Ok(n.to_string())
            }
            key if key.starts_with("media.") => Err("Unknown media metadata key.".into()),
            _ => Ok(value.into()),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::engine::tests::engine;

    #[test]
    fn object_commands_preserve_room_and_account_identity_and_metadata() {
        let mut e = engine();
        let id = e.data.rooms["lobby"].id.clone();
        assert!(
            e.execute(None, None, "/user rename room lobby wrong-family")
                .is_err()
        );
        assert!(e.data.rooms.contains_key("lobby"));
        e.execute(None, Some("lobby"), "/user set bob label member")
            .unwrap();
        assert_eq!(
            e.execute(Some("bob"), Some("lobby"), "/user ls").unwrap(),
            "alice\nbob\neve"
        );
        assert!(
            e.execute(Some("bob"), None, "/room ls")
                .unwrap()
                .contains("#lobby")
        );
        e.execute(Some("alice"), Some("lobby"), "/room rename #lobby renamed")
            .unwrap();
        e.execute(Some("bob"), Some("renamed"), "/user rename bobby")
            .unwrap();
        assert_eq!(e.data.rooms["renamed"].id, id);
        assert_eq!(e.data.users["bobby"].metadata[&id]["label"], "member");
        e.execute(Some("alice"), None, "/room delete #renamed")
            .unwrap();
        assert!(!e.data.users["bobby"].metadata.contains_key(&id));
    }
    #[test]
    fn metadata_bulk_selection_is_scoped_private_and_atomic() {
        let mut e = engine();
        e.provision("outside", "hash".into(), false, false).unwrap();
        e.execute(None, Some("lobby"), "/user set @all label room-member")
            .unwrap();
        let all: serde_json::Value = serde_json::from_str(
            &e.execute(Some("alice"), None, "/user get @all #lobby")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(all.as_object().unwrap().len(), 3);
        assert_eq!(all["bob"]["label"], "room-member");
        assert!(e.data.users["outside"].metadata.is_empty());
        let before = e.data.clone();
        assert!(
            e.execute(Some("bob"), Some("lobby"), "/user get @all #lobby")
                .is_err()
        );
        assert!(
            e.execute(Some("bob"), Some("lobby"), "/user set @all label denied")
                .is_err()
        );
        assert!(
            e.execute(Some("alice"), None, "/user set outside label denied #lobby")
                .is_err()
        );
        assert!(e.data == before);
        assert!(
            e.read_command(Some("bob"), Some("lobby"), "/user get bob")
                .unwrap()
                .is_ok()
        );
        e.execute(None, None, "/grant eve su").unwrap();
        let before = e.data.clone();
        assert!(
            e.execute(Some("alice"), None, "/user set @all label denied #lobby")
                .is_err()
        );
        assert!(e.data == before);
    }
    #[test]
    fn disabled_superuser_metadata_remains_protected_from_admin_bulk_and_direct_writes() {
        let mut e = engine();
        e.execute(None, None, "/grant eve su").unwrap();
        e.execute(None, None, "/user disable eve").unwrap();
        let before = e.data.clone();
        for input in ["/user set eve label denied", "/user set @all label denied"] {
            assert!(
                e.execute(Some("alice"), None, input)
                    .unwrap_err()
                    .contains("Only su")
            );
            assert!(e.data == before);
        }
        e.execute(None, None, "/user set eve label recovery")
            .unwrap();
    }
    #[test]
    fn metadata_and_grants_bulk_writes_rollback_on_storage_failure() {
        let mut e = engine();
        let before = e.data.clone();
        e.db.lock().unwrap().execute_batch("CREATE TRIGGER deny_metadata BEFORE UPDATE ON entities WHEN NEW.kind='account' AND NEW.key='eve' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(e.execute(None, None, "/user set @all label test").is_err());
        assert!(e.data == before);
        e.db.lock().unwrap().execute_batch("DROP TRIGGER deny_metadata; CREATE TRIGGER deny_policy BEFORE UPDATE ON state BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(
            e.execute(None, None, "/grant @all #lobby r:message.metadata")
                .is_err()
        );
        assert!(e.data == before);
    }
    #[test]
    fn bulk_grants_and_revokes_check_every_target_and_last_admin() {
        let mut e = engine();
        e.execute(Some("alice"), None, "/grant @all #lobby w:message.react")
            .unwrap();
        assert!(
            e.data
                .policy
                .grants
                .iter()
                .filter(|g| g.permissions.contains("w:message.react"))
                .count()
                >= 3
        );
        e.execute(Some("alice"), None, "/revoke @all #lobby w:message.react")
            .unwrap();
        assert!(
            !e.data
                .policy
                .grants
                .iter()
                .any(|g| g.permissions.contains("w:message.react"))
        );
        e.provision("otheradmin", "hash".into(), true, false)
            .unwrap();
        let before = e.data.clone();
        assert!(
            e.execute(Some("alice"), None, "/revoke @all admin")
                .is_err()
        );
        assert!(e.data == before);
        // The owner is among @all, so participant-group removal must reject all.
        assert!(
            e.execute(Some("alice"), None, "/revoke @all user #lobby")
                .is_err()
        );
        assert!(e.data == before);
        e.execute(None, None, "/grant eve su").unwrap();
        let before = e.data.clone();
        assert!(
            e.execute(Some("alice"), None, "/grant @all #lobby w:message.react")
                .is_err()
        );
        assert!(e.data == before);
    }
    #[test]
    fn metadata_limits_inherit_validate_and_enforce_account_quota() {
        let mut e = engine();
        let room = e.room_scope("lobby").unwrap();
        let id = e.data.rooms["lobby"].id.clone();
        e.execute(None, None, "/user set bob media.max_size 16")
            .unwrap();
        e.execute(None, None, "/user set bob media.max_age 1d #lobby")
            .unwrap();
        e.execute(None, None, "/user set bob media.user_quota 20")
            .unwrap();
        assert_eq!(e.media_allowance("bob", &room, 16, None).unwrap(), 86400);
        assert!(e.allocate_media("bob", &id, "file", 17).is_err());
        e.allocate_media("bob", &id, "file", 16).unwrap();
        assert!(e.allocate_media("bob", &id, "file", 8).is_err());
        e.execute(None, None, "/user set bob media.max_size 8 #lobby")
            .unwrap();
        assert!(e.media_allowance("bob", &room, 9, None).is_err());
        e.execute(None, None, "/user set bob media.max_size @unset #lobby")
            .unwrap();
        assert!(e.media_allowance("bob", &room, 9, None).is_ok());
        for input in [
            "/user set bob media.max_size 0",
            "/user set bob media.max_age 0d",
            "/user set bob media.user_quota 8 #lobby",
            "/user set bob media.unknown 1",
            "/user set bob media.unknown @unset",
            "/user set bob media.user_quota @unset #lobby",
            "/grant bob #lobby w:media.upload 16 7d",
        ] {
            let before = e.data.clone();
            assert!(e.execute(None, None, input).is_err(), "{input}");
            assert!(e.data == before);
        }
    }
    #[test]
    fn account_scoped_metadata_reads_apply_only_to_the_named_target_and_writes_need_su() {
        let mut e = engine();
        e.execute(None, None, "/revoke bob user").unwrap();
        for permission in ["/user", "r:account.metadata", "x:account.metadata"] {
            e.execute(
                None,
                None,
                &format!("/grant bob @account:alice {permission}"),
            )
            .unwrap();
        }
        e.execute(None, None, "/user set alice label authorized")
            .unwrap();
        assert!(
            e.execute(Some("bob"), None, "/user set alice label denied")
                .unwrap_err()
                .contains("Only su")
        );
        assert!(
            e.read_command(Some("bob"), None, "/user get alice")
                .unwrap()
                .unwrap()
                .contains("authorized")
        );
        assert!(e.execute(Some("bob"), None, "/user get eve").is_err());
        assert!(
            e.execute(Some("bob"), None, "/user set eve label denied")
                .is_err()
        );
    }
    #[test]
    fn metadata_persists_and_read_commands_do_not_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite");
        let mut e = Engine::open(&path).unwrap();
        e.provision("alice", "hash".into(), true, false).unwrap();
        e.execute(None, None, "/user set alice label saved")
            .unwrap();
        drop(e);
        let e = Engine::open(&path).unwrap();
        let writes = e.db.lock().unwrap().total_changes();
        let result = e
            .read_command(Some("alice"), None, "/user get @all")
            .unwrap()
            .unwrap();
        assert!(result.contains("saved"));
        assert!(!result.contains("hash"));
        assert_eq!(e.db.lock().unwrap().total_changes(), writes);
    }
    #[test]
    fn only_head_database_upgrades_and_transfers_media_limits_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite");
        let mut e = Engine::open(&path).unwrap();
        e.provision("alice", "hash".into(), true, false).unwrap();
        e.execute(None, None, "/grant alice @global w:media.upload")
            .unwrap();
        e.execute(None, None, "/grant alice @global /rename")
            .unwrap();
        e.execute(None, None, "/revoke alice admin").unwrap();
        e.db.lock().unwrap().execute_batch("UPDATE state SET json=json_set(json,'$.policy.grants[0].media',json('{\"max_size\":16,\"max_age\":86400}')); PRAGMA user_version=15;").unwrap();
        drop(e);
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TRIGGER deny_upgrade BEFORE UPDATE ON entities BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(Engine::open(&path).is_err());
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                .unwrap(),
            15
        );
        db.execute_batch("DROP TRIGGER deny_upgrade;").unwrap();
        let e = Engine::open(&path).unwrap();
        assert!(
            e.read_command(Some("alice"), None, "/user get alice")
                .unwrap()
                .is_ok()
        );
        assert_eq!(
            e.data.users["alice"].metadata["@global"]["media.max_size"],
            "16"
        );
        assert!(
            !serde_json::to_string(&e.data.policy)
                .unwrap()
                .contains("\"media\"")
        );
        drop(e);
        for version in 1..=14 {
            db.pragma_update(None, "user_version", version).unwrap();
            let before: String = db
                .query_row("SELECT json FROM state", [], |r| r.get(0))
                .unwrap();
            assert!(Engine::open(&path).is_err());
            assert_eq!(
                db.query_row("SELECT json FROM state", [], |r| r.get::<_, String>(0))
                    .unwrap(),
                before
            );
            assert_eq!(
                db.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                    .unwrap(),
                version
            );
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod room_interface_tests {

    #[test]
    fn room_owner_can_invite_with_implicit_or_explicit_scope_and_errors_use_public_syntax() {
        let mut e = crate::engine::tests::engine();
        e.execute(Some("alice"), None, "/room new #support")
            .unwrap();
        for (view, command, member) in [
            (Some("support"), "/room add bob", "bob"),
            (None, "/room add eve #support", "eve"),
        ] {
            e.execute(Some("alice"), view, command).unwrap();
            assert!(e.data.rooms["support"].members.contains(member));
            assert!(e.execute(Some(member), Some("support"), "/history").is_ok());
        }
        for (command, expected) in [
            ("/room add", "Usage: /room add user [#room]"),
            ("/room kick", "Usage: /room kick user [#room]"),
            ("/room join", "Usage: /room join #room"),
            ("/room owner", "Usage: /room owner user [#room]"),
            ("/room new", "Usage: /room new #name"),
            ("/room delete", "Usage: /room delete #name"),
        ] {
            assert_eq!(
                e.execute(Some("alice"), Some("support"), command)
                    .unwrap_err(),
                expected
            );
        }
        assert_eq!(
            e.execute(Some("alice"), None, "/room add bob").unwrap_err(),
            "Specify a room."
        );
        e.execute(Some("alice"), Some("support"), "/room kick bob")
            .unwrap();
        assert!(!e.data.rooms["support"].members.contains("bob"));
        assert!(e.execute(Some("bob"), Some("support"), "/history").is_err());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod scope_default_tests {
    use super::*;

    #[test]
    fn user_lists_and_metadata_default_to_current_scope() {
        let mut e = crate::engine::tests::engine();
        e.provision("outside", "hash".into(), false, false).unwrap();
        let room = e.room_scope("lobby").unwrap();
        let key = Engine::metadata_scope_key(&room).unwrap().to_owned();
        assert_eq!(
            e.execute(Some("bob"), Some("lobby"), "/user ls").unwrap(),
            "alice\nbob\neve"
        );
        assert!(
            e.execute(Some("bob"), Some("lobby"), "/user ls @global")
                .unwrap()
                .contains("outside")
        );
        e.execute(None, Some("lobby"), "/user set @all label local")
            .unwrap();
        assert_eq!(e.data.users["bob"].metadata[&key]["label"], "local");
        assert!(e.data.users["outside"].metadata.is_empty());
        let read = e
            .execute(Some("alice"), Some("lobby"), "/user get bob")
            .unwrap();
        assert!(read.contains("local"));
        e.execute(None, Some("lobby"), "/user set bob label global @global")
            .unwrap();
        assert_eq!(e.data.users["bob"].metadata["@global"]["label"], "global");
        assert!(
            e.execute(None, None, "/user get bob")
                .unwrap()
                .contains("global")
        );
    }

    #[test]
    fn grant_and_revoke_keep_explicit_permission_scopes_and_global_group_defaults() {
        let mut e = crate::engine::tests::engine();
        let before = e.data.clone();
        for command in [
            "/grant bob w:message.react",
            "/revoke bob w:message.react",
            "/grant @all /reply",
            "/revoke @all /reply",
        ] {
            assert!(
                e.execute(None, Some("lobby"), command).is_err(),
                "{command}"
            );
            assert!(e.data == before);
        }
        e.execute(None, Some("lobby"), "/grant eve admin").unwrap();
        assert!(e.is_admin("eve"));
        e.execute(None, Some("lobby"), "/revoke eve admin").unwrap();
        assert!(!e.is_admin("eve"));
        e.execute(
            Some("alice"),
            Some("lobby"),
            "/grant bob #lobby w:message.react",
        )
        .unwrap();
        e.execute(
            Some("alice"),
            Some("lobby"),
            "/revoke bob #lobby w:message.react",
        )
        .unwrap();
    }

    #[test]
    fn unsupported_private_defaults_fail_without_falling_back_to_global() {
        let mut e = crate::engine::tests::engine();
        e.execute(Some("bob"), None, "/tell eve hi").unwrap();
        let before = e.data.clone();
        for text in [
            "/user ls",
            "/user get bob",
            "/user set bob label private",
            "/bot ls",
        ] {
            assert!(
                e.execute(Some("alice"), Some("@private:bob:eve"), text)
                    .is_err(),
                "{text}"
            );
            assert!(e.data == before);
        }
        assert!(
            e.execute(None, Some("@private:bob:eve"), "/user ls @global")
                .unwrap()
                .contains("bob")
        );
    }

    #[test]
    fn console_bot_defaults_and_explicit_room_scopes_match_other_commands() {
        let mut e = crate::engine::tests::engine();
        e.execute(None, None, "/bot new worldwide").unwrap();
        assert!(!e.data.rooms["lobby"].members.contains("worldwide"));
        assert!(
            e.execute(None, Some("@command"), "/bot ls")
                .unwrap()
                .contains("worldwide")
        );
        e.execute(None, Some("lobby"), "/bot new local").unwrap();
        assert!(e.data.rooms["lobby"].members.contains("local"));
        assert!(
            !e.execute(None, Some("lobby"), "/bot ls")
                .unwrap()
                .contains("worldwide")
        );
        assert!(
            e.execute(None, None, "/bot ls #lobby")
                .unwrap()
                .contains("local")
        );
        assert!(e.execute(None, None, "/clean 7d #lobby").is_ok());
        assert!(e.execute(None, Some("@command"), "/clean 7d").is_err());
        assert!(e.execute(None, Some("lobby"), "/clean 7d @global").is_ok());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod metadata_interface_tests {
    use super::*;

    #[test]
    fn metadata_all_includes_effective_defaults_and_quota_shorthand_is_account_wide() {
        let mut e = crate::engine::tests::engine();
        e.execute(None, None, "/user set bob secret global-only")
            .unwrap();
        let get = |e: &Engine, command: &str| -> serde_json::Value {
            serde_json::from_str(
                &e.read_command(Some("bob"), Some("lobby"), command)
                    .unwrap()
                    .unwrap(),
            )
            .unwrap()
        };
        let initial = get(&e, "/user get bob @all");
        assert_eq!(initial["bob"]["media.user_quota"], "1073741824");
        assert_eq!(initial["bob"]["media.max_size"], "26214400");
        assert!(initial["bob"].get("secret").is_none());
        e.execute(None, Some("lobby"), "/user set bob media.user_quota 1g")
            .unwrap();
        assert_eq!(
            e.data.users["bob"].metadata["@global"]["media.user_quota"],
            "1073741824"
        );
        e.execute(None, Some("lobby"), "/user set bob media.user_quota 512m")
            .unwrap();
        assert_eq!(
            get(&e, "/user get bob @all")["bob"]["media.user_quota"],
            "536870912"
        );
        e.execute(None, Some("lobby"), "/user set bob media.max_size 512k")
            .unwrap();
        assert_eq!(
            get(&e, "/user get bob @all #lobby")["bob"]["media.max_size"],
            "524288"
        );
        e.provision("outside", "hash".into(), false, false).unwrap();
        e.execute(None, Some("lobby"), "/user set @all media.user_quota 128m")
            .unwrap();
        assert!(e.data.users["outside"].metadata.is_empty());
        let selected: serde_json::Value = serde_json::from_str(
            &e.execute(None, Some("lobby"), "/user get @all media.user_quota")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(selected.as_object().unwrap().len(), 3);
        assert!(selected.get("outside").is_none());
        e.execute(None, None, "/user set @all media.user_quota 256m")
            .unwrap();
        let all: serde_json::Value =
            serde_json::from_str(&e.execute(None, None, "/user get @all @all").unwrap()).unwrap();
        for name in ["alice", "bob", "eve"] {
            assert_eq!(all[name]["media.user_quota"], "268435456");
        }
        e.execute(None, None, "/user set bob media.user_quota @unset")
            .unwrap();
        assert_eq!(
            get(&e, "/user get bob media.user_quota")["bob"]["media.user_quota"],
            "1073741824"
        );
        for value in ["0g", "2g", "1.5g", "18446744073709551615g"] {
            assert!(
                e.execute(
                    None,
                    None,
                    &format!("/user set bob media.user_quota {value}")
                )
                .is_err()
            );
        }
    }

    #[test]
    fn every_explicit_room_scope_requires_hash_and_omitted_scopes_still_work() {
        let mut e = crate::engine::tests::engine();
        let before = e.data.clone();
        for command in [
            "/grant bob lobby w:message.react",
            "/revoke bob lobby w:message.react",
            "/grant bob user lobby",
            "/revoke bob user lobby",
            "/grant @all lobby /reply",
            "/permissions lobby",
            "/user ls lobby",
            "/user ls room",
            "/user get bob @all lobby",
            "/user set bob label test lobby",
            "/bot ls lobby",
            "/clean 7d lobby",
            "/room add bob lobby",
            "/room join lobby",
            "/room rename lobby renamed",
        ] {
            assert!(
                e.execute(None, Some("lobby"), command)
                    .unwrap_err()
                    .contains("#name"),
                "{command}"
            );
            assert!(e.data == before);
        }
        for command in [
            "/user ls",
            "/user get bob @all",
            "/permissions",
            "/room add bob",
            "/clean 7d #lobby",
            "/user ls #lobby",
            "/permissions #lobby",
        ] {
            assert!(
                e.execute(Some("alice"), Some("lobby"), command).is_ok(),
                "{command}"
            );
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod metadata_authority_tests {
    use super::*;
    #[test]
    fn admins_and_room_managers_can_read_but_only_su_can_change_metadata() {
        let mut e = crate::engine::tests::engine();
        e.execute(None, Some("lobby"), "/user set @all label seeded")
            .unwrap();
        assert!(e.allows(Some("alice"), &Scope::Server, Action::AccountMetadataRead));
        assert!(!e.allows(Some("alice"), &Scope::Server, Action::AccountMetadataWrite));
        assert!(
            e.execute(Some("alice"), Some("lobby"), "/user get @all")
                .unwrap()
                .contains("seeded")
        );
        e.execute(None, None, "/grant bob admin #lobby").unwrap();
        assert!(
            e.execute(Some("bob"), Some("lobby"), "/user get eve")
                .unwrap()
                .contains("seeded")
        );
        // Explicit or previously stored write flags cannot bypass the su boundary.
        e.execute(None, None, "/grant alice @global x:account.metadata")
            .unwrap();
        let before = e.data.clone();
        let writes = e.db.lock().unwrap().total_changes();
        for actor in ["alice", "bob"] {
            for command in [
                "/user set bob label changed",
                "/user set @all label changed #lobby",
                "/user set bob label @unset #lobby",
                "/user set bob media.user_quota 1g",
            ] {
                assert!(
                    e.execute(Some(actor), Some("lobby"), command)
                        .unwrap_err()
                        .contains("Only su"),
                    "{actor}: {command}"
                );
                assert!(e.data == before);
                assert_eq!(e.db.lock().unwrap().total_changes(), writes);
            }
        }
        e.execute(None, None, "/grant bob su").unwrap();
        e.execute(Some("bob"), Some("lobby"), "/user set @all label changed")
            .unwrap();
        assert!(
            e.execute(Some("bob"), Some("lobby"), "/user get alice")
                .unwrap()
                .contains("changed")
        );
    }
}
