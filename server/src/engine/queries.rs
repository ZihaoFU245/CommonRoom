use super::*;
use crate::commands;
use authorization::{Action, Scope};

type SnapshotRights = BTreeMap<Scope, authorization::Rights>;
struct VisibleConversations {
    jobs: Vec<super::snapshot::Conversation>,
    rooms: Vec<RoomView>,
    private_access: BTreeMap<String, Access>,
    peers: Vec<String>,
}
struct SnapshotDirectory {
    users: Vec<String>,
    available_rooms: Vec<String>,
    avatars: BTreeMap<String, String>,
}

impl Engine {
    fn room_owner_name(&self, owner_id: &str) -> &str {
        self.bot_index
            .accounts
            .get(owner_id)
            .map_or("su", String::as_str)
    }

    pub(crate) fn conversation(
        &self,
        user: &str,
        view: &str,
    ) -> Result<(String, String, u64), EngineError> {
        if !self.active(user) {
            return Err(EngineError::Forbidden("Account unavailable.".into()));
        }
        if let Some(key) = view.strip_prefix("@private:") {
            self.require(Some(user), &self.private_scope(key), Action::Read)?;
            let chat = self
                .data
                .private
                .get(key)
                .ok_or_else(|| EngineError::NotFound("Conversation not found.".into()))?;
            return Ok((format!("dm:{key}"), chat.id.clone(), chat.revision));
        }
        if let Some(peer) = view.strip_prefix("@direct:") {
            let key = private_key(user, peer);
            self.require(Some(user), &self.private_scope(&key), Action::Read)?;
            let chat = self
                .data
                .private
                .get(&key)
                .ok_or_else(|| EngineError::NotFound("Conversation not found.".into()))?;
            return Ok((format!("dm:{key}"), chat.id.clone(), chat.revision));
        }
        let room = self
            .data
            .rooms
            .get(view)
            .ok_or_else(|| EngineError::NotFound("Room not found.".into()))?;
        self.require(Some(user), &Scope::Room(room.id.clone()), Action::Read)?;
        Ok((format!("room:{view}"), room.id.clone(), room.revision))
    }
    #[cfg(test)]
    pub fn history(&self, user: &str, view: &str) -> Result<History, EngineError> {
        self.history_page(user, view, history::HistoryRange::default())
    }
    #[cfg(test)]
    pub fn history_page(
        &self,
        user: &str,
        view: &str,
        range: history::HistoryRange,
    ) -> Result<History, EngineError> {
        self.history_page_with_colors(user, view, range, false)
    }
    #[cfg(test)]
    pub fn history_page_with_colors(
        &self,
        user: &str,
        view: &str,
        range: history::HistoryRange,
        colors: bool,
    ) -> Result<History, EngineError> {
        let (_, id, revision) = self.conversation(user, view)?;
        let mut page = self
            .history_store()
            .page_checked(&id, revision, range, view)
            .map_err(EngineError::from)?;
        if colors {
            page.name_colors = Some(self.message_colors(&id, &page.messages));
        }
        Ok(page)
    }
    #[cfg(test)]
    pub fn mark_read(&mut self, user: &str, view: &str, through: u64) -> Result<bool, EngineError> {
        let (key, id, _) = self.conversation(user, view)?;
        if !self.history_store().contains_sequence(&id, through)? {
            return Err("Message is no longer in this conversation.".into());
        }
        self.advance_read_position(user, &key, through)
    }
    pub(super) fn read_position(&self, user: &str, key: &str) -> u64 {
        self.store
            .read_positions
            .get(user)
            .and_then(|p| p.get(key))
            .copied()
            .unwrap_or(0)
    }
    #[cfg(test)]
    pub(super) fn advance_read_position(
        &mut self,
        user: &str,
        key: &str,
        through: u64,
    ) -> Result<bool, EngineError> {
        if self.read_position(user, key) >= through {
            return Ok(false);
        }
        self.store_read_position(user, key, through)?;
        self.publish_read_position(user, key, through)
    }
    pub(super) fn publish_read_position(
        &mut self,
        user: &str,
        key: &str,
        through: u64,
    ) -> Result<bool, EngineError> {
        if self.read_position(user, key) >= through {
            return Ok(false);
        }
        self.read_clock = self.read_clock.wrapping_add(1);
        let version = self
            .read_revisions
            .entry(
                self.data
                    .users
                    .get(user)
                    .ok_or_else(|| EngineError::Forbidden("Account unavailable.".into()))?
                    .id
                    .clone(),
            )
            .or_default();
        *version = version.wrapping_add(1);
        self.store
            .read_positions
            .entry(user.into())
            .or_default()
            .insert(key.into(), through);
        Ok(true)
    }
    #[cfg(test)]
    pub fn unreads(&self, name: &str) -> Result<BTreeMap<String, Unread>, EngineError> {
        self.unread_plan(name)?.finish()
    }
    pub fn connect(&mut self, name: &str) -> bool {
        self.presence.connect(name)
    }
    pub fn disconnect(&mut self, name: &str) -> bool {
        self.presence.disconnect(name)
    }
    pub(crate) fn presence_revision(&self) -> u64 {
        self.presence.revision
    }
    pub(crate) fn presence_frame(&self, name: &str) -> std::sync::Arc<str> {
        if !self.allows(Some(name), &Scope::Server, Action::Directory) {
            return std::sync::Arc::from("{\"kind\":\"presence\",\"online\":[]}");
        }
        self.presence.frame(self.revision, |name| self.active(name))
    }
    pub fn online(&self, name: &str) -> Vec<String> {
        if !self.allows(Some(name), &Scope::Server, Action::Directory) {
            return Vec::new();
        }
        self.presence
            .online(self.revision, |name| self.active(name))
    }
    #[cfg(test)]
    pub fn conversation_update(
        &self,
        user: &str,
        id: &str,
    ) -> Result<Option<serde_json::Value>, EngineError> {
        self.conversation_plan(user, id)?
            .map(|plan| plan.finish())
            .transpose()
    }
    pub(crate) fn unread_plan(
        &self,
        user: &str,
    ) -> Result<super::snapshot::UnreadPlan, EngineError> {
        let plan = self
            .snapshot_plan_for(user, false)
            .ok_or_else(|| EngineError::Forbidden("Account unavailable.".into()))?;
        Ok(super::snapshot::UnreadPlan {
            user: user.into(),
            conversations: plan.jobs,
            history: plan.history,
        })
    }
    #[cfg(test)]
    pub(crate) fn conversation_plan(
        &self,
        user: &str,
        id: &str,
    ) -> Result<Option<super::snapshot::ConversationPlan>, EngineError> {
        self.conversation_plan_for(user, id, true)
    }
    pub(crate) fn conversation_plan_for(
        &self,
        user: &str,
        id: &str,
        name_colors: bool,
    ) -> Result<Option<super::snapshot::ConversationPlan>, EngineError> {
        if !self.active(user) {
            return Err(EngineError::Forbidden("Account unavailable.".into()));
        }
        let view = self.bot_index.conversations.get(id).map(|view| {
            view.strip_prefix("@private:").map_or_else(
                || view.clone(),
                |key| {
                    private_peer(key, user)
                        .map_or_else(|| view.clone(), |peer| format!("@direct:{peer}"))
                },
            )
        });
        let Some(view) = view else {
            return Ok(None);
        };
        let Ok((key, id, revision)) = self.conversation(user, &view) else {
            return Ok(None);
        };
        Ok(Some(super::snapshot::ConversationPlan {
            user: user.into(),
            colors: if name_colors {
                self.conversation_color_directory(&id)
            } else {
                Default::default()
            },
            color_version: if name_colors {
                self.color_version()
            } else {
                String::new()
            },
            conversation: super::snapshot::Conversation {
                view,
                id,
                revision,
                read: self.read_position(user, &key),
                direct: false,
            },
            history: self.history_store().clone(),
        }))
    }
    #[cfg(test)]
    pub fn try_snapshot(&self, name: &str) -> Result<Option<Snapshot>, EngineError> {
        Ok(self.snapshot_baseline(name)?.map(|(snapshot, _)| snapshot))
    }
    #[cfg(test)]
    pub(crate) fn snapshot_baseline(
        &self,
        name: &str,
    ) -> Result<Option<(Snapshot, bool)>, EngineError> {
        self.snapshot_plan(name)
            .map(|plan| plan.finish())
            .transpose()
    }
    #[cfg(test)]
    pub fn snapshot(&self, name: &str) -> Option<Snapshot> {
        self.try_snapshot(name).ok().flatten()
    }
    #[cfg(test)]
    pub(crate) fn snapshot_plan(&self, name: &str) -> Option<super::snapshot::SnapshotPlan> {
        self.snapshot_plan_for(name, true)
    }
    pub(crate) fn snapshot_plan_for(
        &self,
        name: &str,
        name_colors: bool,
    ) -> Option<super::snapshot::SnapshotPlan> {
        let account = self.data.users.get(name).filter(|u| !u.disabled)?;
        if let Some(mut plan) = self.read_model.browser(&account.id) {
            for job in &mut plan.jobs {
                (job.revision, job.read) = self.conversation_position(name, &job.id)?;
            }
            plan.snapshot.sync.revision = self
                .revision
                .saturating_add(self.presence.revision)
                .saturating_add(self.read_clock);
            plan.snapshot.online = self.online(name);
            if name_colors {
                plan.snapshot.color_version = self.color_version();
                plan.colors = self.color_directory();
            }
            return Some(plan);
        }
        let account_scope = Scope::Account(account.id.clone());
        let scope_rights = self.snapshot_scope_rights(name, &account_scope);
        let rights = |scope: &Scope| scope_rights.get(scope).copied().unwrap_or_default();
        let server_commands = self.commands_for_rights(Some(name), None, rights(&Scope::Server));
        let visible = self.visible_conversations(name, &scope_rights);
        let directory = self.snapshot_directory(name, &scope_rights);
        let snapshot = Snapshot {
            color_version: String::new(),
            sync: SyncVersion {
                instance: self.instance.clone(),
                revision: self
                    .revision
                    .saturating_add(self.presence.revision)
                    .saturating_add(self.read_clock),
            },
            kind: "snapshot",
            username: name.into(),
            admin: self.is_admin(name),
            groups: self.groups(Some(name)),
            permissions: rights(&Scope::Server).names(),
            account_access: Access {
                id: account.id.clone(),
                permissions: rights(&account_scope).names(),
                commands: server_commands
                    .iter()
                    .filter(|command| rights(&account_scope).command(command.name))
                    .cloned()
                    .collect(),
            },
            private_access: visible.private_access,
            private_permissions: rights(&self.private_scope(&private_key(name, name))).names(),
            private_commands: self.commands_for_rights(
                Some(name),
                Some(&format!("@direct:{name}")),
                rights(&self.private_scope(&private_key(name, name))),
            ),
            policy_revision: self.data.policy.revision,
            avatars: BTreeMap::new(),
            colors: BTreeMap::new(),
            online: self.online(name),
            users: directory.users,
            rooms: visible.rooms,
            direct: Vec::new(),
            private_peers: visible.peers,
            unread: BTreeMap::new(),
            commands: server_commands,
            available_rooms: directory.available_rooms,
        };
        let mut plan = super::snapshot::SnapshotPlan {
            snapshot,
            jobs: visible.jobs,
            history: self.history_store().clone(),
            avatars: directory.avatars,
            colors: Default::default(),
        };
        self.read_model.remember_browser(account.id.clone(), &plan);
        if name_colors {
            plan.snapshot.color_version = self.color_version();
            plan.colors = self.color_directory();
        }
        Some(plan)
    }
    fn snapshot_scope_rights(&self, name: &str, account_scope: &Scope) -> SnapshotRights {
        let mut scope_rights = BTreeMap::new();
        for scope in [
            Scope::Server,
            account_scope.clone(),
            self.private_scope(&private_key(name, name)),
        ]
        .into_iter()
        .chain(self.data.rooms.values().map(|r| Scope::Room(r.id.clone())))
        .chain(self.data.private.keys().map(|key| self.private_scope(key)))
        {
            scope_rights
                .entry(scope.clone())
                .or_insert_with(|| self.effective(Some(name), &scope));
        }
        scope_rights
    }
    fn visible_conversations(
        &self,
        name: &str,
        scope_rights: &SnapshotRights,
    ) -> VisibleConversations {
        let rights = |scope: &Scope| scope_rights.get(scope).copied().unwrap_or_default();
        let mut visible = VisibleConversations {
            jobs: Vec::new(),
            rooms: Vec::new(),
            private_access: BTreeMap::new(),
            peers: Vec::new(),
        };
        for (view, room) in &self.data.rooms {
            let scope = Scope::Room(room.id.clone());
            let rights = rights(&scope);
            if !rights.has(Action::Read) {
                continue;
            }
            visible.jobs.push(super::snapshot::Conversation {
                view: view.clone(),
                id: room.id.clone(),
                revision: room.revision,
                read: self.read_position(name, &format!("room:{view}")),
                direct: false,
            });
            visible.rooms.push(RoomView {
                id: room.id.clone(),
                name: view.clone(),
                owner: self.room_owner_name(&room.owner_id).into(),
                permissions: rights.names(),
                commands: self.commands_for_rights(Some(name), Some(view), rights),
                members: if rights.has(Action::Members) {
                    room.members.clone()
                } else {
                    BTreeSet::new()
                },
                messages: Default::default(),
                colors: BTreeMap::new(),
            });
        }
        for (key, chat) in &self.data.private {
            let scope = self.private_scope(key);
            let rights = rights(&scope);
            if !rights.has(Action::Read) {
                continue;
            }
            let peer = private_peer(key, name);
            let view = peer.map_or_else(
                || format!("@private:{key}"),
                |peer| format!("@direct:{peer}"),
            );
            let commands = self.commands_for_rights(Some(name), Some(&view), rights);
            visible.jobs.push(super::snapshot::Conversation {
                view: view.clone(),
                id: chat.id.clone(),
                revision: chat.revision,
                read: self.read_position(name, &format!("dm:{key}")),
                direct: peer.is_some(),
            });
            if let Some(peer) = peer {
                visible.peers.push(peer.into());
                visible.private_access.insert(
                    view,
                    Access {
                        id: chat.id.clone(),
                        permissions: rights.names(),
                        commands,
                    },
                );
            } else {
                visible.rooms.push(RoomView {
                    id: chat.id.clone(),
                    name: view,
                    owner: "participants".into(),
                    permissions: rights.names(),
                    commands,
                    members: if rights.has(Action::Members) {
                        key.split(':').map(str::to_owned).collect()
                    } else {
                        BTreeSet::new()
                    },
                    messages: Default::default(),
                    colors: BTreeMap::new(),
                });
            }
        }
        visible
    }
    fn snapshot_directory(&self, name: &str, scope_rights: &SnapshotRights) -> SnapshotDirectory {
        let rights = |scope: &Scope| scope_rights.get(scope).copied().unwrap_or_default();
        let directory = rights(&Scope::Server).has(Action::Directory);
        let admin = self.is_admin(name);
        SnapshotDirectory {
            users: self
                .data
                .users
                .iter()
                .filter(|(_, user)| directory && (admin || !user.disabled))
                .map(|(name, _)| name.clone())
                .collect(),
            available_rooms: self
                .data
                .rooms
                .iter()
                .filter(|(_, room)| {
                    let rights = rights(&Scope::Room(room.id.clone()));
                    rights.has(Action::Discover) || rights.has(Action::Read)
                })
                .map(|(name, _)| name.clone())
                .collect(),
            avatars: self
                .avatar_directory_for(&self.data.users.keys().map(String::as_str).collect()),
        }
    }
    pub(super) fn apply_queries(
        &self,
        context: &CommandContext<'_>,
    ) -> Result<String, EngineError> {
        let CommandContext {
            actor, room, parts, ..
        } = *context;
        match parts[0] {
            "/console" => {
                require_len(parts, 1, "/console")?;
                if actor.is_none() {
                    return Err("The Command view is available in the web UI only.".into());
                }
                Ok("Command view opened.".into())
            }
            "/debug" => {
                let allowed = self.allows(actor, &context.scope, Action::Metadata)
                    || (context.scope == Scope::Server
                        && (self.data.rooms.values().any(|r| {
                            self.allows(actor, &Scope::Room(r.id.clone()), Action::Metadata)
                        }) || self.is_su(actor)));
                if !allowed {
                    return Err(EngineError::Forbidden(
                        "Permission required: r:message.metadata".into(),
                    ));
                }
                if actor.is_none() {
                    return Err("Debug details are available in the web UI only.".into());
                }
                require_len(parts, 2, "/debug on|off")?;
                if !["on", "off"].contains(&parts[1]) {
                    return Err("Usage: /debug on|off".into());
                }
                Ok(format!("Debug {}.", parts[1]))
            }
            "/man" => {
                if parts.len() > 2 {
                    return Err("Usage: /man [command|topic], e.g. /man grant".into());
                }
                crate::manual::manual(parts.get(1).copied()).map_err(EngineError::Invalid)
            }
            "/help" => {
                require_len(parts, 1, "/help")?;
                Ok(crate::commands::help_for(
                    &self.commands_for(actor, context.view),
                ))
            }
            "/whoami" => {
                require_len(parts, 1, "/whoami")?;
                Ok(format!(
                    "Name: {}\nPermission: {}",
                    actor.unwrap_or("su"),
                    self.permission_label(actor)
                ))
            }
            "/ls" if parts.len() == 1 || parts.get(1) == Some(&"room") => {
                if parts.len() > 2 {
                    return Err(EngineError::Invalid(format!(
                        "Usage: {}",
                        commands::LS_USAGE
                    )));
                }
                let rooms = self
                    .data
                    .rooms
                    .iter()
                    .filter(|(_, r)| {
                        self.allows(actor, &Scope::Room(r.id.clone()), Action::Discover)
                            || self.allows(actor, &Scope::Room(r.id.clone()), Action::Read)
                    })
                    .map(|(n, r)| format!("#{n} — owner: {}", self.room_owner_name(&r.owner_id)))
                    .collect::<Vec<_>>()
                    .join("\n");
                let private = self
                    .data
                    .private
                    .iter()
                    .filter(|(key, _)| {
                        actor.is_some_and(|user| private_peer(key, user).is_some())
                            && self.allows(actor, &self.private_scope(key), Action::Read)
                    })
                    .map(|(key, _)| format!("@private:{key}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                Ok(format!(
                    "Your rooms:\n{}\n\nPrivate conversations:\n{}",
                    if rooms.is_empty() { "None." } else { &rooms },
                    if private.is_empty() {
                        "None."
                    } else {
                        &private
                    }
                ))
            }
            "/ls" => {
                require_len(parts, 2, commands::LS_USAGE)?;
                if parts[1] == "group" {
                    return Ok("Groups: user, admin, su (predefined permission bundles).\nUse /ls @user, /ls @admin, or /ls @su for global members; /man groups explains the bundles.".into());
                }
                if !["user", "@user", "@admin", "@su"].contains(&parts[1]) {
                    return Err(EngineError::Invalid(format!(
                        "Usage: {}",
                        commands::LS_USAGE
                    )));
                }
                self.require_target_command(actor, &Scope::Server, context.command)?;
                self.require(actor, &Scope::Server, Action::Directory)?;
                Ok(format!(
                    "Users:\n{}",
                    self.data
                        .users
                        .iter()
                        .filter(|(_, u)| self.is_su(actor) || !u.disabled)
                        .filter_map(|(n, u)| {
                            let groups: Vec<_> = self
                                .data
                                .policy
                                .assignments
                                .iter()
                                .filter(|a| a.account_id == u.id && a.scope == Scope::Server)
                                .map(|a| a.group)
                                .collect();
                            if parts[1] != "user"
                                && !groups.iter().any(|g| format!("@{}", g.name()) == parts[1])
                            {
                                return None;
                            }
                            Some(format!(
                                "{n} — {}{}",
                                groups.iter().max().map_or("user", |g| g.name()),
                                if u.disabled { " (disabled)" } else { "" }
                            ))
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                ))
            }
            "/members" => {
                if parts.len() > 2 {
                    return Err("Usage: /members [room]".into());
                }
                if parts.len() == 1
                    && let Scope::Private(id) = &context.scope
                {
                    self.require(actor, &context.scope, Action::Members)?;
                    let key = self
                        .data
                        .private
                        .iter()
                        .find(|(_, chat)| chat.id == *id)
                        .map(|(key, _)| key)
                        .ok_or_else(|| {
                            EngineError::NotFound("Private conversation not found.".into())
                        })?;
                    return Ok(key
                        .split(':')
                        .map(|n| format!("{n} — {}", self.permission_label(Some(n))))
                        .collect::<Vec<_>>()
                        .join("\n"));
                }
                let name = parts
                    .get(1)
                    .map(|v| v.trim_start_matches('#'))
                    .or(room)
                    .ok_or("Select or specify a room.")?;
                let target = self
                    .data
                    .rooms
                    .get(name)
                    .ok_or_else(|| EngineError::NotFound("Room not found.".into()))?;
                self.require_target_command(actor, &Scope::Room(target.id.clone()), parts[0])?;
                self.require(actor, &Scope::Room(target.id.clone()), Action::Members)?;
                Ok(target
                    .members
                    .iter()
                    .map(|n| format!("{n} — {}", self.permission_label(Some(n))))
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
            #[cfg(test)]
            "/history" => self.history_command_plan(context)?.finish(),

            _ => Err("Unknown command.".into()),
        }
    }
}
