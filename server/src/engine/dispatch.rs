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
    pub(super) markdown: bool,
    pub(super) author: &'a str,
}
impl Engine {
    pub fn read_command(
        &self,
        actor: Option<&str>,
        view: Option<&str>,
        input: &str,
    ) -> Option<Result<String, String>> {
        let parts: Vec<_> = input.split_whitespace().collect();
        if !matches!(
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
            return None;
        }
        Some((|| {
            if input.chars().count() > 4100 {
                return Err("Enter a message of up to 4000 characters or a command.".into());
            }
            if actor.is_some_and(|name| !self.active(name)) {
                return Err("Account unavailable.".into());
            }
            self.require_command(actor, view, parts[0])?;
            let context = CommandContext {
                actor,
                room: view.filter(|v| !v.starts_with('@')),
                input,
                parts: &parts,
                view,
                scope: self.command_scope(actor, view)?,
                markdown: false,
                author: actor.unwrap_or("console"),
            };
            if parts[0] == "/permissions" {
                self.apply_permissions(&context)
            } else {
                self.apply_queries(&context)
            }
        })())
    }

    pub fn execute(
        &mut self,
        actor: Option<&str>,
        room: Option<&str>,
        input: &str,
    ) -> Result<String, String> {
        self.execute_formatted(actor, room, input, false)
    }
    pub fn execute_formatted(
        &mut self,
        actor: Option<&str>,
        room: Option<&str>,
        input: &str,
        markdown: bool,
    ) -> Result<String, String> {
        if actor.is_some_and(|user| !self.active(user)) {
            return Err("Account unavailable.".into());
        }
        self.changed_conversation = None;
        let result = self.execute_inner(actor, room, input, markdown);
        self.clear_working_messages();
        result
    }
    pub(super) fn clear_working_messages(&mut self) {
        // Working messages are transient. Durable history and bounded pages live in storage.
        for room in self.data.rooms.values_mut() {
            room.messages.clear();
        }
        for chat in self.data.private.values_mut() {
            chat.messages.clear();
        }
        self.pending_deletes.clear();
        self.pending_cleanups.clear();
    }
    fn load_command_message(
        &mut self,
        actor: Option<&str>,
        view: Option<&str>,
        parts: &[&str],
    ) -> Result<(), String> {
        if !matches!(
            parts.first().copied(),
            Some("/react" | "/reply" | "/retract" | "/edit")
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
        markdown: bool,
    ) -> Result<String, String> {
        if let Some(command) = sudo_command(input)? {
            return self.with_command_authority(actor, room, true, |engine| {
                engine.execute_inner(actor, room, command, markdown)
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
                [_, "room", name, new_name] => return self.rename_room(actor, name, new_name),
                [_, "user", name, new_name] => (*name, *new_name),
                [_, new_name] => (
                    actor.ok_or_else(|| format!("Usage: {}", commands::RENAME_USAGE))?,
                    *new_name,
                ),
                [_, name, new_name] => (*name, *new_name),
                _ => return Err(format!("Usage: {}", commands::RENAME_USAGE)),
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
            return self.apply(actor, room, input, markdown);
        }
        if let Some(change) = self.message_change(actor, room, &parts, input) {
            let result = self.apply(actor, room, input, markdown);
            return self.finish_message_change(change, result, None, std::convert::identity);
        }
        if matches!(parts.first().copied(), Some("/retract" | "/clean")) {
            let revision = self.revision;
            let changed = self.changed_conversation.clone();
            let result = self.apply(actor, room, input, markdown).and_then(|reply| {
                self.save_history_maintenance()?;
                Ok(reply)
            });
            if result.is_err() {
                self.restore_command_state(revision, changed)
                    .map_err(|e| e.to_string())?;
            }
            return result;
        }
        let before = self.data.clone();
        let result = self.apply(actor, room, input, markdown);
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
                let removed_rooms = before
                    .rooms
                    .keys()
                    .filter(|name| !self.data.rooms.contains_key(*name))
                    .cloned()
                    .collect::<Vec<_>>();
                if let Err(e) = self.save_with_removed_rooms(&removed_rooms, Some(&before)) {
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
        markdown: bool,
    ) -> Result<String, String> {
        if input.chars().count() > 4000 {
            return Err("Messages support at most 4000 characters.".into());
        }
        let room = room.ok_or("Select a room first.")?;
        if let Some(key) = room.strip_prefix("@private:") {
            self.require(actor, &self.private_scope(key), authorization::Action::Send)?;
        } else {
            self.require(actor, &self.room_scope(room)?, authorization::Action::Send)?;
        }
        self.append_text(actor, room, input, markdown, None)?;
        Ok(String::new())
    }

    pub(super) fn apply(
        &mut self,
        actor: Option<&str>,
        room: Option<&str>,
        input: &str,
        markdown: bool,
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
            return self.apply_text(actor, room, input, markdown);
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
            markdown,
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
            "/react" | "/reply" | "/retract" | "/edit" | "/clean" | "/tell" => {
                self.apply_messages(&context)
            }
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
    pub(super) revision: Option<u64>,
}
impl MessageChange {
    pub(super) fn new_conversation(&self) -> bool {
        self.revision.is_none()
    }
}
impl Engine {
    pub(super) fn message_change(
        &self,
        actor: Option<&str>,
        view: Option<&str>,
        parts: &[&str],
        input: &str,
    ) -> Option<MessageChange> {
        let command = *parts.first()?;
        let reaction = command == "/react";
        if input.trim_start().starts_with('/')
            && !matches!(command, "/tell" | "/reply" | "/react" | "/edit")
        {
            return None;
        }
        let (private, key) = if command == "/tell" {
            (true, private_key(actor?, parts.get(1)?))
        } else if let Some(room) = view.filter(|v| !v.starts_with('@')) {
            (false, room.to_owned())
        } else if command == "/edit"
            && let Some(name) = self.message_action_room(
                actor,
                None,
                &self.command_scope(actor, view).ok()?,
                parts.get(1)?,
            )
        {
            (false, name)
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
        let view = if private {
            format!("@private:{key}")
        } else {
            key
        };
        self.capture_message_change(
            &view,
            if reaction || command == "/edit" {
                parts.get(1).copied()
            } else {
                None
            },
            reaction,
        )
    }
    pub(super) fn capture_message_change(
        &self,
        view: &str,
        message_id: Option<&str>,
        reaction: bool,
    ) -> Option<MessageChange> {
        let private = view.starts_with("@private:");
        let key = view.strip_prefix("@private:").unwrap_or(view).to_owned();
        let conversation = if private {
            self.data
                .private
                .get(&key)
                .map(|c| (&c.messages, c.revision))
        } else {
            self.data.rooms.get(&key).map(|r| (&r.messages, r.revision))
        };
        let index = if let Some(id) = message_id {
            Some(conversation?.0.iter().position(|m| m.id == id)?)
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
    pub(super) fn finish_message_change<T, E>(
        &mut self,
        change: MessageChange,
        result: Result<T, E>,
        receipt: Option<&crate::bots::models::Receipt>,
        storage_error: impl FnOnce(String) -> E,
    ) -> Result<T, E> {
        let result = result.and_then(|value| {
            self.save_message_change_with_receipt(&change, receipt)
                .map_err(storage_error)?;
            Ok(value)
        });
        if result.is_err() {
            self.restore_message_change(change);
        }
        result
    }
    pub(super) fn restore_message_change(&mut self, change: MessageChange) {
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
            if change.index.is_some() {
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
