use super::*;

impl Engine {
    pub fn is_admin(&self, name: &str) -> bool {
        self.data
            .users
            .get(name)
            .is_some_and(|u| u.admin && !u.disabled)
    }
    pub fn change_password(
        &mut self,
        user: &str,
        expected_hash: &str,
        hash: String,
        keep: &str,
    ) -> Result<String, String> {
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
        self.data.users.get_mut(user).unwrap().hash = hash;
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
        if !self
            .data
            .users
            .get(username)
            .is_some_and(|u| !u.disabled && u.hash == expected_hash)
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
                    admin,
                    disabled: false,
                },
            );
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
        if let Some(actor) = actor {
            let account = self
                .data
                .users
                .get(actor)
                .filter(|u| !u.disabled)
                .ok_or("Account unavailable.")?;
            require_admin(account.admin)?;
            if actor == name {
                return Err("You cannot delete your own account from the web.".into());
            }
        }
        let account = self.data.users.get(name).ok_or("User not found.")?;
        if actor.is_some()
            && account.admin
            && !account.disabled
            && self
                .data
                .users
                .values()
                .filter(|u| u.admin && !u.disabled)
                .count()
                == 1
        {
            return Err("Cannot delete the last active administrator.".into());
        }
        let previous = self.data.clone();
        self.data.users.remove(name);
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
        let CommandContext {
            actor,
            parts,
            admin,
            ..
        } = *context;
        match parts[0] {
            "/grant" | "/revoke" => {
                require_admin(admin)?;
                require_len(parts, 2, "/grant user or /revoke user")?;
                let name = parts[1];
                if !self.active(name) {
                    return Err("User not found.".into());
                }
                let grant = parts[0] == "/grant";
                if !grant
                    && actor.is_some()
                    && self.data.users[name].admin
                    && self
                        .data
                        .users
                        .values()
                        .filter(|u| u.admin && !u.disabled)
                        .count()
                        == 1
                {
                    return Err("Cannot revoke the last active administrator.".into());
                }
                self.data.users.get_mut(name).unwrap().admin = grant;
                Ok(format!(
                    "{name}: permission {}.",
                    if grant { "admin" } else { "user" }
                ))
            }
            "/enable" => {
                require_admin(admin)?;
                require_len(parts, 2, "/enable user")?;
                self.data
                    .users
                    .get_mut(parts[1])
                    .ok_or("User not found.")?
                    .disabled = false;
                Ok(format!("Enabled {}.", parts[1]))
            }
            "/disable" => {
                require_admin(admin)?;
                require_len(parts, 2, "/disable user")?;
                if actor.is_some()
                    && self
                        .data
                        .users
                        .get(parts[1])
                        .is_some_and(|u| u.admin && !u.disabled)
                    && self
                        .data
                        .users
                        .values()
                        .filter(|u| u.admin && !u.disabled)
                        .count()
                        == 1
                {
                    return Err("Cannot disable the last active admin.".into());
                }
                self.data
                    .users
                    .get_mut(parts[1])
                    .ok_or("User not found.")?
                    .disabled = true;
                self.data.sessions.retain(|_, s| s.username != parts[1]);
                for room in self.data.rooms.values_mut() {
                    room.members.remove(parts[1]);
                }
                Ok(format!("Disabled {}.", parts[1]))
            }
            _ => unreachable!("Dispatcher selected the wrong command domain"),
        }
    }
}
