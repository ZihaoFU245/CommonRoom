use super::*;
use crate::commands;

pub fn sudo_command(input: &str) -> Result<Option<&str>, String> {
    let input = input.trim();
    if input.split_whitespace().next() != Some("/sudo") {
        return Ok(None);
    }
    let command = input
        .strip_prefix("/sudo")
        .ok_or("Invalid command.")?
        .trim_start();
    if input.chars().count() > 4100 || !command.starts_with('/') || command == "/" {
        return Err("Usage: /sudo /command [arguments]".into());
    }
    if command.split_whitespace().next() == Some("/sudo") {
        return Err("Nested /sudo commands are not supported.".into());
    }
    Ok(Some(command))
}

pub(super) struct CommandContext<'a> {
    pub(super) actor: Option<&'a str>,
    pub(super) room: Option<&'a str>,
    pub(super) input: &'a str,
    pub(super) parts: &'a [&'a str],
    pub(super) view: Option<&'a str>,
    pub(super) scope: authorization::Scope,
    pub(super) author: &'a str,
}
impl Engine {
    pub fn execute(
        &mut self,
        actor: Option<&str>,
        room: Option<&str>,
        input: &str,
    ) -> Result<String, String> {
        if actor.is_some_and(|user| !self.active(user)) {
            return Err("Account unavailable.".into());
        }
        self.changed_conversation = None;
        let result = self.execute_inner(actor, room, input);
        // Working messages are transient. Durable history and bounded pages live in storage.
        for room in self.data.rooms.values_mut() {
            room.messages.clear();
        }
        for chat in self.data.private.values_mut() {
            chat.messages.clear();
        }
        self.pending_deletes.clear();
        self.pending_cleanups.clear();
        result
    }
    fn load_command_message(
        &mut self,
        actor: Option<&str>,
        view: Option<&str>,
        parts: &[&str],
    ) -> Result<(), String> {
        if !matches!(
            parts.first().copied(),
            Some("/react" | "/reply" | "/retract")
        ) {
            return Ok(());
        }
        let Some(message_id) = parts.get(1) else {
            return Ok(());
        };
        self.require_command(actor, view, parts[0])?;
        let scope = self.command_scope(actor, view)?;
        let Some((id, message)) = self.history_store.locate(message_id)? else {
            return Ok(());
        };
        if let Some(name) = view.filter(|v| !v.starts_with('@')) {
            let target = self.room_scope(name)?;
            self.require(actor, &target, authorization::Action::Read)?;
            if target == authorization::Scope::Room(id) {
                self.data
                    .rooms
                    .get_mut(name)
                    .ok_or("Room not found.")?
                    .messages
                    .push_back(message);
            }
            return Ok(());
        }
        if let Some(key) = self
            .data
            .private
            .iter()
            .find(|(_, chat)| chat.id == id)
            .map(|(key, _)| key.clone())
        {
            let target = self.private_scope(&key);
            if self.allows(actor, &target, authorization::Action::Read)
                && !matches!(&scope,authorization::Scope::Private(selected) if *selected!=id)
            {
                self.data
                    .private
                    .get_mut(&key)
                    .ok_or("Private conversation not found.")?
                    .messages
                    .push_back(message);
            }
        } else if self.is_su(actor)
            && scope == authorization::Scope::Server
            && let Some(room) = self.data.rooms.values_mut().find(|room| room.id == id)
        {
            room.messages.push_back(message);
        }
        Ok(())
    }
    fn execute_inner(
        &mut self,
        actor: Option<&str>,
        room: Option<&str>,
        input: &str,
    ) -> Result<String, String> {
        if let Some(command) = sudo_command(input)? {
            return self.with_command_authority(actor, room, true, |engine| {
                engine.execute_inner(actor, room, command)
            });
        }
        let parts: Vec<_> = input.split_whitespace().collect();
        self.load_command_message(actor, room, &parts)?;
        if parts.first() == Some(&"/bot") {
            return self.bot_command(actor, room, &parts);
        }
        if parts.first() == Some(&"/rename") {
            self.require_command(actor, room, "/rename")?;
            let (name, new_name) = match parts.as_slice() {
                [_, new_name] => (actor.ok_or("Usage: /rename user new-name")?, *new_name),
                [_, name, new_name] => (*name, *new_name),
                _ => return Err("Usage: /rename [user] new-name".into()),
            };
            return self.rename_user(actor, name, new_name);
        }
        if parts.first() == Some(&"/rm") && parts.get(1) == Some(&"user") {
            require_len(&parts, 3, "/rm user name")?;
            self.require_command(actor, room, "/rm")?;
            let scope =
                self.account_target(actor, parts[2], authorization::Action::DeleteAccount)?;
            self.require_target_command(actor, &scope, "/rm")?;
            return self.delete_user(actor, parts[2]);
        }
        if matches!(
            parts.first().copied(),
            Some(
                "/permissions"
                    | "/debug"
                    | "/help"
                    | "/man"
                    | "/whoami"
                    | "/console"
                    | "/ls"
                    | "/members"
                    | "/history"
            )
        ) {
            return self.apply(actor, room, input);
        }
        if let Some(change) = self.message_change(actor, room, &parts, input) {
            let result = self.apply(actor, room, input);
            match result {
                Ok(reply) => {
                    if let Err(error) = self.save_message_change(&change) {
                        self.restore_message_change(change);
                        return Err(error);
                    }
                    return Ok(reply);
                }
                Err(error) => {
                    self.restore_message_change(change);
                    return Err(error);
                }
            }
        }
        let before = self.data.clone();
        let result = self.apply(actor, room, input);
        match result {
            Ok(reply) => {
                if self.data == before {
                    return Ok(reply);
                }
                if self.data.policy.assignments != before.policy.assignments
                    || self.data.policy.grants != before.policy.grants
                {
                    self.data.policy.revision = before.policy.revision.wrapping_add(1);
                }
                if let Err(e) = self.save() {
                    self.data = before;
                    return Err(e);
                }
                Ok(reply)
            }
            Err(e) => {
                self.data = before;
                Err(e)
            }
        }
    }
    fn apply_text(
        &mut self,
        actor: Option<&str>,
        room: Option<&str>,
        input: &str,
    ) -> Result<String, String> {
        if input.chars().count() > 4000 {
            return Err("Messages support at most 4000 characters.".into());
        }
        let room = room.ok_or("Select a room first.")?;
        if let Some(key) = room.strip_prefix("@private:") {
            self.require(actor, &self.private_scope(key), authorization::Action::Send)?;
            let (_, recipient) = key.split_once(':').ok_or("Invalid private conversation.")?;
            if !self.data.private.contains_key(key) {
                return Err("Private conversation not found.".into());
            }
            let recipient = actor
                .and_then(|name| private_peer(key, name))
                .unwrap_or(recipient);
            let mut message = self.message(actor, Some(recipient), input)?;
            message
                .mentions
                .retain(|name| key.split(':').any(|p| p == name));
            return self.push_private_to(key, message).map(|()| String::new());
        }

        self.require(actor, &self.room_scope(room)?, authorization::Action::Send)?;
        let mut message = self.message(actor, None, input)?;
        let target = self.data.rooms.get_mut(room).ok_or("Room not found.")?;
        if target.messages.len() == self.max_messages {
            target.messages.pop_front();
        }
        message
            .mentions
            .retain(|name| target.members.contains(name));
        target.messages.push_back(message);
        target.revision = target.revision.wrapping_add(1);
        Ok(String::new())
    }

