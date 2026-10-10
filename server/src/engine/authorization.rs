//! Scoped, allow-only grants. Persist names; compile masks outside message state.
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

macro_rules! actions {
    ($($variant:ident => $name:literal),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Action { $($variant),+ }
        impl Action {
            pub const ALL: &[Self] = &[$(Self::$variant),+];
            pub fn name(self) -> &'static str { match self { $(Self::$variant => $name),+ } }
            fn bit(self) -> u64 { 1u64 << (self as u32) }
            fn parse(name: &str) -> Option<Self> { Self::ALL.iter().copied().find(|a| a.name() == name) }
        }
    }
}
actions! {
    Discover => "r:room.discover",
    Read => "r:message.read",
    Metadata => "r:message.metadata",
    Members => "r:member.list",
    Directory => "r:account.list",
    Config => "r:server.config",
    PolicyRead => "r:policy.read",
    Send => "w:message.create",
    React => "w:message.react",
    RetractOwn => "w:message.retract.own",
    RetractAny => "w:message.retract.any",
    CreateRoom => "w:room.create",
    Direct => "w:private.create",
    Password => "w:account.password.own",
    Invite => "x:member.add",
    Kick => "x:member.remove",
    Join => "x:room.join",
    DeleteRoom => "x:room.delete",
    Transfer => "x:room.owner.transfer",
    Clean => "x:history.clean",
    CreateAccount => "x:account.create",
    Reset => "x:account.password.reset",
    Disable => "x:account.disable",
    Enable => "x:account.enable",
    DeleteAccount => "x:account.delete",
    AssignAdmin => "x:group.admin.assign",
    AssignSu => "x:group.su.assign",
    PolicyWrite => "x:policy.change",
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Group {
    User,
    Admin,
    Su,
}
impl Group {
    pub fn name(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Admin => "admin",
            Self::Su => "su",
        }
    }
    pub fn parse(name: &str) -> Result<Self, String> {
        match name {
            "user" => Ok(Self::User),
            "admin" => Ok(Self::Admin),
            "su" => Ok(Self::Su),
            _ => Err("Group must be user, admin or su.".into()),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum Scope {
    Server,
    Room(String),
    Account(String),
    Private(String),
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum Subject {
    Account(String),
    Group(Group),
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Assignment {
    pub account_id: String,
    pub group: Group,
    pub scope: Scope,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Grant {
    #[serde(default)]
    pub owner: bool,
    pub subject: Subject,
    pub scope: Scope,
    pub permissions: BTreeSet<String>,
    #[serde(default)]
    pub minimum_age: u64,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Audit {
    pub time: u64,
    pub actor_id: String,
    pub operation: String,
    pub target: String,
}
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Policy {
    pub revision: u64,
    pub assignments: Vec<Assignment>,
    pub grants: Vec<Grant>,
    pub audit: VecDeque<Audit>,
}
#[derive(Clone, Copy, Default)]
pub struct Rights {
    actions: u64,
    commands: u64,
}
impl Rights {
    fn add(&mut self, other: Self) {
        self.actions |= other.actions;
        self.commands |= other.commands;
    }
    pub fn has(self, action: Action) -> bool {
        self.actions & action.bit() != 0
    }
    fn command(self, command: &str) -> bool {
        command_bit(command).is_some_and(|b| self.commands & b != 0)
    }
    fn insert(&mut self, name: &str) -> Result<(), String> {
        if let Some(action) = Action::parse(name) {
            self.actions |= action.bit();
        } else if let Some(bit) = command_bit(name) {
            self.commands |= bit;
        } else {
            return Err(format!("Unknown permission: {name}"));
        }
        Ok(())
    }
    pub fn names(self) -> Vec<String> {
        Action::ALL
            .iter()
            .filter(|a| self.has(**a))
            .map(|a| a.name().to_owned())
            .collect()
    }
}
const COMMANDS: &[&str] = &[
    "/help",
    "/whoami",
    "/passwd",
    "/users",
    "/rooms",
    "/members",
    "/history",
    "/join",
    "/leave",
    "/tell",
    "/react",
    "/retract",
    "/reply",
    "/new",
    "/add",
    "/kick",
    "/delete",
    "/grant",
    "/revoke",
    "/configs",
    "/clean",
    "/clear",
    "/logout",
    "/user",
    "/reset",
    "/disable",
    "/enable",
    "/deleteuser",
    "/debug",
    "/permissions",
    "/man",
    "/owner",
    "/console",
];
fn direct_grant(parts: &[&str]) -> bool {
    parts.len() >= 4
        && (parts
            .get(2)
            .is_some_and(|value| Group::parse(value).is_err())
            || parts.get(3).is_some_and(|value| {
                value.starts_with('/')
                    || ["r:", "w:", "x:"]
                        .iter()
                        .any(|prefix| value.starts_with(prefix))
            }))
}

pub(super) fn migrate_command_grants(data: &mut Data) {
    let mut changed = false;
    for grant in &mut data.policy.grants {
        for (old, new) in [("/permit", "/grant"), ("/unpermit", "/revoke")] {
            if grant.permissions.remove(old) {
                grant.permissions.insert(new.into());
                changed = true;
            }
        }
    }
    if changed {
        data.policy.revision = data.policy.revision.wrapping_add(1);
    }
}

fn command_bit(name: &str) -> Option<u64> {
    COMMANDS
        .iter()
        .position(|c| *c == name)
        .and_then(|n| u32::try_from(n).ok())
        .map(|n| 1u64 << n)
}
fn rights(actions: &[Action], commands: &[&str]) -> Rights {
    let mut result = Rights::default();
    for a in actions {
        result.actions |= a.bit();
    }
    for c in commands {
        if let Some(bit) = command_bit(c) {
            result.commands |= bit;
        }
    }
    result
}
fn member_rights() -> Rights {
    rights(
        &[
            Action::Discover,
            Action::Join,
            Action::Read,
            Action::Members,
            Action::Send,
            Action::React,
            Action::RetractOwn,
        ],
        &["/members", "/history", "/react", "/retract", "/reply"],
    )
}
fn manager_rights() -> Rights {
    rights(
        &[
            Action::Discover,
            Action::Metadata,
            Action::PolicyRead,
            Action::PolicyWrite,
            Action::Invite,
            Action::Kick,
            Action::Join,
            Action::DeleteRoom,
            Action::Clean,
            Action::Transfer,
        ],
        &[
            "/add",
            "/kick",
            "/delete",
            "/clean",
            "/grant",
            "/revoke",
            "/owner",
            "/permissions",
            "/debug",
        ],
    )
}
fn group_rights(group: Group, scope: &Scope) -> Rights {
    if group == Group::Su {
        return rights(Action::ALL, COMMANDS);
    }
    if matches!(scope, Scope::Room(_)) {
        let mut r = member_rights();
        if group == Group::Admin {
            r.add(manager_rights());
        }
        return r;
    }
    if !matches!(scope, Scope::Server) {
        return Rights::default();
    }
    let mut r = rights(
        &[Action::Directory, Action::Password, Action::Direct],
        &[
            "/help",
            "/man",
            "/whoami",
            "/passwd",
            "/users",
            "/rooms",
            "/join",
            "/leave",
            "/tell",
            "/clear",
            "/logout",
            "/permissions",
            "/console",
        ],
    );
    if group == Group::Admin {
        r.add(rights(
            &[
                Action::CreateRoom,
                Action::CreateAccount,
                Action::Disable,
                Action::Enable,
                Action::DeleteAccount,
                Action::AssignAdmin,
                Action::Config,
                Action::PolicyRead,
            ],
            &[
                "/new",
                "/user",
                "/disable",
                "/enable",
                "/deleteuser",
                "/grant",
                "/revoke",
                "/configs",
                "/debug",
            ],
        ));
    }
    r
}
fn describe_rights(rights: Rights) -> String {
    let mut sections = Vec::new();
    for (prefix, title) in [
        ("r:", "Read actions"),
        ("w:", "Write actions"),
        ("x:", "Management actions"),
    ] {
        let items = rights
            .names()
            .into_iter()
            .filter(|name| name.starts_with(prefix))
            .map(|name| format!("- `{name}`"))
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!(
            "### {title}\n\n{}",
            if items.is_empty() { "None." } else { &items }
        ));
    }
    let commands = crate::commands::available(true, false)
        .iter()
        .filter(|command| rights.command(command.name))
        .map(|command| {
            if command.requirements.is_empty() {
                format!("- `{}` — command grant only.", command.name)
            } else {
                format!("- `{}` — Requires: {}.", command.name, command.requirements)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    sections.push(format!("### Command grants\n\n{}\n\nA command grant does not grant its action permissions. Requirements are checked on each target.", if commands.is_empty() { "None." } else { &commands }));
    sections.join("\n\n")
}
#[derive(Default)]
pub(super) struct CompiledPolicy {
    pub(super) revision: u64,
    scopes: HashMap<String, HashMap<Scope, Rights>>,
    groups: HashMap<String, BTreeSet<Group>>,
    conditional: HashMap<String, HashMap<Scope, Vec<(Rights, u64)>>>,
}
impl CompiledPolicy {
    pub(super) fn compile(data: &Data) -> Result<Self, String> {
        let mut result = Self {
            revision: data.policy.revision,
            ..Self::default()
        };
        for a in &data.policy.assignments {
            if a.group == Group::Su && a.scope != Scope::Server {
                return Err("su assignments must have global scope.".into());
            }
            result
                .scopes
                .entry(a.account_id.clone())
                .or_default()
                .entry(a.scope.clone())
                .or_default()
                .add(group_rights(a.group, &a.scope));
            if a.scope == Scope::Server {
                result
                    .groups
                    .entry(a.account_id.clone())
                    .or_default()
                    .insert(a.group);
            }
        }
        for grant in &data.policy.grants {
            let mut mask = Rights::default();
            for p in &grant.permissions {
                mask.insert(p)?;
            }
            let ids: Vec<_> = match &grant.subject {
                Subject::Account(id) => vec![id.clone()],
                Subject::Group(group) => result
                    .groups
                    .iter()
                    .filter(|(_, groups)| groups.contains(group))
                    .map(|(id, _)| id.clone())
                    .collect(),
            };
            for id in ids {
                if grant.minimum_age == 0 {
                    result
                        .scopes
                        .entry(id)
                        .or_default()
                        .entry(grant.scope.clone())
                        .or_default()
                        .add(mask);
                } else {
                    result
                        .conditional
                        .entry(id)
                        .or_default()
                        .entry(grant.scope.clone())
                        .or_default()
                        .push((mask, grant.minimum_age));
                }
            }
        }
        Ok(result)
    }
}
impl Engine {
    pub(super) fn rebuild_authorization(&mut self) -> Result<(), String> {
        self.authorization = CompiledPolicy::compile(&self.data)?;
        Ok(())
    }
    fn principal_id<'a>(&'a self, actor: Option<&str>) -> Option<&'a str> {
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
    pub(super) fn room_scope(&self, name: &str) -> Result<Scope, String> {
        self.data
            .rooms
            .get(name)
            .map(|r| Scope::Room(r.id.clone()))
            .ok_or("Room not found.".into())
    }
    pub(super) fn private_scope(&self, key: &str) -> Scope {
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
        let mut r = self
            .authorization
            .scopes
            .get(id)
            .and_then(|scopes| scopes.get(&Scope::Server))
            .copied()
            .unwrap_or_default();
        if scope != &Scope::Server {
            r.add(
                self.authorization
                    .scopes
                    .get(id)
                    .and_then(|scopes| scopes.get(scope))
                    .copied()
                    .unwrap_or_default(),
            );
        }
        if let Scope::Private(key) = scope
            && key.split_once(':').is_some_and(|(a, b)| a == id || b == id)
            && self
                .authorization
                .groups
                .get(id)
                .is_some_and(|groups| !groups.is_empty())
        {
            r.add(member_rights());
            if self.has_group(actor, Group::Admin) {
                r.add(rights(&[Action::Metadata], &["/debug"]));
            }
        }

        r
    }
    pub fn allows(&self, actor: Option<&str>, scope: &Scope, action: Action) -> bool {
        self.effective(actor, scope).has(action)
    }
    pub fn require(
        &self,
        actor: Option<&str>,
        scope: &Scope,
        action: Action,
    ) -> Result<(), String> {
        if self.allows(actor, scope, action) {
            Ok(())
        } else {
            Err(format!(
                "Permission required: {} at {}. Check /permissions {} for effective access.",
                action.name(),
                self.scope_label(scope),
                self.scope_label(scope)
            ))
        }
    }
    pub fn require_clean(
        &self,
        actor: Option<&str>,
        scope: &Scope,
        age: u64,
    ) -> Result<(), String> {
        if self.allows(actor, scope, Action::Clean) {
            return Ok(());
        }
        let id = self.principal_id(actor).ok_or("Account unavailable.")?;
        let allowed = [Scope::Server, scope.clone()].iter().any(|scope| {
            self.authorization
                .conditional
                .get(id)
                .and_then(|scopes| scopes.get(scope))
                .is_some_and(|rules| {
                    rules
                        .iter()
                        .any(|(r, minimum)| age >= *minimum && r.has(Action::Clean))
                })
        });
        if allowed {
            Ok(())
        } else {
            Err("Permission required: x:history.clean (age constraint may apply).".into())
        }
    }
    pub(super) fn command_scope(
        &self,
        actor: Option<&str>,
        view: Option<&str>,
    ) -> Result<Scope, String> {
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
    ) -> Result<(), String> {
        self.principal_id(actor).ok_or("Account unavailable.")?;
        let scope = self.command_scope(actor, room)?;
        let allowed = self.effective(actor, &scope).command(command)
            || (room.is_none() && self.any_command(actor, command));
        if allowed {
            Ok(())
        } else {
            Err(format!(
                "Command not permitted in this context: {command}. Missing command grant at {}. Action permissions alone do not enable commands. Check /permissions or /man {}.",
                self.scope_label(&scope),
                command.trim_start_matches('/')
            ))
        }
    }
    pub fn require_target_command(
        &self,
        actor: Option<&str>,
        scope: &Scope,
        command: &str,
    ) -> Result<(), String> {
        if self.effective(actor, scope).command(command) {
            Ok(())
        } else {
            Err(format!(
                "Command not permitted on this target: {command}. Missing command grant at {}. Check /permissions {} or /man {}.",
                self.scope_label(scope),
                self.scope_label(scope),
                command.trim_start_matches('/')
            ))
        }
    }
    fn any_command(&self, actor: Option<&str>, command: &str) -> bool {
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
        crate::commands::available(true, actor.is_none())
            .into_iter()
            .filter(|c| self.require_command(actor, room, c.name).is_ok())
            .collect()
    }
    pub(super) fn assign(&mut self, account_id: String, group: Group, scope: Scope) {
        if !self
            .data
            .policy
            .assignments
            .iter()
            .any(|a| a.account_id == account_id && a.group == group && a.scope == scope)
        {
            self.data.policy.assignments.push(Assignment {
                account_id,
                group,
                scope,
            });
        }
    }
    pub(super) fn require_member_delegation(
        &self,
        actor: Option<&str>,
        scope: &Scope,
    ) -> Result<(), String> {
        let own = self.effective(actor, scope);
        let member = member_rights();
        if member.actions & !own.actions != 0 || member.commands & !own.commands != 0 {
            return Err(format!(
                "Inviting or assigning the user group requires authority to delegate every participant permission in {}. Ask the room owner to add you with /add first; /add also requires x:member.add and the /add command grant.",
                self.scope_label(scope)
            ));
        }
        Ok(())
    }
    pub(super) fn add_owner_grants(&mut self, id: String, scope: Scope) {
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
            minimum_age: 0,
        });
    }
    pub(super) fn audit(&mut self, actor: Option<&str>, operation: &str, target: &str) {
        let actor_id = self.principal_id(actor).unwrap_or("unavailable").to_owned();
        if self.data.policy.audit.len() >= 1000 {
            self.data.policy.audit.pop_front();
        }
        self.data.policy.audit.push_back(Audit {
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
    ) -> Result<Scope, String> {
        let account = self.data.users.get(name).ok_or("User not found.")?;
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
            return Err("Only su can manage a su account.".into());
        }
        Ok(scope)
    }
    pub(super) fn scope_label(&self, scope: &Scope) -> String {
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
    pub(super) fn parse_scope(&self, value: &str) -> Result<Scope, String> {
        if matches!(value, "@global" | "@server") {
            return Ok(Scope::Server);
        }
        if let Some(name) = value.strip_prefix("@account:") {
            return self
                .data
                .users
                .get(name)
                .map(|u| Scope::Account(u.id.clone()))
                .ok_or("User not found.".into());
        }
        if let Some(key) = value.strip_prefix("@private:") {
            if self.data.private.contains_key(key) {
                return Ok(self.private_scope(key));
            }
            return Err("Private conversation not found.".into());
        }
        if value.starts_with('@') {
            return Err(format!(
                "Unknown scope: {value}. Use a room name, @global, @account:user, or @private:a:b."
            ));
        }
        self.room_scope(value)
    }
    pub(super) fn apply_policy(&mut self, context: &CommandContext<'_>) -> Result<String, String> {
        let actor = context.actor;
        let parts = context.parts;
        match parts[0] {
            "/permissions" => {
                if parts.len() > 2 {
                    return Err(
                        "Usage: /permissions [room|@global|@account:user|@private:a:b|@groups|@audit]".into(),
                    );
                }
                if parts.get(1) == Some(&"@groups") {
                    let mut text = String::from(
                        "# Predefined groups\n\nGroups are bundles of grants. The scope determines where each bundle applies.",
                    );
                    for group in [Group::User, Group::Admin, Group::Su] {
                        text.push_str(&format!(
                            "\n\n# {}\n\n## Global bundle\n\n{}",
                            group.name(),
                            describe_rights(group_rights(group, &Scope::Server))
                        ));
                        if group != Group::Su {
                            text.push_str(&format!(
                                "\n\n## Room bundle\n\n{}",
                                describe_rights(group_rights(group, &Scope::Room(String::new())))
                            ));
                        } else {
                            text.push_str("\n\nsu is global and has every action and command on every resource.");
                        }
                    }
                    return Ok(text);
                }
                if parts.get(1) == Some(&"@audit") {
                    if !self.is_su(actor) {
                        return Err("Only su can inspect the global audit log.".into());
                    }
                    let entries = self
                        .data
                        .policy
                        .audit
                        .iter()
                        .map(|event| {
                            format!(
                                "- `{}` · actor `{}` · `{}` · {}",
                                event.time, event.actor_id, event.operation, event.target
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    return Ok(format!(
                        "# Access audit\n\n{}",
                        if entries.is_empty() {
                            "No recorded changes."
                        } else {
                            &entries
                        }
                    ));
                }
                let scope = parts
                    .get(1)
                    .map(|value| self.parse_scope(value))
                    .transpose()?
                    .unwrap_or_else(|| context.scope.clone());
                self.require_target_command(actor, &scope, parts[0])?;
                if scope != Scope::Server {
                    self.require(actor, &scope, Action::PolicyRead)
                        .or_else(|_| self.require(actor, &scope, Action::Read))?;
                }
                let label = self.scope_label(&scope);
                let label = if matches!(scope, Scope::Room(_)) {
                    format!("#{label}")
                } else {
                    label
                };
                let groups = self.groups(actor);
                let groups = if groups.is_empty() {
                    "None".into()
                } else {
                    groups.join(", ")
                };
                let mut text = format!(
                    "# Access at {label}\n\nAccount: `{}`. Global groups: {groups}.\n\nEffective access includes applicable global grants, group bundles, and resource grants.\n\n## Effective access\n\n{}",
                    actor.unwrap_or("su (stdin)"),
                    describe_rights(self.effective(actor, &scope))
                );
                if let Some(scopes) = self
                    .principal_id(actor)
                    .and_then(|id| self.authorization.conditional.get(id))
                {
                    let mut ages = BTreeSet::new();
                    for candidate in [Scope::Server, scope.clone()] {
                        if let Some(grants) = scopes.get(&candidate) {
                            ages.extend(
                                grants
                                    .iter()
                                    .filter(|(rights, _)| rights.has(Action::Clean))
                                    .map(|(_, age)| *age),
                            );
                        }
                    }
                    if !ages.is_empty() {
                        text.push_str("\n\n### Conditional actions");
                        for age in ages {
                            text.push_str(&format!("\n\n- `x:history.clean` — messages must be at least {age} seconds old."));
                        }
                        text.push_str("\n\nThese limits apply to individual grants. Any unrestricted cleanup grant still allows cleanup without this limit.");
                    }
                }
                if !self.allows(actor, &scope, Action::PolicyRead) {
                    text.push_str("\n\n## Grant assignments\n\nViewing assignments requires `r:policy.read`. Only your effective access is shown.");
                    return Ok(text);
                }
                let subject_name = |id: &str| {
                    self.data
                        .users
                        .iter()
                        .find(|(_, user)| user.id == id)
                        .map_or_else(
                            || {
                                if id == "console" {
                                    "su (stdin)".into()
                                } else {
                                    id.to_owned()
                                }
                            },
                            |(name, _)| name.clone(),
                        )
                };
                let assignments = self
                    .data
                    .policy
                    .assignments
                    .iter()
                    .filter(|assignment| assignment.scope == scope)
                    .map(|assignment| {
                        format!(
                            "- `{}` → `{}`",
                            subject_name(&assignment.account_id),
                            assignment.group.name()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                text.push_str(&format!(
                    "\n\n## Group assignments at {label}\n\n{}",
                    if assignments.is_empty() {
                        "None."
                    } else {
                        &assignments
                    }
                ));
                let mut grants: BTreeMap<(String, u64, bool), BTreeSet<String>> = BTreeMap::new();
                for grant in self
                    .data
                    .policy
                    .grants
                    .iter()
                    .filter(|grant| grant.scope == scope)
                {
                    let subject = match &grant.subject {
                        Subject::Account(id) => subject_name(id),
                        Subject::Group(group) => format!("group:{}", group.name()),
                    };
                    grants
                        .entry((subject, grant.minimum_age, grant.owner))
                        .or_default()
                        .extend(grant.permissions.iter().cloned());
                }
                text.push_str(&format!("\n\n## Direct grants at {label}"));
                if grants.is_empty() {
                    text.push_str("\n\nNone.");
                }
                for ((subject, minimum_age, owner), permissions) in grants {
                    text.push_str(&format!(
                        "\n\n### {subject} — {}",
                        if owner {
                            "owner grants"
                        } else {
                            "direct grants"
                        }
                    ));
                    if owner {
                        text.push_str("\n\nGranted through room ownership.");
                    }
                    if minimum_age > 0 {
                        text.push_str(&format!("\n\nCleanup minimum age: {minimum_age} seconds."));
                    }
                    text.push_str(&format!(
                        "\n\n{}",
                        permissions
                            .iter()
                            .map(|permission| format!("- `{permission}`"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    ));
                }
                if scope != Scope::Server {
                    text.push_str("\n\nGlobal assignments also apply. Use `/permissions @global` to inspect them.");
                }
                Ok(text)
            }
            "/grant" | "/revoke" if direct_grant(parts) => {
                if !(4..=5).contains(&parts.len()) {
                    return Err("Usage: /grant user scope permission [minimum-age] or /revoke user scope permission".into());
                }
                let scope = self.parse_scope(parts[2])?;
                let label = self.scope_label(&scope);
                self.require_target_command(actor, &scope, parts[0])?;
                let target = self
                    .data
                    .users
                    .get(parts[1])
                    .filter(|u| !u.disabled)
                    .ok_or("User not found.")?
                    .id
                    .clone();
                self.require_target_command(actor, &scope, parts[0])?;
                self.require(actor, &scope, Action::PolicyWrite)?;
                let mut requested = Rights::default();
                requested.insert(parts[3])?;
                if !matches!(scope, Scope::Room(_)) && !self.is_su(actor) {
                    return Err("Only su can change @global, account or private grants.".into());
                }
                if !self.is_su(actor) {
                    let own = self.effective(actor, &scope);
                    let mut ceiling = member_rights();
                    let owner = match &scope {
                        Scope::Room(room_id) => self.principal_id(actor).is_some_and(|id| {
                            self.data
                                .rooms
                                .values()
                                .any(|room| room.id == *room_id && room.owner_id == id)
                        }),
                        _ => false,
                    };
                    if owner {
                        ceiling.add(rights(&[Action::Invite], &["/add"]));
                    }
                    let delegable = requested.actions & !ceiling.actions == 0
                        && requested.commands & !ceiling.commands == 0;
                    if !delegable
                        || requested.actions & !own.actions != 0
                        || requested.commands & !own.commands != 0
                    {
                        return Err(format!(
                            "Room managers may delegate only participant permissions they hold in {label}; the room owner may also delegate x:member.add and /add. {} is outside that authority or is not held here. Global permissions such as w:room.create must be granted by su at @global. Use /man grant.",
                            parts[3]
                        ));
                    }
                }
                let minimum_age = parts
                    .get(4)
                    .map(|age| parse_age(age))
                    .transpose()?
                    .unwrap_or(0);
                if minimum_age > 0 && parts[3] != Action::Clean.name() {
                    return Err("Minimum age applies only to x:history.clean.".into());
                }
                let subject = Subject::Account(target);
                if parts[0] == "/grant" {
                    if !self.data.policy.grants.iter().any(|g| {
                        g.subject == subject
                            && g.scope == scope
                            && g.minimum_age == minimum_age
                            && g.permissions.contains(parts[3])
                    }) {
                        self.data.policy.grants.push(Grant {
                            owner: false,
                            subject,
                            scope,
                            permissions: BTreeSet::from([parts[3].into()]),
                            minimum_age,
                        });
                    }
                } else {
                    if parts.len() != 4 {
                        return Err("Usage: /revoke user scope permission".into());
                    }
                    for grant in &mut self.data.policy.grants {
                        if grant.subject == subject && grant.scope == scope {
                            grant.permissions.remove(parts[3]);
                        }
                    }
                    self.data
                        .policy
                        .grants
                        .retain(|g| !g.permissions.is_empty());
                }
                self.audit(
                    actor,
                    parts[0],
                    &format!("{} {} {}", parts[1], parts[2], parts[3]),
                );
                let operation = if parts[0] == "/grant" {
                    "Added"
                } else {
                    "Removed"
                };
                let mut reply = format!(
                    "# Grant updated.\n\n{operation} `{}` for `{}` at `{label}`.\n\nOther direct and group grants still apply.",
                    parts[3], parts[1]
                );
                if parts[0] == "/grant" && parts[3].starts_with('/') {
                    let requirements = crate::commands::requirements(parts[3]);
                    reply.push_str("\n\n## Command requirements\n\nThe command grant does not add action permissions.");
                    if !requirements.is_empty() {
                        reply.push_str(&format!("\n\nRequires: {requirements}."));
                    }
                    reply.push_str(&format!(
                        "\n\nUse `/man {}` for syntax and scope rules.",
                        parts[3].trim_start_matches('/')
                    ));
                } else if parts[0] == "/grant" {
                    reply.push_str("\n\nAction grants do not enable slash commands. Grant the matching command separately; `/man permissions` explains the two checks.");
                }
                if parts[0] == "/grant"
                    && label != "@global"
                    && matches!(
                        parts[3],
                        "w:room.create"
                            | "x:account.create"
                            | "w:private.create"
                            | "r:server.config"
                            | "r:account.list"
                            | "/new"
                            | "/user"
                            | "/configs"
                            | "/users"
                    )
                {
                    reply.push_str(&format!("\n\n## Scope hint\n\n`{}` is used at `@global`. This resource-scoped grant does not authorize that global operation.", parts[3]));
                }
                Ok(reply)
            }
            "/grant" | "/revoke" => {
                if !(2..=4).contains(&parts.len()) {
                    return Err(format!(
                        "Invalid arguments. Use /man {} for group and permission syntax.",
                        parts[0].trim_start_matches('/')
                    ));
                }
                let group = Group::parse(parts.get(2).copied().unwrap_or("admin"))?;
                let scope = parts
                    .get(3)
                    .map(|room| self.room_scope(room))
                    .transpose()?
                    .unwrap_or(Scope::Server);
                self.require_target_command(actor, &scope, parts[0])?;
                let target = self
                    .data
                    .users
                    .get(parts[1])
                    .filter(|u| !u.disabled)
                    .ok_or("User not found.")?
                    .id
                    .clone();
                if group == Group::Su {
                    if scope != Scope::Server || !self.is_su(actor) {
                        return Err("Only su can assign or revoke the server-wide su group.".into());
                    }
                } else if scope == Scope::Server {
                    self.require(actor, &scope, Action::AssignAdmin)?;
                } else {
                    self.require_target_command(actor, &scope, parts[0])?;
                    self.require(actor, &scope, Action::PolicyWrite)?;
                    if group == Group::Admin && !self.is_su(actor) {
                        return Err("Only su can delegate room-management groups.".into());
                    }
                }
                if self.is_su(Some(parts[1])) && !self.is_su(actor) {
                    return Err("Only su can manage a su account.".into());
                }
                if parts[0] == "/revoke"
                    && group == Group::Admin
                    && scope == Scope::Server
                    && !self.is_su(actor)
                    && self.is_admin(parts[1])
                    && self.data.users.keys().filter(|n| self.is_admin(n)).count() == 1
                {
                    return Err("Cannot revoke the last active administrator.".into());
                }
                if parts[0] == "/grant" {
                    if group == Group::User && matches!(scope, Scope::Room(_)) {
                        self.require_member_delegation(actor, &scope)?;
                    }
                    self.assign(target, group, scope.clone());
                    if let Some(name) = parts.get(3) {
                        self.data
                            .rooms
                            .get_mut(*name)
                            .ok_or("Room not found.")?
                            .members
                            .insert(parts[1].into());
                    }
                } else {
                    self.data.policy.assignments.retain(|a| {
                        !(a.account_id == target && a.group == group && a.scope == scope)
                    });
                }
                self.audit(
                    actor,
                    parts[0],
                    &format!(
                        "{} {} {}",
                        parts[1],
                        group.name(),
                        parts.get(3).copied().unwrap_or("@global")
                    ),
                );
                Ok(format!(
                    "# Group assignment updated\n\n{} `{}` group for `{}` at `{}`.\n\nOther group assignments and direct grants still apply.",
                    if parts[0] == "/grant" {
                        "Assigned"
                    } else {
                        "Removed"
                    },
                    group.name(),
                    parts[1],
                    self.scope_label(&scope)
                ))
            }
            _ => Err("Unknown policy command.".into()),
        }
    }
}

pub(super) fn migrate(data: &mut Data) {
    data.policy.assignments.push(Assignment {
        account_id: "console".into(),
        group: Group::Su,
        scope: Scope::Server,
    });
    for user in data.users.values_mut() {
        user.id = uuid::Uuid::new_v4().to_string();
        data.policy.assignments.push(Assignment {
            account_id: user.id.clone(),
            group: if user.admin {
                Group::Admin
            } else {
                Group::User
            },
            scope: Scope::Server,
        });
        if user.admin {
            data.policy.grants.push(Grant {
                owner: false,
                subject: Subject::Account(user.id.clone()),
                scope: Scope::Server,
                permissions: BTreeSet::from([
                    Action::Reset.name().into(),
                    Action::Clean.name().into(),
                    "/reset".into(),
                    "/clean".into(),
                ]),
                minimum_age: 0,
            });
        }
        user.admin = false;
    }
    for room in data.rooms.values_mut() {
        room.id = uuid::Uuid::new_v4().to_string();
        room.owner_id = "console".into();
        for name in &room.members {
            if let Some(user) = data.users.get(name) {
                data.policy.assignments.push(Assignment {
                    account_id: user.id.clone(),
                    group: Group::User,
                    scope: Scope::Room(room.id.clone()),
                });
            }
        }
        let mut r = manager_rights();
        r.add(member_rights());
        let mut permissions: BTreeSet<_> = r.names().into_iter().collect();
        permissions.extend(
            COMMANDS
                .iter()
                .filter(|c| r.command(c))
                .map(|c| (*c).to_owned()),
        );
        data.policy.grants.push(Grant {
            owner: false,
            subject: Subject::Group(Group::Admin),
            scope: Scope::Room(room.id.clone()),
            permissions,
            minimum_age: 0,
        });
    }
    for (key, chat) in &mut data.private {
        chat.id = key
            .split_once(':')
            .and_then(|(a, b)| data.users.get(a).zip(data.users.get(b)))
            .map_or_else(
                || uuid::Uuid::new_v4().to_string(),
                |(a, b)| format!("{}:{}", a.id, b.id),
            );
        for message in &mut chat.messages {
            message.private_id.clone_from(&chat.id);
        }
    }
    for message in data
        .rooms
        .values_mut()
        .flat_map(|r| r.messages.iter_mut())
        .chain(
            data.private
                .values_mut()
                .flat_map(|r| r.messages.iter_mut()),
        )
    {
        message.author_id = data
            .users
            .get(&message.from)
            .map_or_else(|| "deleted".into(), |u| u.id.clone());
    }
}
