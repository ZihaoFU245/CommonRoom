//! Name colors are the scoped `color` metadata key: `@global` is the default,
//! a stable room ID branch overrides it inside that room.
use super::*;
use authorization::{Action, Scope};

pub(crate) const COLOR_KEY: &str = "color";
/// Fixed palette; clients map each name to a theme-appropriate shade.
pub const NAME_COLORS: [&str; 16] = [
    "red", "orange", "amber", "yellow", "lime", "green", "emerald", "teal", "cyan", "sky", "blue",
    "indigo", "violet", "purple", "fuchsia", "pink",
];
pub(crate) fn valid_color(value: &str) -> bool {
    NAME_COLORS.contains(&value)
}

/// Every active account's valid colors, before per-viewer visibility filtering.
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct ColorDirectory {
    pub(super) global: BTreeMap<String, String>,
    pub(super) rooms: BTreeMap<String, BTreeMap<String, String>>,
}
impl ColorDirectory {
    pub(super) fn bytes(&self) -> usize {
        self.global
            .iter()
            .chain(self.rooms.values().flatten())
            .map(|(name, color)| (name.len() + color.len()) * 2 + 128)
            .sum::<usize>()
            .saturating_add(self.rooms.keys().map(|id| id.len() * 2 + 128).sum())
    }
}

/// Colors for names disclosed by one authorized message page. Null means default ink.
#[derive(Clone, serde::Serialize)]
pub struct MessageColors {
    pub version: String,
    pub values: BTreeMap<String, Option<String>>,
}

pub(crate) fn message_names(messages: &[Message]) -> BTreeSet<&str> {
    messages
        .iter()
        .flat_map(|message| {
            std::iter::once(message.from.as_str())
                .chain(message.reply.iter().map(|reply| reply.from.as_str()))
                .chain(message.mentions.iter().map(String::as_str))
        })
        .collect()
}
impl ColorDirectory {
    pub(crate) fn for_messages(
        &self,
        version: String,
        room: Option<&str>,
        messages: &[Message],
    ) -> MessageColors {
        let overrides = room.and_then(|id| self.rooms.get(id));
        MessageColors {
            version,
            values: message_names(messages)
                .into_iter()
                .map(|name| {
                    let color = overrides
                        .and_then(|colors| colors.get(name))
                        .or_else(|| self.global.get(name));
                    (name.to_owned(), color.cloned())
                })
                .collect(),
        }
    }
}

