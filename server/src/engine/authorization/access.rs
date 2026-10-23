//! Principal resolution, effective access, command gates and delegation.
use super::*;

impl Engine {
    /// Elevate only during a synchronous command phase. Never carry authority
    /// across asynchronous work; callers must recheck at the commit phase.
    pub fn with_command_authority<T>(
        &mut self,
        actor: Option<&str>,
        view: Option<&str>,
        sudo: bool,
        operation: impl FnOnce(&mut Self) -> Result<T, EngineError>,
    ) -> Result<T, EngineError> {
        if !sudo {
            return operation(self);
        }
        self.require_command(actor, view, "/sudo")?;
        self.require_target_command(actor, &Scope::Server, "/sudo")?;
        self.require(actor, &Scope::Server, Action::Sudo)?;
        let id = self
            .principal_id(actor)
            .ok_or_else(|| EngineError::Forbidden("Account unavailable.".into()))?
            .to_owned();
        self.read_model.suspend();
        self.authorization
            .groups
            .entry(id.clone())
            .or_default()
            .insert(Group::Su);
        self.authorization
            .scopes
            .entry(id)
            .or_default()
            .entry(Scope::Server)
            .or_default()
            .add(rights(Action::ALL, COMMANDS));
        let result = operation(self);
        self.read_model.resume();
        // Rebuild from the actual persisted assignments, including policy
        // changes performed by the command, on success and on rejection.
        self.rebuild_authorization()?;
        result
    }

