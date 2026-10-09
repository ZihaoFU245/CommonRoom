use super::*;

impl Engine {
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
        if let Some(peer) = view.strip_prefix("@direct:") {
            let key = private_key(user, peer);
            let chat = self
                .data
                .private
                .get(&key)
                .ok_or("Conversation not found.")?;
            return Ok((format!("dm:{key}"), &chat.messages, chat.revision));
        }
        let room = self
            .data
            .rooms
            .get(view)
            .filter(|r| r.members.contains(user))
            .ok_or("You are not a member of this room.")?;
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
            .filter(|(_, room)| room.members.contains(name))
            .map(|(view, room)| {
                (
                    view.clone(),
                    self.unread(name, &format!("room:{view}"), &room.messages, room.revision),
                )
            })
            .chain(self.data.private.iter().filter_map(|(key, chat)| {
                private_peer(key, name).map(|peer| {
                    (
                        format!("@direct:{peer}"),
                        self.unread(name, &format!("dm:{key}"), &chat.messages, chat.revision),
                    )
                })
            }))
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
        let user = self.data.users.get(name).filter(|u| !u.disabled)?;
        Some(Snapshot {
            kind: "snapshot",
            username: name.into(),
            admin: user.admin,
            online: self
                .online_connections
                .keys()
                .filter(|name| self.active(name))
                .cloned()
                .collect(),
            users: self
                .data
                .users
                .iter()
                .filter(|(_, u)| user.admin || !u.disabled)
                .map(|(n, _)| n.clone())
                .collect(),
            rooms: self
                .data
                .rooms
                .iter()
                .filter(|(_, r)| r.members.contains(name))
                .map(|(n, r)| RoomView {
                    name: n.clone(),
                    members: r.members.clone(),
                    messages: r
                        .messages
                        .iter()
                        .skip(r.messages.len().saturating_sub(VISIBLE_HISTORY))
                        .cloned()
                        .collect(),
                })
                .collect(),
            direct: {
                let mut messages: Vec<_> = self
                    .data
                    .private
                    .iter()
                    .filter(|(key, _)| private_peer(key, name).is_some())
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
                .filter_map(|key| private_peer(key, name))
                .map(String::from)
                .collect(),
            unread: self.unreads(name),
            commands: crate::commands::available(user.admin, false),
            available_rooms: self
                .data
                .rooms
                .iter()
                .filter(|(_, r)| user.admin || r.members.contains(name))
                .map(|(n, _)| n.clone())
                .collect(),
        })
    }
    pub(super) fn apply_queries(&mut self, context: &CommandContext<'_>) -> Result<String, String> {
        let CommandContext {
            actor,
            room,
            parts,
            admin,
            author,
            ..
        } = *context;
        match parts[0] {
            "/help" => {
                require_len(parts, 1, "/help")?;
                Ok(crate::commands::help(admin, actor.is_none()))
            }
            "/whoami" => {
                require_len(parts, 1, "/whoami")?;
                Ok(format!(
                    "Name: {author}\nPermission: {}",
                    if actor.is_none() {
                        "superuser"
                    } else if admin {
                        "admin"
                    } else {
                        "user"
                    }
                ))
            }
            "/rooms" => {
                require_len(parts, 1, "/rooms")?;
                Ok(format!(
                    "Your rooms: {}",
                    self.data
                        .rooms
                        .iter()
                        .filter(|(_, r)| admin || r.members.contains(author))
                        .map(|(n, _)| format!("#{n}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                ))
            }
            "/users" => {
                require_len(parts, 1, "/users")?;
                Ok(format!(
                    "Users:\n{}",
                    self.data
                        .users
                        .iter()
                        .filter(|(_, u)| admin || !u.disabled)
                        .map(|(n, u)| format!(
                            "{n} — {}{}",
                            if u.admin { "admin" } else { "user" },
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
                let name = parts
                    .get(1)
                    .copied()
                    .or(room)
                    .ok_or("Select or specify a room.")?;
                let target = self.data.rooms.get(name).ok_or("Room not found.")?;
                if actor.is_some() && !target.members.contains(author) {
                    return Err("You are not a member of this room.".into());
                }
                Ok(target
                    .members
                    .iter()
                    .map(|n| {
                        format!(
                            "{n} — {}",
                            if self.data.users[n].admin {
                                "admin"
                            } else {
                                "user"
                            }
                        )
                    })
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
                    if actor.is_some() && !target.members.contains(author) {
                        return Err("You are not a member of this room.".into());
                    }
                    target.messages.iter().collect()
                } else if actor.is_some() {
                    self.data
                        .private
                        .iter()
                        .filter(|(key, _)| {
                            private_peer(key, author).is_some_and(|peer| {
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
