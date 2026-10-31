//! Optional global display names and stable-room overrides share account metadata.
use super::*;

impl Engine {
    pub(crate) fn prepare_display_name(
        &mut self,
        user: &str,
        value: Option<String>,
        room: Option<String>,
    ) -> Result<Option<metadata_write::PendingMetadata>, EngineError> {
        if !self.active(user) {
            return Err(EngineError::Forbidden("Account unavailable.".into()));
        }
        let value = value
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if let Some(value) = &value {
            self.validate_display_name(value)?;
        }
        let scope = room.map_or(authorization::Scope::Server, authorization::Scope::Room);
        if let authorization::Scope::Room(id) = &scope {
            if !self.data.rooms.values().any(|room| &room.id == id)
                || !self.allows(Some(user), &scope, authorization::Action::Read)
            {
                return Err(EngineError::NotFound("Room not found.".into()));
            }
            if !self.allows(Some(user), &scope, authorization::Action::DisplayNameAny) {
                self.require(Some(user), &scope, authorization::Action::DisplayNameOwn)?;
            }
        }
        let scope_key = Self::metadata_scope_key(&scope)?.to_owned();
        let before = MetadataChange::accounts(&self.data, [user.to_owned()], false);
        let result = (|| {
            let account = self
                .data
                .users
                .get_mut(user)
                .ok_or("Account unavailable.")?;
            let root = account.metadata.entry(scope_key).or_default();
            if let Some(value) = value {
                if super::preferences::metadata_full(root, "displayname") {
                    return Err("At most 32 metadata keys per scope.".into());
                }
                root.insert("displayname".into(), value);
            } else {
                root.remove("displayname");
            }
            account.metadata.retain(|_, entries| !entries.is_empty());
            if before.unchanged(&self.data) {
                return Ok(None);
            }
            self.capture_metadata(&before, vec![]).map(Some)
        })();
        before.restore(&mut self.data);
        result
    }

    pub(super) fn validate_display_name(&self, value: &str) -> Result<(), EngineError> {
        if value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
            return Err("Display name must contain 1–256 bytes without control characters.".into());
        }
        Ok(())
    }
}

