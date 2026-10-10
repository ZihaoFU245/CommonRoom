use super::*;
use authorization::{Action, Scope};

impl Engine {
    fn room_owner_name(&self, owner_id: &str) -> &str {
        self.data
            .users
            .iter()
            .find(|(_, user)| user.id == owner_id)
            .map_or("su", |(name, _)| name.as_str())
    }

    pub(super) fn unread(
        &self,
        user: &str,
        key: &str,
        messages: &VecDeque<Message>,
        revision: u64,
    ) -> Unread {
        let read = self
            .read_positions
            .get(user)
            .and_then(|p| p.get(key))
            .copied()
            .unwrap_or(0);
        let start = messages.partition_point(|m| m.sequence <= read);
        let mut incoming = messages.iter().skip(start).filter(|m| m.from != user);
        let first = incoming.next().map(|m| m.sequence);
        Unread {
            count: usize::from(first.is_some()) + incoming.count(),
            first,
            through: messages.back().map_or(0, |m| m.sequence),
            oldest: messages.front().map_or(0, |m| m.sequence),
            revision,
        }
    }
    pub(super) fn conversation(
        &self,
        user: &str,
        view: &str,
    ) -> Result<(String, &VecDeque<Message>, u64), String> {
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
            return Ok((format!("dm:{key}"), &chat.messages, chat.revision));
        }
        if let Some(peer) = view.strip_prefix("@direct:") {
            let key = private_key(user, peer);
            self.require(Some(user), &self.private_scope(&key), Action::Read)?;
            let chat = self
                .data
                .private
                .get(&key)
                .ok_or("Conversation not found.")?;
            return Ok((format!("dm:{key}"), &chat.messages, chat.revision));
        }
        let room = self.data.rooms.get(view).ok_or("Room not found.")?;
        self.require(Some(user), &Scope::Room(room.id.clone()), Action::Read)?;
        Ok((format!("room:{view}"), &room.messages, room.revision))
    }
    pub fn history(&self, user: &str, view: &str) -> Result<History, String> {
        let (_, messages, revision) = self.conversation(user, view)?;
        Ok(History {
            view: view.into(),
            messages: messages.iter().cloned().collect(),
            revision,
        })
    }
    pub fn mark_read(&mut self, user: &str, view: &str, through: u64) -> Result<bool, String> {
        let (key, messages, _) = self.conversation(user, view)?;
        if messages
            .binary_search_by_key(&through, |m| m.sequence)
            .is_err()
        {
            return Err("Message is no longer in this conversation.".into());
        }
        if self
            .read_positions
            .get(user)
            .and_then(|p| p.get(&key))
            .copied()
            .unwrap_or(0)
            >= through
        {
            return Ok(false);
        }
        self.store_read_position(user, &key, through)?;
        self.read_positions
            .entry(user.into())
            .or_default()
            .insert(key, through);
        Ok(true)
    }
    pub fn unreads(&self, name: &str) -> BTreeMap<String, Unread> {
        self.data
            .rooms
            .iter()
            .filter(|(_, room)| {
                self.allows(Some(name), &Scope::Room(room.id.clone()), Action::Read)
            })
            .map(|(view, room)| {
                (
                    view.clone(),
                    self.unread(name, &format!("room:{view}"), &room.messages, room.revision),
                )
            })
            .chain(self.data.private.iter().filter_map(|(key, chat)| {
                private_peer(key, name)
                    .filter(|_| self.allows(Some(name), &self.private_scope(key), Action::Read))
                    .map(|peer| {
                        (
                            format!("@direct:{peer}"),
                            self.unread(name, &format!("dm:{key}"), &chat.messages, chat.revision),
                        )
                    })
            }))
            .chain(
                self.data
                    .private
                    .iter()
                    .filter(|(key, _)| {
                        private_peer(key, name).is_none()
                            && self.allows(Some(name), &self.private_scope(key), Action::Read)
                    })
                    .map(|(key, chat)| {
                        (
                            format!("@private:{key}"),
                            self.unread(name, &format!("dm:{key}"), &chat.messages, chat.revision),
                        )
                    }),
            )
            .collect()
    }
    pub fn connect(&mut self, name: &str) {
        *self.online_connections.entry(name.into()).or_default() += 1;
    }
    pub fn disconnect(&mut self, name: &str) {
        if let Some(count) = self.online_connections.get_mut(name) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.online_connections.remove(name);
            }
        }
    }
    pub fn snapshot(&self, name: &str) -> Option<Snapshot> {
        let account = self.data.users.get(name).filter(|u| !u.disabled)?;
        let account_scope = Scope::Account(account.id.clone());
        Some(Snapshot {
            kind: "snapshot",
            username: name.into(),
            admin: self.is_admin(name),
            groups: self.groups(Some(name)),
            permissions: self.effective(Some(name), &Scope::Server).names(),
            account_access: Access {
                id: account.id.clone(),
                permissions: self.effective(Some(name), &account_scope).names(),
                commands: self
                    .commands_for(Some(name), None)
                    .into_iter()
                    .filter(|command| {
                        self.require_target_command(Some(name), &account_scope, command.name)
                            .is_ok()
                    })
                    .collect(),
            },
            private_access: self
                .data
                .private
                .iter()
                .filter(|(key, _)| self.allows(Some(name), &self.private_scope(key), Action::Read))
                .filter_map(|(key, chat)| {
                    private_peer(key, name).map(|peer| {
                        let view = format!("@direct:{peer}");
                        (
                            view.clone(),
                            Access {
                                id: chat.id.clone(),
                                permissions: self
                                    .effective(Some(name), &self.private_scope(key))
                                    .names(),
                                commands: self.commands_for(Some(name), Some(&view)),
                            },
                        )
                    })
                })
                .collect(),
            private_permissions: self
                .effective(Some(name), &self.private_scope(&private_key(name, name)))
                .names(),
            private_commands: self.commands_for(Some(name), Some(&format!("@direct:{name}"))),
            policy_revision: self.data.policy.revision,
            online: self
                .online_connections
                .keys()
                .filter(|online| {
                    self.active(online)
                        && self.allows(Some(name), &Scope::Server, Action::Directory)
                })
                .cloned()
                .collect(),
            users: self
                .data
                .users
                .iter()
                .filter(|(_, u)| {
                    self.allows(Some(name), &Scope::Server, Action::Directory)
                        && (self.is_admin(name) || !u.disabled)
                })
                .map(|(n, _)| n.clone())
                .collect(),
            rooms: self
                .data
                .rooms
                .iter()
                .filter(|(_, r)| self.allows(Some(name), &Scope::Room(r.id.clone()), Action::Read))
                .map(|(n, r)| RoomView {
                    id: r.id.clone(),
                    owner: self.room_owner_name(&r.owner_id).into(),
                    permissions: self
                        .effective(Some(name), &Scope::Room(r.id.clone()))
                        .names(),
                    commands: self.commands_for(Some(name), Some(n)),
                    name: n.clone(),
                    members: if self.allows(Some(name), &Scope::Room(r.id.clone()), Action::Members)
                    {
                        r.members.clone()
                    } else {
                        BTreeSet::new()
                    },
                    messages: r
                        .messages
                        .iter()
                        .skip(r.messages.len().saturating_sub(VISIBLE_HISTORY))
                        .cloned()
                        .collect(),
                })
                .chain(
                    self.data
                        .private
                        .iter()
                        .filter(|(key, _)| {
                            private_peer(key, name).is_none()
                                && self.allows(Some(name), &self.private_scope(key), Action::Read)
                        })
                        .map(|(key, chat)| {
                            let view = format!("@private:{key}");
                            let scope = self.private_scope(key);
                            RoomView {
                                id: chat.id.clone(),
                                name: view.clone(),
                                owner: "participants".into(),
                                permissions: self.effective(Some(name), &scope).names(),
                                commands: self.commands_for(Some(name), Some(&view)),
                                members: if self.allows(Some(name), &scope, Action::Members) {
                                    key.split(':').map(str::to_owned).collect()
                                } else {
                                    BTreeSet::new()
                                },
                                messages: chat
                                    .messages
                                    .iter()
                                    .skip(chat.messages.len().saturating_sub(VISIBLE_HISTORY))
                                    .cloned()
                                    .collect(),
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
                            && self.allows(Some(name), &self.private_scope(key), Action::Read)
                    })
                    .flat_map(|(_, chat)| {
                        chat.messages
                            .iter()
                            .skip(chat.messages.len().saturating_sub(VISIBLE_HISTORY))
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
                .filter(|key| self.allows(Some(name), &self.private_scope(key), Action::Read))
                .filter_map(|key| private_peer(key, name))
                .map(String::from)
                .collect(),
            unread: self.unreads(name),
            commands: self.commands_for(Some(name), None),
            available_rooms: self
                .data
                .rooms
                .iter()
                .filter(|(_, r)| {
                    self.allows(Some(name), &Scope::Room(r.id.clone()), Action::Discover)
                        || self.allows(Some(name), &Scope::Room(r.id.clone()), Action::Read)
                })
                .map(|(n, _)| n.clone())
                .collect(),
        })
    }
    pub(super) fn apply_queries(&mut self, context: &CommandContext<'_>) -> Result<String, String> {
        let CommandContext {
            actor,
            room,
            parts,
            author,
            ..
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
            "/rooms" => {
                require_len(parts, 1, "/rooms")?;
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
            "/users" => {
                require_len(parts, 1, "/users")?;
                self.require_target_command(actor, &Scope::Server, parts[0])?;
                self.require(actor, &Scope::Server, Action::Directory)?;
                Ok(format!(
                    "Users:\n{}",
                    self.data
                        .users
                        .iter()
                        .filter(|(_, u)| self.is_su(actor) || !u.disabled)
                        .map(|(n, u)| format!(
                            "{n} — {}{}",
                            self.permission_label(Some(n)),
                            if u.disabled { " (disabled)" } else { "" }
                        ))
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
                    .copied()
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
                if parts.len() > 3 || (room.is_some() && parts.len() > 2) {
                    return Err(
                        "Usage: /history [count] [user] (user is for private history)".into(),
                    );
                }
                let limit = self.max_messages;
                let count_error = format!("Count must be between 1 and {limit}.");
                let count = parts
                    .get(1)
                    .map(|n| n.parse::<usize>())
                    .transpose()
                    .map_err(|_| count_error.clone())?
                    .unwrap_or(VISIBLE_HISTORY.min(limit));
                if !(1..=limit).contains(&count) {
                    return Err(count_error);
                }
                let mut messages: Vec<_> = if let Some(name) = room {
                    let target = self.data.rooms.get(name).ok_or("Room not found.")?;
                    self.require_target_command(actor, &Scope::Room(target.id.clone()), parts[0])?;
                    self.require(actor, &Scope::Room(target.id.clone()), Action::Read)?;
                    target.messages.iter().collect()
                } else if let Some(key) = parts.get(2).and_then(|v| v.strip_prefix("@private:")) {
                    self.require_target_command(actor, &self.private_scope(key), parts[0])?;
                    self.require(actor, &self.private_scope(key), Action::Read)?;
                    self.data
                        .private
                        .get(key)
                        .ok_or("Private conversation not found.")?
                        .messages
                        .iter()
                        .collect()
                } else if parts.len() <= 2
                    && let Scope::Private(id) = &context.scope
                {
                    self.require(actor, &context.scope, Action::Read)?;
                    self.data
                        .private
                        .values()
                        .find(|chat| chat.id == *id)
                        .ok_or("Private conversation not found.")?
                        .messages
                        .iter()
                        .collect()
                } else if actor.is_some() {
                    self.data
                        .private
                        .iter()
                        .filter(|(key, _)| {
                            self.allows(actor, &self.private_scope(key), Action::Read)
                                && self
                                    .require_target_command(
                                        actor,
                                        &self.private_scope(key),
                                        parts[0],
                                    )
                                    .is_ok()
                                && private_peer(key, author).is_some_and(|peer| {
                                    parts.get(2).is_none_or(|requested| peer == *requested)
                                })
                        })
                        .flat_map(|(_, chat)| &chat.messages)
                        .collect()
                } else {
                    return Err("Specify a room via a web session to read history.".into());
                };
                messages.sort_by_key(|m| m.sequence);
                let text = messages
                    .iter()
                    .skip(messages.len().saturating_sub(count))
                    .map(|m| {
                        format!(
                            "{} {}{}: {}",
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
                    "No messages.".into()
                } else {
                    text
                })
            }
            _ => Err("Unknown command.".into()),
        }
    }
}
