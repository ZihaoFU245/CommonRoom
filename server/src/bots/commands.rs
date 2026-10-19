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
    pub fn admit_bot_input(&mut self, secret: &str) -> Result<(), String> {
        let name = self.bot_identity(secret).ok_or("Credential revoked.")?;
        let id = self
            .data
            .users
            .get(&name)
            .ok_or("Account unavailable.")?
            .id
            .clone();
        self.admit_bot_account(&id)
    }
    pub(crate) fn admit_bot_account(&mut self, id: &str) -> Result<(), String> {
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
        if !(self.is_su(actor) || self.has_group(actor, Group::Admin)) {
            return Err("Only global administrators may manage bot connections.".into());
        }
        // Deletion already owns the transaction for account/history/cursor cleanup.
        if let [_, "rm", name, "@global"] = parts {
            self.registered_id(name)?;
            self.require_target_command(actor, &Scope::Server, "/bot")?;
            return self.delete_user(actor, name);
        }
        if matches!(parts.get(1).copied(), Some("ls" | "info")) {
            return self.apply_bot_command(actor, view, parts);
        }
        let before = self.data.clone();
        let result = self.apply_bot_command(actor, view, parts);
        match result {
            Ok(reply) => {
                if self.data == before {
                    return Ok(reply);
                }
                self.data.policy.revision = before.policy.revision.wrapping_add(1);
                if let Err(error) = self.save_delta(&before) {
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
                self.data.bots.insert(id.clone(), Registration {owner_id:self.principal_id(actor).filter(|id| *id != "console").map(str::to_owned), ..Default::default()});
                self.data.policy.grants.push(Grant {
                    owner: false, subject: Subject::Account(id), scope: scope.clone(), minimum_age: 0,
            media: None,
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
            [_, "owner", name, owner] => {
                let id = self.registered_id(name)?;
                self.account_target(actor, name, Action::BotCredentials)?;
                self.require_target_command(actor, &Scope::Server, "/bot")?;
                let owner_id = self.data.users.get(*owner).filter(|user| !user.disabled).ok_or("Owner account unavailable.")?.id.clone();
                self.data.bots.get_mut(&id).ok_or("Bot not found.")?.owner_id = Some(owner_id);
                self.audit(actor, "/bot owner", name);
                Ok(format!("Bot {name} is now owned by {owner}."))
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
                Ok(format!("{name} ({id}) at {}\nOwner: {}\nDisabled: {}\nConnected: {}\nPermissions: {}\nCommands: {commands}", self.scope_label(&scope), self.bot_owner(&id).unwrap_or_else(|| "administrators".into()), !self.active(name), self.online_connections.get(*name).copied().unwrap_or(0), self.effective(Some(name), &scope).names().join(", ")))
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
                registration.tokens.insert(token_id.clone(), Credential { label: (*label).into(), digest: crate::bots::digest(&secret), created: now(), expires: None, conversations: None, operations: None });
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
            _ => Err("Usage: /bot new name [room|@global] | ls [room|@global] | info name [room|@global] | owner name user | rm name @global | token new name label | token ls name | token rm name id".into()),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "engine_tests.rs"]
mod tests;
