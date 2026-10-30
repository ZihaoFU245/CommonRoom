use super::*;

struct AccountDeletion {
    before: MetadataChange,
    deleted_id: String,
    removed: BTreeSet<String>,
    reply: String,
}

struct AccountRename {
    before: MetadataChange,
    conversations: BTreeMap<String, String>,
}
pub(crate) struct AccountRenameRead {
    database: database::Handle,
    name: String,
    new_name: String,
    account: String,
    command: String,
}
pub(crate) struct AccountRenameReferences {
    request: AccountRenameRead,
    affected: BTreeSet<String>,
}
impl AccountRenameRead {
    pub(crate) async fn read(self) -> Result<AccountRenameReferences, EngineError> {
        let affected = self.database.account_conversations(&self.name).await?;
        Ok(AccountRenameReferences {
            request: self,
            affected,
        })
    }
}

impl Engine {
    #[cfg(test)]
    pub(super) fn rename_user(
        &mut self,
        actor: Option<&str>,
        name: &str,
        new_name: &str,
        command: &str,
    ) -> Result<String, EngineError> {
        if !self.validate_account_rename(actor, name, new_name, command)? {
            return Ok("Username unchanged.".into());
        }
        let affected = self.account_reference_ids(name).map_err(|error| {
            tracing::error!(error=%error,"Account rename reference lookup failed");
            EngineError::Unavailable("Storage unavailable; rename was not applied.".into())
        })?;
        let AccountRename {
            before,
            conversations,
        } = self.apply_account_rename(actor, name, new_name, command, &affected)?;
        if let Err(error) = self.store_account_rename(name, new_name, &conversations, &before) {
            before.restore(&mut self.data);
            tracing::error!(error = %error, "Account rename failed");
            return Err(EngineError::Unavailable(
                "Storage unavailable; rename was not applied.".into(),
            ));
        }

        self.store.read_positions.remove(new_name);
        if let Some(positions) = self.store.read_positions.remove(name) {
            self.store.read_positions.insert(new_name.into(), positions);
        }
        for positions in self.store.read_positions.values_mut() {
            for (old, new) in &conversations {
                positions.remove(new);
                if let Some(sequence) = positions.remove(old) {
                    positions.insert(new.clone(), sequence);
                }
            }
        }
        self.bot_index = crate::bots::models::Index::build(&self.data);
        let notice = MetadataNotice::between(
            &self.data,
            before.view(),
            &self.authorization,
            &self.authorization,
        );
        self.advance_colors();
        self.finish_metadata_commit(notice);
        Ok(format!("Renamed {name} to {new_name}."))
    }
    fn validate_account_rename(
        &self,
        actor: Option<&str>,
        name: &str,
        new_name: &str,
        command: &str,
    ) -> Result<bool, EngineError> {
        let own_scope = self
            .data
            .users
            .get(name)
            .map(|account| authorization::Scope::Account(account.id.clone()))
            .ok_or_else(|| EngineError::NotFound("User not found.".into()))?;
        let action = if actor == Some(name)
            && self.allows(actor, &own_scope, authorization::Action::RenameOwn)
        {
            authorization::Action::RenameOwn
        } else {
            authorization::Action::RenameAny
        };
        let scope = self.account_target(actor, name, action)?;
        self.require_target_command(actor, &scope, command)?;
        if !valid_name(new_name) {
            return Err("Names must contain 1–32 Unicode characters without whitespace, controls or ASCII punctuation other than _ and -.".into());
        }
        if name == new_name {
            return Ok(false);
        }
        if self.data.users.contains_key(new_name) {
            return Err(EngineError::Conflict("Username already exists.".into()));
        }
        Ok(true)
    }
    fn apply_account_rename(
        &mut self,
        actor: Option<&str>,
        name: &str,
        new_name: &str,
        command: &str,
        affected: &BTreeSet<String>,
    ) -> Result<AccountRename, EngineError> {
        if !self.validate_account_rename(actor, name, new_name, command)? {
            return Err(EngineError::Conflict(
                "Account rename changed; retry.".into(),
            ));
        }
        let mut keys = changes::DirtyKeys {
            users: [name.to_owned(), new_name.to_owned()].into(),
            sessions: self
                .data
                .sessions
                .iter()
                .filter(|(_, session)| session.username == name)
                .map(|(key, _)| key.clone())
                .collect(),
            rooms: self
                .data
                .rooms
                .iter()
                .filter(|(_, room)| room.members.contains(name) || affected.contains(&room.id))
                .map(|(name, _)| name.clone())
                .collect(),
            ..Default::default()
        };
        for (key, chat) in &self.data.private {
            if let Some(peer) = private_peer(key, name) {
                keys.private.insert(key.clone());
                keys.private.insert(private_key(new_name, peer));
            } else if affected.contains(&chat.id) {
                keys.private.insert(key.clone());
            }
        }
        let before = MetadataChange::keyed(&self.data, keys);
        self.audit(actor, "/rename", &format!("{name} -> {new_name}"));
        let account = self
            .data
            .users
            .remove(name)
            .ok_or_else(|| EngineError::NotFound("User not found.".into()))?;
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
        for (id, revision) in self
            .data
            .rooms
            .values_mut()
            .map(|room| (&room.id, &mut room.revision))
            .chain(
                self.data
                    .private
                    .values_mut()
                    .map(|chat| (&chat.id, &mut chat.revision)),
            )
        {
            if affected.contains(id) {
                *revision = revision.wrapping_add(1);
            }
        }
        Ok(AccountRename {
            before,
            conversations,
        })
    }
    pub(super) fn account_rename_read(
        &self,
        actor: Option<&str>,
        view: Option<&str>,
        input: &str,
    ) -> Result<Option<AccountRenameRead>, EngineError> {
        crate::limits::validate_input(input).map_err(EngineError::Invalid)?;
        crate::commands::validate_room_arguments(input).map_err(EngineError::Invalid)?;
        let command = input.split_whitespace().next().unwrap_or("");
        self.require_command(actor, view, command)?;
        let routed = crate::commands::route(input);
        let parts = routed.split_whitespace().collect::<Vec<_>>();
        let usage = || EngineError::Invalid(format!("Usage: {}", crate::commands::RENAME_USAGE));
        let (name, new_name) = match parts.as_slice() {
            [_, "user", name, new_name] | [_, name, new_name] => (*name, *new_name),
            [_, new_name] => (actor.ok_or_else(usage)?, *new_name),
            _ => return Err(usage()),
        };
        if !self.validate_account_rename(actor, name, new_name, command)? {
            return Ok(None);
        }
        Ok(Some(AccountRenameRead {
            database: self.store.db.clone(),
            name: name.into(),
            new_name: new_name.into(),
            account: self.account_id(name).ok_or("User not found.")?.into(),
            command: command.into(),
        }))
    }
    pub(crate) fn prepare_referenced_rename(
        &mut self,
        actor: Option<&str>,
        references: AccountRenameReferences,
    ) -> Result<(String, metadata_write::PendingMetadata), EngineError> {
        let request = references.request;
        if self.account_id(&request.name) != Some(request.account.as_str()) {
            return Err(EngineError::Conflict(
                "Account changed; retry rename.".into(),
            ));
        }
        let dirty = self.metadata_dirty;
        let AccountRename {
            before,
            conversations,
        } = self.apply_account_rename(
            actor,
            &request.name,
            &request.new_name,
            &request.command,
            &references.affected,
        )?;
        let pending = self
            .capture_metadata(&before, conversations.into_iter().collect())
            .map(|pending| pending.account_rename(request.name.clone(), request.new_name.clone()));
        before.restore(&mut self.data);
        self.metadata_dirty = dirty;
        pending.map(|pending| {
            (
                format!("Renamed {} to {}.", request.name, request.new_name),
                pending,
            )
        })
    }
    pub fn is_admin(&self, name: &str) -> bool {
        self.has_group(Some(name), authorization::Group::Admin)
            || self.has_group(Some(name), authorization::Group::Su)
    }
    #[cfg(test)]
    pub fn change_password(
        &mut self,
        user: &str,
        expected_hash: &str,
        hash: String,
        keep: &str,
    ) -> Result<String, EngineError> {
        let (reply, before) = self.apply_password_change(user, expected_hash, hash, keep)?;
        self.commit_account_metadata(before)?;
        Ok(reply)
    }
    fn apply_password_change(
        &mut self,
        user: &str,
        expected_hash: &str,
        hash: String,
        keep: &str,
    ) -> Result<(String, MetadataChange), EngineError> {
        self.account_target(Some(user), user, authorization::Action::Password)?;
        if !self
            .data
            .users
            .get(user)
            .is_some_and(|u| !u.disabled && u.hash == expected_hash)
            || self
                .session(keep)
                .or_else(|| self.bot_identity(keep))
                .as_deref()
                != Some(user)
        {
            return Err(EngineError::Conflict(
                "Account or session changed. Please log in again.".into(),
            ));
        }
        let before = MetadataChange::accounts(&self.data, [user.to_owned()], true);
        self.data
            .users
            .get_mut(user)
            .ok_or_else(|| EngineError::Forbidden("Account unavailable.".into()))?
            .hash = hash;
        let keep = session_digest(keep);
        self.data
            .sessions
            .retain(|token, session| session.username != user || token == &keep);
        Ok((
            "Password changed. Other sessions signed out.".into(),
            before,
        ))
    }
    /// Stable identity lookup. Disabled accounts cannot acquire new work leases.
    pub(crate) fn active_account_id(&self, name: &str) -> Option<&str> {
        self.data
            .users
            .get(name)
            .filter(|account| !account.disabled)
            .map(|account| account.id.as_str())
    }