    pub(in crate::engine) fn rebuild_authorization(&mut self) -> Result<(), EngineError> {
        match CompiledPolicy::compile(&self.data) {
            Ok(policy) => {
                self.authorization = policy;
                Ok(())
            }
            Err(error) => {
                self.authorization = CompiledPolicy::default();
                self.read_model.advance();
                Err(EngineError::unavailable(error))
            }
        }
    }
    pub(in crate::engine) fn principal_id<'a>(&'a self, actor: Option<&str>) -> Option<&'a str> {
        match actor {
            None => Some("console"),
            Some(name) => self
                .data
                .users
                .get(name)
                .filter(|u| !u.disabled)
                .map(|u| u.id.as_str()),
        }
    }
    pub fn groups(&self, actor: Option<&str>) -> Vec<String> {
        self.principal_id(actor)
            .and_then(|id| self.authorization.groups.get(id))
            .map(|groups| groups.iter().map(|g| g.name().to_owned()).collect())
            .unwrap_or_default()
    }
    pub fn permission_label(&self, actor: Option<&str>) -> String {
        let groups = self.groups(actor);
        if groups.iter().any(|g| g == "su") {
            "su".into()
        } else if groups.iter().any(|g| g == "admin") {
            "admin".into()
        } else {
            "user".into()
        }
    }
    pub fn has_group(&self, actor: Option<&str>, group: Group) -> bool {
        self.principal_id(actor)
            .and_then(|id| self.authorization.groups.get(id))
            .is_some_and(|groups| groups.contains(&group))
    }
    pub fn is_su(&self, actor: Option<&str>) -> bool {
        self.has_group(actor, Group::Su)
    }
    pub(in crate::engine) fn room_scope(&self, name: &str) -> Result<Scope, EngineError> {
        self.data
            .rooms
            .get(name)
            .map(|r| Scope::Room(r.id.clone()))
            .ok_or_else(|| EngineError::NotFound("Room not found.".into()))
    }
    pub(in crate::engine) fn private_scope(&self, key: &str) -> Scope {
        if let Some(chat) = self.data.private.get(key) {
            return Scope::Private(chat.id.clone());
        }
        let id = key
            .split_once(':')
            .and_then(|(a, b)| self.data.users.get(a).zip(self.data.users.get(b)))
            .map_or_else(
                || "unavailable".into(),
                |(a, b)| format!("{}:{}", a.id, b.id),
            );
        Scope::Private(id)
    }
    pub fn effective(&self, actor: Option<&str>, scope: &Scope) -> Rights {
        let Some(id) = self.principal_id(actor) else {
            return Rights::default();
        };
        self.authorization.effective_for(id, scope)
    }
    pub fn allows(&self, actor: Option<&str>, scope: &Scope, action: Action) -> bool {
        self.effective(actor, scope).has(action)
    }
    /// Global policy inspection does not disclose another resource's grants.
    pub(in crate::engine) fn can_inspect_policy(&self, actor: Option<&str>, scope: &Scope) -> bool {
        self.is_su(actor)
            || self
                .principal_id(actor)
                .and_then(|id| self.authorization.scopes.get(id))
                .and_then(|scopes| scopes.get(scope))
                .is_some_and(|rights| rights.has(Action::PolicyRead))
    }
    pub(in crate::engine) fn policy_scope(
        &self,
        actor: Option<&str>,
        value: &str,
        write: bool,
    ) -> Result<Scope, EngineError> {
        if !value.starts_with('#') && !value.starts_with('@') {
            return Err("Room scopes require #name.".into());
        }
        let denied = "Scope not found or access denied.";
        let scope = self.parse_scope(value).map_err(|error| {
            if self.is_su(actor) {
                error
            } else {
                EngineError::Forbidden(denied.into())
            }
        })?;
        if !self.is_su(actor) {
            let allowed = if write {
                matches!(scope, Scope::Room(_)) && self.allows(actor, &scope, Action::PolicyWrite)
            } else {
                scope == Scope::Server
                    || self.can_inspect_policy(actor, &scope)
                    || self.allows(actor, &scope, Action::Read)
            };
            if !allowed {
                return Err(EngineError::Forbidden(denied.into()));
            }
        }
        Ok(scope)
    }
    pub fn require(
        &self,
        actor: Option<&str>,
        scope: &Scope,
        action: Action,
    ) -> Result<(), EngineError> {
        if self.allows(actor, scope, action) {
            Ok(())
        } else {
            Err(EngineError::Forbidden(format!(
                "Permission required: {} at {}. Check /permissions {} for effective access.",
                action.name(),
                self.scope_label(scope),
                match scope {
                    Scope::Room(_) => format!("#{}", self.scope_label(scope)),
                    _ => self.scope_label(scope),
                }
            )))
        }
    }
    pub fn require_clean(
        &self,
        actor: Option<&str>,
        scope: &Scope,
        age: u64,
    ) -> Result<(), EngineError> {
        if self.is_su(actor) {
            return Ok(());
        }
        self.require(actor, scope, Action::Clean)?;
        let user = actor.ok_or_else(|| EngineError::Forbidden("Account unavailable.".into()))?;
        let (limit, _) = self.cleanup_limit(user, scope)?;
        if limit == -1
            || (limit > 0 && age >= u64::try_from(limit).map_err(|_| "Invalid cleanup limit.")?)
        {
            Ok(())
        } else if limit == 0 {
            Err(EngineError::Forbidden(
                "Cleanup disabled by history.clean_min_age metadata.".into(),
            ))
        } else {
            Err(EngineError::Forbidden(format!(
                "history.clean_min_age requires messages at least {limit} seconds old."
            )))
        }
    }

    pub(in crate::engine) fn command_scope(
        &self,
        actor: Option<&str>,
        view: Option<&str>,
    ) -> Result<Scope, EngineError> {
        match view {
            None | Some("@command" | "@direct") => Ok(Scope::Server),
            Some(view) if view.starts_with("@direct:") => {
                let user = actor.ok_or("Private views require an account.")?;
                Ok(self.private_scope(&private_key(user, &view[8..])))
            }
            Some(view) if view.starts_with("@private:") => self.parse_scope(view),
            Some(name) => self.room_scope(name),
        }
    }
    pub fn require_command(
        &self,
        actor: Option<&str>,
        room: Option<&str>,
        command: &str,
    ) -> Result<(), EngineError> {
        self.principal_id(actor)
            .ok_or_else(|| EngineError::Forbidden("Account unavailable.".into()))?;
        let scope = self.command_scope(actor, room)?;
        let allowed = self.effective(actor, &scope).command(command)
            || (room.is_none() && self.any_command(actor, command));
        if allowed {
            Ok(())
        } else {
            Err(EngineError::Forbidden(format!(
                "Command not permitted in this context: {command}. Missing command grant at {}. Action permissions alone do not enable commands. Check /permissions or /man {}.",
                self.scope_label(&scope),
                command.trim_start_matches('/')
            )))
        }
    }
    pub fn require_target_command(
        &self,
        actor: Option<&str>,
        scope: &Scope,
        command: &str,
    ) -> Result<(), EngineError> {
        if self.effective(actor, scope).command(command) {
            Ok(())
        } else {
            Err(EngineError::Forbidden(format!(
                "Command not permitted on this target: {command}. Missing command grant at {}. Check /permissions {} or /man {}.",
                self.scope_label(scope),
                match scope {
                    Scope::Room(_) => format!("#{}", self.scope_label(scope)),
                    _ => self.scope_label(scope),
                },
                command.trim_start_matches('/')
            )))
        }
    }
    pub(in crate::engine) fn any_command(&self, actor: Option<&str>, command: &str) -> bool {
        let Some(id) = self.principal_id(actor) else {
            return false;
        };
        self.authorization
            .scopes
            .get(id)
            .is_some_and(|scopes| scopes.values().any(|r| r.command(command)))
            || (actor.is_some_and(|name| {
                self.authorization
                    .groups
                    .get(id)
                    .is_some_and(|groups| !groups.is_empty())
                    && self
                        .data
                        .private
                        .keys()
                        .any(|key| private_peer(key, name).is_some())
            }) && member_rights().command(command))
    }
    pub fn commands_for(
        &self,
        actor: Option<&str>,
        room: Option<&str>,
    ) -> Vec<crate::commands::Command> {
        let Ok(scope) = self.command_scope(actor, room) else {
            return Vec::new();
        };
        self.commands_for_rights(actor, room, self.effective(actor, &scope))
    }
    pub(in crate::engine) fn commands_for_rights(
        &self,
        actor: Option<&str>,
        room: Option<&str>,
        mut allowed: Rights,
    ) -> Vec<crate::commands::Command> {
        let Some(id) = self.principal_id(actor) else {
            return Vec::new();
        };

        if room.is_none() {
            if let Some(scopes) = self.authorization.scopes.get(id) {
                for rights in scopes.values() {
                    allowed.add(*rights);
                }
            }
            if actor.is_some_and(|name| {
                self.authorization
                    .groups
                    .get(id)
                    .is_some_and(|groups| !groups.is_empty())
                    && self
                        .data
                        .private
                        .keys()
                        .any(|key| private_peer(key, name).is_some())
            }) {
                allowed.add(member_rights());
            }
        }
        crate::commands::available(true, actor.is_none())
            .iter()
            .filter(|command| allowed.command(command.name))
            .cloned()
            .collect()
    }
    pub(in crate::engine) fn assign(&mut self, account_id: String, group: Group, scope: Scope) {
        if !self
            .data
            .policy
            .assignments
            .iter()
            .any(|a| a.account_id == account_id && a.group == group && a.scope == scope)
        {
            self.metadata_dirty = true;
            self.data.policy.assignments.push(Assignment {
                account_id,
                group,
                scope,
            });
        }
    }
    pub(in crate::engine) fn require_member_delegation(
        &self,
        actor: Option<&str>,
        scope: &Scope,
    ) -> Result<(), EngineError> {
        let own = self.effective(actor, scope);
        let member = member_rights();
        if member.actions & !own.actions != 0 || member.commands & !own.commands != 0 {
            return Err(EngineError::Forbidden(format!(
                "Inviting or assigning the user group requires authority to delegate every participant permission in {}. Ask the room owner to add you with /room add first; /room add also requires x:member.add and the /room command grant.",
                match scope {
                    Scope::Room(_) => format!("#{}", self.scope_label(scope)),
                    _ => self.scope_label(scope),
                }
            )));
        }
        Ok(())
    }
    pub(in crate::engine) fn add_owner_grants(&mut self, id: String, scope: Scope) {
        let rights = manager_rights();
        let mut permissions: BTreeSet<_> = rights.names().into_iter().collect();
        permissions.extend(
            COMMANDS
                .iter()
                .filter(|c| rights.command(c))
                .map(|c| (*c).to_owned()),
        );
        self.data.policy.grants.push(Grant {
            owner: true,
            subject: Subject::Account(id),
            scope,
            permissions,
        });
    }
    pub(in crate::engine) fn audit(&mut self, actor: Option<&str>, operation: &str, target: &str) {
        self.metadata_dirty = true;
        let actor_id = self.principal_id(actor).unwrap_or("unavailable").to_owned();
        if self.data.policy.audit.len() >= 1000 {
            self.data.policy.audit.pop_front();
        }
        self.data.policy.audit.push_back(Audit {
            id: uuid::Uuid::new_v4().to_string(),
            time: now(),
            actor_id,
            operation: operation.into(),
            target: target.into(),
        });
    }
    pub fn account_target(
        &self,
        actor: Option<&str>,
        name: &str,
        action: Action,
    ) -> Result<Scope, EngineError> {
        let account = self
            .data
            .users
            .get(name)
            .ok_or_else(|| EngineError::NotFound("User not found.".into()))?;
        let scope = Scope::Account(account.id.clone());
        self.require(actor, &scope, action)?;
        // Protection follows the target's assignment even while disabled;
        // actor authority still requires an active principal through is_su.
        let protected = self
            .authorization
            .groups
            .get(&account.id)
            .is_some_and(|groups| groups.contains(&Group::Su));
        if protected && !self.is_su(actor) {
            return Err(EngineError::Forbidden(
                "Only su can manage a su account.".into(),
            ));
        }
        Ok(scope)
    }
    pub(in crate::engine) fn scope_label(&self, scope: &Scope) -> String {
        match scope {
            Scope::Server => "@global".into(),
            Scope::Room(id) => self
                .data
                .rooms
                .iter()
                .find(|(_, room)| room.id == *id)
                .map_or_else(|| "room".into(), |(name, _)| name.clone()),
            Scope::Account(id) => self
                .data
                .users
                .iter()
                .find(|(_, user)| user.id == *id)
                .map_or_else(|| "account".into(), |(name, _)| format!("@account:{name}")),
            Scope::Private(id) => self
                .data
                .private
                .iter()
                .find(|(_, chat)| chat.id == *id)
                .map_or_else(
                    || "private conversation".into(),
                    |(key, _)| format!("@private:{key}"),
                ),
        }
    }
    pub(in crate::engine) fn parse_scope(&self, value: &str) -> Result<Scope, EngineError> {
        if matches!(value, "@global" | "@server") {
            return Ok(Scope::Server);
        }
        if let Some(name) = value.strip_prefix("@account:") {
            return self
                .data
                .users
                .get(name)
                .map(|u| Scope::Account(u.id.clone()))
                .ok_or_else(|| EngineError::NotFound("User not found.".into()));
        }
        if let Some(key) = value.strip_prefix("@private:") {
            if self.data.private.contains_key(key) {
                return Ok(self.private_scope(key));
            }
            return Err(EngineError::NotFound(
                "Private conversation not found.".into(),
            ));
        }
        if value.starts_with('@') {
            return Err(EngineError::Invalid(format!(
                "Unknown scope: {value}. Use a room name, @global, @account:user, or @private:a:b."
            )));
        }
        self.room_scope(value.strip_prefix('#').unwrap_or(value))
    }
}
