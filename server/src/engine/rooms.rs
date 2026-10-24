use super::*;
use authorization::{Action, Group, Scope, Subject};

impl Engine {
    pub(super) fn rename_room(
        &mut self,
        actor: Option<&str>,
        name: &str,
        new_name: &str,
        command: &str,
    ) -> Result<String, EngineError> {
        let scope = self.room_scope(name)?;
        self.require_target_command(actor, &scope, command)?;
        self.require(actor, &scope, Action::RenameRoom)?;
        if !valid_name(new_name) {
            return Err("Names must contain 1–32 Unicode characters without whitespace, controls or ASCII punctuation other than _ and -.".into());
        }
        if name == new_name {
            return Ok("Room name unchanged.".into());
        }
        if self.data.rooms.contains_key(new_name) {
            return Err(EngineError::Conflict("Room already exists.".into()));
        }
        let before = MetadataChange::keyed(
            &self.data,
            changes::DirtyKeys {
                rooms: [name.to_owned(), new_name.to_owned()].into(),
                ..Default::default()
            },
        );
        self.audit(actor, "/rename room", &format!("{name} -> {new_name}"));
        let mut room = self
            .data
            .rooms
            .remove(name)
            .ok_or_else(|| EngineError::NotFound("Room not found.".into()))?;
        room.revision = room.revision.wrapping_add(1);
        self.data.rooms.insert(new_name.into(), room);
        if let Err(error) = self.store_room_rename(name, new_name, &before) {
            before.restore(&mut self.data);
            tracing::error!(error = %error, "Room rename failed");
            return Err(EngineError::Unavailable(
                "Storage unavailable; rename was not applied.".into(),
            ));
        }
        let old_key = format!("room:{name}");
        let new_key = format!("room:{new_name}");
        for positions in self.store.read_positions.values_mut() {
            positions.remove(&new_key);
            if let Some(sequence) = positions.remove(&old_key) {
                positions.insert(new_key.clone(), sequence);
            }
        }
        self.bot_index = crate::bots::models::Index::build(&self.data);
        let notice = MetadataNotice::between(
            &self.data,
            before.view(),
            &self.authorization,
            &self.authorization,
        );
        self.read_model.advance();
        self.revision = self.revision.wrapping_add(1);
        self.record_metadata(notice);
        Ok(format!("Renamed #{name} to #{new_name}."))
    }

