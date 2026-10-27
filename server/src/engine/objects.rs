//! Object-oriented commands and bounded, scoped account metadata.
use super::*;
use authorization::{Action, Scope};

/// Read projection only: inherited values are never persisted in room branches.
#[derive(serde::Serialize)]
struct MetadataEntry {
    value: String,
    source: String,
}

pub(super) struct AccountMetadataPlan {
    before: MetadataChange,
    accounts: BTreeMap<String, Account>,
    operation: &'static str,
    target: String,
    reply: String,
}

impl Engine {
    pub(super) fn object_scope(
        &self,
        actor: Option<&str>,
        view: Option<&str>,
        value: Option<&str>,
    ) -> Result<Scope, EngineError> {
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
    ) -> Result<Vec<String>, EngineError> {
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
                    .ok_or_else(|| EngineError::NotFound("Room not found.".into()))?
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
    ) -> Result<Vec<String>, EngineError> {
        if who == "@all" {
            return self.scope_users(actor, scope);
        }
        let name = who.strip_prefix('@').unwrap_or(who);
        if !self.data.users.contains_key(name) {
            return Err(EngineError::NotFound("User not found.".into()));
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
    ) -> Result<(), EngineError> {
        let account = self
            .data
            .users
            .get(name)
            .ok_or_else(|| EngineError::NotFound("User not found.".into()))?;
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
            return Err(EngineError::Forbidden(
                "Only su can change user metadata.".into(),
            ));
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
    fn color_metadata_access(
        &self,
        actor: Option<&str>,
        name: &str,
        scope: &Scope,
    ) -> Result<(), EngineError> {
        if !self.is_su(actor) {
            let own_default = *scope == Scope::Server && actor == Some(name) && self.active(name);
            let own_room = actor == Some(name)
                && matches!(scope, Scope::Room(_))
                && self.allows(actor, scope, Action::ColorOwn);
            if !own_default && !own_room && !self.can_manage_room_color(actor, scope) {
                return Err(EngineError::Forbidden("Color write requires account ownership at @global, or w:account.color.own/any in the target room.".into()));
            }
            let protected = self
                .data
                .users
                .get(name)
                .and_then(|account| self.authorization.groups.get(&account.id))
                .is_some_and(|groups| groups.contains(&authorization::Group::Su));
            if protected && actor != Some(name) {
                return Err(EngineError::Forbidden(
                    "Only su can manage a su account.".into(),
                ));
            }
        }
        self.require_target_command(actor, scope, "/user")
    }

    pub(super) fn metadata_scope_key(scope: &Scope) -> Result<&str, EngineError> {
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
    ) -> Result<String, EngineError> {
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
                    // Room authority must not reveal arbitrary global account metadata.
                    let global_read = self
                        .metadata_access(context.actor, &name, &Scope::Server, false)
                        .is_ok();
                    let mut sections = BTreeMap::new();
                    if global_read {
                        sections.insert(
                            "@global".to_owned(),
                            self.metadata_section(&name, &Scope::Server, key, true)?,
                        );
                    }
                    if scope != Scope::Server {
                        sections.insert(
                            format!("#{}", self.scope_label(&scope)),
                            self.metadata_section(&name, &scope, key, global_read)?,
                        );
                    }
                    output.insert(name, sections);
                }
                serde_json::to_string_pretty(&output).map_err(EngineError::unavailable)
            }
            _ => Err("Usage: /user ls [scope] | /user get who [key] [scope] | /room ls".into()),
        }
    }
    /// Role defaults are resolved live; role/ownership changes leave no stored privilege.
    pub(super) fn cleanup_limit(
        &self,
        name: &str,
        scope: &Scope,
    ) -> Result<(i64, String), EngineError> {
        let account = self
            .data
            .users
            .get(name)
            .ok_or_else(|| EngineError::NotFound("User not found.".into()))?;
        let room = if let Scope::Room(id) = scope {
            account.metadata.get(id)
        } else {
            None
        };
        for (entries, source) in [
            (room, format!("#{}", self.scope_label(scope))),
            (account.metadata.get("@global"), "@global".into()),
        ] {
            if let Some(value) = entries.and_then(|entries| entries.get("history.clean_min_age")) {
                let limit = value
                    .parse::<i64>()
                    .map_err(|_| "Invalid history.clean_min_age metadata.")?;
                if limit < -1 {
                    return Err("Invalid history.clean_min_age metadata.".into());
                }
                return Ok((limit, source));
            }
        }
        let admin = self.data.policy.assignments.iter().any(|a| {
            a.account_id == account.id
                && a.group == authorization::Group::Admin
                && (a.scope == Scope::Server || &a.scope == scope)
        });
        let owner = matches!(scope, Scope::Room(id) if self.data.rooms.values().any(|r| &r.id == id && r.owner_id == account.id));
        Ok((
            if admin || owner || self.is_su(Some(name)) {
                -1
            } else {
                0
            },
            "role".into(),
        ))
    }
    fn metadata_section(
        &self,
        name: &str,
        scope: &Scope,
        filter: Option<&str>,
        global_read: bool,
    ) -> Result<BTreeMap<String, MetadataEntry>, EngineError> {
        let metadata = &self.data.users[name].metadata;
        let label = if *scope == Scope::Server {
            "@global".into()
        } else {
            format!("#{}", self.scope_label(scope))
        };
        let mut values = BTreeMap::new();
        if global_read && let Some(root) = metadata.get("@global") {
            for (key, value) in root {
                if scope != &Scope::Server && key == "media.user_quota" {
                    continue;
                }
                values.insert(
                    key.clone(),
                    MetadataEntry {
                        value: value.clone(),
                        source: "@global".into(),
                    },
                );
            }
        }
        if scope != &Scope::Server
            && let Some(branch) = metadata.get(Self::metadata_scope_key(scope)?)
        {
            for (key, value) in branch {
                if key == "media.user_quota" {
                    continue;
                }
                values.insert(
                    key.clone(),
                    MetadataEntry {
                        value: value.clone(),
                        source: label.clone(),
                    },
                );
            }
        }
        for (key, default) in [
            ("media.max_size", self.media.config.max_size.to_string()),
            ("media.max_age", self.media.config.max_age.to_string()),
            ("media.user_quota", self.media.config.user_quota.to_string()),
        ] {
            if key == "media.user_quota" && scope != &Scope::Server {
                continue;
            }
            let local = metadata
                .get(Self::metadata_scope_key(scope)?)
                .and_then(|m| m.get(key));
            let global = metadata.get("@global").and_then(|m| m.get(key));
            let (raw, source) = if let Some(value) = local {
                (value.clone(), label.clone())
            } else if let Some(value) = global {
                (value.clone(), "@global".into())
            } else {
                (default, "config".into())
            };
            let value = if key == "media.max_age" {
                raw
            } else {
                let ceiling = if key == "media.max_size" {
                    self.media.config.max_size
                } else {
                    self.media.config.user_quota
                };
                raw.parse::<u64>()
                    .unwrap_or(ceiling)
                    .min(ceiling)
                    .to_string()
            };
            values.insert(key.into(), MetadataEntry { value, source });
        }
        let (limit, source) = self.cleanup_limit(name, scope)?;
        values.insert(
            "history.clean_min_age".into(),
            MetadataEntry {
                value: limit.to_string(),
                source,
            },
        );
        values.retain(|key, _| filter.is_none_or(|filter| key == filter));
        Ok(values)
    }
    #[cfg(test)]
    pub(super) fn apply_objects(
        &mut self,
        context: &dispatch::CommandContext<'_>,
    ) -> Result<String, EngineError> {
        if let Some(plan) = self.account_metadata_plan(context)? {
            let (reply, before) = self.apply_account_metadata(context.actor, plan);
            self.commit_account_metadata(before)?;
            Ok(reply)
        } else {
            self.query_objects(context)
        }
    }
    pub(super) fn prepare_account_metadata(
        &self,
        actor: Option<&str>,
        view: Option<&str>,
        input: &str,
    ) -> Result<AccountMetadataPlan, EngineError> {
        crate::limits::validate_input(input).map_err(EngineError::Invalid)?;
        crate::commands::validate_room_arguments(input).map_err(EngineError::Invalid)?;
        if actor.is_some_and(|name| !self.active(name)) {
            return Err(EngineError::Forbidden("Account unavailable.".into()));
        }
        let command = input.split_whitespace().next().unwrap_or("");
        self.require_command(actor, view, command)?;
        let parts = input.split_whitespace().collect::<Vec<_>>();
        let context = CommandContext {
            actor,
            command,
            room: view.filter(|v| !v.starts_with('@')),
            #[cfg(test)]
            input,
            parts: &parts,
            view,
            scope: self.command_scope(actor, view)?,
            #[cfg(test)]
            markdown: false,
            author: actor.unwrap_or("console"),
        };
        match self.account_metadata_plan(&context)? {
            Some(plan) => Ok(plan),
            None => {
                self.query_objects(&context)?;
                Err("Expected an account metadata mutation.".into())
            }
        }
    }
    pub(super) fn apply_account_metadata(
        &mut self,
        actor: Option<&str>,
        plan: AccountMetadataPlan,
    ) -> (String, MetadataChange) {
        self.data.users.extend(plan.accounts);
        self.audit(actor, plan.operation, &plan.target);
        (plan.reply, plan.before)
    }
    fn account_metadata_plan(
        &self,
        context: &dispatch::CommandContext<'_>,
    ) -> Result<Option<AccountMetadataPlan>, EngineError> {
        if let ["/user", "set", who, key, value, rest @ ..] = context.parts {
            if *key != super::colors::COLOR_KEY && !self.is_su(context.actor) {
                return Err(EngineError::Forbidden(
                    "Only su can change user metadata.".into(),
                ));
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
            // Quota is account-wide; the omitted scope still selects @all from
            // the current view. Explicit @global selects every account instead.
            let selection = if *who == "@all" && *key == "media.user_quota" && rest.is_empty() {
                self.object_scope(context.actor, context.view, None)?
            } else {
                scope.clone()
            };
            let targets = self.metadata_targets(context.actor, who, &selection)?;
            for name in &targets {
                if *key == super::colors::COLOR_KEY {
                    self.color_metadata_access(context.actor, name, &scope)?;
                } else {
                    self.metadata_access(context.actor, name, &scope, true)?;
                }
                if value != "@unset"
                    && let Some(entries) = self.data.users[name].metadata.get(&scope_key)
                    && entries.len() >= 32
                    && !entries.contains_key(*key)
                {
                    return Err("At most 32 metadata keys per scope.".into());
                }
            }
            let before = MetadataChange::accounts(&self.data, targets.iter().cloned(), false);
            let mut accounts = targets
                .iter()
                .map(|name| (name.clone(), self.data.users[name].clone()))
                .collect::<BTreeMap<_, _>>();
            for account in accounts.values_mut() {
                let metadata = &mut account.metadata;
                let entries = metadata.entry(scope_key.clone()).or_default();
                if value == "@unset" {
                    entries.remove(*key);
                } else {
                    entries.insert((*key).into(), value.clone());
                }
                metadata.retain(|_, values| !values.is_empty());
            }
            return Ok(Some(AccountMetadataPlan {
                before,
                accounts,
                operation: "/user set",
                target: format!("{who} {key} {}", self.scope_label(&scope)),
                reply: format!("Updated metadata for {} users.", targets.len()),
            }));
        }
        if let ["/user", "reset", who, key, rest @ ..] = context.parts {
            if *key != super::colors::COLOR_KEY && !self.is_su(context.actor) {
                return Err(EngineError::Forbidden(
                    "Only su can change user metadata.".into(),
                ));
            }
            if rest.len() > 1 || *key == "password" {
                return Err("Usage: /user reset who key|@all [scope] | /user reset who password new-password".into());
            }
            let scope = self.object_scope(
                context.actor,
                context.view,
                rest.first()
                    .copied()
                    .or((*key == "media.user_quota").then_some("@global")),
            )?;
            let scope_key = Self::metadata_scope_key(&scope)?.to_owned();
            if *key != "@all" {
                self.validate_metadata(key, "@unset", &scope)?;
            }
            // Quota is account-wide; the omitted scope still selects @all from
            // the current view. Explicit @global selects every account instead.
            let selection = if *who == "@all" && *key == "media.user_quota" && rest.is_empty() {
                self.object_scope(context.actor, context.view, None)?
            } else {
                scope.clone()
            };
            let targets = self.metadata_targets(context.actor, who, &selection)?;
            for name in &targets {
                if *key == super::colors::COLOR_KEY {
                    self.color_metadata_access(context.actor, name, &scope)?;
                } else {
                    self.metadata_access(context.actor, name, &scope, true)?;
                }
            }
            let before = MetadataChange::accounts(&self.data, targets.iter().cloned(), false);
            let mut accounts = targets
                .iter()
                .map(|name| (name.clone(), self.data.users[name].clone()))
                .collect::<BTreeMap<_, _>>();
            for account in accounts.values_mut() {
                let metadata = &mut account.metadata;
                if *key == "@all" {
                    metadata.remove(&scope_key);
                } else if let Some(entries) = metadata.get_mut(&scope_key) {
                    entries.remove(*key);
                }
                metadata.retain(|_, entries| !entries.is_empty());
            }
            return Ok(Some(AccountMetadataPlan {
                before,
                accounts,
                operation: "/user reset",
                target: format!("{who} {key} {}", self.scope_label(&scope)),
                reply: format!(
                    "Reset metadata for {} users at {}.",
                    targets.len(),
                    if scope == Scope::Server {
                        "@global".into()
                    } else {
                        format!("#{}", self.scope_label(&scope))
                    }
                ),
            }));
        }
        if context.parts.starts_with(&["/user", "reset"]) {
            return Err(
                "Usage: /user reset who key|@all [scope] | /user reset who password new-password"
                    .into(),
            );
        }
        Ok(None)
    }
    #[cfg(test)]
    pub(super) fn commit_account_metadata(
        &mut self,
        before: MetadataChange,
    ) -> Result<(), EngineError> {
        if let Err(error) = self.save_metadata(&[], changes::Baseline::Captured(&before)) {
            before.restore(&mut self.data);
            return Err(error);
        }
        Ok(())
    }
    fn validate_metadata(
        &self,
        key: &str,
        value: &str,
        scope: &Scope,
    ) -> Result<String, EngineError> {
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
        if key.starts_with("history.") && key != "history.clean_min_age" {
            return Err("Unknown history metadata key.".into());
        }
        if key.starts_with("media.")
            && !["media.max_size", "media.max_age", "media.user_quota"].contains(&key)
        {
            return Err("Unknown media metadata key.".into());
        }
        if key == "media.user_quota" && *scope != Scope::Server {
            return Err("media.user_quota is account-wide; use @global.".into());
        }
        if key == super::colors::COLOR_KEY && matches!(scope, Scope::Private(_)) {
            return Err("color applies at @global or in a room.".into());
        }
        if value == "@unset" {
            return Ok(value.into());
        }
        match key {
            "history.clean_min_age" => {
                if matches!(value, "-1" | "0") {
                    return Ok(value.into());
                }
                let age = parse_age(value)?;
                i64::try_from(age).map_err(|_| "Cleanup age exceeds the supported range.")?;
                Ok(age.to_string())
            }
            "media.max_size" | "media.user_quota" => {
                let n = crate::config::parse_byte_size(value).map_err(EngineError::Invalid)?;
                let ceiling = if key == "media.max_size" {
                    self.media.config.max_size
                } else {
                    self.media.config.user_quota
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
            super::colors::COLOR_KEY if !super::colors::valid_color(value) => {
                Err(EngineError::Invalid(format!(
                    "color must be one of: {}.",
                    super::colors::NAME_COLORS.join(", ")
                )))
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
        assert_eq!(all["bob"]["#lobby"]["label"]["value"], "room-member");
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
        e.store.db.lock().unwrap().execute_batch("CREATE TRIGGER deny_metadata BEFORE UPDATE ON entities WHEN NEW.kind='account' AND NEW.key='eve' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(e.execute(None, None, "/user set @all label test").is_err());
        assert!(e.data == before);
        e.store.db.lock().unwrap().execute_batch("DROP TRIGGER deny_metadata; CREATE TRIGGER deny_policy BEFORE UPDATE ON state BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
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
        let writes = e.store.db.lock().unwrap().total_changes();
        let result = e
            .read_command(Some("alice"), None, "/user get @all")
            .unwrap()
            .unwrap();
        assert!(result.contains("saved"));
        assert!(!result.contains("hash"));
        assert_eq!(e.store.db.lock().unwrap().total_changes(), writes);
    }
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
                    .unwrap_err()
                    .message(),
                expected
            );
        }
        assert_eq!(
            e.execute(Some("alice"), None, "/room add bob")
                .unwrap_err()
                .message(),
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
    fn metadata_reset_restores_inheritance_and_clears_only_the_selected_branch() {
        let mut e = crate::engine::tests::engine();
        let room = e.room_scope("lobby").unwrap();
        let id = e.data.rooms["lobby"].id.clone();
        e.execute(None, None, "/user set bob media.max_size 10m @global")
            .unwrap();
        e.execute(None, None, "/user set bob media.max_size 5m #lobby")
            .unwrap();
        e.execute(None, None, "/user set bob label local #lobby")
            .unwrap();
        e.execute(None, None, "/user reset bob media.max_size #lobby")
            .unwrap();
        assert_eq!(
            e.metadata_value("bob", &room, "media.max_size"),
            Some("10485760")
        );
        assert_eq!(e.data.users["bob"].metadata[&id]["label"], "local");
        e.execute(None, Some("lobby"), "/user reset bob @all")
            .unwrap();
        assert!(!e.data.users["bob"].metadata.contains_key(&id));
        assert_eq!(
            e.data.users["bob"].metadata["@global"]["media.max_size"],
            "10485760"
        );
        e.execute(None, None, "/user set bob label local #lobby")
            .unwrap();
        e.execute(None, None, "/user set bob media.user_quota 512m")
            .unwrap();
        e.execute(None, Some("lobby"), "/user reset bob media.user_quota")
            .unwrap();
        assert!(!e.data.users["bob"].metadata["@global"].contains_key("media.user_quota"));
        e.execute(None, None, "/user reset bob @all @global")
            .unwrap();
        assert!(!e.data.users["bob"].metadata.contains_key("@global"));
        assert_eq!(e.data.users["bob"].metadata[&id]["label"], "local");
        let before = e.data.clone();
        for command in [
            "/user reset bob media.user_quota #lobby",
            "/user reset bob @all lobby",
            "/user reset bob media.unknown #lobby",
            "/user reset bob label #lobby extra",
        ] {
            assert!(e.execute(None, None, command).is_err(), "{command}");
            assert!(e.data == before);
        }
    }

    #[test]
    fn bulk_metadata_reset_is_su_only_scoped_and_rolls_back_failed_writes() {
        let mut e = crate::engine::tests::engine();
        e.provision("outside", "hash".into(), false, false).unwrap();
        e.execute(None, None, "/user set @all label root @global")
            .unwrap();
        e.execute(None, None, "/user set @all label local #lobby")
            .unwrap();
        e.execute(None, None, "/user set @all media.user_quota 512m @global")
            .unwrap();
        let before = e.data.clone();
        for actor in ["alice", "bob"] {
            for command in [
                "/user reset bob label #lobby",
                "/user reset @all @all #lobby",
            ] {
                assert!(
                    e.execute(Some(actor), Some("lobby"), command)
                        .unwrap_err()
                        .contains("Only su")
                );
                assert!(e.data == before);
            }
        }
        e.store.db.lock().unwrap().execute_batch("CREATE TRIGGER deny_reset BEFORE UPDATE ON entities WHEN NEW.kind='account' AND NEW.key='bob' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(
            e.execute(None, None, "/user reset @all @all #lobby")
                .is_err()
        );
        assert!(e.data == before);
        e.store
            .db
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER deny_reset;")
            .unwrap();
        let hash = e.data.users["bob"].hash.clone();
        e.execute(None, Some("lobby"), "/user reset @all @all")
            .unwrap();
        let id = &e.data.rooms["lobby"].id;
        assert!(
            e.data
                .users
                .values()
                .all(|user| !user.metadata.contains_key(id))
        );
        assert_eq!(e.data.users["outside"].metadata["@global"]["label"], "root");
        assert_eq!(e.data.users["bob"].hash, hash);
        e.execute(None, Some("lobby"), "/user reset @all media.user_quota")
            .unwrap();
        assert!(!e.data.users["bob"].metadata["@global"].contains_key("media.user_quota"));
        assert_eq!(
            e.data.users["outside"].metadata["@global"]["media.user_quota"],
            "536870912"
        );
    }

    #[test]
    fn metadata_reset_persists_pruned_branches_without_touching_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite");
        let mut e = Engine::open(&path).unwrap();
        e.provision("bob", "hash".into(), false, false).unwrap();
        e.execute(None, None, "/user set bob label root").unwrap();
        e.execute(None, None, "/user reset bob @all").unwrap();
        drop(e);
        let e = Engine::open(&path).unwrap();
        assert!(e.data.users["bob"].metadata.is_empty());
        assert_eq!(e.data.users["bob"].hash, "hash");
    }

    #[test]
    fn metadata_tree_reads_resolve_sources_without_materializing_inheritance() {
        let mut e = crate::engine::tests::engine();
        e.execute(None, None, "/user set bob media.max_age 2d @global")
            .unwrap();
        e.execute(None, None, "/user set bob label root @global")
            .unwrap();
        let id = e.data.rooms["lobby"].id.clone();
        let get = |e: &Engine| -> serde_json::Value {
            serde_json::from_str(
                &e.read_command(Some("bob"), Some("lobby"), "/user get bob #lobby")
                    .unwrap()
                    .unwrap(),
            )
            .unwrap()
        };
        let before = e.data.clone();
        let initial = get(&e);
        assert_eq!(initial["bob"]["#lobby"]["media.max_age"]["value"], "172800");
        assert_eq!(
            initial["bob"]["#lobby"]["media.max_age"]["source"],
            "@global"
        );
        assert_eq!(
            initial["bob"]["#lobby"]["media.max_size"]["source"],
            "config"
        );
        assert_eq!(initial["bob"]["#lobby"]["label"]["value"], "root");
        assert!(initial["bob"]["#lobby"].get("media.user_quota").is_none());
        assert!(!e.data.users["bob"].metadata.contains_key(&id));
        assert!(e.data == before);
        e.execute(None, None, "/user set bob media.max_age 1d #lobby")
            .unwrap();
        let local = get(&e);
        assert_eq!(local["bob"]["#lobby"]["media.max_age"]["value"], "86400");
        assert_eq!(local["bob"]["#lobby"]["media.max_age"]["source"], "#lobby");
        assert_eq!(local["bob"]["@global"]["media.max_age"]["value"], "172800");
        e.execute(None, None, "/user set bob media.max_age @unset #lobby")
            .unwrap();
        assert_eq!(
            get(&e)["bob"]["#lobby"]["media.max_age"]["source"],
            "@global"
        );
        assert!(!e.data.users["bob"].metadata.contains_key(&id));
        e.execute(None, None, "/user set bob secret hidden @global")
            .unwrap();
        e.execute(None, None, "/grant eve #lobby x:policy.change")
            .unwrap();
        let restricted = e
            .read_command(Some("eve"), Some("lobby"), "/user get bob #lobby")
            .unwrap()
            .unwrap();
        assert!(!restricted.contains("hidden"));
    }

    #[test]
    fn deleting_room_prunes_all_persisted_branches_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite");
        let mut e = Engine::open(&path).unwrap();
        for name in ["alice", "bob"] {
            e.provision(name, "hash".into(), false, false).unwrap();
        }
        for room in ["a", "b"] {
            e.execute(None, None, &format!("/room new {room}")).unwrap();
            for name in ["alice", "bob"] {
                e.execute(None, None, &format!("/room add {name} #{room}"))
                    .unwrap();
                e.execute(
                    None,
                    None,
                    &format!("/user set {name} label {room} #{room}"),
                )
                .unwrap();
            }
        }
        e.execute(None, None, "/user set bob label root @global")
            .unwrap();
        let id = e.data.rooms["a"].id.clone();
        let other = e.data.rooms["b"].id.clone();
        e.execute(None, None, "/room kick bob #a").unwrap();
        e.store.db.lock().unwrap().execute_batch("CREATE TRIGGER deny_prune BEFORE UPDATE ON entities WHEN NEW.kind='account' AND NEW.key='bob' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        let before = e.data.clone();
        assert!(e.execute(None, None, "/room delete #a").is_err());
        assert!(e.data == before);
        e.store
            .db
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER deny_prune;")
            .unwrap();
        e.execute(None, None, "/room delete #a").unwrap();
        drop(e);
        let mut e = Engine::open(&path).unwrap();
        assert!(!e.data.rooms.contains_key("a"));
        for user in e.data.users.values() {
            assert!(!user.metadata.contains_key(&id));
            assert_eq!(user.metadata[&other]["label"], "b");
        }
        assert_eq!(e.data.users["bob"].metadata["@global"]["label"], "root");
        let db = e.store.db.lock().unwrap();
        let stale: u64 = db.query_row("SELECT count(*) FROM entities WHERE kind='account' AND json_type(json, '$.metadata.' || ?) IS NOT NULL", [&id], |row| row.get(0)).unwrap();
        assert_eq!(stale, 0);
        drop(db);
        e.execute(None, None, "/room new a").unwrap();
        assert_ne!(e.data.rooms["a"].id, id);
        assert!(
            !e.data.users["bob"]
                .metadata
                .contains_key(&e.data.rooms["a"].id)
        );
    }

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
        assert_eq!(
            initial["bob"]["@global"]["media.user_quota"]["value"],
            "1073741824"
        );
        assert_eq!(
            initial["bob"]["@global"]["media.max_size"]["value"],
            "26214400"
        );
        assert_eq!(initial["bob"]["@global"]["secret"]["value"], "global-only");
        e.execute(None, Some("lobby"), "/user set bob media.user_quota 1g")
            .unwrap();
        assert_eq!(
            e.data.users["bob"].metadata["@global"]["media.user_quota"],
            "1073741824"
        );
        e.execute(None, Some("lobby"), "/user set bob media.user_quota 512m")
            .unwrap();
        assert_eq!(
            get(&e, "/user get bob @all")["bob"]["@global"]["media.user_quota"]["value"],
            "536870912"
        );
        e.execute(None, Some("lobby"), "/user set bob media.max_size 512k")
            .unwrap();
        assert_eq!(
            get(&e, "/user get bob @all #lobby")["bob"]["#lobby"]["media.max_size"]["value"],
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
            assert_eq!(
                all[name]["@global"]["media.user_quota"]["value"],
                "268435456"
            );
        }
        e.execute(None, None, "/user set bob media.user_quota @unset")
            .unwrap();
        assert_eq!(
            get(&e, "/user get bob media.user_quota")["bob"]["@global"]["media.user_quota"]["value"],
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
        let writes = e.store.db.lock().unwrap().total_changes();
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
                assert_eq!(e.store.db.lock().unwrap().total_changes(), writes);
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
