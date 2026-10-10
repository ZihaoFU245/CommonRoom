use super::*;

pub(super) struct CommandContext<'a> {
    pub(super) actor: Option<&'a str>,
    pub(super) room: Option<&'a str>,
    pub(super) input: &'a str,
    pub(super) parts: &'a [&'a str],
    pub(super) admin: bool,
    pub(super) author: &'a str,
}
impl Engine {
    pub fn execute(
        &mut self,
        actor: Option<&str>,
        room: Option<&str>,
        input: &str,
    ) -> Result<String, String> {
        if input.split_whitespace().next() == Some("/deleteuser") {
            let parts: Vec<_> = input.split_whitespace().collect();
            require_len(&parts, 2, "/deleteuser user")?;
            return self.delete_user(actor, parts[1]);
        }
        if matches!(
            input.split_whitespace().next(),
            Some("/debug" | "/help" | "/whoami" | "/rooms" | "/users" | "/members" | "/history")
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
        let admin = match actor {
            None => true,
            Some(name) => {
                self.data
                    .users
                    .get(name)
                    .filter(|u| !u.disabled)
                    .ok_or("Account unavailable.")?
                    .admin
            }
        };
        let author = actor.unwrap_or("console");
        let parts: Vec<&str> = input.split_whitespace().collect();
        if !input.starts_with('/') {
            if input.chars().count() > 4000 {
                return Err("Messages support at most 4000 characters.".into());
            }
            let room = room.ok_or("Select a room first.")?;
            let mut message = self.message(author, None, input)?;
            let target = self.data.rooms.get_mut(room).ok_or("Room not found.")?;
            if actor.is_some() && !target.members.contains(author) {
                return Err("You are not a member of this room.".into());
            }
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
        let context = CommandContext {
            actor,
            room,
            input,
            parts: &parts,
            admin,
            author,
        };
        match parts[0] {
            "/grant" | "/revoke" | "/enable" | "/disable" => self.apply_accounts(&context),
            "/new" | "/add" | "/kick" | "/join" | "/leave" | "/delete" => {
                self.apply_rooms(&context)
            }
            "/react" | "/reply" | "/retract" | "/clean" | "/tell" => self.apply_messages(&context),
            "/debug" | "/help" | "/whoami" | "/rooms" | "/users" | "/members" | "/history" => {
                self.apply_queries(&context)
            }
            "/user" | "/reset" => Err("Account provisioning is console-only.".into()),
            _ => Err("Unknown command. Try /help.".into()),
        }
    }
}
