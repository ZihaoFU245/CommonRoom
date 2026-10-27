//! Self-service avatars use one public profile metadata key, never arbitrary metadata writes.
use super::media::Media;
use super::*;

pub(crate) const MAX_AVATAR_SIZE: u64 = 64 * 1024;
pub(crate) fn avatar_path(id: &str) -> String {
    format!("/api/avatars/{id}/file")
}
pub(crate) fn valid_avatar_url(value: &str) -> bool {
    if value.len() > 2048 || value.chars().any(char::is_control) {
        return false;
    }
    url::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
    })
}
/// Only owned authorization inputs cross the history read boundary.
pub(crate) struct AvatarPlan {
    owner: String,
    value: Option<String>,
    known: bool,
    conversations: Vec<String>,
    history: std::sync::Arc<HistoryStore>,
}
impl AvatarPlan {
    pub(crate) fn history_ids(&self) -> impl Iterator<Item = &str> {
        self.conversations
            .iter()
            .filter(|_| !self.known)
            .map(String::as_str)
    }

    #[cfg(test)]
    pub(crate) fn finish(self) -> Result<Option<serde_json::Value>, EngineError> {
        database::block_on_storage(self.finish_async())?
    }
    pub(crate) async fn finish_async(self) -> Result<Option<serde_json::Value>, EngineError> {
        if !self.known
            && !self
                .history
                .has_author_in_async(&self.conversations, &self.owner)
                .await?
        {
            return Ok(None);
        }
        Ok(Some(
            serde_json::json!({"kind":"avatar","username":self.owner,"url":self.value}),
        ))
    }
}

impl Engine {
    pub(crate) fn avatar_plan(&self, viewer: &str, owner_id: &str) -> Option<AvatarPlan> {
        use authorization::{Action, Scope};
        if !self.active(viewer) {
            return None;
        }
        let owner = self.active_account_name(owner_id)?;
        let mut known =
            viewer == owner || self.allows(Some(viewer), &Scope::Server, Action::Directory);
        let mut conversations = Vec::new();
        if !known {
            for room in self.data.rooms.values() {
                let scope = Scope::Room(room.id.clone());
                if self.allows(Some(viewer), &scope, Action::Read) {
                    known |= self.allows(Some(viewer), &scope, Action::Members)
                        && room.members.contains(owner);
                    conversations.push(room.id.clone());
                }
            }
            for (key, chat) in &self.data.private {
                let scope = self.private_scope(key);
                if self.allows(Some(viewer), &scope, Action::Read) {
                    known |= private_peer(key, viewer) == Some(owner)
                        || (self.allows(Some(viewer), &scope, Action::Members)
                            && key.split(':').any(|name| name == owner));
                    conversations.push(chat.id.clone());
                }
            }
        }
        let value = self.avatar_directory_for(&[owner].into()).remove(owner);
        Some(AvatarPlan {
            owner: owner.into(),
            value,
            known,
            conversations,
            history: self.history_store().clone(),
        })
    }

    pub(crate) fn set_avatar_hosts(&mut self, hosts: BTreeSet<String>) {
        if self.media.avatar_hosts != hosts {
            self.read_model.advance();
            self.media.avatar_hosts = hosts;
        }
    }

    fn allowed_avatar_url(&self, value: &str) -> bool {
        self.media.allows_avatar(value)
    }