    pub fn send_literal(&mut self, user: &str, view: &str, text: &str) -> Result<Message, String> {
        if !self.active(user) {
            return Err("Account unavailable.".into());
        }
        let text = text.trim();
        if text.is_empty() || text.chars().count() > 4000 {
            return Err("Messages support 1–4000 characters.".into());
        }
        let normalized = view
            .strip_prefix("@direct:")
            .map(|peer| format!("@private:{}", private_key(user, peer)));
        let view = normalized.as_deref().unwrap_or(view);
        self.changed_conversation = None;
        // Use the same bounded transaction/rollback path as ordinary browser sends.
        let change = self
            .message_change(Some(user), Some(view), &["literal"], "literal")
            .ok_or("Select an existing room or private conversation.")?;
        let result = self.apply_text(Some(user), Some(view), text).and_then(|_| {
            let message = if let Some(key) = view.strip_prefix("@private:") {
                self.data.private.get(key).and_then(|c| c.messages.back())
            } else {
                self.data.rooms.get(view).and_then(|r| r.messages.back())
            }
            .cloned()
            .ok_or("Message unavailable.")?;
            self.save_message_change(&change)?;
            Ok(message)
        });
        if result.is_err() {
            self.restore_message_change(change);
        }
        for room in self.data.rooms.values_mut() {
            room.messages.clear();
        }
        for chat in self.data.private.values_mut() {
            chat.messages.clear();
        }
        result
    }

