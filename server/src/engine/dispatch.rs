use super::*;

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
        if let Some(command) = sudo_command(input)? {
            return self.with_command_authority(actor, room, true, |engine| {
                engine.execute(actor, room, command)
            });
        }
        if input.split_whitespace().next() == Some("/rename") {
            let parts: Vec<_> = input.split_whitespace().collect();
            self.require_command(actor, room, "/rename")?;
            let (name, new_name) = match parts.as_slice() {
                [_, new_name] => (actor.ok_or("Usage: /rename user new-name")?, *new_name),
                [_, name, new_name] => (*name, *new_name),
                _ => return Err("Usage: /rename [user] new-name".into()),
            };
            return self.rename_user(actor, name, new_name);
        }
        if input.split_whitespace().next() == Some("/deleteuser") {
            let parts: Vec<_> = input.split_whitespace().collect();
            require_len(&parts, 2, "/deleteuser user")?;
            self.require_command(actor, room, "/deleteuser")?;
            let scope =
                self.account_target(actor, parts[1], authorization::Action::DeleteAccount)?;
            self.require_target_command(actor, &scope, "/deleteuser")?;
            return self.delete_user(actor, parts[1]);
        }
        if matches!(
            input.split_whitespace().next(),
            Some(
                "/permissions"
                    | "/debug"
                    | "/help"
                    | "/man"
                    | "/whoami"
                    | "/console"
                    | "/rooms"
                    | "/users"
                    | "/members"
                    | "/history"
            )
        ) {
            return self.apply(actor, room, input);
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
            return Ok(String::new());
        }
        self.require_command(actor, room, parts[0])?;
        let view = room;
        let scope = self.command_scope(actor, room)?;
        let room = room.filter(|name| !name.starts_with('@'));
        let context = CommandContext {
            actor,
            room,
            input,
            parts: &parts,
            author,
            scope,
            view,
        };
        match parts[0] {
            "/enable" | "/disable" => self.apply_accounts(&context),
            "/grant" | "/revoke" | "/permissions" => self.apply_policy(&context),
            "/new" | "/add" | "/kick" | "/join" | "/leave" | "/delete" | "/owner" => {
                self.apply_rooms(&context)
            }
            "/react" | "/reply" | "/retract" | "/clean" | "/tell" => self.apply_messages(&context),
            "/debug" | "/help" | "/man" | "/whoami" | "/console" | "/rooms" | "/users"
            | "/members" | "/history" => self.apply_queries(&context),
            "/user" | "/reset" => Err("Account provisioning is console-only.".into()),
            _ => Err("Unknown command. Try /help.".into()),
        }
    }
}
