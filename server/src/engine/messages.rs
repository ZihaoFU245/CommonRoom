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
    pub(super) fn push_private(&mut self, message: Message) -> Result<(), String> {
        let key = private_key(
            &message.from,
            message
                .to
                .as_deref()
                .ok_or("Private message has no recipient.")?,
        );
        let chat = self.data.private.entry(key).or_default();
        if chat.messages.len() == self.max_messages {
            chat.messages.pop_front();
        }
        chat.messages.push_back(message);
        chat.revision = chat.revision.wrapping_add(1);
        Ok(())
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
            "/retract" => {
                require_len(parts, 2, "/retract message-id")?;
                let id = parts[1];
                let room = room.map(str::to_owned).or_else(|| {
                    actor
                        .is_none()
                        .then(|| {
                            self.data
                                .rooms
                                .iter()
                                .find(|(_, target)| target.messages.iter().any(|m| m.id == id))
                                .map(|(name, _)| name.clone())
                        })
                        .flatten()
                });
                let (messages, revision) = if let Some(name) = room.as_deref() {
                    let target = self.data.rooms.get_mut(name).ok_or("Room not found.")?;
                    if actor.is_some_and(|user| !target.members.contains(user)) {
                        return Err("You are not a member of this room.".into());
                    }
                    (&mut target.messages, &mut target.revision)
                } else {
                    let chat = self
                        .data
                        .private
                        .iter_mut()
                        .filter(|(key, _)| {
                            actor.is_none_or(|user| private_peer(key, user).is_some())
                        })
                        .map(|(_, chat)| chat)
                        .find(|chat| chat.messages.iter().any(|m| m.id == id))
                        .ok_or("Message not found in this conversation.")?;
                    (&mut chat.messages, &mut chat.revision)
                };
                let original = messages
                    .iter()
                    .find(|m| m.id == id)
                    .ok_or("Message not found in this conversation (it may have expired).")?;
                if actor.is_some_and(|user| original.from != user) {
                    return Err("You can only delete your own messages.".into());
                }
                messages.retain(|m| m.id != id);
                for message in messages {
                    if message.reply.as_ref().is_some_and(|quote| quote.id == id) {
                        message.reply = None;
                    }
                }
                *revision = revision.wrapping_add(1);
                Ok("Message deleted.".into())
            }
            "/react" | "/reply" => {
                let user = actor.ok_or("Message actions require a user account.")?;
                let content = input
                    .strip_prefix(parts[0])
                    .ok_or("Invalid command.")?
                    .trim_start();
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
                    if value.chars().count() > 128 || value.chars().any(char::is_control) {
                        return Err(
                            "Reactions support 1–128 characters without control characters.".into(),
                        );
                    }
                    let target = if let Some(name) = room {
                        self.data
                            .rooms
                            .get_mut(name)
                            .ok_or("Room not found.")?
                            .messages
                            .iter_mut()
                            .find(|m| m.id == id)
                    } else {
                        self.data
                            .private
                            .get_mut(&private_key(
                                &original.from,
                                original
                                    .to
                                    .as_deref()
                                    .ok_or("Private message has no recipient.")?,
                            ))
                            .ok_or("Private conversation not found.")?
                            .messages
                            .iter_mut()
                            .find(|m| m.id == id)
                    }
                    .ok_or("Message not found in this conversation.")?;
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
                        let room = self.data.rooms.get_mut(name).ok_or("Room not found.")?;
                        room.revision = room.revision.wrapping_add(1);
                    } else {
                        let chat = self
                            .data
                            .private
                            .get_mut(&private_key(
                                &original.from,
                                original
                                    .to
                                    .as_deref()
                                    .ok_or("Private message has no recipient.")?,
                            ))
                            .ok_or("Message not found in this conversation.")?;
                        chat.revision = chat.revision.wrapping_add(1);
                    }
                    return Ok("Reaction updated.".into());
                }
                if value.chars().count() > 4000 {
                    return Err("Messages support at most 4000 characters.".into());
                }
                let recipient = if room.is_none() {
                    let peer = if original.from == user {
                        original
                            .to
                            .as_deref()
                            .ok_or("Private message has no recipient.")?
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
                    let target = self.data.rooms.get_mut(name).ok_or("Room not found.")?;
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
                    self.push_private(message)?;
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
                let content = input
                    .strip_prefix("/tell")
                    .ok_or("Invalid command.")?
                    .trim_start();
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
                self.push_private(message)?;
                Ok(format!("Private message sent to {recipient}."))
            }
            _ => Err("Unknown command.".into()),
        }
    }
}
