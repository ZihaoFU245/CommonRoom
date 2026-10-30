//! Bot lifecycle is a thin extension of the engine's ordinary account and grants.
use super::*;
use crate::bots::models::{Credential, CredentialKind, Registration};
use authorization::{Action, Grant, Group, Scope, Subject};

const DEFAULT_ACTIONS: &[Action] = &[
    Action::Discover,
    Action::Join,
    Action::Read,
    Action::Members,
    Action::Send,
    Action::React,
];
const DEFAULT_COMMANDS: &[&str] = &[
    "/user", "/room", "/help", "/man", "/whoami", "/history", "/reply", "/react",
];

impl Engine {
    pub(super) fn bot_credential_hints(
        &self,
        viewer: &str,
    ) -> Vec<crate::bots::models::CredentialHint> {
        if self.require_bot_management(Some(viewer), None).is_err()
            || self
                .require_target_command(Some(viewer), &Scope::Server, "/bot")
                .is_err()
        {
            return Vec::new();
        }
        self.data
            .users
            .iter()
            .filter_map(|(name, account)| {
                let registration = self.data.bots.get(&account.id)?;
                self.account_target(Some(viewer), name, Action::BotCredentials)
                    .ok()?;
                Some((name, registration))
            })
            .flat_map(|(name, registration)| {
                registration
                    .tokens
                    .iter()
                    .map(|(id, token)| crate::bots::models::CredentialHint {
                        id: id.clone(),
                        bot: name.clone(),
                        label: token.label.clone(),
                        kind: token.kind,
                    })
            })
            .collect()
    }

    pub(crate) fn bot_credential(&self, account: &str, credential: &str) -> Option<&Credential> {
        self.data.bots.get(account)?.tokens.get(credential)
    }

    pub(super) fn bot_completion_names(&self, viewer: &str) -> Vec<String> {
        if !(self.is_su(Some(viewer)) || self.has_group(Some(viewer), Group::Admin)) {
            return Vec::new();
        }
        let global = self.require_bot_management(Some(viewer), None).is_ok()
            && self
                .require_target_command(Some(viewer), &Scope::Server, "/bot")
                .is_ok();
        self.data
            .users
            .iter()
            .filter(|(name, account)| {
                self.data.bots.contains_key(&account.id)
                    && (global
                        || self.data.rooms.iter().any(|(room_name, room)| {
                            let scope = Scope::Room(room.id.clone());
                            room.members.contains(*name)
                                && self
                                    .require_bot_management(Some(viewer), Some(room_name))
                                    .is_ok()
                                && self
                                    .require_target_command(Some(viewer), &scope, "/bot")
                                    .is_ok()
                                && self.allows(Some(viewer), &scope, Action::Members)
                        }))
            })
            .map(|(name, _)| name.clone())
            .collect()
    }

    pub(crate) fn bot_owner_id(&self, account: &str) -> Option<&str> {
        self.data.bots.get(account)?.owner_id.as_deref()
    }

    pub(crate) fn bot_operator(&self, actor: &str, account: &str) -> bool {
        let Some(bot) = self.bot_index.accounts.get(account) else {
            return false;
        };
        self.active(actor)
            && self.active(bot)
            && (!self.has_group(Some(bot), Group::Su) || self.is_su(Some(actor)))
            && (self.is_su(Some(actor))
                || self.has_group(Some(actor), Group::Admin)
                || self
                    .data
                    .bots
                    .get(account)
                    .and_then(|bot| bot.owner_id.as_deref())
                    == self.principal_id(Some(actor)))
    }
    pub(crate) fn bot_owner(&self, account: &str) -> Option<String> {
        let owner = self.data.bots.get(account)?.owner_id.as_ref()?;
        self.data
            .users
            .iter()
            .find(|(_, user)| &user.id == owner)
            .map(|(name, _)| name.clone())
    }
    #[cfg(test)]
    pub fn admit_bot_input(&mut self, secret: &str) -> Result<(), EngineError> {
        let name = self.bot_identity(secret).ok_or("Credential revoked.")?;
        let id = self
            .data
            .users
            .get(&name)
            .ok_or_else(|| EngineError::Forbidden("Account unavailable.".into()))?
            .id
            .clone();
        self.admit_bot_account(&id)
    }
    pub(crate) fn admit_bot_account(&mut self, id: &str) -> Result<(), EngineError> {
        let now = std::time::Instant::now();
        self.bot_budgets
            .retain(|_, (at, _)| now.duration_since(*at).as_secs() < 10);
        let entry = self.bot_budgets.entry(id.to_owned()).or_insert((now, 0));
        if entry.1 >= 30 {
            return Err("Slow down; at most 30 bot inputs per 10 seconds per account.".into());
        }
        entry.1 += 1;
        Ok(())
    }

