use super::*;

impl Engine {
    pub(super) fn message(
        &mut self,
        from: &str,
        to: Option<&str>,
        text: &str,
    ) -> Result<Message, String> {
        self.data.next_sequence = self
            .data
            .next_sequence
            .checked_add(1)
            .filter(|n| *n <= i64::MAX as u64)
            .ok_or("Message sequence exhausted.")?;
        let mut message = new_message(from, to, text);
        message.sequence = self.data.next_sequence;
        Ok(message)
    }
    pub(super) fn push_private(&mut self, message: Message) {
        let key = private_key(&message.from, message.to.as_deref().unwrap());
        let chat = self.data.private.entry(key).or_default();
        if chat.messages.len() == self.max_messages {
            chat.messages.pop_front();
        }
        chat.messages.push_back(message);
        chat.revision = chat.revision.wrapping_add(1);
    }
    pub(super) fn apply_messages(
        &mut self,
        context: &CommandContext<'_>,
    ) -> Result<String, String> {
        let CommandContext {
            actor,
            room,
            input,
            parts,
            admin,
            author,
            ..
        } = *context;
        match parts[0] {
            "/react" | "/reply" => {
                let user = actor.ok_or("Message actions require a user account.")?;
                let content = input.strip_prefix(parts[0]).unwrap().trim_start();
                let (id, value) = content
                    .split_once(char::is_whitespace)
                    .ok_or("Usage: /react message-id reaction or /reply message-id message")?;
                let value = value.trim();
                if value.is_empty() {
                    return Err("A reaction or reply cannot be empty.".into());
                }
                let original = if let Some(name) = room {
                    let target = self.data.rooms.get(name).ok_or("Room not found.")?;
                    if !target.members.contains(user) {
                        return Err("You are not a member of this room.".into());
                    }
                    target.messages.iter().find(|m| m.id == id)
                } else {
                    self.data
                        .private
                        .iter()
                        .filter(|(key, _)| private_peer(key, user).is_some())
                        .flat_map(|(_, chat)| &chat.messages)
                        .find(|m| m.id == id)
                }
                .cloned()
                .ok_or("Message not found in this conversation (it may have expired).")?;
                if parts[0] == "/react" {
                    if value.chars().count() > 16 || value.chars().any(char::is_control) {
                        return Err(
                            "Reactions support 1–16 characters without control characters.".into(),
                        );
                    }
                    let target = if let Some(name) = room {
                        self.data
                            .rooms
                            .get_mut(name)
                            .unwrap()
                            .messages
                            .iter_mut()
                            .find(|m| m.id == id)
                    } else {
                        self.data
                            .private
                            .get_mut(&private_key(
                                &original.from,
                                original.to.as_deref().unwrap(),
                            ))
                            .unwrap()
                            .messages
                            .iter_mut()
                            .find(|m| m.id == id)
                    }
                    .unwrap();
                    if !target.reactions.contains_key(value) && target.reactions.len() >= 32 {
                        return Err("This message already has 32 different reactions.".into());
                    }
                    let users = target.reactions.entry(value.into()).or_default();
                    if !users.insert(user.into()) {
                        users.remove(user);
                    }
                    if users.is_empty() {
                        target.reactions.remove(value);
                    }
                    if let Some(name) = room {
                        let room = self.data.rooms.get_mut(name).unwrap();
                        room.revision = room.revision.wrapping_add(1);
                    } else {
                        let chat = self
                            .data
                            .private
                            .get_mut(&private_key(
                                &original.from,
                                original.to.as_deref().unwrap(),
                            ))
                            .unwrap();
                        chat.revision = chat.revision.wrapping_add(1);
                    }
                    return Ok("Reaction updated.".into());
                }
                if value.chars().count() > 4000 {
                    return Err("Messages support at most 4000 characters.".into());
                }
                let recipient = if room.is_none() {
                    let peer = if original.from == user {
                        original.to.as_deref().unwrap()
                    } else {
                        &original.from
                    };
                    if !self.active(peer) {
                        return Err("User not found.".into());
                    }
                    Some(peer)
                } else {
                    None
                };
                let mut message = self.message(user, recipient, value)?;
                message.reply = Some(Reply {
                    id: original.id.clone(),
                    from: original.from.clone(),
                    text: original.text.chars().take(160).collect(),
                });
                if let Some(name) = room {
                    let target = self.data.rooms.get_mut(name).unwrap();
                    message
                        .mentions
                        .retain(|name| target.members.contains(name));
                    if target.messages.len() == self.max_messages {
                        target.messages.pop_front();
                    }
                    target.messages.push_back(message);
                    target.revision = target.revision.wrapping_add(1);
                } else {
                    message
                        .mentions
                        .retain(|name| name == user || Some(name.as_str()) == recipient);
                    self.push_private(message);
                }
                Ok(String::new())
            }
            "/clean" => {
                require_admin(admin)?;
                if !(2..=3).contains(&parts.len()) {
                    return Err(
                        "Usage: /clean age [room|@private|@all], e.g. /clean 7d @all".into(),
                    );
                }
                let age = parse_age(parts[1])?;
                let cutoff = now()
                    .checked_sub(age)
                    .ok_or("Age is older than the clock permits.")?;
                let scope = parts.get(2).copied().or(room).unwrap_or("@all");
                let mut removed = 0;
                if scope == "@all" || scope == "@private" {
                    for chat in self.data.private.values_mut() {
                        let before = chat.messages.len();
                        chat.messages.retain(|m| m.time >= cutoff);
                        let deleted = before - chat.messages.len();
                        if deleted > 0 {
                            chat.revision = chat.revision.wrapping_add(1);
                        }
                        removed += deleted;
                    }
                }
                if scope == "@all" {
                    for target in self.data.rooms.values_mut() {
                        let before = target.messages.len();
                        target.messages.retain(|m| m.time >= cutoff);
                        let deleted = before - target.messages.len();
                        if deleted > 0 {
                            target.revision = target.revision.wrapping_add(1);
                        }
                        removed += deleted;
                    }
                } else if scope != "@private" {
                    let target = self.data.rooms.get_mut(scope).ok_or("Room not found.")?;
                    let before = target.messages.len();
                    target.messages.retain(|m| m.time >= cutoff);
                    let deleted = before - target.messages.len();
                    if deleted > 0 {
                        target.revision = target.revision.wrapping_add(1);
                    }
                    removed += deleted;
                }
                Ok(format!(
                    "Deleted {removed} messages older than {} from {scope}.",
                    parts[1]
                ))
            }
            "/tell" => {
                if actor.is_none() {
                    return Err("Direct messages require a user account.".into());
                }
                let content = input.strip_prefix("/tell").unwrap().trim_start();
                let (recipient, text) = content
                    .split_once(char::is_whitespace)
                    .ok_or("Usage: /tell user message")?;
                let text = text.trim();
                if text.is_empty() {
                    return Err("Usage: /tell user message".into());
                }
                if text.chars().count() > 4000 {
                    return Err("Messages support at most 4000 characters.".into());
                }
                if !self.active(recipient) {
                    return Err("User not found.".into());
                }
                let mut message = self.message(author, Some(recipient), text)?;
                message
                    .mentions
                    .retain(|name| name == author || name == recipient);
                self.push_private(message);
                Ok(format!("Private message sent to {recipient}."))
            }
            _ => unreachable!("Dispatcher selected the wrong command domain"),
        }
    }
}
