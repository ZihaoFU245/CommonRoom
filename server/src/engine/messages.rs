use super::*;
use authorization::{Action, Scope};

pub(super) struct WorkingMessage {
    pub(super) conversation: String,
    pub(super) message: Message,
    pub(super) reaction: Option<ReactionChange>,
}

pub(super) struct ReactionChange {
    pub(super) user: String,
    pub(super) reaction: String,
    pub(super) active: bool,
}

pub(super) enum ReactionError {
    NotFound,
    Limit,
}
impl std::fmt::Display for ReactionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotFound => "Message not found in this conversation.",
            Self::Limit => "This message already has 32 different reactions.",
        })
    }
}

pub(super) fn reply_quote(original: &Message) -> Reply {
    Reply {
        id: original.id.clone(),
        from: original.from.clone(),
        text: original.text.chars().take(160).collect(),
        sequence: original.sequence,
    }
}

impl Engine {
    pub(super) fn working_message(&self, conversation: &str) -> Option<&Message> {
        self.working
            .as_ref()
            .filter(|row| row.conversation == conversation)
            .map(|row| &row.message)
    }

    fn advance_conversation(&mut self, scope: &Scope) -> Result<(), EngineError> {
        let id = match scope {
            Scope::Room(id) | Scope::Private(id) => id,
            _ => return Err(EngineError::NotFound("Conversation not found.".into())),
        };
        let view = self.bot_index.conversations.get(id);
        let revision = match scope {
            Scope::Room(_) => view
                .and_then(|view| self.data.rooms.get_mut(view))
                .filter(|r| r.id == *id)
                .map(|r| &mut r.revision),
            Scope::Private(_) => view
                .and_then(|view| view.strip_prefix("@private:"))
                .and_then(|key| self.data.private.get_mut(key))
                .filter(|c| c.id == *id)
                .map(|c| &mut c.revision),
            _ => None,
        }
        .ok_or_else(|| EngineError::NotFound("Conversation not found.".into()))?;
        *revision = revision.wrapping_add(1);
        Ok(())
    }

    // Callers resolve and authorize the target before using these mutation helpers.
    pub(super) fn append_text(
        &mut self,
        actor: Option<&str>,
        view: &str,
        text: &str,
        markdown: bool,
        reply: Option<Reply>,
    ) -> Result<(), EngineError> {
        let recipient = if let Some(key) = view.strip_prefix("@private:") {
            let (_, fallback) = key.split_once(':').ok_or("Invalid private conversation.")?;
            if !self.data.private.contains_key(key) {
                return Err(EngineError::NotFound(
                    "Private conversation not found.".into(),
                ));
            }
            Some(
                actor
                    .and_then(|name| private_peer(key, name))
                    .unwrap_or(fallback),
            )
        } else {
            None
        };
        let mut message = self.message(actor, recipient, text, markdown)?;
        message.reply = reply;
        if let Some(key) = view.strip_prefix("@private:") {
            message
                .mentions
                .retain(|name| key.split(':').any(|p| p == name));
            self.push_private_to(key, message)
        } else {
            self.push_room_message(view, message)
        }
    }
    fn push_room_message(&mut self, name: &str, mut message: Message) -> Result<(), EngineError> {
        self.authorize_media_text(
            Some(&message.from),
            &self.room_scope(name)?,
            &message.text,
            "",
        )?;
        let room = self
            .data
            .rooms
            .get_mut(name)
            .ok_or_else(|| EngineError::NotFound("Room not found.".into()))?;
        message.mentions.retain(|name| room.members.contains(name));
        self.working = Some(WorkingMessage {
            reaction: None,
            conversation: room.id.clone(),
            message,
        });
        room.revision = room.revision.wrapping_add(1);
        Ok(())
    }
    pub(super) fn require_message_action(
        &self,
        actor: Option<&str>,
        view: Option<&str>,
        scope: &Scope,
        command: &str,
        action: Action,
    ) -> Result<(), EngineError> {
        self.require_command(actor, view, command)?;
        self.require_target_command(actor, scope, command)?;
        self.require(actor, scope, Action::Read)?;
        self.require(actor, scope, action)
    }
    pub(super) fn update_reaction(
        &mut self,
        scope: &Scope,
        user: &str,
        message_id: &str,
        reaction: &str,
        active: Option<bool>,
    ) -> Result<bool, ReactionError> {
        let id = match scope {
            Scope::Room(id) | Scope::Private(id) => id,
            _ => return Err(ReactionError::NotFound),
        };
        let message = self
            .working
            .as_mut()
            .filter(|row| row.conversation == *id && row.message.id == message_id)
            .map(|row| &mut row.message)
            .ok_or(ReactionError::NotFound)?;
        let present = message
            .reactions
            .get(reaction)
            .is_some_and(|users| users.contains(user));
        let active = active.unwrap_or(!present);
        if present == active {
            return Ok(false);
        }
        if active && !message.reactions.contains_key(reaction) && message.reactions.len() >= 32 {
            return Err(ReactionError::Limit);
        }
        if active {
            message
                .reactions
                .entry(reaction.into())
                .or_default()
                .insert(user.into());
        } else if let Some(users) = message.reactions.get_mut(reaction) {
            users.remove(user);
            if users.is_empty() {
                message.reactions.remove(reaction);
            }
        }
        let row = self.working.as_mut().ok_or(ReactionError::NotFound)?;
        row.reaction = Some(ReactionChange {
            user: user.into(),
            reaction: reaction.into(),
            active,
        });
        self.advance_conversation(scope)
            .map_err(|_| ReactionError::NotFound)?;
        Ok(true)
    }
    // Resolve only the selected room, or a retained target for global su actions.
    pub(super) fn message_action_room(
        &self,
        actor: Option<&str>,
        room: Option<&str>,
        scope: &Scope,
        id: &str,
    ) -> Option<String> {
        room.map(str::to_owned).or_else(|| {
            if !self.is_su(actor) || *scope != Scope::Server {
                return None;
            }
            let row = self.working.as_ref().filter(|row| row.message.id == id)?;
            self.bot_index
                .conversations
                .get(&row.conversation)
                .filter(|view| !view.starts_with("@private:"))
                .cloned()
        })
    }