    /// Legacy account credential lookup for bot administration and tests.
    /// Browser authentication is session-only; operational workers use Context.
    pub fn bot_identity(&self, secret: &str) -> Option<String> {
        if secret.len() != 68 || !secret.starts_with("bot_") {
            return None;
        }
        let digest = crate::bots::digest(secret);
        let (account_id, id) = self.bot_index.credentials.get(&digest)?;
        let token = self.data.bots.get(account_id)?.tokens.get(id)?;
        // Restricted credentials are accepted only by the versioned bot adapter.
        if token.conversations.is_some()
            || token.operations.is_some()
            || token.expires.is_some_and(|at| at <= now())
        {
            return None;
        }
        let name = self.bot_index.accounts.get(account_id)?;
        self.data
            .users
            .get(name)
            .filter(|a| a.id == *account_id && !a.disabled)
            .map(|_| name.clone())
    }

    fn bot_scope(&self, view: Option<&str>, explicit: Option<&str>) -> Result<Scope, EngineError> {
        if explicit.is_some_and(|value| !value.starts_with('#') && !value.starts_with('@')) {
            return Err("Room scopes require #name.".into());
        }
        match explicit.or(view) {
            None | Some("@command" | "@global") => Ok(Scope::Server),
            Some(name) if !name.starts_with('@') => self.room_scope(name.trim_start_matches('#')),
            _ => Err(
                "Bot commands need a room or @global; specify the scope in private views.".into(),
            ),
        }
    }

    fn bot_scope_label(&self, scope: &Scope) -> String {
        let label = self.scope_label(scope);
        if matches!(scope, Scope::Room(_)) {
            format!("#{label}")
        } else {
            label
        }
    }

    pub(super) fn registered_id(&self, name: &str) -> Result<String, EngineError> {
        let id = self
            .data
            .users
            .get(name)
            .ok_or_else(|| EngineError::NotFound("User not found.".into()))?
            .id
            .clone();
        if !self.data.bots.contains_key(&id) {
            return Err("No bot connection registered for this account.".into());
        }
        Ok(id)
    }

    pub(super) fn require_bot_management(
        &self,
        actor: Option<&str>,
        view: Option<&str>,
    ) -> Result<(), EngineError> {
        self.require_command(actor, view, "/bot")?;
        if !(self.is_su(actor) || self.has_group(actor, Group::Admin)) {
            return Err("Only global administrators may manage bot connections.".into());
        }
        Ok(())
    }
    fn apply_bot_metadata(
        &mut self,
        actor: Option<&str>,
        view: Option<&str>,
        parts: &[&str],
    ) -> Result<(String, Option<MetadataChange>), EngineError> {
        self.require_bot_management(actor, view)?;
        if let [_, "token", "rm", token_id] = parts {
            self.require_target_command(actor, &Scope::Server, "/bot")?;
            let mut owners = self.data.users.iter().filter(|(name, account)| {
                self.data
                    .bots
                    .get(&account.id)
                    .is_some_and(|bot| bot.tokens.contains_key(*token_id))
                    && self
                        .account_target(actor, name, Action::BotCredentials)
                        .is_ok()
            });
            let name = owners
                .next()
                .map(|(name, _)| name.clone())
                .ok_or("Credential not found.")?;
            if owners.next().is_some() {
                return Err("Credential ID is ambiguous; specify the bot name.".into());
            }
            return self.apply_bot_metadata(actor, view, &["/bot", "token", "rm", &name, token_id]);
        }
        if let Some(result) = self.query_bot_command(actor, view, parts) {
            return result.map(|reply| (reply, None));
        }
        let name = if parts.get(1) == Some(&"token") {
            parts.get(3).copied()
        } else {
            parts.get(2).copied()
        };
        let mut keys = changes::DirtyKeys::default();
        if let Some(name) = name {
            keys.users.insert(name.into());
            if let Some(account) = self.data.users.get(name) {
                keys.bots.insert(account.id.clone());
            }
        }
        if parts.get(1) == Some(&"new")
            && let Ok(Scope::Room(id)) = self.bot_scope(view, parts.get(3).copied())
            && let Some(name) = self.bot_index.conversations.get(&id)
        {
            keys.rooms.insert(name.clone());
        }
        let mut before = MetadataChange::keyed(&self.data, keys);
        let result = self.apply_bot_command(actor, view, parts);
        if parts.get(1) == Some(&"new")
            && let Some(account) = name.and_then(|name| self.data.users.get(name))
        {
            before.include_new_bot(account.id.clone());
        }
        match result {
            Ok(reply) => {
                if before.unchanged(&self.data) {
                    return Ok((reply, None));
                }
                self.data.policy.revision = before.policy.revision.wrapping_add(1);
                Ok((reply, Some(before)))
            }
            Err(error) => {
                before.restore(&mut self.data);
                Err(error)
            }
        }
    }

