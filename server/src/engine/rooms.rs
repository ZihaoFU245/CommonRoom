use super::*;
use authorization::{Action, Group, Scope, Subject};

impl Engine {
    pub(super) fn apply_rooms(&mut self, context: &CommandContext<'_>) -> Result<String, String> {
        let CommandContext {
            actor,
            room,
            parts,
            author,
            ..
        } = *context;
        match parts[0] {
            "/new" => {
                self.require_target_command(actor, &Scope::Server, parts[0])?;
                self.require(actor, &Scope::Server, Action::CreateRoom)?;
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
                let owner_id = actor
                    .and_then(|n| self.data.users.get(n))
                    .map_or_else(|| "console".into(), |u| u.id.clone());
                let mut target = Room {
                    id: uuid::Uuid::new_v4().to_string(),
                    owner_id: owner_id.clone(),
                    ..Room::default()
                };
                let scope = Scope::Room(target.id.clone());
                if actor.is_some() {
                    target.members.insert(author.into());
                    self.assign(owner_id.clone(), Group::User, scope.clone());
                }
                self.add_owner_grants(owner_id, scope);
                self.data.rooms.insert(name.into(), target);
                self.audit(actor, "/new", name);
                Ok(format!("Created #{name}. Admission: invitation-only."))
            }
            "/add" | "/kick" => {
                if !(2..=3).contains(&parts.len()) {
                    return Err("Usage: /add user [room] or /kick user [room]".into());
                }
                let name = parts[1];
                let id = self
                    .data
                    .users
                    .get(name)
                    .filter(|u| !u.disabled)
                    .ok_or("User not found.")?
                    .id
                    .clone();
                let room = parts.get(2).copied().or(room).ok_or("Specify a room.")?;
                let scope = self.room_scope(room)?;
                self.require_target_command(actor, &scope, parts[0])?;
                self.require(
                    actor,
                    &scope,
                    if parts[0] == "/add" {
                        Action::Invite
                    } else {
                        Action::Kick
                    },
                )?;
                if parts[0] == "/add" {
                    self.require_member_delegation(actor, &scope)?;
                }
                if parts[0] == "/kick"
                    && self.data.rooms.get(room).is_some_and(|r| r.owner_id == id)
                {
                    return Err("Transfer ownership before removing the owner.".into());
                }
                let target = self.data.rooms.get_mut(room).ok_or("Room not found.")?;
                if parts[0] == "/add" {
                    target.members.insert(name.into());
                    self.assign(id, Group::User, scope);
                } else {
                    target.members.remove(name);
                    self.data
                        .policy
                        .assignments
                        .retain(|a| !(a.account_id == id && a.scope == scope));
                    self.data.policy.grants.retain(|g| {
                        !(g.subject == Subject::Account(id.clone()) && g.scope == scope)
                    });
                }
                self.audit(actor, parts[0], &format!("{name} {room}"));
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
                let user = actor.ok_or("Use /add user room from the console.")?;
                let name = parts
                    .get(1)
                    .copied()
                    .or(room)
                    .ok_or("Select or specify a room.")?;
                let scope = self.room_scope(name)?;
                self.require_target_command(actor, &scope, parts[0])?;
                if parts[0] == "/join" {
                    self.require(actor, &scope, Action::Join)?;
                    self.require(actor, &scope, Action::Read)?;
                    // Opening a view never manufactures grants or membership.
                } else {
                    let id = self
                        .data
                        .users
                        .get(user)
                        .ok_or("Account unavailable.")?
                        .id
                        .clone();
                    if self.data.rooms.get(name).is_some_and(|r| r.owner_id == id) {
                        return Err("Transfer ownership before leaving your room.".into());
                    }
                    self.data
                        .rooms
                        .get_mut(name)
                        .ok_or("Room not found.")?
                        .members
                        .remove(user);
                    self.data
                        .policy
                        .assignments
                        .retain(|a| !(a.account_id == id && a.scope == scope));
                    self.data.policy.grants.retain(|g| {
                        !(g.subject == Subject::Account(id.clone()) && g.scope == scope)
                    });
                }
                Ok(format!(
                    "{} #{name}.",
                    if parts[0] == "/join" {
                        "Joined"
                    } else {
                        "Left"
                    }
                ))
            }
            "/owner" => {
                if !(2..=3).contains(&parts.len()) {
                    return Err("Usage: /owner user [room]".into());
                }
                let name = parts.get(2).copied().or(room).ok_or("Specify a room.")?;
                let scope = self.room_scope(name)?;
                self.require_target_command(actor, &scope, parts[0])?;
                self.require(actor, &scope, Action::Transfer)?;
                let target_id = self
                    .data
                    .users
                    .get(parts[1])
                    .filter(|u| !u.disabled)
                    .ok_or("User not found.")?
                    .id
                    .clone();
                self.data
                    .policy
                    .grants
                    .retain(|g| !(g.owner && g.scope == scope));
                self.add_owner_grants(target_id.clone(), scope.clone());
                self.assign(target_id.clone(), Group::User, scope);
                let target = self.data.rooms.get_mut(name).ok_or("Room not found.")?;
                target.owner_id = target_id;
                target.members.insert(parts[1].into());
                self.audit(actor, "/owner", &format!("{} {name}", parts[1]));
                Ok(format!("{} owns #{name}.", parts[1]))
            }
            "/delete" => {
                require_len(parts, 2, "/delete room")?;
                let scope = self.room_scope(parts[1])?;
                self.require_target_command(actor, &scope, parts[0])?;
                self.require(actor, &scope, Action::DeleteRoom)?;
                self.data.rooms.remove(parts[1]).ok_or("Room not found.")?;
                self.data.policy.assignments.retain(|a| a.scope != scope);
                self.data.policy.grants.retain(|g| g.scope != scope);
                self.audit(actor, "/delete", parts[1]);
                Ok(format!("Deleted #{}.", parts[1]))
            }
            _ => Err("Unknown command.".into()),
        }
    }
}