#[derive(Clone, Default, Hash)]
pub(super) struct DisplayNameDirectory {
    pub(super) global: BTreeMap<String, String>,
    pub(super) rooms: BTreeMap<String, BTreeMap<String, String>>,
}
#[derive(Clone, serde::Serialize)]
pub struct MessageDisplayNames {
    pub version: String,
    pub values: BTreeMap<String, Option<String>>,
}
impl DisplayNameDirectory {
    pub(super) fn for_messages(
        &self,
        version: String,
        room: &str,
        messages: &[Message],
    ) -> MessageDisplayNames {
        let overrides = self.rooms.get(room);
        MessageDisplayNames {
            version,
            values: colors::message_names(messages)
                .into_iter()
                .map(|name| {
                    (
                        name.to_owned(),
                        overrides
                            .and_then(|names| names.get(name))
                            .or_else(|| self.global.get(name))
                            .cloned(),
                    )
                })
                .collect(),
        }
    }
    pub(super) fn bytes(&self) -> usize {
        self.global
            .iter()
            .chain(self.rooms.values().flatten())
            .map(|(name, value)| (name.len() + value.len()) * 2 + 128)
            .sum::<usize>()
            .saturating_add(self.rooms.keys().map(|id| id.len() * 2 + 128).sum())
    }
}
impl Engine {
    pub(crate) fn display_name_version(&self) -> String {
        use std::hash::{Hash, Hasher};
        let mut hash = std::hash::DefaultHasher::new();
        self.display_name_directory().hash(&mut hash);
        format!("{}:{}", self.instance, hash.finish())
    }
    pub(super) fn display_name_directory(&self) -> DisplayNameDirectory {
        let mut directory = DisplayNameDirectory::default();
        for (name, account) in &self.data.users {
            for (scope, entries) in &account.metadata {
                let Some(value) = entries
                    .get("displayname")
                    .filter(|value| self.validate_display_name(value).is_ok())
                else {
                    continue;
                };
                if scope == "@global" {
                    directory.global.insert(name.clone(), value.clone());
                } else if self.data.rooms.values().any(|room| &room.id == scope) {
                    directory
                        .rooms
                        .entry(scope.clone())
                        .or_default()
                        .insert(name.clone(), value.clone());
                }
            }
        }
        directory
    }
    pub(crate) fn message_display_names(
        &self,
        room: &str,
        messages: &[Message],
    ) -> MessageDisplayNames {
        self.display_name_directory()
            .for_messages(self.display_name_version(), room, messages)
    }
    pub(super) fn display_name_access(
        &self,
        actor: Option<&str>,
        name: &str,
        scope: &authorization::Scope,
    ) -> Result<(), EngineError> {
        use authorization::{Action, Scope};
        if !self.is_su(actor) {
            let own_global = *scope == Scope::Server && actor == Some(name) && self.active(name);
            let own_room = matches!(scope, Scope::Room(_))
                && actor == Some(name)
                && self.allows(actor, scope, Action::DisplayNameOwn);
            let manage_room = matches!(scope, Scope::Room(_))
                && self.allows(actor, scope, Action::DisplayNameAny);
            if !own_global && !own_room && !manage_room {
                return Err(EngineError::Forbidden("Display name write requires account ownership at @global, or w:account.displayname.own/any in the target room.".into()));
            }
            let protected = self
                .data
                .users
                .get(name)
                .and_then(|account| self.authorization.groups.get(&account.id))
                .is_some_and(|groups| groups.contains(&authorization::Group::Su));
            if protected && actor != Some(name) {
                return Err(EngineError::Forbidden(
                    "Only su can manage a su account.".into(),
                ));
            }
        }
        self.require_target_command(actor, scope, "/user")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn display_names_follow_authorized_live_and_paged_authors_and_room_inheritance() {
        let mut engine = super::super::tests::engine();
        let room = engine.data.rooms["lobby"].id.clone();
        engine
            .execute(None, None, "/user set bob displayname Global @global")
            .unwrap();
        engine
            .execute(None, None, "/user set bob displayname Local #lobby")
            .unwrap();
        engine
            .execute(None, None, "/revoke eve user @global")
            .unwrap();
        engine
            .execute(None, None, "/revoke eve user #lobby")
            .unwrap();
        engine
            .execute(None, None, "/grant eve #lobby r:message.read")
            .unwrap();
        assert!(engine.snapshot("eve").unwrap().display_names.is_empty());
        engine.execute(Some("bob"), Some("lobby"), "First").unwrap();
        let frame = engine
            .conversation_plan("eve", &room)
            .unwrap()
            .unwrap()
            .finish()
            .unwrap();
        assert_eq!(
            frame["message_display_names"]["values"],
            serde_json::json!({"bob":"Local"})
        );
        let first = engine.history("eve", "lobby").unwrap().messages[0].sequence;
        for _ in 0..51 {
            engine
                .execute(Some("alice"), Some("lobby"), "Newer")
                .unwrap();
        }
        assert!(
            !engine
                .snapshot("eve")
                .unwrap()
                .display_names
                .contains_key("bob")
        );
        let range = HistoryRange {
            before: Some(first + 1),
            ..Default::default()
        };
        let page = engine
            .history_page("eve", "lobby", range)
            .unwrap()
            .message_display_names
            .unwrap();
        assert_eq!(page.values["bob"].as_deref(), Some("Local"));
        engine
            .execute(None, None, "/user reset bob displayname #lobby")
            .unwrap();
        let inherited = engine
            .history_page("eve", "lobby", range)
            .unwrap()
            .message_display_names
            .unwrap();
        assert_ne!(inherited.version, page.version);
        assert_eq!(inherited.values["bob"].as_deref(), Some("Global"));
        engine
            .execute(Some("bob"), None, "/tell alice Private")
            .unwrap();
        let private = engine
            .history("alice", "@direct:bob")
            .unwrap()
            .message_display_names
            .unwrap();
        assert_eq!(private.values["bob"].as_deref(), Some("Global"));
        engine
            .execute(None, None, "/user reset bob displayname @global")
            .unwrap();
        let reset = engine
            .history_page("eve", "lobby", range)
            .unwrap()
            .message_display_names
            .unwrap();
        assert_eq!(reset.values["bob"], None);
    }
}
