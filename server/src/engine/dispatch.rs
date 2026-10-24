use super::*;
use crate::commands;

pub fn sudo_command(input: &str) -> Result<Option<&str>, EngineError> {
    let input = input.trim();
    if input.split_whitespace().next() != Some("/sudo") {
        return Ok(None);
    }
    let command = input
        .strip_prefix("/sudo")
        .ok_or("Invalid command.")?
        .trim_start();
    if input.chars().count() > crate::limits::MAX_COMMAND_CHARS
        || !command.starts_with('/')
        || command == "/"
    {
        return Err("Usage: /sudo /command [arguments]".into());
    }
    if command.split_whitespace().next() == Some("/sudo") {
        return Err("Nested /sudo commands are not supported.".into());
    }
    Ok(Some(command))
}

pub(super) struct CommandContext<'a> {
    pub(super) actor: Option<&'a str>,
    pub(super) command: &'a str,
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
    ) -> Option<Result<String, EngineError>> {
        let command = input.split_whitespace().next().unwrap_or("");
        if let Err(error) = commands::validate_room_arguments(input) {
            return Some(Err(EngineError::Invalid(error)));
        }
        let routed = commands::route(input);
        let parts: Vec<_> = routed.split_whitespace().collect();
        if parts.first() == Some(&"/clean") && parts.last() != Some(&"--dry-run") {
            return None;
        }
        if !matches!(
            parts.first().copied(),
            Some(
                "/user"
                    | "/clean"
                    | "/room"
                    | "/permissions"
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
        if matches!(command, "/user" | "/room")
            && !matches!(
                parts.as_slice(),
                ["/user", "ls" | "get", ..] | ["/room", "ls"]
            )
        {
            return None;
        }
        Some((|| {
            crate::limits::validate_input(input).map_err(EngineError::Invalid)?;
            if actor.is_some_and(|name| !self.active(name)) {
                return Err(EngineError::Forbidden("Account unavailable.".into()));
            }
            self.require_command(actor, view, command)?;
            let context = CommandContext {
                command,
                actor,
                room: view.filter(|v| !v.starts_with('@')),
                input,
                parts: &parts,
                view,
                scope: self.command_scope(actor, view)?,
                markdown: false,
                author: actor.unwrap_or("console"),
            };
            if command == "/clean" {
                self.preview_cleanup(&context)
            } else if matches!(command, "/user" | "/room") {
                self.query_objects(&context)
            } else if parts[0] == "/permissions" {
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
    ) -> Result<String, EngineError> {
        self.execute_formatted(actor, room, input, false)
    }
    pub(crate) fn execute_command(
        &mut self,
        actor: Option<&str>,
        room: Option<&str>,
        input: &str,
        markdown: bool,
    ) -> Result<CommandOutcome, EngineError> {
        let revision = self.revision;
        let reply = self.execute_formatted(actor, room, input, markdown)?;
        let change = if self.revision == revision {
            CommandChange::Unchanged
        } else if let Some(id) = &self.changed_conversation {
            CommandChange::Conversation(id.clone())
        } else {
            CommandChange::Metadata(self.metadata_change())
        };
        Ok(CommandOutcome { reply, change })
    }
    pub fn execute_formatted(
        &mut self,
        actor: Option<&str>,
        room: Option<&str>,
        input: &str,
        markdown: bool,
    ) -> Result<String, EngineError> {
        if actor.is_some_and(|user| !self.active(user)) {
            return Err(EngineError::Forbidden("Account unavailable.".into()));
        }
        self.changed_conversation = None;
        let result = self.execute_inner(actor, room, input, markdown);
        self.clear_working_messages();
        result
    }
    pub(super) fn clear_working_messages(&mut self) {
        self.working = None;
        self.store.pending_deletes.clear();
        self.store.pending_cleanups.clear();
    }
    fn load_command_message(
        &mut self,
        actor: Option<&str>,
        view: Option<&str>,
        parts: &[&str],
    ) -> Result<(), EngineError> {
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
        let Some((id, message)) = self.history_store().locate(message_id)? else {
            return Ok(());
        };
        if let Some(name) = view.filter(|v| !v.starts_with('@')) {
            let target = self.room_scope(name)?;
            self.require(actor, &target, authorization::Action::Read)?;
            if target == authorization::Scope::Room(id.clone()) {
                self.working = Some(messages::WorkingMessage {
                    reaction: None,
                    conversation: id,
                    message,
                });
            }
            return Ok(());
        }
        let target_view = self.bot_index.conversations.get(&id);
        if let Some(key) = target_view.and_then(|view| view.strip_prefix("@private:")) {
            let target = self.private_scope(key);
            if self.allows(actor, &target, authorization::Action::Read)
                && !matches!(&scope,authorization::Scope::Private(selected) if *selected!=id)
            {
                self.working = Some(messages::WorkingMessage {
                    reaction: None,
                    conversation: id,
                    message,
                });
            }
        } else if self.is_su(actor)
            && scope == authorization::Scope::Server
            && target_view.is_some()
        {
            self.working = Some(messages::WorkingMessage {
                reaction: None,
                conversation: id,
                message,
            });
        }
        Ok(())
    }
    fn execute_inner(
        &mut self,
        actor: Option<&str>,
        room: Option<&str>,
        input: &str,
        markdown: bool,
    ) -> Result<String, EngineError> {
        if let Some(command) = sudo_command(input)? {
            return self.with_command_authority(actor, room, true, |engine| {
                engine.execute_inner(actor, room, command, markdown)
            });
        }
        if let Some(result) = self.read_command(actor, room, input) {
            return result;
        }
        let command = input.split_whitespace().next().unwrap_or("");
        commands::validate_room_arguments(input).map_err(EngineError::Invalid)?;
        let routed = commands::route(input);
        let parts: Vec<_> = routed.split_whitespace().collect();
        self.load_command_message(actor, room, &parts)?;
        if parts.first() == Some(&"/bot") {
            return self.bot_command(actor, room, &parts);
        }
        if parts.first() == Some(&"/rename") {
            self.require_command(actor, room, command)?;
            let (name, new_name) = match parts.as_slice() {
                [_, "room", name, new_name] => {
                    return self.rename_room(actor, name, new_name, command);
                }
                [_, "user", name, new_name] => (*name, *new_name),
                [_, new_name] => (
                    actor.ok_or_else(|| {
                        EngineError::Invalid(format!("Usage: {}", commands::RENAME_USAGE))
                    })?,
                    *new_name,
                ),
                [_, name, new_name] => (*name, *new_name),
                _ => {
                    return Err(EngineError::Invalid(format!(
                        "Usage: {}",
                        commands::RENAME_USAGE
                    )));
                }
            };
            return self.rename_user(actor, name, new_name, command);
        }
        if parts.first() == Some(&"/rm") && parts.get(1) == Some(&"user") {
            require_len(&parts, 3, "/user delete who")?;
            self.require_command(actor, room, command)?;
            let scope =
                self.account_target(actor, parts[2], authorization::Action::DeleteAccount)?;
            self.require_target_command(actor, &scope, command)?;
            return self.delete_user(actor, parts[2]);
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
                    .map_err(EngineError::unavailable)?;
            }
            return result;
        }
        if parts.first() == Some(&"/user") {
            // The metadata handler resolves the exact account keys, validates
            // the entire selection, and owns its atomic commit/rollback.
            return self.apply(actor, room, input, markdown);
        }
        let before = self.command_metadata_change(room, &parts);
        self.metadata_dirty = false;
        let result = self.apply(actor, room, input, markdown);
        match result {
            Ok(reply) => {
                if !self.metadata_dirty {
                    return Ok(reply);
                }
                if self.data.policy.assignments != before.policy.assignments
                    || self.data.policy.grants != before.policy.grants
                    || self.data.policy.media_revocations != before.policy.media_revocations
                {
                    self.data.policy.revision = before.policy.revision.wrapping_add(1);
                }
                let removed_rooms = before.removed_rooms(&self.data);
                if let Err(e) =
                    self.save_metadata(&removed_rooms, changes::Baseline::Captured(&before))
                {
                    before.restore(&mut self.data);
                    return Err(e);
                }
                Ok(reply)
            }
            Err(e) => {
                before.restore(&mut self.data);
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
    ) -> Result<String, EngineError> {
        if input.chars().count() > crate::limits::MAX_MESSAGE_CHARS {
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
    ) -> Result<String, EngineError> {
        let input = input.trim();
        crate::limits::validate_input(input).map_err(EngineError::Invalid)?;
        if actor.is_some_and(|name| !self.active(name)) {
            return Err(EngineError::Forbidden("Account unavailable.".into()));
        }
        let author = actor.unwrap_or("console");
        let command = input.split_whitespace().next().unwrap_or("");
        let routed = commands::route(input);
        let parts: Vec<&str> = routed.split_whitespace().collect();
        if !input.starts_with('/') {
            return self.apply_text(actor, room, input, markdown);
        }
        self.require_command(actor, room, command)?;
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
                    return Err(EngineError::Invalid(format!(
                        "Usage: {}",
                        if parts[0] == "/new" {
                            commands::NEW_USAGE
                        } else {
                            commands::RM_USAGE
                        }
                    )));
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
            command,
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
            "/user" | "/room" => self.apply_objects(&context),
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

// A mutation owns its target coordinates and only the prior attachment IDs for edits.
// Message bodies live only in Engine::working, outside metadata rollback snapshots.
pub(super) struct MessageChange {
    pub(super) key: String,
    pub(super) private: bool,
    pub(super) mutation: MessageMutation,
    pub(super) sequence: u64,
    pub(super) revision: Option<u64>,
}
pub(super) enum MessageMutation {
    Append,
    Edit { attachments: BTreeSet<String> },
    Reaction,
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
            let row = self.working.as_ref().filter(|row| row.message.id == *id)?;
            let key = self
                .bot_index
                .conversations
                .get(&row.conversation)?
                .strip_prefix("@private:")?;
            if !self.allows(actor, &self.private_scope(key), authorization::Action::Read)
                || matches!(&scope, authorization::Scope::Private(id) if *id != row.conversation)
            {
                return None;
            }
            let key = key.to_owned();
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
            self.data.private.get(&key).map(|c| (&c.id, c.revision))
        } else {
            self.data.rooms.get(&key).map(|r| (&r.id, r.revision))
        };
        let mutation = if let Some(message_id) = message_id {
            let message = self
                .working_message(conversation?.0)
                .filter(|m| m.id == message_id)?;
            if reaction {
                MessageMutation::Reaction
            } else {
                MessageMutation::Edit {
                    attachments: media::media_references(&message.text),
                }
            }
        } else {
            MessageMutation::Append
        };
        Some(MessageChange {
            key,
            private,
            mutation,
            sequence: self.data.next_sequence,
            revision: conversation.map(|(_, r)| r),
        })
    }
    pub(super) fn finish_message_change<T, E>(
        &mut self,
        change: MessageChange,
        result: Result<T, E>,
        receipt: Option<&crate::bots::models::Receipt>,
        storage_error: impl FnOnce(EngineError) -> E,
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
        self.working = None;
        if change.private && change.revision.is_none() {
            self.data.private.remove(&change.key);
            return;
        }
        let revision = if change.private {
            self.data
                .private
                .get_mut(&change.key)
                .map(|c| &mut c.revision)
        } else {
            self.data
                .rooms
                .get_mut(&change.key)
                .map(|r| &mut r.revision)
        };
        if let (Some(revision), Some(previous)) = (revision, change.revision) {
            *revision = previous;
        }
    }
}

/// The engine reports committed effects; transports only choose the wire event.
pub(crate) enum CommandChange {
    Unchanged,
    Metadata(std::sync::Arc<MetadataNotice>),
    Conversation(String),
}
pub(crate) struct CommandOutcome {
    pub(crate) reply: String,
    pub(crate) change: CommandChange,
}
