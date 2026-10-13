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
    EditOwn => "w:message.edit.own",
    EditAny => "w:message.edit.any",
    RetractOwn => "w:message.retract.own",
    RetractAny => "w:message.retract.any",
    CreateRoom => "w:room.create",
    Direct => "w:private.create",
    RenameOwn => "w:account.rename.own",
    RenameAny => "x:account.rename.any",
    Password => "w:account.password.own",
    Invite => "x:member.add",
    Kick => "x:member.remove",
    Join => "x:room.join",
    DeleteRoom => "x:room.delete",
    RenameRoom => "x:room.rename",
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
    Sudo => "x:command.sudo",
    BotCredentials => "x:bot.credentials",
    BotConfigRead => "r:bot.config",
    BotConfigWrite => "w:bot.config",
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
    pub owner: bool,
    pub subject: Subject,
    pub scope: Scope,
    pub permissions: BTreeSet<String>,
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
    "/rename",
    "/passwd",
    "/ls",
    "/members",
    "/history",
    "/join",
    "/leave",
    "/tell",
    "/react",
    "/edit",
    "/retract",
    "/reply",
    "/new",
    "/add",
    "/kick",
    "/rm",
    "/grant",
    "/revoke",
    "/configs",
    "/clean",
    "/clear",
    "/logout",
    "/reset",
    "/disable",
    "/enable",
    "/debug",
    "/permissions",
    "/man",
    "/owner",
    "/console",
    "/sudo",
    "/bot",
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
            Action::EditOwn,
            Action::RetractOwn,
        ],
        &[
            "/members", "/history", "/react", "/edit", "/retract", "/reply",
        ],
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
            Action::RenameRoom,
            Action::Clean,
            Action::Transfer,
        ],
        &[
            "/add",
            "/kick",
            "/rm",
            "/clean",
            "/grant",
            "/revoke",
            "/owner",
            "/rename",
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
        &[
            Action::Directory,
            Action::Password,
            Action::RenameOwn,
            Action::Direct,
        ],
        &[
            "/help",
            "/man",
            "/whoami",
            "/rename",
            "/passwd",
            "/ls",
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
                Action::BotCredentials,
                Action::BotConfigRead,
                Action::BotConfigWrite,
                Action::PolicyRead,
            ],
            &[
                "/new", "/disable", "/enable", "/rm", "/grant", "/revoke", "/configs", "/debug",
                "/bot",
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

mod access;
mod commands;
