//! Self-service avatars use one public profile metadata key, never arbitrary metadata writes.
use super::media::Media;
use super::*;

pub(crate) const MAX_AVATAR_SIZE: u64 = 2 * 1024 * 1024;
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
impl Engine {
    pub(crate) fn avatar_directory(&self) -> BTreeMap<String, String> {
        self.data
            .users
            .iter()
            .filter(|(_, account)| !account.disabled)
            .filter_map(|(name, account)| {
                let value = account.metadata.get("@global")?.get("avatar")?;
                (valid_avatar_url(value)
                    || value
                        .strip_prefix("/api/avatars/")
                        .and_then(|s| s.strip_suffix("/file"))
                        .is_some_and(|id| {
                            uuid::Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == id)
                        }))
                .then(|| (name.clone(), value.clone()))
            })
            .collect()
    }
    pub(crate) fn set_avatar(
        &mut self,
        user: &str,
        value: Option<String>,
        managed: bool,
    ) -> Result<(), String> {
        if !self.active(user) {
            return Err("Account unavailable.".into());
        }
        if let Some(value) = &value
            && !managed
            && !valid_avatar_url(value)
        {
            return Err(
                "Avatar URL must be HTTPS without credentials (maximum 2048 bytes).".into(),
            );
        }
        let before = MetadataChange::capture(&self.data, ChangeFamily::Metadata);
        let account = self
            .data
            .users
            .get_mut(user)
            .ok_or("Account unavailable.")?;
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
        if let Err(error) = self.save_metadata(&[], None, Some(&before)) {
            before.restore(&mut self.data);
            return Err(error);
        }
        Ok(())
    }
    pub(crate) fn allocate_avatar(&self, user: &str, size: u64) -> Result<Media, String> {
        if !self.active(user) {
            return Err("Account unavailable.".into());
        }
        if size == 0 || size > MAX_AVATAR_SIZE {
            return Err("Avatar must be between 1 byte and 2 MiB.".into());
        }
        let account = self.data.users.get(user).ok_or("Account unavailable.")?;
        let media = Media {
            id: uuid::Uuid::new_v4().to_string(),
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
    pub(crate) fn avatar_owner(&self, user: &str, id: &str) -> Result<Media, String> {
        let media = self.stored_media(id)?;
        if !self.active(user)
            || self
                .data
                .users
                .get(user)
                .is_none_or(|u| u.id != media.uploader)
            || media.conversation != "@avatar"
            || media.deleting
            || media.deleted
            || now().saturating_sub(media.created) >= self.media_config.pending_ttl
        {
            return Err("Avatar upload unavailable.".into());
        }
        Ok(media)
    }
    pub(crate) fn finish_avatar(&mut self, user: &str, media: &Media) -> Result<(), String> {
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
    pub(crate) fn readable_avatar(&self, id: &str) -> Result<Media, String> {
        let media = self.stored_media(id)?;
        if media.conversation != "@avatar"
            || !media.ready
            || media.deleting
            || media.deleted
            || !self
                .avatar_directory()
                .values()
                .any(|value| value == &avatar_path(id))
        {
            return Err("Avatar not found.".into());
        }
        Ok(media)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    fn ready(engine: &Engine, user: &str) -> Media {
        let mut media = engine.allocate_avatar(user, 64).unwrap();
        media.ready = true;
        media.object_key = media.final_key();
        media.content_type = "image/png".into();
        media
    }
    #[test]
    fn self_service_url_validation_remove_and_metadata_isolation() {
        let mut e = super::super::tests::engine();
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
        for size in [0, MAX_AVATAR_SIZE + 1] {
            assert!(e.allocate_avatar("bob", size).is_err());
        }
        e.media_config.pending_limit = 1;
        let mut media = ready(&e, "bob");
        assert!(
            media
                .object_key
                .starts_with(&format!("avatars/{}/", e.data.users["bob"].id))
        );
        assert!(e.avatar_owner("alice", &media.id).is_err());
        assert!(e.readable_avatar(&media.id).is_err());
        media.content_type = "text/html".into();
        assert!(e.finish_avatar("bob", &media).is_err());
        media.content_type = "image/png".into();
        e.finish_avatar("bob", &media).unwrap();
        assert!(e.readable_avatar(&media.id).is_ok());
        let next = e.allocate_avatar("bob", 64).unwrap();
        e.abandon_media("bob", &next.id).unwrap();
        e.expired_media().unwrap();
        e.remove_media_record(&next.id).unwrap();
        e.prepare_media_cleanup().unwrap();
        assert!(e.expired_media().unwrap().is_empty());
        assert!(e.readable_avatar(&media.id).is_ok());
        e.db.lock()
            .unwrap()
            .execute("UPDATE media SET unreferenced=0,created=0", [])
            .unwrap();
        assert!(e.expired_media().unwrap().is_empty());
        e.set_avatar("bob", Some("https://example.com/new.png".into()), false)
            .unwrap();
        assert!(e.readable_avatar(&media.id).is_err());
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
            e.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_avatar BEFORE UPDATE ON entities WHEN OLD.kind='account' BEGIN SELECT RAISE(FAIL,'injected'); END;").unwrap();
            assert!(e.set_avatar("bob", None, false).is_err());
            assert!(e.data == before);
            assert_eq!(e.revision, revision);
            e.db.lock()
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
        assert!(e.readable_avatar(&id).is_ok());
        assert!(e.expired_media().unwrap().is_empty());
    }
    #[test]
    fn avatar_projection_does_not_reveal_hidden_directory_identities() {
        let mut e = Engine::open(Path::new(":memory:")).unwrap();
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
    fn expired_reservations_and_replaced_accounts_cannot_complete() {
        let mut e = super::super::tests::engine();
        let media = ready(&e, "bob");
        e.db.lock()
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