    pub(super) fn apply_rooms(
        &mut self,
        context: &CommandContext<'_>,
    ) -> Result<String, EngineError> {
        let CommandContext {
            actor,
            room,
            parts,
            author,
            ..
        } = *context;
        match parts[0] {
            "/new" => {
                self.require_target_command(actor, &Scope::Server, context.command)?;
                self.require(actor, &Scope::Server, Action::CreateRoom)?;
                require_len(parts, 2, "/room new #name")?;
                let name = parts[1];
                if !valid_name(name) {
                    return Err("Invalid room name.".into());
                }
                if self.data.rooms.contains_key(name) {
                    return Err(EngineError::Conflict("Room already exists.".into()));
                }
                if self.data.rooms.len() >= self.max_rooms {
                    return Err(EngineError::Invalid(format!(
                        "Room limit reached ({}).",
                        self.max_rooms
                    )));
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
                    self.add_member_color_grant(owner_id.clone(), scope.clone());
                }
                self.add_owner_grants(owner_id, scope);
                self.data.rooms.insert(name.into(), target);
                self.audit(actor, "/new", name);
                Ok(format!("Created #{name}. Admission: invitation-only."))
            }
            "/add" | "/kick" => {
                if !(2..=3).contains(&parts.len()) {
                    return Err(EngineError::Invalid(format!(
                        "Usage: /room {} user [#room]",
                        parts[0].trim_start_matches('/')
                    )));
                }
                let name = parts[1];
                let id = self
                    .data
                    .users
                    .get(name)
                    .filter(|u| !u.disabled)
                    .ok_or_else(|| EngineError::NotFound("User not found.".into()))?
                    .id
                    .clone();
                let room = parts
                    .get(2)
                    .map(|v| v.trim_start_matches('#'))
                    .or(room)
                    .ok_or("Specify a room.")?;
                let scope = self.room_scope(room)?;
                self.require_target_command(actor, &scope, context.command)?;
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
                let target = self
                    .data
                    .rooms
                    .get_mut(room)
                    .ok_or_else(|| EngineError::NotFound("Room not found.".into()))?;
                if parts[0] == "/add" {
                    let added = target.members.insert(name.into());
                    if added {
                        self.add_member_color_grant(id.clone(), scope.clone());
                    }
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
                    require_len(parts, 2, "/room join #room")?;
                } else if parts.len() > 2 {
                    return Err("Usage: /room leave [#room]".into());
                }
                let user = actor.ok_or("Use /room add user #room from the console.")?;
                let name = parts
                    .get(1)
                    .map(|v| v.trim_start_matches('#'))
                    .or(room)
                    .ok_or("Select or specify a room.")?;
                let scope = self.room_scope(name)?;
                self.require_target_command(actor, &scope, context.command)?;
                if parts[0] == "/join" {
                    self.require(actor, &scope, Action::Join)?;
                    self.require(actor, &scope, Action::Read)?;
                    if self.is_su(actor) {
                        // Superusers (including sudo callers) can admit themselves.
                        // Ordinary joins only open a view with existing access.
                        let id = self
                            .data
                            .users
                            .get(user)
                            .ok_or_else(|| EngineError::Forbidden("Account unavailable.".into()))?
                            .id
                            .clone();
                        let added = self
                            .data
                            .rooms
                            .get_mut(name)
                            .ok_or_else(|| EngineError::NotFound("Room not found.".into()))?
                            .members
                            .insert(user.into());
                        if added {
                            self.add_member_color_grant(id.clone(), scope.clone());
                        }
                        self.metadata_dirty |= added;
                        self.assign(id, Group::User, scope);
                    }
                } else {
                    let id = self
                        .data
                        .users
                        .get(user)
                        .ok_or_else(|| EngineError::Forbidden("Account unavailable.".into()))?
                        .id
                        .clone();
                    if self.data.rooms.get(name).is_some_and(|r| r.owner_id == id) {
                        return Err("Transfer ownership before leaving your room.".into());
                    }
                    self.metadata_dirty |= self
                        .data
                        .rooms
                        .get_mut(name)
                        .ok_or_else(|| EngineError::NotFound("Room not found.".into()))?
                        .members
                        .remove(user);
                    let assignment_count = self.data.policy.assignments.len();
                    let grant_count = self.data.policy.grants.len();
                    self.data
                        .policy
                        .assignments
                        .retain(|a| !(a.account_id == id && a.scope == scope));
                    self.data.policy.grants.retain(|g| {
                        !(g.subject == Subject::Account(id.clone()) && g.scope == scope)
                    });
                    self.metadata_dirty |= assignment_count != self.data.policy.assignments.len()
                        || grant_count != self.data.policy.grants.len();
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
                    return Err("Usage: /room owner user [#room]".into());
                }
                let name = parts
                    .get(2)
                    .map(|v| v.trim_start_matches('#'))
                    .or(room)
                    .ok_or("Specify a room.")?;
                let scope = self.room_scope(name)?;
                self.require_target_command(actor, &scope, context.command)?;
                self.require(actor, &scope, Action::Transfer)?;
                let target_id = self
                    .data
                    .users
                    .get(parts[1])
                    .filter(|u| !u.disabled)
                    .ok_or_else(|| EngineError::NotFound("User not found.".into()))?
                    .id
                    .clone();
                self.data
                    .policy
                    .grants
                    .retain(|g| !(g.owner && g.scope == scope));
                self.add_owner_grants(target_id.clone(), scope.clone());
                self.assign(target_id.clone(), Group::User, scope.clone());
                let target = self
                    .data
                    .rooms
                    .get_mut(name)
                    .ok_or_else(|| EngineError::NotFound("Room not found.".into()))?;
                target.owner_id = target_id.clone();
                let added = target.members.insert(parts[1].into());
                if added {
                    self.add_member_color_grant(target_id, scope);
                }
                self.audit(actor, "/owner", &format!("{} {name}", parts[1]));
                Ok(format!("{} owns #{name}.", parts[1]))
            }
            "/rm" => {
                require_len(parts, 2, "/room delete #name")?;
                let scope = self.room_scope(parts[1])?;
                self.require_target_command(actor, &scope, context.command)?;
                self.require(actor, &scope, Action::DeleteRoom)?;
                self.data
                    .rooms
                    .remove(parts[1])
                    .ok_or_else(|| EngineError::NotFound("Room not found.".into()))?;
                self.data.policy.assignments.retain(|a| a.scope != scope);
                self.data.policy.grants.retain(|g| g.scope != scope);
                if let Scope::Room(id) = &scope {
                    for user in self.data.users.values_mut() {
                        user.metadata.remove(id);
                    }
                    for rooms in self.data.policy.media_revocations.values_mut() {
                        rooms.remove(id);
                    }
                    self.data
                        .policy
                        .media_revocations
                        .retain(|_, rooms| !rooms.is_empty());
                }
                self.audit(actor, "/rm", parts[1]);
                Ok(format!("Deleted #{}.", parts[1]))
            }
            _ => Err("Unknown command.".into()),
        }
    }
}
