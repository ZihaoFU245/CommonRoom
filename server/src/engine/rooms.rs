use super::*;

impl Engine {
    pub(super) fn apply_rooms(&mut self, context: &CommandContext<'_>) -> Result<String, String> {
        let CommandContext {
            actor,
            room,
            parts,
            admin,
            author,
            ..
        } = *context;
        match parts[0] {
            "/new" => {
                require_admin(admin)?;
                require_len(parts, 2, "/new room")?;
                let name = parts[1];
                if !valid_name(name) {
                    return Err("Invalid room name.".into());
                }
                if self.data.rooms.contains_key(name) {
                    return Err("Room already exists.".into());
                }
                if self.data.rooms.len() >= self.max_rooms {
                    return Err(format!("Room limit reached ({}).", self.max_rooms));
                }
                let mut r = Room::default();
                if actor.is_some() {
                    r.members.insert(author.into());
                }
                self.data.rooms.insert(name.into(), r);
                Ok(format!("Created #{name}."))
            }
            "/add" | "/kick" => {
                require_admin(admin)?;
                if !(2..=3).contains(&parts.len()) {
                    return Err("Usage: /add user [room] or /kick user [room]".into());
                }
                let name = parts[1];
                if !self.active(name) {
                    return Err("User not found.".into());
                }
                let room = parts.get(2).copied().or(room).ok_or("Specify a room.")?;
                let target = self.data.rooms.get_mut(room).ok_or("Room not found.")?;
                if parts[0] == "/add" {
                    target.members.insert(name.into());
                } else {
                    target.members.remove(name);
                }
                Ok(format!(
                    "{name} {} #{room}.",
                    if parts[0] == "/add" {
                        "added to"
                    } else {
                        "removed from"
                    }
                ))
            }
            "/join" | "/leave" => {
                if parts[0] == "/join" {
                    require_len(parts, 2, "/join room")?;
                } else if parts.len() > 2 {
                    return Err("Usage: /leave [room]".into());
                }
                if actor.is_none() {
                    return Err("Use /add user room from the console.".into());
                }
                let name = parts
                    .get(1)
                    .copied()
                    .or(room)
                    .ok_or("Select or specify a room.")?;
                let r = self.data.rooms.get_mut(name).ok_or("Room not found.")?;
                if parts[0] == "/join" {
                    if !admin && !r.members.contains(author) {
                        return Err("An admin must add you to this room first.".into());
                    }
                    r.members.insert(author.into());
                } else {
                    r.members.remove(author);
                }
                Ok(format!(
                    "{} #{}.",
                    if parts[0] == "/join" {
                        "Joined"
                    } else {
                        "Left"
                    },
                    name
                ))
            }
            "/delete" => {
                require_admin(admin)?;
                require_len(parts, 2, "/delete room")?;
                self.data.rooms.remove(parts[1]).ok_or("Room not found.")?;
                Ok(format!("Deleted #{}.", parts[1]))
            }
            _ => unreachable!("Dispatcher selected the wrong command domain"),
        }
    }
}
