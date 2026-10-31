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
                Ok(self
                    .scope_users(context.actor, &scope)?
                    .iter()
                    .map(|name| {
                        format!(
                            "{name}({})",
                            self.metadata_value(name, &scope, "displayname")
                                .unwrap_or(name)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"))
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
            parts: &parts,
            view,
            scope: self.command_scope(actor, view)?,
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
        if let ["/user", "set" | "reset", _, key, ..] = context.parts
            && key.starts_with(super::preferences::PREFIX)
        {
            return Err(EngineError::Forbidden(
                "Preferences can only be changed through your own account settings.".into(),
            ));
        }
        if let ["/user", "set", who, key, value, rest @ ..] = context.parts {
            if !matches!(*key, super::colors::COLOR_KEY | "displayname")
                && !self.is_su(context.actor)
            {
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
                } else if *key == "displayname" {
                    self.display_name_access(context.actor, name, &scope)?;
                } else {
                    self.metadata_access(context.actor, name, &scope, true)?;
                }
                if value != "@unset"
                    && let Some(entries) = self.data.users[name].metadata.get(&scope_key)
                    && super::preferences::metadata_full(entries, key)
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
            if !matches!(*key, super::colors::COLOR_KEY | "displayname")
                && !self.is_su(context.actor)
            {
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
                } else if *key == "displayname" {
                    self.display_name_access(context.actor, name, &scope)?;
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
                    if scope_key == "@global" {
                        if let Some(entries) = metadata.get_mut(&scope_key) {
                            entries.retain(|key, _| key.starts_with(super::preferences::PREFIX));
                        }
                    } else {
                        metadata.remove(&scope_key);
                    }
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
        if key == "displayname" {
            if matches!(scope, Scope::Private(_)) {
                return Err("Display names apply at @global or in a room.".into());
            }
            if value != "@unset" {
                self.validate_display_name(value)?;
            }
        }
        if key == "media.user_quota" && *scope != Scope::Server {
            return Err("media.user_quota is account-wide; use @global.".into());
        }
        if key == super::colors::COLOR_KEY && matches!(scope, Scope::Private(_)) {
            return Err("color applies at @global or in a room.".into());
        }
        if let Some(preference) = key.strip_prefix(super::preferences::PREFIX) {
            if *scope != Scope::Server {
                return Err("Preferences are account-wide; use @global.".into());
            }
            if value != "@unset" {
                super::preferences::validate(preference, value)?;
            }
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
mod tests;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod scope_default_tests;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod metadata_interface_tests;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod metadata_authority_tests;