impl Engine {
    /// Called only after committed metadata/account identity changes.
    pub(super) fn advance_colors(&mut self) {
        let colors = self.color_directory();
        if colors != self.committed_colors {
            self.color_revision = self.color_revision.wrapping_add(1);
            self.committed_colors = colors;
        }
    }
    pub(crate) fn color_version(&self) -> String {
        format!("{}:{}", self.instance, self.color_revision)
    }
    /// Call only after authorizing the page's conversation and rechecking access.
    pub(crate) fn message_colors(&self, id: &str, messages: &[Message]) -> MessageColors {
        self.conversation_color_directory(id)
            .for_messages(self.color_version(), Some(id), messages)
    }
    pub(crate) fn conversation_color_directory(&self, id: &str) -> ColorDirectory {
        let mut directory = ColorDirectory::default();
        for (name, account) in self.data.users.iter().filter(|(_, a)| !a.disabled) {
            for scope in ["@global", id] {
                let Some(color) = account
                    .metadata
                    .get(scope)
                    .and_then(|entries| entries.get(COLOR_KEY))
                    .filter(|color| valid_color(color))
                else {
                    continue;
                };
                if scope == "@global" {
                    directory.global.insert(name.clone(), color.clone());
                } else {
                    directory
                        .rooms
                        .entry(id.to_owned())
                        .or_default()
                        .insert(name.clone(), color.clone());
                }
            }
        }
        directory
    }
    pub(crate) fn color_directory(&self) -> ColorDirectory {
        let mut directory = ColorDirectory::default();
        let rooms: BTreeSet<&str> = self.data.rooms.values().map(|r| r.id.as_str()).collect();
        for (name, account) in self.data.users.iter().filter(|(_, a)| !a.disabled) {
            for (scope, entries) in &account.metadata {
                let Some(color) = entries.get(COLOR_KEY).filter(|c| valid_color(c)) else {
                    continue;
                };
                if scope == "@global" {
                    directory.global.insert(name.clone(), color.clone());
                } else if rooms.contains(scope.as_str()) {
                    directory
                        .rooms
                        .entry(scope.clone())
                        .or_default()
                        .insert(name.clone(), color.clone());
                }
            }
        }
        directory
    }
    pub(crate) fn color_matches(
        &self,
        user: &str,
        room: Option<&str>,
        value: Option<&str>,
    ) -> bool {
        self.data
            .users
            .get(user)
            .and_then(|account| account.metadata.get(room.unwrap_or("@global")))
            .and_then(|metadata| metadata.get(COLOR_KEY))
            .map(String::as_str)
            == value
    }
    /// Validate before reserving a profile-change budget, under the same engine guard.
    pub(crate) fn validate_color(
        &self,
        user: &str,
        room: Option<&str>,
        value: Option<&str>,
    ) -> Result<bool, EngineError> {
        self.color_scope(user, room, value)
            .map(|_| !self.color_matches(user, room, value))
    }
    fn color_scope(
        &self,
        user: &str,
        room: Option<&str>,
        value: Option<&str>,
    ) -> Result<Scope, EngineError> {
        if !self.active(user) {
            return Err(EngineError::Forbidden("Account unavailable.".into()));
        }
        if value.is_some_and(|value| !valid_color(value)) {
            return Err("Unknown name color.".into());
        }
        let scope = match room {
            None => Scope::Server,
            Some(id) => {
                let scope = Scope::Room(id.to_owned());
                // Server grants apply to every scope, so existence and readability
                // are checked first and share one error for unknown room IDs.
                if !self.data.rooms.values().any(|r| r.id == id)
                    || !self.allows(Some(user), &scope, Action::Read)
                {
                    return Err(EngineError::NotFound("Room not found.".into()));
                }
                scope
            }
        };
        self.require(Some(user), &scope, Action::ColorOwn)?;
        let key = Self::metadata_scope_key(&scope)?;
        if value.is_some()
            && self.data.users[user]
                .metadata
                .get(key)
                .is_some_and(|entries| entries.len() >= 32 && !entries.contains_key(COLOR_KEY))
        {
            return Err("At most 32 metadata keys per scope.".into());
        }
        Ok(scope)
    }
    /// Sets or removes the caller's own color after validating scope and permission.
    pub(crate) fn set_color(
        &mut self,
        user: &str,
        room: Option<&str>,
        value: Option<&str>,
    ) -> Result<bool, EngineError> {
        let scope = self.color_scope(user, room, value)?;
        if self.color_matches(user, room, value) {
            return Ok(false);
        }
        let scope_key = Self::metadata_scope_key(&scope)?.to_owned();
        let before = MetadataChange::accounts(&self.data, [user.to_owned()], false);
        let account = self
            .data
            .users
            .get_mut(user)
            .ok_or_else(|| EngineError::Forbidden("Account unavailable.".into()))?;
        let metadata = account.metadata.entry(scope_key).or_default();
        if let Some(value) = value {
            metadata.insert(COLOR_KEY.into(), value.into());
        } else {
            metadata.remove(COLOR_KEY);
        }
        account.metadata.retain(|_, entries| !entries.is_empty());
        if let Err(error) = self.save_metadata(&[], changes::Baseline::Captured(&before)) {
            before.restore(&mut self.data);
            return Err(error);
        }
        Ok(true)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    fn colors(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, color)| ((*name).into(), (*color).into()))
            .collect()
    }
    fn room_colors(e: &Engine, viewer: &str, room: &str) -> BTreeMap<String, String> {
        e.snapshot(viewer)
            .unwrap()
            .rooms
            .into_iter()
            .find(|r| r.name == room)
            .unwrap()
            .colors
    }
    #[test]
    fn global_colors_are_validated_stored_as_metadata_and_survive_restart_and_rename() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite");
        {
            let mut e = Engine::open(&path).unwrap();
            for name in ["alice", "bob", "eve"] {
                e.provision(name, "hash".into(), false, false).unwrap();
            }
            assert!(e.set_color("bob", None, Some("blue")).unwrap());
            assert_eq!(e.data.users["bob"].metadata["@global"][COLOR_KEY], "blue");
            let revision = e.revision;
            assert!(!e.set_color("bob", None, Some("blue")).unwrap());
            for invalid in ["Blue", "#ff0000", "", "white"] {
                assert!(
                    e.set_color("bob", None, Some(invalid)).is_err(),
                    "{invalid}"
                );
            }
            assert!(e.set_color("missing", None, Some("red")).is_err());
            assert_eq!(e.revision, revision);
            e.set_color("eve", None, Some("pink")).unwrap();
            assert_eq!(
                e.snapshot("alice").unwrap().colors,
                colors(&[("bob", "blue"), ("eve", "pink")])
            );
            e.execute(None, None, "/revoke eve user @global").unwrap();
            assert_eq!(
                e.snapshot("eve").unwrap().colors,
                colors(&[("eve", "pink")])
            );
            e.execute(None, None, "/user rename bob robert").unwrap();
            e.execute(None, None, "/user disable eve").unwrap();
            assert!(e.set_color("eve", None, Some("red")).is_err());
        }
        let mut e = Engine::open(&path).unwrap();
        assert_eq!(
            e.data.users["robert"].metadata["@global"][COLOR_KEY],
            "blue"
        );
        assert!(e.snapshot("alice").unwrap().colors == colors(&[("robert", "blue")]));
        assert!(e.set_color("robert", None, None).unwrap());
        assert!(!e.data.users["robert"].metadata.contains_key("@global"));
    }
    #[test]
    fn room_overrides_resolve_per_room_and_are_removed_with_the_room() {
        let mut e = super::super::tests::engine();
        let lobby = e.data.rooms["lobby"].id.clone();
        e.set_color("bob", None, Some("blue")).unwrap();
        e.set_color("bob", Some(&lobby), Some("red")).unwrap();
        let snapshot = e.snapshot("alice").unwrap();
        assert_eq!(snapshot.colors, colors(&[("bob", "blue")]));
        assert_eq!(room_colors(&e, "alice", "lobby"), colors(&[("bob", "red")]));
        for missing in ["unknown", "@global", ""] {
            assert!(matches!(
                e.set_color("bob", Some(missing), Some("red")),
                Err(EngineError::NotFound(_))
            ));
        }
        e.execute(None, None, "/room new secret").unwrap();
        e.execute(None, None, "/room add alice #secret").unwrap();
        let secret = e.data.rooms["secret"].id.clone();
        assert!(matches!(
            e.set_color("bob", Some(&secret), Some("red")),
            Err(EngineError::NotFound(_))
        ));
        e.execute(None, None, "/room delete #lobby").unwrap();
        assert!(!e.data.users["bob"].metadata.contains_key(&lobby));
        assert!(e.color_directory().rooms.is_empty());
    }
    #[test]
    fn color_permission_is_scoped_additive_and_other_accounts_stay_su_only() {
        let mut e = super::super::tests::engine();
        let lobby = e.data.rooms["lobby"].id.clone();
        e.execute(None, None, "/revoke bob user @global").unwrap();
        assert!(matches!(
            e.set_color("bob", None, Some("red")),
            Err(EngineError::Forbidden(_))
        ));
        e.set_color("bob", Some(&lobby), Some("red")).unwrap();
        e.execute(None, None, "/revoke bob user #lobby").unwrap();
        e.execute(None, None, "/grant bob #lobby r:message.read")
            .unwrap();
        assert!(matches!(
            e.set_color("bob", Some(&lobby), Some("teal")),
            Err(EngineError::Forbidden(_))
        ));
        e.execute(None, None, "/grant bob #lobby w:account.color.own")
            .unwrap();
        e.set_color("bob", Some(&lobby), Some("teal")).unwrap();
        assert!(
            e.execute(Some("alice"), Some("lobby"), "/user set bob color red")
                .is_err()
        );
        assert!(
            e.execute(None, None, "/user set bob color white @global")
                .is_err()
        );
        e.execute(None, None, "/user set bob color lime @global")
            .unwrap();
        e.execute(None, None, "/user set eve color sky #lobby")
            .unwrap();
        assert_eq!(e.data.users["bob"].metadata["@global"][COLOR_KEY], "lime");
        assert_eq!(e.data.users["eve"].metadata[&lobby][COLOR_KEY], "sky");
    }
    #[test]
    fn room_overrides_disclose_only_names_that_room_already_reveals() {
        let mut e = super::super::tests::engine();
        let lobby = e.data.rooms["lobby"].id.clone();
        e.set_color("bob", Some(&lobby), Some("red")).unwrap();
        e.set_color("eve", Some(&lobby), Some("pink")).unwrap();
        e.execute(None, None, "/revoke eve user #lobby").unwrap();
        e.execute(None, None, "/grant eve #lobby r:message.read")
            .unwrap();
        assert_eq!(room_colors(&e, "eve", "lobby"), colors(&[("eve", "pink")]));
        e.execute(Some("bob"), Some("lobby"), "Visible author")
            .unwrap();
        assert_eq!(
            room_colors(&e, "eve", "lobby"),
            colors(&[("bob", "red"), ("eve", "pink")])
        );
        assert_eq!(
            room_colors(&e, "alice", "lobby"),
            colors(&[("bob", "red"), ("eve", "pink")])
        );
    }
    #[test]
    fn color_changes_notify_every_viewer_and_roll_back_failed_writes() {
        let mut e = super::super::tests::engine();
        let alice = e.active_account_id("alice").unwrap().to_owned();
        e.set_color("bob", None, Some("red")).unwrap();
        assert!(e.metadata_change().targets(&alice));
        let before = e.data.clone();
        let version = e.color_version();
        e.store.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_color BEFORE UPDATE ON entities WHEN OLD.kind='account' BEGIN SELECT RAISE(FAIL,'injected'); END;").unwrap();
        assert!(e.set_color("bob", None, Some("blue")).is_err());
        assert!(e.data == before);
        assert_eq!(e.color_version(), version);
    }

    #[test]
    fn color_versions_follow_projection_changes_only_and_history_opt_in_is_explicit() {
        let mut e = super::super::tests::engine();
        let version = e.color_version();
        e.set_avatar_hosts(["example.com".into()].into());
        e.set_avatar("bob", Some("https://example.com/avatar.png".into()), false)
            .unwrap();
        for command in [
            "/room new unused",
            "/user set bob unrelated value @global",
            "/grant bob @global r:account.list",
        ] {
            e.execute(None, None, command).unwrap();
            assert_eq!(e.color_version(), version, "{command}");
        }
        e.execute(Some("bob"), Some("lobby"), "A message").unwrap();
        assert!(e.history("alice", "lobby").unwrap().name_colors.is_none());
        assert_eq!(e.color_version(), version);
        let lobby = e.data.rooms["lobby"].id.clone();
        let mut media = e.allocate_media("bob", &lobby, "photo.png", 8).unwrap();
        media.ready = true;
        media.object_key = media.final_key();
        media.content_type = "image/png".into();
        e.finish_media("bob", &media).unwrap();
        e.allocate_media_target("bob", "@direct:eve", "file.txt", 8, BTreeMap::new(), false)
            .unwrap();
        e.execute(Some("bob"), None, "/tell alice Private message")
            .unwrap();
        assert_eq!(
            e.color_version(),
            version,
            "media writes and new private chats leave colors unchanged"
        );
        e.set_color("bob", Some(&lobby), Some("red")).unwrap();
        assert_ne!(e.color_version(), version);
        let mut version = e.color_version();
        for command in [
            "/user set bob color blue @global",
            "/user rename bob robert",
            "/user disable robert",
            "/user enable robert",
            "/user reset robert color @global",
            "/room delete #lobby",
            "/user set robert color teal @global",
            "/user delete robert",
        ] {
            e.execute(None, None, command).unwrap();
            assert_ne!(e.color_version(), version, "{command}");
            version = e.color_version();
        }
    }

    #[test]
    fn message_colors_follow_live_and_paged_disclosures_without_directory_access() {
        let mut e = super::super::tests::engine();
        let lobby = e.data.rooms["lobby"].id.clone();
        e.set_color("bob", None, Some("blue")).unwrap();
        e.set_color("bob", Some(&lobby), Some("red")).unwrap();
        e.set_color("alice", Some(&lobby), Some("lime")).unwrap();
        e.execute(None, None, "/revoke eve user @global").unwrap();
        e.execute(None, None, "/revoke eve user #lobby").unwrap();
        e.execute(None, None, "/grant eve #lobby r:message.read")
            .unwrap();
        let baseline = e.snapshot("eve").unwrap();
        assert!(baseline.colors.is_empty());
        assert!(room_colors(&e, "eve", "lobby").is_empty());
        e.execute(Some("bob"), Some("lobby"), "First author")
            .unwrap();
        let frame = e
            .conversation_plan("eve", &lobby)
            .unwrap()
            .unwrap()
            .finish()
            .unwrap();
        assert_eq!(frame["name_colors"]["version"], baseline.color_version);
        assert_eq!(
            frame["name_colors"]["values"],
            serde_json::json!({"bob":"red"})
        );
        let first = e.history("eve", "lobby").unwrap().messages[0].sequence;
        for _ in 0..51 {
            e.execute(Some("alice"), Some("lobby"), "Newer message")
                .unwrap();
        }
        assert!(!room_colors(&e, "eve", "lobby").contains_key("bob"));
        let range = HistoryRange {
            before: Some(first + 1),
            ..Default::default()
        };
        let page = e
            .history_page_with_colors("eve", "lobby", range, true)
            .unwrap();
        assert_eq!(
            page.name_colors.unwrap().values,
            [("bob".into(), Some("red".into()))].into()
        );
        e.set_color("bob", Some(&lobby), None).unwrap();
        let inherited = e
            .history_page_with_colors("eve", "lobby", range, true)
            .unwrap()
            .name_colors
            .unwrap();
        assert_ne!(inherited.version, baseline.color_version);
        assert_eq!(inherited.values["bob"].as_deref(), Some("blue"));
        e.set_color("bob", None, None).unwrap();
        let reset = e
            .history_page_with_colors("eve", "lobby", range, true)
            .unwrap()
            .name_colors
            .unwrap();
        assert_ne!(reset.version, inherited.version);
        assert_eq!(reset.values["bob"], None);
    }

    #[test]
    fn reply_and_mention_colors_are_disclosed_but_other_room_profiles_are_not() {
        let mut e = super::super::tests::engine();
        let lobby = e.data.rooms["lobby"].id.clone();
        e.set_color("bob", Some(&lobby), Some("red")).unwrap();
        e.set_color("eve", Some(&lobby), Some("pink")).unwrap();
        e.execute(Some("bob"), Some("lobby"), "Quoted author")
            .unwrap();
        let id = e.history("alice", "lobby").unwrap().messages[0].id.clone();
        e.execute(
            Some("alice"),
            Some("lobby"),
            &format!("/reply {id} Hello @eve"),
        )
        .unwrap();
        let page = e
            .history_page_with_colors(
                "alice",
                "lobby",
                HistoryRange {
                    limit: Some(1),
                    ..Default::default()
                },
                true,
            )
            .unwrap();
        let colors = page.name_colors.unwrap();
        assert_eq!(colors.values["bob"].as_deref(), Some("red"));
        assert_eq!(colors.values["eve"].as_deref(), Some("pink"));
        assert_eq!(colors.values["alice"], None);
        e.set_color("bob", None, Some("blue")).unwrap();
        e.execute(Some("bob"), None, "/tell alice Private colors")
            .unwrap();
        let private = e
            .history_page_with_colors("alice", "@direct:bob", HistoryRange::default(), true)
            .unwrap()
            .name_colors
            .unwrap();
        assert_eq!(private.values, [("bob".into(), Some("blue".into()))].into());
        let version = e.color_version();
        let replacement = Engine::open(std::path::Path::new(":memory:")).unwrap();
        assert_ne!(version, replacement.color_version());
    }
}