    fn message_action_scope(
        &self,
        actor: Option<&str>,
        room: Option<&str>,
        context: &Scope,
        id: &str,
        search_rooms: bool,
    ) -> Result<(Option<String>, Scope), EngineError> {
        let room = if search_rooms {
            self.message_action_room(actor, room, context, id)
        } else {
            room.map(str::to_owned)
        };
        let scope = if let Some(name) = &room {
            self.room_scope(name)?
        } else {
            let row = self
                .working
                .as_ref()
                .filter(|row| row.message.id == id)
                .ok_or("Message not found in this conversation.")?;
            let key = self
                .bot_index
                .conversations
                .get(&row.conversation)
                .and_then(|view| view.strip_prefix("@private:"))
                .filter(|key| {
                    self.allows(actor, &self.private_scope(key), Action::Read)
                        && !matches!(context, Scope::Private(id) if *id != row.conversation)
                })
                .ok_or("Message not found in this conversation.")?;
            self.private_scope(key)
        };
        Ok((room, scope))
    }
    pub(super) fn message(
        &mut self,
        actor: Option<&str>,
        to: Option<&str>,
        text: &str,
        markdown: bool,
    ) -> Result<Message, EngineError> {
        if !has_message_content(text) {
            return Err("Messages cannot be blank.".into());
        }
        self.data.next_sequence = self
            .data
            .next_sequence
            .checked_add(1)
            .filter(|n| *n <= 9_007_199_254_740_991)
            .ok_or("Message sequence exhausted.")?;
        let mut message = new_message(actor.unwrap_or("console"), to, text);
        message.markdown = markdown;
        message.author_id = actor
            .and_then(|name| self.data.users.get(name))
            .map_or_else(|| "console".into(), |u| u.id.clone());
        message.sequence = self.data.next_sequence;
        Ok(message)
    }
    pub(super) fn push_private(&mut self, message: Message) -> Result<(), EngineError> {
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
    ) -> Result<(), EngineError> {
        let Scope::Private(id) = self.private_scope(key) else {
            return Err("Invalid private scope.".into());
        };
        self.authorize_media_text(
            Some(&message.from),
            &Scope::Private(id.clone()),
            &message.text,
            "",
        )?;
        message.private_id.clone_from(&id);
        let chat = self
            .data
            .private
            .entry(key.into())
            .or_insert_with(|| PrivateChat {
                id,
                ..PrivateChat::default()
            });
        self.working = Some(WorkingMessage {
            reaction: None,
            conversation: chat.id.clone(),
            message,
        });
        chat.revision = chat.revision.wrapping_add(1);
        Ok(())
    }
    pub(super) fn apply_messages(
        &mut self,
        context: &CommandContext<'_>,
    ) -> Result<String, EngineError> {
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
                let (_, scope) =
                    self.message_action_scope(actor, room, &context.scope, id, true)?;
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
                let conversation_id = match &scope {
                    Scope::Room(id) | Scope::Private(id) => id.clone(),
                    _ => return Err("Invalid message scope.".into()),
                };
                let original = self
                    .working_message(&conversation_id)
                    .filter(|m| m.id == id)
                    .ok_or("Message not found in this conversation (it may have expired).")?;
                if !can_delete_any && original.author_id != actor_id {
                    return Err(EngineError::Forbidden(
                        "You can only delete your own messages.".into(),
                    ));
                }
                let deleted_sequence = original.sequence;
                self.store
                    .pending_deletes
                    .push((conversation_id, deleted_sequence));
                self.working = None;
                self.advance_conversation(&scope)?;
                self.audit(actor, "/retract", id);
                Ok("Message deleted.".into())
            }
            "/react" | "/reply" | "/edit" => {
                let user = if parts[0] == "/edit" {
                    actor.unwrap_or("console")
                } else {
                    actor.ok_or("Message actions require a user account.")?
                };
                let content = input
                    .strip_prefix(parts[0])
                    .ok_or("Invalid command.")?
                    .trim_start();
                let (id, value) = content
                    .split_once(char::is_whitespace)
                    .ok_or("Usage: /react message-id reaction, /reply message-id message or /edit message-id message")?;
                let value = value.trim();
                if value.is_empty() {
                    return Err("A reaction, reply or edit cannot be empty.".into());
                }
                if parts[0] != "/react" && !has_message_content(value) {
                    return Err("Messages cannot be blank.".into());
                }
                let (target_room, scope) = self.message_action_scope(
                    actor,
                    room,
                    &context.scope,
                    id,
                    parts[0] == "/edit",
                )?;
                let room = target_room.as_deref();
                self.require_message_action(
                    actor,
                    context.view,
                    &scope,
                    parts[0],
                    if parts[0] == "/react" {
                        Action::React
                    } else if parts[0] == "/edit" {
                        if self.allows(actor, &scope, Action::EditAny) {
                            Action::EditAny
                        } else {
                            Action::EditOwn
                        }
                    } else {
                        Action::Send
                    },
                )?;
                let conversation_id = match &scope {
                    Scope::Room(id) | Scope::Private(id) => id,
                    _ => return Err("Invalid message scope.".into()),
                };
                if parts[0] == "/react" {
                    if value.chars().count() > crate::limits::MAX_REACTION_CHARS
                        || value.chars().any(char::is_control)
                    {
                        return Err(
                            "Reactions support 1–128 characters without control characters.".into(),
                        );
                    }
                    self.update_reaction(&scope, user, id, value, None)
                        .map_err(EngineError::unavailable)?;
                    return Ok("Reaction updated.".into());
                }
                let original = self
                    .working_message(conversation_id)
                    .filter(|m| m.id == id)
                    .cloned()
                    .ok_or("Message not found in this conversation (it may have expired).")?;
                if value.chars().count() > crate::limits::MAX_MESSAGE_CHARS {
                    return Err("Messages support at most 4000 characters.".into());
                }
                if parts[0] == "/edit" {
                    let actor_id = self
                        .principal_id(actor)
                        .ok_or_else(|| EngineError::Forbidden("Account unavailable.".into()))?;
                    if original.author_id != actor_id
                        && !self.allows(actor, &scope, Action::EditAny)
                    {
                        return Err(EngineError::Forbidden(
                            "You can only edit your own messages.".into(),
                        ));
                    }
                    self.authorize_media_text(actor, &scope, value, &original.text)?;
                    if original.text == value {
                        return Ok("Message unchanged.".into());
                    }
                    let mut mentions = mentioned_names(value);
                    if let Some(name) = room {
                        let target = self
                            .data
                            .rooms
                            .get(name)
                            .ok_or_else(|| EngineError::NotFound("Room not found.".into()))?;
                        mentions.retain(|name| target.members.contains(name));
                    } else {
                        mentions.retain(|name| {
                            name == &original.from || original.to.as_ref() == Some(name)
                        });
                    }
                    let mut message = original;
                    message.text = value.into();
                    message.mentions = mentions;
                    message.edited = true;
                    self.working = Some(WorkingMessage {
                        reaction: None,
                        conversation: conversation_id.clone(),
                        message,
                    });
                    self.advance_conversation(&scope)?;
                    return Ok("Message edited.".into());
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
                        return Err(EngineError::NotFound("User not found.".into()));
                    }
                    Some(peer)
                } else {
                    None
                };
                let mut message = self.message(actor, recipient, value, context.markdown)?;
                message.reply = Some(reply_quote(&original));
                if let Some(name) = room {
                    self.push_room_message(name, message)?;
                } else {
                    message
                        .mentions
                        .retain(|name| name == user || Some(name.as_str()) == recipient);
                    let key = self
                        .bot_index
                        .conversations
                        .get(conversation_id)
                        .and_then(|view| view.strip_prefix("@private:"))
                        .ok_or_else(|| {
                            EngineError::NotFound("Private conversation not found.".into())
                        })?
                        .to_owned();
                    self.push_private_to(&key, message)?;
                }
                Ok(String::new())
            }
            "/clean" => self.apply_cleanup(context),
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
                if text.chars().count() > crate::limits::MAX_MESSAGE_CHARS {
                    return Err("Messages support at most 4000 characters.".into());
                }
                if !self.active(recipient) {
                    return Err(EngineError::NotFound("User not found.".into()));
                }
                let key = private_key(author, recipient);
                self.require_target_command(actor, &self.private_scope(&key), parts[0])?;
                if !self.data.private.contains_key(&key) {
                    self.require(actor, &Scope::Server, Action::Direct)?;
                }
                self.require(actor, &self.private_scope(&key), Action::Send)?;
                let mut message = self.message(actor, Some(recipient), text, context.markdown)?;
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
