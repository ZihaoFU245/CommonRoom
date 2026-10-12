//! Bot lifecycle is a thin extension of the engine's ordinary account and grants.
use super::*;
use crate::bots::models::{Credential, Registration};
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
    "/help", "/man", "/whoami", "/members", "/history", "/join", "/reply", "/react",
];

impl Engine {
    pub fn authorize_bot_config(
        &self,
        actor: Option<&str>,
        view: Option<&str>,
        name: &str,
        write: bool,
    ) -> Result<String, String> {
        self.require_command(actor, view, "/bot")?;
        let scope = self.account_target(
            actor,
            name,
            if write {
                Action::BotConfigWrite
            } else {
                Action::BotConfigRead
            },
        )?;
        self.require_target_command(actor, &scope, "/bot")?;
        if !self.active(name) {
            return Err("Bot account disabled.".into());
        }
        self.registered_id(name)
    }

    pub fn admit_bot_input(&mut self, secret: &str) -> Result<(), String> {
        let name = self.bot_identity(secret).ok_or("Credential revoked.")?;
        let id = self
            .data
            .users
            .get(&name)
            .ok_or("Account unavailable.")?
            .id
            .clone();
        let now = std::time::Instant::now();
        self.bot_budgets
            .retain(|_, (at, _)| now.duration_since(*at).as_secs() < 10);
        let entry = self.bot_budgets.entry(id).or_insert((now, 0));
        if entry.1 >= 30 {
            return Err("Slow down; at most 30 bot inputs per 10 seconds per account.".into());
        }
        entry.1 += 1;
        Ok(())
    }

    pub fn bot_identity(&self, secret: &str) -> Option<String> {
        if secret.len() != 68 || !secret.starts_with("bot_") {
            return None;
        }
        let digest = crate::bots::digest(secret);
        let account_id = self
            .data
            .bots
            .iter()
            .find(|(_, registration)| {
                registration
                    .tokens
                    .values()
                    .any(|token| token.digest == digest)
            })?
            .0;
        self.data
            .users
            .iter()
            .find(|(_, account)| account.id == *account_id && !account.disabled)
            .map(|(name, _)| name.clone())
    }

    fn bot_scope(&self, view: Option<&str>, explicit: Option<&str>) -> Result<Scope, String> {
        match explicit.or(view) {
            Some("@global") if explicit.is_some() => Ok(Scope::Server),
            Some(name) if !name.starts_with('@') => self.room_scope(name),
            _ => Err("Specify a room or @global explicitly; Console and private views have no default bot scope.".into()),
        }
    }

    fn registered_id(&self, name: &str) -> Result<String, String> {
        let id = self
            .data
            .users
            .get(name)
            .ok_or("User not found.")?
            .id
            .clone();
        if !self.data.bots.contains_key(&id) {
            return Err("No bot connection registered for this account.".into());
        }
        Ok(id)
    }

    pub(super) fn bot_command(
        &mut self,
        actor: Option<&str>,
        view: Option<&str>,
        parts: &[&str],
    ) -> Result<String, String> {
        self.require_command(actor, view, "/bot")?;
        if parts.get(1) == Some(&"config") {
            let request = crate::bots::runtime::ConfigCommand::parse(&parts.join(" "))?
                .ok_or("Invalid config command.")?;
            self.authorize_bot_config(actor, view, &request.name, request.operation == "set")?;
            return Err("Bot configuration requires a connected external runtime; use the server console or API/WebSocket command transport.".into());
        }
        if !(self.is_su(actor) || self.has_group(actor, Group::Admin)) {
            return Err("Only global administrators may manage bot connections.".into());
        }
        // Deletion already owns the transaction for account/history/cursor cleanup.
        if let [_, "rm", name, "@global"] = parts {
            self.registered_id(name)?;
            self.require_target_command(actor, &Scope::Server, "/bot")?;
            return self.delete_user(actor, name);
        }
        let before = self.data.clone();
        let result = self.apply_bot_command(actor, view, parts);
        match result {
            Ok(reply) => {
                if self.data == before {
                    return Ok(reply);
                }
                self.data.policy.revision = before.policy.revision.wrapping_add(1);
                if let Err(error) = self.save() {
                    self.data = before;
                    return Err(error);
                }
                Ok(reply)
            }
            Err(error) => {
                self.data = before;
                Err(error)
            }
        }
    }