    pub(super) fn apply(
        &mut self,
        actor: Option<&str>,
        room: Option<&str>,
        input: &str,
    ) -> Result<String, String> {
        let input = input.trim();
        if input.is_empty() || input.chars().count() > 4100 {
            return Err("Enter a message of up to 4000 characters or a command.".into());
        }
        if actor.is_some_and(|name| !self.active(name)) {
            return Err("Account unavailable.".into());
        }
        let author = actor.unwrap_or("console");
        let parts: Vec<&str> = input.split_whitespace().collect();
        if !input.starts_with('/') {
            return self.apply_text(actor, room, input);
        }
        self.require_command(actor, room, parts[0])?;
        let view = room;
        let scope = self.command_scope(actor, room)?;
        let room = room.filter(|name| !name.starts_with('@'));
        let subcommand = parts.get(1).copied();
        let routed_parts;
        let handler_parts = if matches!(parts[0], "/new" | "/rm") {
            match (parts[0], subcommand) {
                ("/new", Some("room")) | ("/rm", Some("room")) => {}
                ("/rm", Some("group")) if (4..=5).contains(&parts.len()) => {}
                _ => {
                    return Err(format!(
                        "Usage: {}",
                        if parts[0] == "/new" {
                            commands::NEW_USAGE
                        } else {
                            commands::RM_USAGE
                        }
                    ));
                }
            }
            routed_parts = std::iter::once(parts[0])
                .chain(parts.iter().skip(2).copied())
                .collect::<Vec<_>>();
            &routed_parts
        } else {
            &parts
        };
        let context = CommandContext {
            actor,
            room,
            input,
            parts: handler_parts,
            author,
            scope,
            view,
        };
        match parts[0] {
            "/enable" | "/disable" => self.apply_accounts(&context),
            "/grant" | "/revoke" | "/permissions" => self.apply_policy(&context),
            "/rm" if subcommand == Some("group") => self.apply_policy(&context),
            "/new" | "/add" | "/kick" | "/join" | "/leave" | "/rm" | "/owner" => {
                self.apply_rooms(&context)
            }
            "/react" | "/reply" | "/retract" | "/clean" | "/tell" => self.apply_messages(&context),
            "/debug" | "/help" | "/man" | "/whoami" | "/console" | "/ls" | "/members"
            | "/history" => self.apply_queries(&context),
            "/reset" => Err("Account provisioning is console-only.".into()),
            _ => Err("Unknown command. Try /help.".into()),
        }
    }
}

// A send holds only the evicted message; a reaction holds only its old message.
// No retained conversation or unrelated entity is cloned for rollback.
pub(super) struct MessageChange {
    pub(super) key: String,
    pub(super) private: bool,
    pub(super) reaction: bool,
    pub(super) index: Option<usize>,
    pub(super) previous: Option<Message>,
    pub(super) sequence: u64,
    revision: Option<u64>,
}
impl MessageChange {
    pub(super) fn new_conversation(&self) -> bool {
        self.revision.is_none()
    }
}
impl Engine {
    fn message_change(
        &self,
        actor: Option<&str>,
        view: Option<&str>,
        parts: &[&str],
        input: &str,
    ) -> Option<MessageChange> {
        let command = *parts.first()?;
        let reaction = command == "/react";
        if input.trim_start().starts_with('/') && !matches!(command, "/tell" | "/reply" | "/react")
        {
            return None;
        }
        let (private, key) = if command == "/tell" {
            (true, private_key(actor?, parts.get(1)?))
        } else if let Some(room) = view.filter(|v| !v.starts_with('@')) {
            (false, room.to_owned())
        } else if !input.trim_start().starts_with('/') {
            (true, view?.strip_prefix("@private:")?.to_owned())
        } else {
            let id = parts.get(1)?;
            let scope = self.command_scope(actor, view).ok()?;
            let key = self
                .data
                .private
                .iter()
                .find(|(key, chat)| {
                    self.allows(actor, &self.private_scope(key), authorization::Action::Read)
                        && !matches!(&scope,authorization::Scope::Private(id) if *id != chat.id)
                        && chat.messages.iter().any(|m| m.id == *id)
                })?
                .0
                .clone();
            (true, key)
        };
        let conversation = if private {
            self.data
                .private
                .get(&key)
                .map(|c| (&c.messages, c.revision))
        } else {
            self.data.rooms.get(&key).map(|r| (&r.messages, r.revision))
        };
        let index = if reaction {
            Some(
                conversation?
                    .0
                    .iter()
                    .position(|m| Some(m.id.as_str()) == parts.get(1).copied())?,
            )
        } else {
            None
        };
        let previous = conversation.and_then(|(messages, _)| {
            if let Some(index) = index {
                messages.get(index).cloned()
            } else {
                (messages.len() == self.max_messages)
                    .then(|| messages.front().cloned())
                    .flatten()
            }
        });
        Some(MessageChange {
            key,
            private,
            reaction,
            index,
            previous,
            sequence: self.data.next_sequence,
            revision: conversation.map(|(_, r)| r),
        })
    }
    fn restore_message_change(&mut self, change: MessageChange) {
        self.data.next_sequence = change.sequence;
        if change.private && change.revision.is_none() {
            self.data.private.remove(&change.key);
            return;
        }
        let conversation = if change.private {
            self.data
                .private
                .get_mut(&change.key)
                .map(|c| (&mut c.messages, &mut c.revision))
        } else {
            self.data
                .rooms
                .get_mut(&change.key)
                .map(|r| (&mut r.messages, &mut r.revision))
        };
        if let Some((messages, revision)) = conversation {
            if let Some(old_revision) = change.revision {
                *revision = old_revision;
            }
            if change.reaction {
                if let (Some(index), Some(previous)) = (change.index, change.previous)
                    && let Some(message) = messages.get_mut(index)
                {
                    *message = previous;
                }
            } else if messages
                .back()
                .is_some_and(|m| m.sequence > change.sequence)
            {
                messages.pop_back();
                if let Some(previous) = change.previous {
                    messages.push_front(previous);
                }
            }
        }
    }
}
