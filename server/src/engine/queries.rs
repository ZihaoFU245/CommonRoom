use super::*;
use crate::commands;
use authorization::{Action, Scope};

impl Engine {
    fn room_owner_name(&self, owner_id: &str) -> &str {
        self.data
            .users
            .iter()
            .find(|(_, user)| user.id == owner_id)
            .map_or("su", |(name, _)| name.as_str())
    }

    pub(crate) fn conversation(
        &self,
        user: &str,
        view: &str,
    ) -> Result<(String, String, u64), String> {
        if !self.active(user) {
            return Err("Account unavailable.".into());
        }
        if let Some(key) = view.strip_prefix("@private:") {
            self.require(Some(user), &self.private_scope(key), Action::Read)?;
            let chat = self
                .data
                .private
                .get(key)
                .ok_or("Conversation not found.")?;
            return Ok((format!("dm:{key}"), chat.id.clone(), chat.revision));
        }
        if let Some(peer) = view.strip_prefix("@direct:") {
            let key = private_key(user, peer);
            self.require(Some(user), &self.private_scope(&key), Action::Read)?;
            let chat = self
                .data
                .private
                .get(&key)
                .ok_or("Conversation not found.")?;
            return Ok((format!("dm:{key}"), chat.id.clone(), chat.revision));
        }
        let room = self.data.rooms.get(view).ok_or("Room not found.")?;
        self.require(Some(user), &Scope::Room(room.id.clone()), Action::Read)?;
        Ok((format!("room:{view}"), room.id.clone(), room.revision))
    }
    #[cfg(test)]
    pub fn history(&self, user: &str, view: &str) -> Result<History, String> {
        self.history_page(user, view, history::HistoryRange::default())
    }
    #[cfg(test)]
    pub fn history_page(
        &self,
        user: &str,
        view: &str,
        range: history::HistoryRange,
    ) -> Result<History, String> {
        let (_, id, revision) = self.conversation(user, view)?;
        self.history_store.page(&id, revision, range, view)
    }
    pub fn mark_read(&mut self, user: &str, view: &str, through: u64) -> Result<bool, String> {
        let (key, id, _) = self.conversation(user, view)?;
        if !self.history_store.contains_sequence(&id, through)? {
            return Err("Message is no longer in this conversation.".into());
        }
        self.advance_read_position(user, &key, through)
    }
    pub(super) fn read_position(&self, user: &str, key: &str) -> u64 {
        self.read_positions
            .get(user)
            .and_then(|p| p.get(key))
            .copied()
            .unwrap_or(0)
    }
    pub(super) fn advance_read_position(
        &mut self,
        user: &str,
        key: &str,
        through: u64,
    ) -> Result<bool, String> {
        if self.read_position(user, key) >= through {
            return Ok(false);
        }
        self.store_read_position(user, key, through)?;
        self.read_clock = self.read_clock.wrapping_add(1);
        let version = self
            .read_revisions
            .entry(
                self.data
                    .users
                    .get(user)
                    .ok_or("Account unavailable.")?
                    .id
                    .clone(),
            )
            .or_default();
        *version = version.wrapping_add(1);
        self.read_positions
            .entry(user.into())
            .or_default()
            .insert(key.into(), through);
        Ok(true)
    }
    pub fn unreads(&self, name: &str) -> Result<BTreeMap<String, Unread>, String> {
        self.collect_unreads(name, false, None)
    }
    fn collect_unreads(
        &self,
        name: &str,
        partial: bool,
        rights: Option<&BTreeMap<Scope, authorization::Rights>>,
    ) -> Result<BTreeMap<String, Unread>, String> {
        self.history_store.ensure_available()?;
        let readable = |scope: &Scope| {
            rights.map_or_else(
                || self.allows(Some(name), scope, Action::Read),
                |rights| {
                    rights
                        .get(scope)
                        .copied()
                        .unwrap_or_default()
                        .has(Action::Read)
                },
            )
        };
        let mut result = BTreeMap::new();
        for (view, id, revision, key) in self
            .data
            .rooms
            .iter()
            .filter(|(_, r)| readable(&Scope::Room(r.id.clone())))
            .map(|(name, r)| {
                (
                    name.clone(),
                    r.id.clone(),
                    r.revision,
                    format!("room:{name}"),
                )
            })
            .chain(
                self.data
                    .private
                    .iter()
                    .filter(|(key, _)| readable(&self.private_scope(key)))
                    .map(|(key, c)| {
                        (
                            private_peer(key, name).map_or_else(
                                || format!("@private:{key}"),
                                |peer| format!("@direct:{peer}"),
                            ),
                            c.id.clone(),
                            c.revision,
                            format!("dm:{key}"),
                        )
                    }),
            )
        {
            let read = self.read_position(name, &key);
            match self.history_store.unread(&id, name, read, revision) {
                Ok(unread) => {
                    result.insert(view, unread);
                }
                Err(_) if partial => {
                    tracing::warn!(conversation_id = %id, "Conversation unread data unavailable; snapshot entry omitted.");
                }
                Err(error) => return Err(error),
            }
        }
        Ok(result)
    }
    pub fn connect(&mut self, name: &str) -> bool {
        let count = self.online_connections.entry(name.into()).or_default();
        *count += 1;
        let changed = *count == 1;
        if changed {
            self.presence_revision = self.presence_revision.wrapping_add(1);
        }
        changed
    }
    pub fn disconnect(&mut self, name: &str) -> bool {
        if let Some(count) = self.online_connections.get_mut(name) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.online_connections.remove(name);
                self.presence_revision = self.presence_revision.wrapping_add(1);
                return true;
            }
        }
        false
    }
    pub fn online(&self, name: &str) -> Vec<String> {
        if !self.allows(Some(name), &Scope::Server, Action::Directory) {
            return Vec::new();
        }
        self.online_connections
            .keys()
            .filter(|online| self.active(online))
            .cloned()
            .collect()
    }
    pub fn conversation_update(
        &self,
        user: &str,
        id: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        if !self.active(user) {
            return Err("Account unavailable.".into());
        }
        let view = self
            .data
            .rooms
            .iter()
            .find(|(_, r)| r.id == id)
            .map(|(name, _)| name.clone())
            .or_else(|| {
                self.data
                    .private
                    .iter()
                    .find(|(_, c)| c.id == id)
                    .map(|(key, _)| {
                        private_peer(key, user).map_or_else(
                            || format!("@private:{key}"),
                            |peer| format!("@direct:{peer}"),
                        )
                    })
            });
        let Some(view) = view else {
            return Ok(None);
        };
        let Ok((key, id, revision)) = self.conversation(user, &view) else {
            return Ok(None);
        };
        let page = self.history_store.page(
            &id,
            revision,
            history::HistoryRange {
                limit: Some(VISIBLE_HISTORY),
                ..Default::default()
            },
            &view,
        )?;
        let read = self
            .read_positions
            .get(user)
            .and_then(|p| p.get(&key))
            .copied()
            .unwrap_or(0);
        let unread = self.history_store.unread(&id, user, read, revision)?;
        Ok(Some(
            serde_json::json!({"kind":"conversation","view":view,"id":id,"messages":page.messages,"unread":unread}),
        ))
    }
    #[cfg(test)]
    pub fn try_snapshot(&self, name: &str) -> Result<Option<Snapshot>, String> {
        Ok(self.snapshot_baseline(name)?.map(|(snapshot, _)| snapshot))
    }
    pub(crate) fn snapshot_baseline(&self, name: &str) -> Result<Option<(Snapshot, bool)>, String> {
        self.history_store.ensure_available()?;
        let snapshot = self.snapshot_inner(name);
        self.history_store.ensure_available()?;
        Ok(snapshot)
    }
    #[cfg(test)]
    pub fn snapshot(&self, name: &str) -> Option<Snapshot> {
        self.try_snapshot(name).ok().flatten()
    }
    fn snapshot_inner(&self, name: &str) -> Option<(Snapshot, bool)> {
        let account = self.data.users.get(name).filter(|u| !u.disabled)?;
        let account_scope = Scope::Account(account.id.clone());
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
        let rights = |scope: &Scope| scope_rights.get(scope).copied().unwrap_or_default();
        let server_commands = self.commands_for_rights(Some(name), None, rights(&Scope::Server));
        let mut tails = BTreeMap::new();
        let mut complete = true;
        let mut readable = 0;
        let mut tail_budget = 1000usize;
        let mut byte_budget = 4 * 1024 * 1024usize;
        for (id, revision, scope) in self
            .data
            .rooms
            .values()
            .map(|r| (&r.id, r.revision, Scope::Room(r.id.clone())))
            .chain(
                self.data
                    .private
                    .iter()
                    .map(|(key, c)| (&c.id, c.revision, self.private_scope(key))),
            )
        {
            if rights(&scope).has(Action::Read) {
                readable += 1;
                if tail_budget == 0 {
                    continue;
                }
                let page = match self.history_store.page(
                    id,
                    revision,
                    history::HistoryRange {
                        limit: Some(VISIBLE_HISTORY),
                        ..Default::default()
                    },
                    "",
                ) {
                    Ok(page) => page,
                    Err(_) => {
                        tracing::warn!(conversation_id = %id, "Conversation history unavailable; snapshot tail omitted.");
                        complete = false;
                        continue;
                    }
                };
                let messages = page
                    .messages
                    .bounded_tail(&mut tail_budget, &mut byte_budget);
                tails.insert(id.clone(), messages);
            }
        }
        let unread = self.collect_unreads(name, true, Some(&scope_rights)).ok()?;
        complete &= unread.len() == readable;
        let snapshot = Snapshot {
            sync: SyncVersion {
                instance: self.instance.clone(),
                revision: self
                    .revision
                    .saturating_add(self.presence_revision)
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
            private_access: self
                .data
                .private
                .iter()
                .filter(|(key, _)| rights(&self.private_scope(key)).has(Action::Read))
                .filter_map(|(key, chat)| {
                    private_peer(key, name).map(|peer| {
                        let view = format!("@direct:{peer}");
                        (
                            view.clone(),
                            Access {
                                id: chat.id.clone(),
                                permissions: rights(&self.private_scope(key)).names(),
                                commands: self.commands_for_rights(
                                    Some(name),
                                    Some(&view),
                                    rights(&self.private_scope(key)),
                                ),
                            },
                        )
                    })
                })
                .collect(),
            private_permissions: rights(&self.private_scope(&private_key(name, name))).names(),
            private_commands: self.commands_for_rights(
                Some(name),
                Some(&format!("@direct:{name}")),
                rights(&self.private_scope(&private_key(name, name))),
            ),
            policy_revision: self.data.policy.revision,
            online: self.online(name),
            users: self
                .data
                .users
                .iter()
                .filter(|(_, u)| {
                    rights(&Scope::Server).has(Action::Directory)
                        && (self.is_admin(name) || !u.disabled)
                })
                .map(|(n, _)| n.clone())
                .collect(),
            rooms: self
                .data
                .rooms
                .iter()
                .filter(|(_, r)| rights(&Scope::Room(r.id.clone())).has(Action::Read))
                .map(|(n, r)| RoomView {
                    id: r.id.clone(),
                    owner: self.room_owner_name(&r.owner_id).into(),
                    permissions: rights(&Scope::Room(r.id.clone())).names(),
                    commands: self.commands_for_rights(
                        Some(name),
                        Some(n),
                        rights(&Scope::Room(r.id.clone())),
                    ),
                    name: n.clone(),
                    members: if rights(&Scope::Room(r.id.clone())).has(Action::Members) {
                        r.members.clone()
                    } else {
                        BTreeSet::new()
                    },
                    messages: tails.get(&r.id).cloned().unwrap_or_default(),
                })
                .chain(
                    self.data
                        .private
                        .iter()
                        .filter(|(key, _)| {
                            private_peer(key, name).is_none()
                                && rights(&self.private_scope(key)).has(Action::Read)
                        })
                        .map(|(key, chat)| {
                            let view = format!("@private:{key}");
                            let scope = self.private_scope(key);
                            RoomView {
                                id: chat.id.clone(),
                                name: view.clone(),
                                owner: "participants".into(),
                                permissions: rights(&scope).names(),
                                commands: self.commands_for_rights(
                                    Some(name),
                                    Some(&view),
                                    rights(&self.private_scope(key)),
                                ),
                                members: if rights(&scope).has(Action::Members) {
                                    key.split(':').map(str::to_owned).collect()
                                } else {
                                    BTreeSet::new()
                                },
                                messages: tails.get(&chat.id).cloned().unwrap_or_default(),
                            }
                        }),
                )
                .collect(),
            direct: {
                let mut messages: Vec<_> = self
                    .data
                    .private
                    .iter()
                    .filter(|(key, _)| {
                        private_peer(key, name).is_some()
                            && rights(&self.private_scope(key)).has(Action::Read)
                    })
                    .flat_map(|(_, chat)| {
                        tails
                            .get(&chat.id)
                            .into_iter()
                            .flat_map(|page| page.iter())
                            .cloned()
                    })
                    .collect();
                messages.sort_by_key(|m| m.sequence);
                messages
            },
            private_peers: self
                .data
                .private
                .keys()
                .filter(|key| rights(&self.private_scope(key)).has(Action::Read))
                .filter_map(|key| private_peer(key, name))
                .map(String::from)
                .collect(),
            unread,
            commands: server_commands,
            available_rooms: self
                .data
                .rooms
                .iter()
                .filter(|(_, r)| {
                    rights(&Scope::Room(r.id.clone())).has(Action::Discover)
                        || rights(&Scope::Room(r.id.clone())).has(Action::Read)
                })
                .map(|(n, _)| n.clone())
                .collect(),
        };
        Some((snapshot, complete))
    }
    pub(super) fn apply_queries(&self, context: &CommandContext<'_>) -> Result<String, String> {
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
                    return Err("Permission required: r:message.metadata".into());
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
                crate::manual::manual(parts.get(1).copied())
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
                    return Err(format!("Usage: {}", commands::LS_USAGE));
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
                    return Err(format!("Usage: {}", commands::LS_USAGE));
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
                        .ok_or("Private conversation not found.")?;
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
                let target = self.data.rooms.get(name).ok_or("Room not found.")?;
                self.require_target_command(actor, &Scope::Room(target.id.clone()), parts[0])?;
                self.require(actor, &Scope::Room(target.id.clone()), Action::Members)?;
                Ok(target
                    .members
                    .iter()
                    .map(|n| format!("{n} — {}", self.permission_label(Some(n))))
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
            "/history" => {
                let usage = "Usage: /history [start end] or /history #room";
                let (view, range) = match parts {
                    [_] => (
                        context
                            .view
                            .ok_or("Select a conversation or use /history #room.")?,
                        history::HistoryRange::default(),
                    ),
                    [_, target] if target.starts_with('#') && target.len() > 1 => (
                        target.strip_prefix('#').ok_or(usage)?,
                        history::HistoryRange::default(),
                    ),
                    [_, start, end] => (
                        context
                            .view
                            .ok_or("Select a conversation for sequence ranges.")?,
                        history::HistoryRange {
                            start: Some(start.parse().map_err(|_| usage)?),
                            end: Some(end.parse().map_err(|_| usage)?),
                            ..Default::default()
                        },
                    ),
                    _ => return Err(usage.into()),
                };
                let (id, revision) = if let Some(user) = actor {
                    let (_, id, revision) = self.conversation(user, view)?;
                    (id, revision)
                } else if let Some(key) = view.strip_prefix("@private:") {
                    let chat = self
                        .data
                        .private
                        .get(key)
                        .ok_or("Conversation not found.")?;
                    (chat.id.clone(), chat.revision)
                } else {
                    let room = self.data.rooms.get(view).ok_or("Room not found.")?;
                    (room.id.clone(), room.revision)
                };
                let scope = if view.starts_with('@') {
                    Scope::Private(id.clone())
                } else {
                    Scope::Room(id.clone())
                };
                self.require_target_command(actor, &scope, context.command)?;
                let page = self.history_store.page(&id, revision, range, view)?;
                let text = page
                    .messages
                    .iter()
                    .map(|m| {
                        format!(
                            "[{}] {} {}{}: {}",
                            m.sequence,
                            m.time,
                            m.from,
                            m.to.as_ref()
                                .map(|to| format!(" → {to}"))
                                .unwrap_or_default(),
                            m.text
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                Ok(if text.is_empty() {
                    "No messages in this range.".into()
                } else if page.has_after
                    && range
                        .end
                        .zip(page.messages.last())
                        .is_some_and(|(end, message)| message.sequence < end)
                {
                    format!("{text}\nMore history available; continue after the last sequence.")
                } else {
                    text
                })
            }

            _ => Err("Unknown command.".into()),
        }
    }
}