    fn apply_bot_command(
        &mut self,
        actor: Option<&str>,
        view: Option<&str>,
        parts: &[&str],
    ) -> Result<String, String> {
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
                if self.data.users.contains_key(*name) { return Err("Username already exists.".into()); }
                if self.data.users.len() >= self.max_users { return Err(format!("User limit reached ({}).", self.max_users)); }
                let id = uuid::Uuid::new_v4().to_string();
                // No password is issued. This is an ordinary account; /reset can later set one.
                self.data.users.insert((*name).into(), Account { id: id.clone(), hash: "!no-password".into(), disabled: false });
                self.data.bots.insert(id.clone(), Registration::default());
                self.data.policy.grants.push(Grant {
                    owner: false, subject: Subject::Account(id), scope: scope.clone(), minimum_age: 0,
                    permissions: DEFAULT_ACTIONS.iter().map(|a| a.name().to_owned())
                        .chain(DEFAULT_COMMANDS.iter().map(|c| (*c).to_owned())).collect(),
                });
                if let Scope::Room(room_id) = &scope {
                    let room = self.data.rooms.values_mut().find(|r| r.id == *room_id).ok_or("Room not found.")?;
                    room.members.insert((*name).into());
                }
                self.audit(actor, "/bot new", name);
                Ok(format!("Created bot connection for {name} at {}. Read/send/reply/react and manuals granted; settings and deletion excluded. Issue an account-wide credential with /bot token new {name} LABEL.", self.scope_label(&scope)))
            }
            [_, "ls"] | [_, "ls", _] => {
                let scope = self.bot_scope(view, parts.get(2).copied())?;
                self.require_target_command(actor, &scope, "/bot")?;
                if matches!(scope, Scope::Room(_)) { self.require(actor, &scope, Action::Members)?; }
                let names: Vec<_> = self.data.users.iter().filter(|(name, account)| {
                    self.data.bots.contains_key(&account.id) && match &scope {
                        Scope::Server => true,
                        Scope::Room(id) => self.data.rooms.values().any(|r| r.id == *id && r.members.contains(*name)),
                        _ => false,
                    }
                }).map(|(name, _)| name.as_str()).collect();
                Ok(format!("Bot connections at {}: {}", self.scope_label(&scope), names.join(", ")))
            }
            [_, "info", name] | [_, "info", name, _] => {
                let scope = self.bot_scope(view, parts.get(3).copied())?;
                self.require_target_command(actor, &scope, "/bot")?;
                let id = self.registered_id(name)?;
                if let Scope::Room(room_id) = &scope {
                    self.require(actor, &scope, Action::Members)?;
                    if !self.data.rooms.values().any(|r| r.id == *room_id && r.members.contains(*name)) {
                        return Err("Account is not a member of this room.".into());
                    }
                }
                let commands = self.commands_for(Some(name), match &scope {
                    Scope::Room(id) => self.data.rooms.iter().find(|(_, r)| r.id == *id).map(|(n, _)| n.as_str()),
                    _ => None,
                }).iter().map(|c| c.name).collect::<Vec<_>>().join(", ");
                Ok(format!("{name} ({id}) at {}\nDisabled: {}\nConnected: {}\nPermissions: {}\nCommands: {commands}", self.scope_label(&scope), !self.active(name), self.online_connections.get(*name).copied().unwrap_or(0), self.effective(Some(name), &scope).names().join(", ")))
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
                registration.tokens.insert(token_id.clone(), Credential { label: (*label).into(), digest: crate::bots::digest(&secret), created: now() });
                self.audit(actor, "/bot token new", name);
                Ok(format!("Account-wide credential {token_id} for {name}; save now, shown once:\n{secret}"))
            }
            [_, "token", "ls", name] => {
                let id = self.registered_id(name)?;
                self.account_target(actor, name, Action::BotCredentials)?;
                self.require_target_command(actor, &Scope::Server, "/bot")?;
                let lines = self.data.bots[&id].tokens.iter().map(|(id, t)| format!("{id} {} created={}", t.label, t.created)).collect::<Vec<_>>();
                Ok(format!("Account-wide credentials for {name}:\n{}", lines.join("\n")))
            }
            [_, "token", "rm", name, token_id] => {
                let id = self.registered_id(name)?;
                self.account_target(actor, name, Action::BotCredentials)?;
                self.require_target_command(actor, &Scope::Server, "/bot")?;
                self.data.bots.get_mut(&id).ok_or("Bot not found.")?.tokens.remove(*token_id).ok_or("Credential not found.")?;
                self.audit(actor, "/bot token rm", name);
                Ok(format!("Revoked account-wide credential {token_id} for {name}."))
            }
            _ => Err("Usage: /bot new name [room|@global] | ls [room|@global] | info name [room|@global] | rm name @global | token new name label | token ls name | token rm name id | config set name key value | config fetch name [key]".into()),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "engine_tests.rs"]
mod tests;