    pub(super) fn query_bot_command(
        &self,
        actor: Option<&str>,
        view: Option<&str>,
        parts: &[&str],
    ) -> Option<Result<String, EngineError>> {
        if !matches!(parts.get(1).copied(), Some("ls" | "info"))
            && !(parts.get(1) == Some(&"token") && parts.get(2) == Some(&"ls"))
        {
            return None;
        }
        Some((|| {
            self.require_bot_management(actor, view)?;
            match parts {
                [_, "ls"] | [_, "ls", _] => {
                    let scope = self.bot_scope(view, parts.get(2).copied())?;
                    self.require_target_command(actor, &scope, "/bot")?;
                    if matches!(scope, Scope::Room(_)) {
                        self.require(actor, &scope, Action::Members)?;
                    }
                    let names: Vec<_> = self
                        .data
                        .users
                        .iter()
                        .filter(|(name, account)| {
                            self.data.bots.contains_key(&account.id)
                                && match &scope {
                                    Scope::Server => true,
                                    Scope::Room(id) => self
                                        .data
                                        .rooms
                                        .values()
                                        .any(|r| r.id == *id && r.members.contains(*name)),
                                    _ => false,
                                }
                        })
                        .map(|(name, _)| name.as_str())
                        .collect();
                    Ok(names.join("\n"))
                }
                [_, "info", name] | [_, "info", name, _] => {
                    let scope = self.bot_scope(view, parts.get(3).copied())?;
                    self.require_target_command(actor, &scope, "/bot")?;
                    let id = self.registered_id(name)?;
                    if let Scope::Room(room_id) = &scope {
                        self.require(actor, &scope, Action::Members)?;
                        if !self
                            .data
                            .rooms
                            .values()
                            .any(|r| r.id == *room_id && r.members.contains(*name))
                        {
                            return Err("Account is not a member of this room.".into());
                        }
                    }
                    let commands = self
                        .commands_for(
                            Some(name),
                            match &scope {
                                Scope::Room(id) => self
                                    .data
                                    .rooms
                                    .iter()
                                    .find(|(_, r)| r.id == *id)
                                    .map(|(n, _)| n.as_str()),
                                _ => None,
                            },
                        )
                        .iter()
                        .map(|c| c.name)
                        .collect::<Vec<_>>()
                        .join(", ");
                    Ok(format!(
                        "{name} ({id}) at {}\nOwner: {}\nDisabled: {}\nConnected: {}\nPermissions: {}\nCommands: {commands}",
                        self.bot_scope_label(&scope),
                        self.bot_owner(&id)
                            .unwrap_or_else(|| "administrators".into()),
                        !self.active(name),
                        self.presence.count(name),
                        self.effective(Some(name), &scope).names().join(", ")
                    ))
                }
                [_, "token", "ls", name] => {
                    let id = self.registered_id(name)?;
                    self.account_target(actor, name, Action::BotCredentials)?;
                    self.require_target_command(actor, &Scope::Server, "/bot")?;
                    let entries = self.data.bots[&id].tokens.iter().map(|(id,t)|
                        serde_json::json!({"id":id,"label":t.label,"kind":t.kind,"created":t.created,
                            "expires":t.expires,"conversations":t.conversations,"operations":t.operations}))
                        .collect::<Vec<_>>();
                    serde_json::to_string(&entries).map_err(EngineError::from)
                }
                _ => Err(
                    "Usage: /bot ls [#room|@global] | info <bot name> [#room|@global] | token ls <bot name>".into(),
                ),
            }
        })())
    }
    pub(super) fn prepare_bot_command(
        &mut self,
        actor: Option<&str>,
        view: Option<&str>,
        parts: &[&str],
    ) -> Result<(String, Option<metadata_write::PendingMetadata>), EngineError> {
        let dirty = self.metadata_dirty;
        let result = self.apply_bot_metadata(actor, view, parts);
        let result = match result {
            Ok((reply, Some(before))) => {
                let pending = self.capture_metadata(&before, vec![]);
                before.restore(&mut self.data);
                pending.map(|pending| (reply, Some(pending)))
            }
            Ok((reply, None)) => Ok((reply, None)),
            Err(error) => Err(error),
        };
        self.metadata_dirty = dirty;
        result
    }

