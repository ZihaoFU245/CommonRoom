use super::*;
use authorization::{Action, Scope};

impl Engine {
    pub(super) fn message(
        &mut self,
        actor: Option<&str>,
        to: Option<&str>,
        text: &str,
    ) -> Result<Message, String> {
        self.data.next_sequence = self
            .data
            .next_sequence
            .checked_add(1)
            .filter(|n| *n <= i64::MAX as u64)
            .ok_or("Message sequence exhausted.")?;
        let mut message = new_message(actor.unwrap_or("console"), to, text);
        message.author_id = actor
            .and_then(|name| self.data.users.get(name))
            .map_or_else(|| "console".into(), |u| u.id.clone());
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
        self.push_private_to(&key, message)
    }
    pub(super) fn push_private_to(
        &mut self,
        key: &str,
        mut message: Message,
    ) -> Result<(), String> {
        let Scope::Private(id) = self.private_scope(key) else {
            return Err("Invalid private scope.".into());
        };
        message.private_id.clone_from(&id);
        let chat = self
            .data
            .private
            .entry(key.into())
            .or_insert_with(|| PrivateChat {
                id,
                ..PrivateChat::default()
            });
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
            author,
            ..
        } = *context;
        match parts[0] {
            "/retract" => {
                require_len(parts, 2, "/retract message-id")?;
                let id = parts[1];
                let room = room.map(str::to_owned).or_else(|| {
                    (self.is_su(actor) && context.scope == Scope::Server)
                        .then(|| {
                            self.data
                                .rooms
                                .iter()
                                .find(|(_, target)| target.messages.iter().any(|m| m.id == id))
                                .map(|(name, _)| name.clone())
                        })
                        .flatten()
                });
                let scope = if let Some(name) = &room {
                    self.room_scope(name)?
                } else {
                    let key = self
                        .data
                        .private
                        .iter()
                        .find(|(key, chat)| {
                            self.allows(actor, &self.private_scope(key), Action::Read)
                                && (!matches!(&context.scope, Scope::Private(id) if *id != chat.id))
                                && chat.messages.iter().any(|m| m.id == id)
                        })
                        .map(|(key, _)| key.clone())
                        .ok_or("Message not found in this conversation.")?;
                    self.private_scope(&key)
                };
                self.require_target_command(actor, &scope, parts[0])?;
                self.require(actor, &scope, Action::Read)?;
                let can_delete_any = self.allows(actor, &scope, Action::RetractAny);
                if !can_delete_any {
                    self.require(actor, &scope, Action::RetractOwn)?;
                }
                let actor_id = actor
                    .and_then(|name| self.data.users.get(name))
                    .map_or("console", |u| u.id.as_str())
                    .to_owned();
                let (messages, revision) = match &scope {
                    Scope::Room(_) => {
                        let target = self
                            .data
                            .rooms
                            .get_mut(room.as_deref().ok_or("Room not found.")?)
                            .ok_or("Room not found.")?;
                        (&mut target.messages, &mut target.revision)
                    }
                    Scope::Private(id) => {
                        let chat = self
                            .data
                            .private
                            .values_mut()
                            .find(|chat| chat.id == *id)
                            .ok_or("Private conversation not found.")?;
                        (&mut chat.messages, &mut chat.revision)
                    }
                    _ => return Err("Invalid message scope.".into()),
                };
                let original = messages
                    .iter()
                    .find(|m| m.id == id)
                    .ok_or("Message not found in this conversation (it may have expired).")?;
                if !can_delete_any && original.author_id != actor_id {
                    return Err("You can only delete your own messages.".into());
                }
                messages.retain(|m| m.id != id);
                for message in messages {
                    if message.reply.as_ref().is_some_and(|quote| quote.id == id) {
                        message.reply = None;
                    }
                }
                *revision = revision.wrapping_add(1);
                self.audit(actor, "/retract", id);
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
                let scope = if let Some(name) = room {
                    self.room_scope(name)?
                } else {
                    self.private_scope(
                        &self
                            .data
                            .private
                            .iter()
                            .find(|(key, chat)| {
                                self.allows(actor, &self.private_scope(key), Action::Read)
                                    && (!matches!(&context.scope, Scope::Private(id) if *id != chat.id))
                                    && chat.messages.iter().any(|m| m.id == id)
                            })
                            .map(|(key, _)| key.clone())
                            .ok_or("Message not found in this conversation.")?,
                    )
                };
                self.require_target_command(actor, &scope, parts[0])?;
                self.require(actor, &scope, Action::Read)?;
                self.require(
                    actor,
                    &scope,
                    if parts[0] == "/react" {
                        Action::React
                    } else {
                        Action::Send
                    },
                )?;
                let original = if let Some(name) = room {
                    let target = self.data.rooms.get(name).ok_or("Room not found.")?;
                    target.messages.iter().find(|m| m.id == id)
                } else {
                    self.data
                        .private
                        .iter()
                        .filter(|(key, chat)| {
                            self.allows(actor, &self.private_scope(key), Action::Read)
                                && (!matches!(&context.scope, Scope::Private(id) if *id != chat.id))
                        })
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
                            .values_mut()
                            .find(|chat| matches!(&scope, Scope::Private(id) if chat.id == *id))
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
                            .values_mut()
                            .find(|chat| matches!(&scope, Scope::Private(id) if chat.id == *id))
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
                let mut message = self.message(actor, recipient, value)?;
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
                    let key = self
                        .data
                        .private
                        .iter()
                        .find(|(_, chat)| Scope::Private(chat.id.clone()) == scope)
                        .map(|(key, _)| key.clone())
                        .ok_or("Private conversation not found.")?;
                    self.push_private_to(&key, message)?;
                }
                Ok(String::new())
            }
            "/clean" => {
                if !(2..=3).contains(&parts.len()) {
                    return Err(
                        "Usage: /clean age [room|@private|@all], e.g. /clean 7d @all".into(),
                    );
                }
                let age = parse_age(parts[1])?;
                let cutoff = now()
                    .checked_sub(age)
                    .ok_or("Age is older than the clock permits.")?;
                let scope = parts.get(2).copied().or(context.view).unwrap_or("@all");
                // Resolve and authorize every affected conversation before mutating any.
                let scopes: Vec<_> = if scope == "@all" || scope == "@private" {
                    self.data
                        .private
                        .keys()
                        .map(|key| self.private_scope(key))
                        .chain(
                            self.data
                                .rooms
                                .values()
                                .filter(|_| scope == "@all")
                                .map(|r| Scope::Room(r.id.clone())),
                        )
                        .collect()
                } else if let Some(key) = scope.strip_prefix("@private:") {
                    if !self.data.private.contains_key(key) {
                        return Err("Private conversation not found.".into());
                    }
                    vec![self.private_scope(key)]
                } else if let Some(peer) = scope.strip_prefix("@direct:") {
                    let key = private_key(author, peer);
                    vec![self.private_scope(&key)]
                } else {
                    vec![self.room_scope(scope)?]
                };
                if scopes.is_empty() {
                    self.require_clean(actor, &Scope::Server, age)?;
                }
                for target in &scopes {
                    self.require_target_command(actor, target, parts[0])?;
                    self.require_clean(actor, target, age)?;
                }
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
                } else if scope.starts_with("@private:") || scope.starts_with("@direct:") {
                    let key = if let Some(key) = scope.strip_prefix("@private:") {
                        key.to_owned()
                    } else {
                        private_key(
                            author,
                            scope.strip_prefix("@direct:").ok_or("Invalid scope.")?,
                        )
                    };
                    let chat = self
                        .data
                        .private
                        .get_mut(&key)
                        .ok_or("Private conversation not found.")?;
                    let before = chat.messages.len();
                    chat.messages.retain(|m| m.time >= cutoff);
                    let deleted = before - chat.messages.len();
                    if deleted > 0 {
                        chat.revision = chat.revision.wrapping_add(1);
                    }
                    removed += deleted;
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
                self.audit(actor, "/clean", scope);
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
                let key = private_key(author, recipient);
                self.require_target_command(actor, &self.private_scope(&key), parts[0])?;
                if !self.data.private.contains_key(&key) {
                    self.require(actor, &Scope::Server, Action::Direct)?;
                }
                self.require(actor, &self.private_scope(&key), Action::Send)?;
                let mut message = self.message(actor, Some(recipient), text)?;
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
