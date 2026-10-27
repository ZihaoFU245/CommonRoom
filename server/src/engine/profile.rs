//! Owned self-service profile changes. Only avatar/color keys are writable here.
use super::*;
use avatars::avatar_path;
use media::Media;

pub(crate) struct PendingProfile {
    database: database::Handle,
    user: String,
    before: Account,
    after: Option<Account>,
    ready: Option<Media>,
    colors: bool,
}
impl PendingProfile {
    pub(crate) fn changes_profile(&self) -> bool {
        self.after.is_some()
    }
    pub(crate) async fn save(&self) -> Result<(), EngineError> {
        let json = self.after.as_ref().map(serde_json::to_string).transpose()?;
        let at = now();
        let ready = self
            .ready
            .as_ref()
            .map(|media| media.completed_record(at))
            .transpose()?;
        self.database
            .save_profile(&database::ProfileWrite {
                user: &self.user,
                account: &self.before.id,
                json: json.as_deref(),
                avatar: ready.as_ref(),
                at,
            })
            .await?;
        Ok(())
    }
}
impl Engine {
    fn profile_key(
        &self,
        user: &str,
        scope: &str,
        key: &str,
        value: Option<&str>,
        colors: bool,
    ) -> Result<Option<PendingProfile>, EngineError> {
        let account = self
            .data
            .users
            .get(user)
            .filter(|account| !account.disabled)
            .ok_or_else(|| EngineError::Forbidden("Account unavailable.".into()))?;
        if account
            .metadata
            .get(scope)
            .and_then(|metadata| metadata.get(key))
            .map(String::as_str)
            == value
        {
            return Ok(None);
        }
        let mut after = account.clone();
        let metadata = after.metadata.entry(scope.into()).or_default();
        if let Some(value) = value {
            if metadata.len() >= 32 && !metadata.contains_key(key) {
                return Err("At most 32 metadata keys per scope.".into());
            }
            metadata.insert(key.into(), value.into());
        } else {
            metadata.remove(key);
        }
        after.metadata.retain(|_, values| !values.is_empty());
        Ok(Some(PendingProfile {
            database: self.store.db.clone(),
            user: user.into(),
            before: account.clone(),
            after: Some(after),
            ready: None,
            colors,
        }))
    }
    pub(crate) fn prepare_avatar_set(
        &self,
        user: &str,
        value: Option<&str>,
    ) -> Result<Option<PendingProfile>, EngineError> {
        if value.is_some_and(|value| !self.media.allows_avatar(value)) {
            return Err("Avatar URL must use an allowed HTTPS host without credentials (maximum 2048 bytes).".into());
        }
        self.profile_key(user, "@global", "avatar", value, false)
    }
    pub(crate) fn prepare_color_set(
        &self,
        user: &str,
        room: Option<&str>,
        value: Option<&str>,
    ) -> Result<Option<PendingProfile>, EngineError> {
        let scope = self.color_scope(user, room, value)?;
        self.profile_key(
            user,
            Self::metadata_scope_key(&scope)?,
            colors::COLOR_KEY,
            value,
            true,
        )
    }
    pub(crate) fn prepare_avatar_completion(
        &self,
        user: &str,
        media: Media,
        current: &Media,
    ) -> Result<Option<PendingProfile>, EngineError> {
        self.authorize_avatar_owner(user, current)?;
        if media.id != current.id
            || media.uploader != current.uploader
            || !media.ready
            || !matches!(
                media.content_type.as_str(),
                "image/png" | "image/jpeg" | "image/webp"
            )
        {
            return Err("Avatar must be a PNG, JPEG or WebP image.".into());
        }
        // Completion is terminal. Replaying an older completed upload must not
        // overwrite a newer profile selection.
        if current.ready {
            return Ok(None);
        }
        let path = avatar_path(&media.id);
        let mut pending = self.profile_key(user, "@global", "avatar", Some(&path), false)?;
        if !current.ready {
            let pending = pending.get_or_insert_with(|| PendingProfile {
                database: self.store.db.clone(),
                user: user.into(),
                before: self.data.users[user].clone(),
                after: None,
                ready: None,
                colors: false,
            });
            pending.ready = Some(media);
        }
        Ok(pending)
    }
    pub(crate) fn publish_profile(&mut self, pending: PendingProfile) -> Result<bool, EngineError> {
        if self.data.users.get(&pending.user) != Some(&pending.before) {
            self.recovery_failed = true;
            return Err(EngineError::Unavailable(
                "Committed profile identity changed; recovery required, retry shortly.".into(),
            ));
        }
        let Some(after) = pending.after else {
            return Ok(false);
        };
        self.data.users.insert(pending.user, after);
        if pending.colors {
            self.advance_colors();
        }
        self.finish_metadata_commit(MetadataNotice::all());
        Ok(true)
    }
}