    fn apply_bot_command(
        &mut self,
        actor: Option<&str>,
        view: Option<&str>,
        parts: &[&str],
    ) -> Result<String, EngineError> {
        match parts {
            [_, "new", name] | [_, "new", name, _] => {
                let scope = self.bot_scope(view, parts.get(3).copied())?;
                self.require(actor, &Scope::Server, Action::CreateAccount)?;
                self.require_target_command(actor, &scope, "/bot")?;
                if matches!(scope, Scope::Room(_)) { self.require(actor, &scope, Action::Invite)?; }
                // Creating a credentialed participant must not delegate rights the manager lacks.
                for action in DEFAULT_ACTIONS { self.require(actor, &scope, *action)?; }
                for command in DEFAULT_COMMANDS { self.require_target_command(actor, &scope, command)?; }
                if !valid_name(name) { return Err("Invalid username.".into()); }
                if self.data.users.contains_key(*name) { return Err(EngineError::Conflict("Username already exists.".into())); }
                if self.data.users.len() >= self.max_users { return Err(EngineError::Invalid(format!("User limit reached ({}).", self.max_users))); }
                let id = uuid::Uuid::new_v4().to_string();
                // No password is issued. This is an ordinary account; /reset can later set one.
                self.data.users.insert((*name).into(), Account { id: id.clone(), hash: "!no-password".into(), disabled: false, metadata: BTreeMap::new() });
                self.data.bots.insert(id.clone(), Registration {owner_id:self.principal_id(actor).filter(|id| *id != "console").map(str::to_owned), ..Default::default()});
                self.data.policy.grants.push(Grant {
                    owner: false, subject: Subject::Account(id), scope: scope.clone(),
                    permissions: DEFAULT_ACTIONS.iter().map(|a| a.name().to_owned())
                        .chain(DEFAULT_COMMANDS.iter().map(|c| (*c).to_owned())).collect(),
                });
                if let Scope::Room(room_id) = &scope {
                    let room = self.data.rooms.values_mut().find(|r| r.id == *room_id).ok_or_else(|| EngineError::NotFound("Room not found.".into()))?;
                    room.members.insert((*name).into());
                }
                self.audit(actor, "/bot new", name);
                Ok(format!("Created bot connection for {name} at {}. Read/send/reply/react and manuals granted; settings and deletion excluded. Issue an issuer token with /bot token new {name} LABEL.", self.bot_scope_label(&scope)))
            }
            [_, "owner", name, owner] => {
                let id = self.registered_id(name)?;
                self.account_target(actor, name, Action::BotCredentials)?;
                self.require_target_command(actor, &Scope::Server, "/bot")?;
                let owner_id = self.data.users.get(*owner).filter(|user| !user.disabled).ok_or("Owner account unavailable.")?.id.clone();
                self.data.bots.get_mut(&id).ok_or("Bot not found.")?.owner_id = Some(owner_id);
                self.audit(actor, "/bot owner", name);
                Ok(format!("Bot {name} is now owned by {owner}."))
            }
            [_, "token", "new", name, label] => {
                let id = self.registered_id(name)?;
                self.account_target(actor, name, Action::BotCredentials)?;
                self.require_target_command(actor, &Scope::Server, "/bot")?;
                if !valid_name(label) { return Err("Invalid credential label.".into()); }
                let registration = self.data.bots.get_mut(&id).ok_or("Bot not found.")?;
                if registration.tokens.len() >= 8 { return Err("At most 8 credentials per account.".into()); }
                if registration.tokens.values().any(|t| t.label == *label) { return Err("Credential label already exists.".into()); }
                let token_id = uuid::Uuid::new_v4().to_string();
                let secret = format!("bot_{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
                registration.tokens.insert(token_id.clone(), Credential { kind: CredentialKind::Issuer, label: (*label).into(), digest: crate::bots::digest(&secret), created: now(), expires: None, conversations: None, operations: None });
                self.audit(actor, "/bot token new", name);
                Ok(format!("Bot issuer token {token_id} for {name}; save now, shown once:\n{secret}"))
            }
            [_, "token", "rm", name, token_id] => {
                let id = self.registered_id(name)?;
                self.account_target(actor, name, Action::BotCredentials)?;
                self.require_target_command(actor, &Scope::Server, "/bot")?;
                self.data.bots.get_mut(&id).ok_or("Bot not found.")?.tokens.remove(*token_id).ok_or("Credential not found.")?;
                self.audit(actor, "/bot token rm", name);
                Ok(format!("Revoked credential {token_id} for {name}."))
            }
            _ => Err("Usage: /bot ls [#room|@global] | new <bot name> [#room|@global] | info <bot name> [#room|@global] | owner <bot name> <user name> | rm <bot name> @global | token new <bot name> <label> | token ls <bot name> | token rm <credential id> | token rm <bot name> <credential id>".into()),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "tests/engine.rs"]
mod tests;
