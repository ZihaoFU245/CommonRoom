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
        self.avatar_plan_scoped(viewer, owner_id, None)
    }
    fn avatar_plan_scoped(
        &self,
        viewer: &str,
        owner_id: &str,
        conversations_scope: Option<&BTreeSet<String>>,
    ) -> Option<AvatarPlan> {
        use authorization::{Action, Scope};
        if !self.active(viewer) {
            return None;
        }
        let owner = self.active_account_name(owner_id)?;
        let mut known = viewer == owner
            || (conversations_scope.is_none()
                && self.allows(Some(viewer), &Scope::Server, Action::Directory));
        let mut conversations = Vec::new();
        if !known {
            for room in self.data.rooms.values() {
                if conversations_scope.is_some_and(|ids| !ids.contains(&room.id)) {
                    continue;
                }
                let scope = Scope::Room(room.id.clone());
                if self.allows(Some(viewer), &scope, Action::Read) {
                    known |= self.allows(Some(viewer), &scope, Action::Members)
                        && room.members.contains(owner);
                    conversations.push(room.id.clone());
                }
            }
            for (key, chat) in &self.data.private {
                if conversations_scope.is_some_and(|ids| !ids.contains(&chat.id)) {
                    continue;
                }
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
        let pending = if managed {
            self.profile_key(user, "@global", "avatar", value.as_deref(), false)?
        } else {
            self.prepare_avatar_set(user, value.as_deref())?
        };
        let Some(pending) = pending else {
            return Ok(false);
        };
        database::block_on_storage(pending.save())??;
        self.publish_profile(pending)
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
    #[cfg(test)]
    pub(crate) fn avatar_read_plan(
        &self,
        viewer: &str,
        media: &Media,
    ) -> Result<AvatarPlan, EngineError> {
        self.avatar_read_plan_scoped(viewer, media, None)
    }
    pub(crate) fn avatar_read_plan_scoped(
        &self,
        viewer: &str,
        media: &Media,
        conversations: Option<&BTreeSet<String>>,
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
        self.avatar_plan_scoped(viewer, &media.uploader, conversations)
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
mod tests;
