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
    pub(super) parts: &'a [&'a str],
    pub(super) view: Option<&'a str>,
    pub(super) scope: authorization::Scope,
    pub(super) author: &'a str,
}
impl Engine {
    #[cfg(test)]
    pub fn read_command(
        &self,
        actor: Option<&str>,
        view: Option<&str>,
        input: &str,
    ) -> Option<Result<String, EngineError>> {
        self.prepare_read_command(actor, view, input)
            .map(|plan| plan.and_then(CommandRead::finish))
    }
    pub(crate) fn prepare_read_command(
        &self,
        actor: Option<&str>,
        view: Option<&str>,
        input: &str,
    ) -> Option<Result<CommandRead, EngineError>> {
        let command = input.split_whitespace().next().unwrap_or("");
        if let Err(error) = commands::validate_room_arguments(input) {
            return Some(Err(EngineError::Invalid(error)));
        }
        let routed = commands::route(input);
        let parts: Vec<_> = routed.split_whitespace().collect();
        if commands::execution(input) != commands::Execution::Read || command == "/system" {
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
                parts: &parts,
                view,
                scope: self.command_scope(actor, view)?,
                author: actor.unwrap_or("console"),
            };
            if command == "/bot" {
                self.query_bot_command(actor, view, &parts)
                    .ok_or("Command is not read-only.")?
                    .map(CommandRead::Text)
            } else if command == "/clean" {
                self.resolve_cleanup_plan(&context)
                    .map(CommandRead::Preview)
            } else if matches!(command, "/user" | "/room") {
                self.query_objects(&context).map(CommandRead::Text)
            } else if parts[0] == "/permissions" {
                self.apply_permissions(&context).map(CommandRead::Text)
            } else if parts[0] == "/history" {
                self.history_command_plan(&context)
                    .map(CommandRead::History)
            } else {
                self.apply_queries(&context).map(CommandRead::Text)
            }
        })())
    }

    #[cfg(test)]
    pub fn execute(
        &mut self,
        actor: Option<&str>,
        room: Option<&str>,
        input: &str,
    ) -> Result<String, EngineError> {
        self.execute_formatted(actor, room, input, false)
    }
    #[cfg(test)]
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
        self.execute_inner(actor, room, input, markdown)
    }
    #[cfg(test)]
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
        commands::validate_room_arguments(input).map_err(EngineError::Invalid)?;
        if native_metadata_command(input) {
            let (reply, pending) = match self.prepare_metadata(actor, room, input)? {
                MetadataPreparation::Ready(reply, pending) => {
                    (reply, pending.map(|pending| *pending))
                }
                MetadataPreparation::Rename(read) => {
                    let references = database::block_on_storage(read.read())??;
                    let (reply, pending) = self.prepare_referenced_rename(actor, references)?;
                    (reply, Some(pending))
                }
            };
            if let Some(pending) = pending {
                let pending = database::block_on_storage(pending.save())??;
                self.publish_metadata(pending)?;
            }
            return Ok(reply);
        }
        let command = input.split_whitespace().next().unwrap_or("");
        if matches!(command, "/reply" | "/edit" | "/react" | "/retract") {
            let plan = self.action_plan(actor, room, input)?;
            let (id, original) =
                database::block_on_storage(plan.history.locate_async(&plan.action.id))??
                    .ok_or("Message not found in this conversation.")?;
            if plan.action.is_retraction() {
                let pending = self.prepare_retraction(actor, room, &plan.action, &id, &original)?;
                database::block_on_storage(pending.save())??;
                self.publish_history(pending)?;
                return Ok("Message deleted.".into());
            }
            let (pending, reply) =
                self.prepare_action(actor, room, plan.action, &id, original, markdown)?;
            if let Some(pending) = pending {
                self.commit_fixture_message(pending)?;
            }
            return Ok(reply);
        }
        if command == "/clean" {
            let mut plan = self.prepare_cleanup(actor, room, input)?;
            database::block_on_storage(plan.count())??;
            if let Some(pending) = self.prepare_cleanup_write(actor, &plan)? {
                database::block_on_storage(pending.save())??;
                self.publish_history(pending)?;
            }
            return Ok(plan.report());
        }
        if !input.trim_start().starts_with('/') || command == "/tell" {
            let pending = if let Some(user) = actor {
                self.prepare_browser_message(user, room, input, markdown)?
            } else {
                if command == "/tell" {
                    return Err("Private messages require a user account.".into());
                }
                self.prepare_console_fixture(room, input, markdown)?
            };
            let reply = if command == "/tell" {
                format!(
                    "Private message sent to {}.",
                    pending
                        .message
                        .to
                        .as_deref()
                        .ok_or("Private message has no recipient.")?
                )
            } else {
                String::new()
            };
            self.commit_fixture_message(pending)?;
            return Ok(reply);
        }
        self.reject_command(actor, room, input)
    }
    /// Synchronous fixtures bridge to the same native message transaction and
    /// publication used by browser/console adapters; they own no parallel writes.
    #[cfg(test)]
    fn commit_fixture_message(&mut self, pending: PendingMessage) -> Result<(), EngineError> {
        let media =
            database::block_on_storage(pending.media_records())?.map_err(EngineError::from)?;
        self.validate_append_media(&pending, &media)?;
        let commit = database::block_on_storage(pending.save(None))??;
        self.publish_message(pending, commit)?;
        Ok(())
    }
    // Some old fixtures seed a synthetic console-authored room message. Real
    // stdin has no selected room and never exposes this test-only constructor.
    #[cfg(test)]
    fn prepare_console_fixture(
        &self,
        room: Option<&str>,
        input: &str,
        markdown: bool,
    ) -> Result<PendingMessage, EngineError> {
        crate::limits::validate_input(input).map_err(EngineError::Invalid)?;
        let view = room.ok_or("Select a room first.")?;
        let (scope, revision) = if let Some(key) = view.strip_prefix("@private:") {
            let chat = self
                .data
                .private
                .get(key)
                .ok_or("Private conversation not found.")?;
            (
                authorization::Scope::Private(chat.id.clone()),
                chat.revision,
            )
        } else {
            let room = self.data.rooms.get(view).ok_or("Room not found.")?;
            (authorization::Scope::Room(room.id.clone()), room.revision)
        };
        let mut message = self.message(None, None, input, markdown)?;
        if let Some(key) = view.strip_prefix("@private:") {
            if let authorization::Scope::Private(id) = &scope {
                message.private_id.clone_from(id);
            }
            message
                .mentions
                .retain(|name| key.split(':').any(|part| part == name));
        } else {
            message
                .mentions
                .retain(|name| self.data.rooms[view].members.contains(name));
        }
        self.pending_message_at(
            "console",
            view,
            scope,
            revision,
            message,
            message_write::PendingMutation::Append,
        )
    }
    pub(super) fn apply_metadata(
        &mut self,
        actor: Option<&str>,
        room: Option<&str>,
        input: &str,
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
            parts: handler_parts,
            author,
            scope,
            view,
        };
        match parts[0] {
            "/user" | "/room" => self.query_objects(&context),
            "/enable" | "/disable" => self.apply_accounts(&context),
            "/grant" | "/revoke" | "/permissions" => self.apply_policy(&context),
            "/rm" if subcommand == Some("group") => self.apply_policy(&context),
            "/new" | "/add" | "/kick" | "/join" | "/leave" | "/rm" | "/owner" => {
                self.apply_rooms(&context)
            }
            _ => Err("Unknown command. Try /help.".into()),
        }
    }
}

/// The engine reports committed effects; transports only choose the wire event.
pub(crate) enum CommandChange {
    Unchanged,
    Metadata(std::sync::Arc<MetadataNotice>),
    Conversation(String),
}
