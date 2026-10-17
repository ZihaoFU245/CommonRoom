//! Policy inspection and grant/group mutations.
use super::*;

impl Engine {
    pub(in crate::engine) fn apply_policy(
        &mut self,
        context: &CommandContext<'_>,
    ) -> Result<String, String> {
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
                    .map(|value| self.policy_scope(actor, value, false))
                    .transpose()?
                    .unwrap_or_else(|| context.scope.clone());
                self.require_target_command(actor, &scope, parts[0])?;
                if scope != Scope::Server && !self.can_inspect_policy(actor, &scope) {
                    self.require(actor, &scope, Action::Read)?;
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
                if !self.can_inspect_policy(actor, &scope) {
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
                let scope = self.policy_scope(actor, parts[2], true)?;
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
                if !(parts[0] == "/revoke" && parts[3] == "x:group.su.assign") {
                    requested.insert(parts[3])?;
                }
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
                            | "/configs"
                            | "/ls"
                    )
                {
                    reply.push_str(&format!("\n\n## Scope hint\n\n`{}` is used at `@global`. This resource-scoped grant does not authorize that global operation.", parts[3]));
                }
                Ok(reply)
            }
            "/grant" | "/revoke" | "/rm" => {
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
                if parts[0] != "/grant"
                    && group == Group::Admin
                    && scope == Scope::Server
                    && !self.is_su(actor)
                    && self.is_admin(parts[1])
                    && self
                        .data
                        .users
                        .keys()
                        .filter(|n| self.active(n) && self.is_admin(n))
                        .count()
                        == 1
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
                    if group == Group::User
                        && !self.is_su(actor)
                        && let Scope::Room(room_id) = &scope
                        && self
                            .data
                            .rooms
                            .values()
                            .any(|room| room.id == *room_id && room.owner_id == target)
                    {
                        return Err(
                            "Transfer room ownership before revoking the owner's user group, or ask su."
                                .into(),
                        );
                    }
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