    /// Includes disabled accounts so administrative reset checks detect replacement.
    pub(crate) fn account_id(&self, name: &str) -> Option<&str> {
        self.data.users.get(name).map(|account| account.id.as_str())
    }

    pub(crate) fn active_account_name(&self, id: &str) -> Option<&str> {
        let name = self.bot_index.accounts.get(id)?;
        (self.active_account_id(name) == Some(id)).then_some(name.as_str())
    }

    pub(crate) fn password_hash(&self, name: &str) -> Option<&str> {
        self.data
            .users
            .get(name)
            .filter(|account| !account.disabled)
            .map(|account| account.hash.as_str())
    }

    pub(crate) fn has_accounts(&self) -> bool {
        !self.data.users.is_empty()
    }

    pub fn active(&self, name: &str) -> bool {
        self.data.users.get(name).is_some_and(|u| !u.disabled)
    }
    pub fn session(&self, token: &str) -> Option<String> {
        self.data
            .sessions
            .get(&session_digest(token))
            .filter(|s| s.expires > now() && self.active(&s.username))
            .map(|s| s.username.clone())
    }
    #[cfg(test)]
    pub fn login(
        &mut self,
        token: String,
        username: &str,
        expected_hash: &str,
        old: Option<&str>,
    ) -> Result<(), EngineError> {
        self.login_recorded(token, username, expected_hash, old, None)
    }
    #[cfg(test)]
    pub(crate) fn login_recorded(
        &mut self,
        token: String,
        username: &str,
        expected_hash: &str,
        old: Option<&str>,
        event: Option<&AuthEvent>,
    ) -> Result<(), EngineError> {
        let sessions = self.login_sessions(token, username, expected_hash, old)?;
        let before = std::mem::replace(&mut self.data.sessions, sessions);
        if let Err(error) = self.save_sessions(&before, event) {
            self.data.sessions = before;
            return Err(error);
        }
        Ok(())
    }
    pub(super) fn login_sessions(
        &self,
        token: String,
        username: &str,
        expected_hash: &str,
        old: Option<&str>,
    ) -> Result<BTreeMap<String, Session>, EngineError> {
        if !self
            .data
            .users
            .get(username)
            .is_some_and(|u| !u.disabled && u.hash == expected_hash)
        {
            return Err(EngineError::Conflict(
                "Account changed. Please try again.".into(),
            ));
        }
        let mut sessions = self.data.sessions.clone();
        sessions.retain(|_, s| s.expires > now());
        if let Some(old) = old {
            sessions.remove(&session_digest(old));
        }
        // One account cannot reserve the instance's login capacity. Expiry
        // orders sessions without changing the persisted session format.
        while sessions.values().filter(|s| s.username == username).count() >= 8 {
            let oldest = sessions
                .iter()
                .filter(|(_, s)| s.username == username)
                .min_by_key(|(_, s)| s.expires)
                .map(|(token, _)| token.clone());
            if let Some(oldest) = oldest {
                sessions.remove(&oldest);
            }
        }
        while sessions.len() >= self.max_users.saturating_mul(8) {
            let oldest = sessions
                .iter()
                .min_by_key(|(_, s)| s.expires)
                .map(|(token, _)| token.clone());
            if let Some(oldest) = oldest {
                sessions.remove(&oldest);
            }
        }
        sessions.insert(
            session_digest(&token),
            Session {
                username: username.into(),
                expires: now() + 43200,
            },
        );
        Ok(sessions)
    }
    #[cfg(test)]
    pub fn logout(&mut self, token: &str) -> Result<(), EngineError> {
        self.logout_recorded(token, None).map(|_| ())
    }
    #[cfg(test)]
    pub(crate) fn logout_recorded(
        &mut self,
        token: &str,
        event: Option<&AuthEvent>,
    ) -> Result<Option<String>, EngineError> {
        let key = session_digest(token);
        if !self.data.sessions.contains_key(&key) {
            return Ok(None);
        }
        let user = self.session(token);
        let before = self.data.sessions.clone();
        self.data.sessions.remove(&key);
        if let Err(error) = self.save_sessions(&before, event.filter(|_| user.is_some())) {
            self.data.sessions = before;
            return Err(error);
        }
        Ok(user)
    }
    #[cfg(test)]
    pub fn provision(
        &mut self,
        name: &str,
        hash: String,
        admin: bool,
        reset: bool,
    ) -> Result<String, EngineError> {
        self.provision_by(None, name, hash, admin, reset)
    }
    #[cfg(test)]
    pub fn provision_by(
        &mut self,
        actor: Option<&str>,
        name: &str,
        hash: String,
        admin: bool,
        reset: bool,
    ) -> Result<String, EngineError> {
        let (reply, before) = self.apply_provision(actor, name, hash, admin, reset)?;
        self.commit_account_metadata(before)?;
        Ok(reply)
    }
    fn apply_provision(
        &mut self,
        actor: Option<&str>,
        name: &str,
        hash: String,
        admin: bool,
        reset: bool,
    ) -> Result<(String, MetadataChange), EngineError> {
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
        let before = MetadataChange::accounts(&self.data, [name.to_owned()], true);
        if reset {
            let user = self
                .data
                .users
                .get_mut(name)
                .ok_or_else(|| EngineError::NotFound("User not found.".into()))?;
            user.hash = hash;
            self.data.sessions.retain(|_, s| s.username != name);
        } else {
            if self.data.users.contains_key(name) {
                return Err(EngineError::Conflict("Username already exists.".into()));
            }
            if self.data.users.len() >= self.max_users {
                return Err(EngineError::Invalid(format!(
                    "User limit reached ({}).",
                    self.max_users
                )));
            }
            self.data.users.insert(
                name.into(),
                Account {
                    hash,
                    id: uuid::Uuid::new_v4().to_string(),
                    disabled: false,
                    metadata: BTreeMap::new(),
                },
            );
        }
        self.audit(actor, if reset { "/reset" } else { "/new" }, name);
        if !reset {
            let id = self
                .data
                .users
                .get(name)
                .ok_or_else(|| EngineError::NotFound("User not found.".into()))?
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
        Ok((
            format!(
                "Account {name} {}.",
                if reset { "password reset" } else { "created" }
            ),
            before,
        ))
    }
    fn prepare_account_change(
        &mut self,
        change: impl FnOnce(&mut Self) -> Result<(String, MetadataChange), EngineError>,
    ) -> Result<(String, metadata_write::PendingMetadata), EngineError> {
        let dirty = self.metadata_dirty;
        let result = change(self);
        let result = match result {
            Ok((reply, before)) => {
                let pending = self.capture_metadata(&before, vec![]);
                before.restore(&mut self.data);
                pending.map(|pending| (reply, pending))
            }
            Err(error) => Err(error),
        };
        self.metadata_dirty = dirty;
        result
    }
    pub(crate) fn prepare_password_change(
        &mut self,
        user: &str,
        expected_hash: &str,
        hash: String,
        keep: &str,
    ) -> Result<(String, metadata_write::PendingMetadata), EngineError> {
        self.prepare_account_change(|engine| {
            engine.apply_password_change(user, expected_hash, hash, keep)
        })
    }
    pub(crate) fn prepare_provision(
        &mut self,
        actor: Option<&str>,
        name: &str,
        hash: String,
        admin: bool,
        reset: bool,
    ) -> Result<(String, metadata_write::PendingMetadata), EngineError> {
        self.prepare_account_change(|engine| {
            engine.apply_provision(actor, name, hash, admin, reset)
        })
    }
    fn apply_account_deletion(
        &mut self,
        actor: Option<&str>,
        name: &str,
    ) -> Result<AccountDeletion, EngineError> {
        self.account_target(actor, name, authorization::Action::DeleteAccount)?;
        if actor.is_some() && !self.is_su(actor) && actor == Some(name) {
            return Err(EngineError::Forbidden(
                "You cannot delete your own account from the web.".into(),
            ));
        }
        if !self.is_su(actor)
            && self.is_admin(name)
            && self
                .data
                .users
                .keys()
                .filter(|n| self.active(n) && self.is_admin(n))
                .count()
                == 1
        {
            return Err(EngineError::Forbidden(
                "Cannot delete the last active administrator.".into(),
            ));
        }
        let deleted_id = self
            .data
            .users
            .get(name)
            .ok_or_else(|| EngineError::NotFound("User not found.".into()))?
            .id
            .clone();
        let previous = MetadataChange::keyed(
            &self.data,
            changes::DirtyKeys {
                users: [name.to_owned()].into(),
                sessions: self
                    .data
                    .sessions
                    .iter()
                    .filter(|(_, session)| session.username == name)
                    .map(|(key, _)| key.clone())
                    .collect(),
                // Deletion anonymizes history and intentionally advances every
                // conversation revision, so each of these rows is touched.
                rooms: self.data.rooms.keys().cloned().collect(),
                private: self.data.private.keys().cloned().collect(),
                bots: self
                    .data
                    .bots
                    .iter()
                    .filter(|(id, bot)| {
                        **id == deleted_id || bot.owner_id.as_ref() == Some(&deleted_id)
                    })
                    .map(|(id, _)| id.clone())
                    .collect(),
            },
        );
        self.audit(actor, "/rm user", name);
        self.data.policy.media_revocations.remove(&deleted_id);
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
        self.data.bots.remove(&deleted_id);
        for bot in self.data.bots.values_mut() {
            if bot.owner_id.as_ref() == Some(&deleted_id) {
                bot.owner_id = None;
            }
        }
        for room in self.data.rooms.values_mut() {
            if room.owner_id == deleted_id {
                room.owner_id = "console".into();
            }
        }
        self.data
            .sessions
            .retain(|_, session| session.username != name);
        for room in self.data.rooms.values_mut() {
            room.revision = room.revision.wrapping_add(1);
            room.members.remove(name);
        }
        for chat in self.data.private.values_mut() {
            chat.revision = chat.revision.wrapping_add(1);
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
        Ok(AccountDeletion {
            before: previous,
            deleted_id,
            removed,
            reply: format!(
                "Deleted account {name} and its private conversations. Room messages retained as {name} (deleted)."
            ),
        })
    }
    pub(crate) fn prepare_account_deletion(
        &mut self,
        actor: Option<&str>,
        name: &str,
    ) -> Result<(String, metadata_write::PendingMetadata), EngineError> {
        let dirty = self.metadata_dirty;
        let AccountDeletion {
            before,
            deleted_id,
            removed,
            reply,
        } = self.apply_account_deletion(actor, name)?;
        let pending = self
            .capture_metadata(&before, vec![])
            .map(|pending| pending.account_deletion(name.to_owned(), deleted_id, removed));
        before.restore(&mut self.data);
        self.metadata_dirty = dirty;
        pending.map(|pending| (reply, pending))
    }
    pub(super) fn apply_accounts(
        &mut self,
        context: &CommandContext<'_>,
    ) -> Result<String, EngineError> {
        let CommandContext { actor, parts, .. } = *context;
        match parts[0] {
            "/enable" => {
                require_len(parts, 2, "/user enable who")?;
                let scope = self.account_target(actor, parts[1], authorization::Action::Enable)?;
                self.require_target_command(actor, &scope, context.command)?;
                self.audit(actor, "/enable", parts[1]);
                self.data
                    .users
                    .get_mut(parts[1])
                    .ok_or_else(|| EngineError::NotFound("User not found.".into()))?
                    .disabled = false;
                Ok(format!(
                    "Enabled {}. Previous room access and ownership are not restored.",
                    parts[1]
                ))
            }
            "/disable" => {
                require_len(parts, 2, "/user disable who")?;
                let scope = self.account_target(actor, parts[1], authorization::Action::Disable)?;
                self.require_target_command(actor, &scope, context.command)?;
                if !self.is_su(actor)
                    && self.is_admin(parts[1])
                    && self
                        .data
                        .users
                        .keys()
                        .filter(|n| self.active(n) && self.is_admin(n))
                        .count()
                        == 1
                {
                    return Err(EngineError::Forbidden(
                        "Cannot disable the last active admin.".into(),
                    ));
                }
                self.audit(actor, "/disable", parts[1]);
                self.data
                    .users
                    .get_mut(parts[1])
                    .ok_or_else(|| EngineError::NotFound("User not found.".into()))?
                    .disabled = true;
                self.data.sessions.retain(|_, s| s.username != parts[1]);
                let id = self
                    .data
                    .users
                    .get(parts[1])
                    .ok_or_else(|| EngineError::NotFound("User not found.".into()))?
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
                Ok(format!(
                    "Disabled {}. Sessions revoked; room memberships and room-scoped roles/grants removed; owned rooms transferred to su. Enabling does not restore room access or ownership.",
                    parts[1]
                ))
            }
            _ => Err("Unknown command.".into()),
        }
    }
}
