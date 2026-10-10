use super::*;

impl Engine {
    pub(super) fn rename_user(
        &mut self,
        actor: Option<&str>,
        name: &str,
        new_name: &str,
    ) -> Result<String, String> {
        let own_scope = self
            .data
            .users
            .get(name)
            .map(|account| authorization::Scope::Account(account.id.clone()))
            .ok_or("User not found.")?;
        let action = if actor == Some(name)
            && self.allows(actor, &own_scope, authorization::Action::RenameOwn)
        {
            authorization::Action::RenameOwn
        } else {
            authorization::Action::RenameAny
        };
        let scope = self.account_target(actor, name, action)?;
        self.require_target_command(actor, &scope, "/rename")?;
        if !valid_name(new_name) {
            return Err("Names must contain 1–32 Unicode characters without whitespace, controls or ASCII punctuation other than _ and -.".into());
        }
        if name == new_name {
            return Ok("Username unchanged.".into());
        }
        if self.data.users.contains_key(new_name) {
            return Err("Username already exists.".into());
        }
        let before = self.data.clone();
        self.audit(actor, "/rename", &format!("{name} -> {new_name}"));
        let account = self.data.users.remove(name).ok_or("User not found.")?;
        self.data.users.insert(new_name.into(), account);
        for session in self.data.sessions.values_mut() {
            if session.username == name {
                session.username = new_name.into();
            }
        }
        let mut conversations = BTreeMap::new();
        let old_private = std::mem::take(&mut self.data.private);
        for (key, chat) in old_private {
            let new_key = if let Some(peer) = private_peer(&key, name) {
                private_key(new_name, peer)
            } else {
                key.clone()
            };
            if new_key != key {
                conversations.insert(format!("dm:{key}"), format!("dm:{new_key}"));
            }
            self.data.private.insert(new_key, chat);
        }
        for room in self.data.rooms.values_mut() {
            if room.members.remove(name) {
                room.members.insert(new_name.into());
            }
        }
        for (messages, revision) in self
            .data
            .rooms
            .values_mut()
            .map(|r| (&mut r.messages, &mut r.revision))
            .chain(
                self.data
                    .private
                    .values_mut()
                    .map(|r| (&mut r.messages, &mut r.revision)),
            )
        {
            for message in messages {
                if message.from == name {
                    message.from = new_name.into();
                }
                if message.to.as_deref() == Some(name) {
                    message.to = Some(new_name.into());
                }
                if let Some(reply) = &mut message.reply
                    && reply.from == name
                {
                    reply.from = new_name.into();
                }
                if message.mentions.remove(name) {
                    message.mentions.insert(new_name.into());
                }
                for users in message.reactions.values_mut() {
                    if users.remove(name) {
                        users.insert(new_name.into());
                    }
                }
            }
            *revision = revision.wrapping_add(1);
        }
        if let Err(error) = self.store_account_rename(name, new_name, &conversations) {
            self.data = before;
            tracing::error!(error = %error, "Account rename failed");
            return Err("Storage unavailable; rename was not applied.".into());
        }
        self.read_positions.remove(new_name);
        if let Some(positions) = self.read_positions.remove(name) {
            self.read_positions.insert(new_name.into(), positions);
        }
        for positions in self.read_positions.values_mut() {
            for (old, new) in &conversations {
                positions.remove(new);
                if let Some(sequence) = positions.remove(old) {
                    positions.insert(new.clone(), sequence);
                }
            }
        }
        self.revision = self.revision.wrapping_add(1);
        Ok(format!("Renamed {name} to {new_name}."))
    }
    /// Administrator authority comes from the policy, so an account cannot be
    /// made an administrator by editing its stored record. An agent never holds
    /// a group, so it can never pass this check.
    pub fn is_admin(&self, name: &str) -> bool {
        self.has_group(Some(name), authorization::Group::Admin)
            || self.has_group(Some(name), authorization::Group::Su)
    }
    pub fn change_password(
        &mut self,
        user: &str,
        expected_hash: &str,
        hash: String,
        keep: &str,
    ) -> Result<String, String> {
        self.account_target(Some(user), user, authorization::Action::Password)?;
        if !self
            .data
            .users
            .get(user)
            .is_some_and(|u| !u.disabled && u.hash == expected_hash)
            || self.session(keep).as_deref() != Some(user)
        {
            return Err("Account or session changed. Please log in again.".into());
        }
        let before = self.data.clone();
        self.data
            .users
            .get_mut(user)
            .ok_or("Account unavailable.")?
            .hash = hash;
        self.data
            .sessions
            .retain(|token, session| session.username != user || token == keep);
        if let Err(error) = self.save() {
            self.data = before;
            return Err(error);
        }
        Ok("Password changed. Other sessions signed out.".into())
    }
    pub fn active(&self, name: &str) -> bool {
        self.data.users.get(name).is_some_and(|u| !u.disabled)
    }
    pub fn session(&self, token: &str) -> Option<String> {
        self.data
            .sessions
            .get(token)
            .filter(|s| s.expires > now() && self.active(&s.username))
            .map(|s| s.username.clone())
    }
    pub fn login(
        &mut self,
        token: String,
        username: &str,
        expected_hash: &str,
        old: Option<&str>,
    ) -> Result<(), String> {
        // Only a human account with a stored password hash may hold a session;
        // agents and other hashless accounts can never log in.
        if expected_hash.is_empty()
            || !self.data.users.get(username).is_some_and(|u| {
                !u.disabled && u.is_human() && !u.hash.is_empty() && u.hash == expected_hash
            })
        {
            return Err("Account changed. Please try again.".into());
        }
        let before = self.data.clone();
        self.data.sessions.retain(|_, s| s.expires > now());
        if let Some(old) = old {
            self.data.sessions.remove(old);
        }
        if self.data.sessions.len() >= 256 {
            self.data = before;
            return Err("Session limit reached.".into());
        }
        self.data.sessions.insert(
            token,
            Session {
                username: username.into(),
                expires: now() + 43200,
            },
        );
        if let Err(e) = self.save() {
            self.data = before;
            return Err(e);
        }
        Ok(())
    }
    pub fn logout(&mut self, token: &str) -> Result<(), String> {
        let before = self.data.clone();
        self.data.sessions.remove(token);
        if let Err(e) = self.save() {
            self.data = before;
            return Err(e);
        }
        Ok(())
    }
    pub fn provision(
        &mut self,
        name: &str,
        hash: String,
        admin: bool,
        reset: bool,
    ) -> Result<String, String> {
        self.provision_by(None, name, hash, admin, reset)
    }
    pub fn provision_by(
        &mut self,
        actor: Option<&str>,
        name: &str,
        hash: String,
        admin: bool,
        reset: bool,
    ) -> Result<String, String> {
        if reset {
            self.account_target(actor, name, authorization::Action::Reset)?;
        } else {
            self.require(
                actor,
                &authorization::Scope::Server,
                authorization::Action::CreateAccount,
            )?;
            if admin {
                self.require(
                    actor,
                    &authorization::Scope::Server,
                    authorization::Action::AssignAdmin,
                )?;
            }
        }
        if !valid_name(name) {
            return Err("Invalid username.".into());
        }
        let before = self.data.clone();
        if reset {
            let user = self.data.users.get_mut(name).ok_or("User not found.")?;
            user.hash = hash;
            self.data.sessions.retain(|_, s| s.username != name);
        } else {
            if self.data.users.contains_key(name) {
                return Err("Username already exists.".into());
            }
            if self.data.users.len() >= self.max_users {
                return Err(format!("User limit reached ({}).", self.max_users));
            }
            self.data.users.insert(
                name.into(),
                Account {
                    hash,
                    id: uuid::Uuid::new_v4().to_string(),
                    disabled: false,
                    agent: false,
                    api_key: String::new(),
                    reply: AgentReply::default(),
                    search_key: String::new(),
                    search: false,
                    sources: SourceMode::default(),
                    prompt: String::new(),
                    provider: String::new(),
                    base_url: String::new(),
                    model: String::new(),
                },
            );
        }
        self.audit(actor, if reset { "/reset" } else { "/user" }, name);
        if !reset {
            let id = self
                .data
                .users
                .get(name)
                .ok_or("User not found.")?
                .id
                .clone();
            self.assign(
                id,
                if admin {
                    authorization::Group::Admin
                } else {
                    authorization::Group::User
                },
                authorization::Scope::Server,
            );
            self.data.policy.revision = self.data.policy.revision.wrapping_add(1);
        }
        if let Err(e) = self.save() {
            self.data = before;
            return Err(e);
        }
        Ok(format!(
            "Account {name} {}.",
            if reset { "password reset" } else { "created" }
        ))
    }
    pub(super) fn delete_user(
        &mut self,
        actor: Option<&str>,
        name: &str,
    ) -> Result<String, String> {
        self.account_target(actor, name, authorization::Action::DeleteAccount)?;
        if actor.is_some() && !self.is_su(actor) && actor == Some(name) {
            return Err("You cannot delete your own account from the web.".into());
        }
        if !self.is_su(actor)
            && self.is_admin(name)
            && self.data.users.keys().filter(|n| self.is_admin(n)).count() == 1
        {
            return Err("Cannot delete the last active administrator.".into());
        }
        let previous = self.data.clone();
        let deleted_id = self
            .data
            .users
            .get(name)
            .ok_or("User not found.")?
            .id
            .clone();
        self.audit(actor, "/deleteuser", name);
        self.data.policy.assignments.retain(|a| {
            a.account_id != deleted_id
                && a.scope != authorization::Scope::Account(deleted_id.clone())
        });
        self.data.policy.grants.retain(|g| {
            g.subject != authorization::Subject::Account(deleted_id.clone())
                && g.scope != authorization::Scope::Account(deleted_id.clone())
        });
        self.data.policy.revision = self.data.policy.revision.wrapping_add(1);
        self.data.users.remove(name);
        for room in self.data.rooms.values_mut() {
            if room.owner_id == deleted_id {
                room.owner_id = "console".into();
            }
        }
        self.data
            .sessions
            .retain(|_, session| session.username != name);
        let deleted_author = format!("{name} (deleted)");
        for room in self.data.rooms.values_mut() {
            room.members.remove(name);
            let mut changed = false;
            for message in &mut room.messages {
                if message.from == name {
                    message.from.clone_from(&deleted_author);
                    changed = true;
                }
                if let Some(reply) = &mut message.reply
                    && reply.from == name
                {
                    reply.from.clone_from(&deleted_author);
                    changed = true;
                }
                changed |= message.mentions.remove(name);
                message.reactions.retain(|_, users| {
                    changed |= users.remove(name);
                    !users.is_empty()
                });
            }
            if changed {
                room.revision = room.revision.wrapping_add(1);
            }
        }
        let mut removed = BTreeSet::new();
        let removed_scopes: Vec<_> = self
            .data
            .private
            .iter()
            .filter(|(key, _)| private_peer(key, name).is_some())
            .map(|(_, chat)| authorization::Scope::Private(chat.id.clone()))
            .collect();
        self.data
            .policy
            .grants
            .retain(|g| !removed_scopes.contains(&g.scope));
        self.data
            .policy
            .assignments
            .retain(|a| !removed_scopes.contains(&a.scope));
        self.data.private.retain(|key, _| {
            if private_peer(key, name).is_some() {
                removed.insert(format!("dm:{key}"));
                false
            } else {
                true
            }
        });
        // Commit account/session/history cleanup and read positions together.
        // Only update the in-memory cursor cache after the transaction succeeds.
        let result = self.store_account_deletion(name, &removed);
        if let Err(error) = result {
            self.data = previous;
            tracing::error!(error = %error, "Account deletion failed");
            return Err("Storage unavailable; account deletion was not applied.".into());
        }
        self.rebuild_authorization()?;
        self.read_positions.remove(name);
        for positions in self.read_positions.values_mut() {
            positions.retain(|view, _| !removed.contains(view));
        }
        self.revision = self.revision.wrapping_add(1);
        Ok(format!(
            "Deleted account {name} and its private conversations. Room messages retained as {deleted_author}."
        ))
    }
    pub(super) fn apply_accounts(
        &mut self,
        context: &CommandContext<'_>,
    ) -> Result<String, String> {
        let CommandContext { actor, parts, .. } = *context;
        match parts[0] {
            "/enable" => {
                require_len(parts, 2, "/enable user")?;
                let scope = self.account_target(actor, parts[1], authorization::Action::Enable)?;
                self.require_target_command(actor, &scope, parts[0])?;
                self.audit(actor, "/enable", parts[1]);
                self.data
                    .users
                    .get_mut(parts[1])
                    .ok_or("User not found.")?
                    .disabled = false;
                Ok(format!("Enabled {}.", parts[1]))
            }
            "/disable" => {
                require_len(parts, 2, "/disable user")?;
                let scope = self.account_target(actor, parts[1], authorization::Action::Disable)?;
                self.require_target_command(actor, &scope, parts[0])?;
                if !self.is_su(actor)
                    && self.is_admin(parts[1])
                    && self.data.users.keys().filter(|n| self.is_admin(n)).count() == 1
                {
                    return Err("Cannot disable the last active admin.".into());
                }
                self.audit(actor, "/disable", parts[1]);
                self.data
                    .users
                    .get_mut(parts[1])
                    .ok_or("User not found.")?
                    .disabled = true;
                self.data.sessions.retain(|_, s| s.username != parts[1]);
                let id = self
                    .data
                    .users
                    .get(parts[1])
                    .ok_or("User not found.")?
                    .id
                    .clone();
                self.data.policy.assignments.retain(|a| {
                    a.account_id != id || !matches!(a.scope, authorization::Scope::Room(_))
                });
                self.data.policy.grants.retain(|g| {
                    g.subject != authorization::Subject::Account(id.clone())
                        || !matches!(g.scope, authorization::Scope::Room(_))
                });
                let mut recovered = Vec::new();
                for room in self.data.rooms.values_mut() {
                    room.members.remove(parts[1]);
                    if room.owner_id == id {
                        room.owner_id = "console".into();
                        recovered.push(authorization::Scope::Room(room.id.clone()));
                    }
                }
                for scope in recovered {
                    self.add_owner_grants("console".into(), scope);
                }
                Ok(format!("Disabled {}.", parts[1]))
            }
            _ => Err("Unknown command.".into()),
        }
    }
}