    #[cfg(test)]
    pub(crate) fn avatar_directory(&self) -> BTreeMap<String, String> {
        self.avatar_directory_for(&self.data.users.keys().map(String::as_str).collect())
    }
    pub(crate) fn avatar_directory_for(
        &self,
        visible: &BTreeSet<&str>,
    ) -> BTreeMap<String, String> {
        visible
            .iter()
            .filter_map(|name| {
                let account = self.data.users.get(*name)?;
                if account.disabled {
                    return None;
                }
                let value = account.metadata.get("@global")?.get("avatar")?;
                (self.allowed_avatar_url(value)
                    || value
                        .strip_prefix("/api/avatars/")
                        .and_then(|s| s.strip_suffix("/file"))
                        .is_some_and(|id| {
                            uuid::Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == id)
                        }))
                .then(|| ((*name).to_owned(), value.clone()))
            })
            .collect()
    }
    #[cfg(test)]
    pub(crate) fn set_avatar(
        &mut self,
        user: &str,
        value: Option<String>,
        managed: bool,
    ) -> Result<bool, EngineError> {
        if !self.active(user) {
            return Err(EngineError::Forbidden("Account unavailable.".into()));
        }
        if let Some(value) = &value
            && !managed
            && !self.allowed_avatar_url(value)
        {
            return Err(
                "Avatar URL must use an allowed HTTPS host without credentials (maximum 2048 bytes).".into(),
            );
        }
        if self
            .data
            .users
            .get(user)
            .and_then(|account| account.metadata.get("@global"))
            .and_then(|metadata| metadata.get("avatar"))
            == value.as_ref()
        {
            return Ok(false);
        }
        let before = MetadataChange::accounts(&self.data, [user.to_owned()], false);
        let account = self
            .data
            .users
            .get_mut(user)
            .ok_or_else(|| EngineError::Forbidden("Account unavailable.".into()))?;
        let metadata = account.metadata.entry("@global".into()).or_default();
        if let Some(value) = value {
            if metadata.len() >= 32 && !metadata.contains_key("avatar") {
                return Err("At most 32 metadata keys per scope.".into());
            }
            metadata.insert("avatar".into(), value);
        } else {
            metadata.remove("avatar");
        }
        account.metadata.retain(|_, entries| !entries.is_empty());
        if let Err(error) = self.save_metadata(&[], changes::Baseline::Captured(&before)) {
            before.restore(&mut self.data);
            return Err(error);
        }
        Ok(true)
    }
    #[cfg(test)]
    pub(crate) fn allocate_avatar(&self, user: &str, size: u64) -> Result<Media, EngineError> {
        self.allocate_avatar_id(user, size, &uuid::Uuid::new_v4().to_string())
    }
    #[cfg(test)]
    pub(crate) fn allocate_avatar_id(
        &self,
        user: &str,
        size: u64,
        id: &str,
    ) -> Result<Media, EngineError> {
        if !self.active(user) {
            return Err(EngineError::Forbidden("Account unavailable.".into()));
        }
        if !uuid::Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == id) {
            return Err("Invalid avatar upload ID.".into());
        }
        if size == 0 || size > MAX_AVATAR_SIZE {
            return Err("Avatar must be between 1 byte and 64 KiB.".into());
        }
        let account = self
            .data
            .users
            .get(user)
            .ok_or_else(|| EngineError::Forbidden("Account unavailable.".into()))?;
        if let Ok(existing) = self.stored_media(id) {
            let existing = self.avatar_owner(user, &existing.id)?;
            if existing.size != size || existing.ready {
                return Err("Avatar upload changed.".into());
            }
            return Ok(existing);
        }
        let media = Media {
            id: id.to_owned(),
            uploader: account.id.clone(),
            conversation: "@avatar".into(),
            filename: "avatar.png".into(),
            hints: BTreeMap::new(),
            size,
            content_type: "application/octet-stream".into(),
            object_key: String::new(),
            ready: false,
            created: now(),
            max_age: -1,
            expires: None,
            deleting: false,
            deleted: false,
        };
        self.store_media_allocation(&media)?;
        Ok(media)
    }
    #[cfg(test)]
    pub(crate) fn avatar_owner(&self, user: &str, id: &str) -> Result<Media, EngineError> {
        let media = self.stored_media(id)?;
        self.authorize_avatar_owner(user, &media)?;
        Ok(media)
    }
    pub(crate) fn authorize_avatar_owner(
        &self,
        user: &str,
        media: &Media,
    ) -> Result<(), EngineError> {
        if !self.active(user)
            || self
                .data
                .users
                .get(user)
                .is_none_or(|u| u.id != media.uploader)
            || media.conversation != "@avatar"
            || media.deleting
            || media.deleted
            || now().saturating_sub(media.created) >= self.media.config.pending_ttl
        {
            return Err("Avatar upload unavailable.".into());
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn finish_avatar(&mut self, user: &str, media: &Media) -> Result<bool, EngineError> {
        let current = self.avatar_owner(user, &media.id)?;
        if !media.ready
            || !matches!(
                media.content_type.as_str(),
                "image/png" | "image/jpeg" | "image/webp"
            )
        {
            return Err("Avatar must be a PNG, JPEG or WebP image.".into());
        }
        if !current.ready {
            self.store_media_ready(media)?;
        }
        self.set_avatar(user, Some(avatar_path(&media.id)), true)
    }
    #[cfg(test)]
    pub(crate) fn readable_avatar(&self, viewer: &str, id: &str) -> Result<Media, EngineError> {
        let media = self.stored_media(id)?;
        let owner = self
            .bot_index
            .accounts
            .get(&media.uploader)
            .ok_or("Avatar not found.")?;
        self.avatar_read_plan(viewer, &media)?;
        if !self.avatar_visible(viewer, owner)? {
            return Err(EngineError::NotFound("Avatar not found.".into()));
        }
        Ok(media)
    }
    pub(crate) fn avatar_read_plan(
        &self,
        viewer: &str,
        media: &Media,
    ) -> Result<AvatarPlan, EngineError> {
        let owner = self
            .bot_index
            .accounts
            .get(&media.uploader)
            .ok_or("Avatar not found.")?;
        if media.conversation != "@avatar"
            || !media.ready
            || media.deleting
            || media.deleted
            || !self.active(viewer)
            || !self.active(owner)
            || self
                .data
                .users
                .get(owner)
                .and_then(|account| account.metadata.get("@global"))
                .and_then(|metadata| metadata.get("avatar"))
                != Some(&avatar_path(&media.id))
        {
            return Err(EngineError::NotFound("Avatar not found.".into()));
        }
        self.avatar_plan(viewer, &media.uploader)
            .ok_or_else(|| EngineError::NotFound("Avatar not found.".into()))
    }
    #[cfg(test)]
    fn avatar_visible(&self, viewer: &str, owner: &str) -> Result<bool, EngineError> {
        use authorization::{Action, Scope};
        if viewer == owner || self.allows(Some(viewer), &Scope::Server, Action::Directory) {
            return Ok(true);
        }
        for room in self.data.rooms.values() {
            let scope = Scope::Room(room.id.clone());
            if self.allows(Some(viewer), &scope, Action::Read)
                && ((self.allows(Some(viewer), &scope, Action::Members)
                    && room.members.contains(owner))
                    || self.history_store().has_author(&room.id, owner)?)
            {
                return Ok(true);
            }
        }
        for (key, chat) in &self.data.private {
            let scope = self.private_scope(key);
            if self.allows(Some(viewer), &scope, Action::Read)
                && (private_peer(key, viewer) == Some(owner)
                    || (self.allows(Some(viewer), &scope, Action::Members)
                        && key.split(':').any(|name| name == owner))
                    || self.history_store().has_author(&chat.id, owner)?)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    #[test]
    fn completing_an_old_avatar_does_not_restore_a_replaced_profile() {
        let mut engine = super::super::tests::engine();
        let media = ready(&engine, "alice");
        engine.finish_avatar("alice", &media).unwrap();
        engine.set_avatar("alice", None, false).unwrap();
        let current = engine.stored_media(&media.id).unwrap();
        assert!(
            engine
                .prepare_avatar_completion("alice", media, &current)
                .unwrap()
                .is_none()
        );
        assert!(!engine.avatar_directory().contains_key("alice"));
    }
    fn ready(engine: &Engine, user: &str) -> Media {
        let mut media = engine.allocate_avatar(user, 64).unwrap();
        media.ready = true;
        media.object_key = media.final_key();
        media.content_type = "image/png".into();
        media
    }
    #[test]
    fn profile_frames_match_live_directory_member_and_historical_author_visibility() {
        let mut e = super::super::tests::engine();
        e.set_avatar_hosts(["example.com".into()].into());
        let owner = e.active_account_id("alice").unwrap().to_owned();
        e.set_avatar("alice", Some("https://example.com/a.png".into()), false)
            .unwrap();
        assert!(
            e.avatar_plan("bob", &owner)
                .unwrap()
                .finish()
                .unwrap()
                .is_some()
        );
        e.execute(None, None, "/revoke bob user @global").unwrap();
        assert!(
            e.avatar_plan("bob", &owner)
                .unwrap()
                .finish()
                .unwrap()
                .is_some()
        );
        e.execute(None, None, "/revoke bob user #lobby").unwrap();
        e.execute(None, None, "/grant bob #lobby r:message.read")
            .unwrap();
        assert!(
            e.avatar_plan("bob", &owner)
                .unwrap()
                .finish()
                .unwrap()
                .is_none()
        );
        e.execute(Some("alice"), Some("lobby"), "Retained author")
            .unwrap();
        let value = e
            .avatar_plan("bob", &owner)
            .unwrap()
            .finish()
            .unwrap()
            .unwrap();
        assert_eq!(
            value,
            serde_json::json!({"kind":"avatar","username":"alice","url":"https://example.com/a.png"})
        );
        assert!(value.get("messages").is_none());
        e.set_avatar("alice", None, false).unwrap();
        assert!(
            e.avatar_plan("bob", &owner)
                .unwrap()
                .finish()
                .unwrap()
                .unwrap()["url"]
                .is_null()
        );
        e.execute(None, None, "/clean -1 #lobby").unwrap();
        assert!(
            e.avatar_plan("bob", &owner)
                .unwrap()
                .finish()
                .unwrap()
                .is_none()
        );
        e.execute(None, None, "/user disable alice").unwrap();
        assert!(e.avatar_plan("bob", &owner).is_none());
    }
    #[test]
    fn external_avatar_hosts_are_denied_by_default_and_filtered_after_policy_changes() {
        let mut e = super::super::tests::engine();
        assert!(
            e.set_avatar("bob", Some("https://example.com/a.png".into()), false)
                .is_err()
        );
        e.media.avatar_hosts.insert("example.com".into());
        e.set_avatar("bob", Some("https://example.com/a.png".into()), false)
            .unwrap();
        for value in [
            "https://tracker.example/a",
            "https://example.com.evil/a",
            "https://evil.example.com/a",
            "https://example.com@evil.test/a",
        ] {
            assert!(e.set_avatar("bob", Some(value.into()), false).is_err());
        }
        assert!(e.snapshot("alice").unwrap().avatars.contains_key("bob"));
        e.set_avatar_hosts(BTreeSet::new());
        assert!(!e.snapshot("alice").unwrap().avatars.contains_key("bob"));
    }
    #[test]
    fn self_service_url_validation_remove_and_metadata_isolation() {
        let mut e = super::super::tests::engine();
        e.media
            .avatar_hosts
            .extend(["example.com".into(), "www.gravatar.com".into()]);
        let before = e.data.users["alice"].clone();
        for url in [
            "http://example.com/a.png",
            "javascript:alert(1)",
            "data:image/png;base64,AA",
            "https://a:b@example.com/a",
            "https://example.com/\n",
            "/api/avatars/forged/file",
        ] {
            assert!(
                e.set_avatar("bob", Some(url.into()), false).is_err(),
                "{url}"
            );
        }
        assert!(e.set_avatar("missing", None, false).is_err());
        e.set_avatar(
            "bob",
            Some("https://www.gravatar.com/avatar/example?s=128".into()),
            false,
        )
        .unwrap();
        assert_eq!(
            e.snapshot("alice").unwrap().avatars["bob"],
            "https://www.gravatar.com/avatar/example?s=128"
        );
        assert!(e.data.users["alice"] == before);
        assert!(
            e.execute(
                Some("bob"),
                None,
                "/user set alice avatar https://example.com/a"
            )
            .is_err()
        );
        e.set_avatar("bob", None, false).unwrap();
        assert!(!e.snapshot("alice").unwrap().avatars.contains_key("bob"));
    }
    #[test]
    fn avatar_upload_ownership_type_limits_and_cleanup_lifecycle() {
        let mut e = super::super::tests::engine();
        e.media
            .avatar_hosts
            .extend(["example.com".into(), "www.gravatar.com".into()]);
        for size in [0, MAX_AVATAR_SIZE + 1] {
            assert!(e.allocate_avatar("bob", size).is_err());
        }
        assert_eq!(MAX_AVATAR_SIZE, 65_536);
        {
            let boundary_engine = super::super::tests::engine();
            let boundary = boundary_engine
                .allocate_avatar("bob", MAX_AVATAR_SIZE)
                .unwrap();
            assert_eq!(boundary.size, MAX_AVATAR_SIZE);
        }
        e.media.config.pending_limit = 1;
        let mut media = ready(&e, "bob");
        assert!(
            media
                .object_key
                .starts_with(&format!("avatars/{}/", e.data.users["bob"].id))
        );
        assert!(e.avatar_owner("alice", &media.id).is_err());
        assert!(e.readable_avatar("bob", &media.id).is_err());
        media.content_type = "text/html".into();
        assert!(e.finish_avatar("bob", &media).is_err());
        media.content_type = "image/png".into();
        e.finish_avatar("bob", &media).unwrap();
        assert!(e.readable_avatar("bob", &media.id).is_ok());
        let next = e.allocate_avatar("bob", 64).unwrap();
        e.abandon_media("bob", &next.id).unwrap();
        e.expired_media().unwrap();
        e.remove_media_record(&next.id).unwrap();
        e.prepare_media_cleanup().unwrap();
        assert!(e.expired_media().unwrap().is_empty());
        assert!(e.readable_avatar("bob", &media.id).is_ok());
        e.store
            .db
            .lock()
            .unwrap()
            .execute("UPDATE media SET unreferenced=0,created=0", [])
            .unwrap();
        assert!(e.expired_media().unwrap().is_empty());
        e.set_avatar("bob", Some("https://example.com/new.png".into()), false)
            .unwrap();
        assert!(e.readable_avatar("bob", &media.id).is_err());
        assert_eq!(e.expired_media().unwrap().len(), 1);
    }
    #[test]
    fn avatar_persists_across_restart_rename_and_failed_writes_roll_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite");
        let id;
        {
            let mut e = Engine::open(&path).unwrap();
            e.provision("bob", "hash".into(), false, false).unwrap();
            let media = ready(&e, "bob");
            id = media.id.clone();
            e.finish_avatar("bob", &media).unwrap();
            let before = e.data.clone();
            let revision = e.revision;
            e.store.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_avatar BEFORE UPDATE ON entities WHEN OLD.kind='account' BEGIN SELECT RAISE(FAIL,'injected'); END;").unwrap();
            assert!(e.set_avatar("bob", None, false).is_err());
            assert!(e.data == before);
            assert_eq!(e.revision, revision);
            e.store
                .db
                .lock()
                .unwrap()
                .execute_batch("DROP TRIGGER reject_avatar;")
                .unwrap();
            e.execute(None, None, "/user rename bob robert").unwrap();
        }
        let e = Engine::open(&path).unwrap();
        e.prepare_media_cleanup().unwrap();
        assert_eq!(
            e.snapshot("robert").unwrap().avatars["robert"],
            avatar_path(&id)
        );
        assert!(e.readable_avatar("robert", &id).is_ok());
        assert!(e.expired_media().unwrap().is_empty());
    }
    #[test]
    fn avatar_projection_does_not_reveal_hidden_directory_identities() {
        let mut e = Engine::open(Path::new(":memory:")).unwrap();
        e.media.avatar_hosts.insert("example.com".into());
        for name in ["alice", "bob"] {
            e.provision(name, "hash".into(), false, false).unwrap();
        }
        e.set_avatar("alice", Some("https://example.com/a.png".into()), false)
            .unwrap();
        e.set_avatar("bob", Some("https://example.com/b.png".into()), false)
            .unwrap();
        e.execute(None, None, "/revoke bob user @global").unwrap();
        let snapshot = e.snapshot("bob").unwrap();
        assert!(snapshot.users.is_empty());
        assert_eq!(snapshot.avatars.len(), 1);
        assert!(snapshot.avatars.contains_key("bob"));
    }
    #[test]
    fn allocation_replays_preserve_quota_and_reject_other_owners_or_payloads() {
        let e = super::super::tests::engine();
        let id = uuid::Uuid::new_v4().to_string();
        let first = e.allocate_avatar_id("bob", 64, &id).unwrap();
        let repeated = e.allocate_avatar_id("bob", 64, &id).unwrap();
        assert_eq!(first.id, repeated.id);
        assert!(e.allocate_avatar_id("alice", 64, &id).is_err());
        assert!(e.allocate_avatar_id("bob", 65, &id).is_err());
        assert!(e.allocate_avatar_id("bob", 64, "bad").is_err());
        assert_eq!(
            e.store
                .db
                .lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM media WHERE id=?1", [&id], |r| r
                    .get::<_, u64>(0))
                .unwrap(),
            1
        );
        e.abandon_media("bob", &id).unwrap();
        assert!(e.allocate_avatar_id("bob", 64, &id).is_err());
    }
    #[test]
    fn managed_avatar_reads_follow_visibility_and_disabled_accounts_keep_bytes() {
        let mut e = Engine::open(Path::new(":memory:")).unwrap();
        for name in ["alice", "bob"] {
            e.provision(name, "hash".into(), false, false).unwrap();
        }
        let media = ready(&e, "alice");
        e.finish_avatar("alice", &media).unwrap();
        assert!(e.readable_avatar("bob", &media.id).is_ok());
        e.execute(None, None, "/revoke bob user @global").unwrap();
        assert!(e.readable_avatar("bob", &media.id).is_err());
        assert!(e.readable_avatar("alice", &media.id).is_ok());
        e.execute(None, None, "/room new shared").unwrap();
        for name in ["alice", "bob"] {
            e.execute(None, None, &format!("/room add {name} #shared"))
                .unwrap();
        }
        assert!(e.readable_avatar("bob", &media.id).is_ok());
        e.execute(None, None, "/revoke bob user #shared").unwrap();
        e.execute(None, None, "/grant bob #shared r:message.read")
            .unwrap();
        e.execute(Some("alice"), Some("shared"), "Retained author")
            .unwrap();
        assert!(e.snapshot("bob").unwrap().rooms[0].members.is_empty());
        assert!(e.readable_avatar("bob", &media.id).is_ok());
        e.execute(None, None, "/clean -1 #shared").unwrap();
        assert!(e.readable_avatar("bob", &media.id).is_err());
        e.execute(None, None, "/room kick bob #shared").unwrap();
        assert!(e.readable_avatar("bob", &media.id).is_err());
        e.execute(None, None, "/user disable alice").unwrap();
        assert!(e.readable_avatar("alice", &media.id).is_err());
        e.prepare_media_cleanup().unwrap();
        assert!(e.expired_media().unwrap().is_empty());
        e.execute(None, None, "/user enable alice").unwrap();
        assert!(e.readable_avatar("alice", &media.id).is_ok());
    }
    #[test]
    fn expired_reservations_and_replaced_accounts_cannot_complete() {
        let mut e = super::super::tests::engine();
        e.media
            .avatar_hosts
            .extend(["example.com".into(), "www.gravatar.com".into()]);
        let media = ready(&e, "bob");
        e.store
            .db
            .lock()
            .unwrap()
            .execute(
                "UPDATE media SET json=json_set(json,'$.created',0) WHERE id=?1",
                [&media.id],
            )
            .unwrap();
        assert!(e.finish_avatar("bob", &media).is_err());
        let other = ready(&e, "eve");
        e.execute(None, None, "/user delete eve").unwrap();
        e.provision("eve", "hash".into(), false, false).unwrap();
        assert!(e.finish_avatar("eve", &other).is_err());
    }
}
