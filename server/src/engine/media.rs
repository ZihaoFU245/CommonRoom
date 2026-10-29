use super::*;
use authorization::{Action, Scope};

/// Media limits and external avatar policy belong to the media subsystem.
#[derive(Default)]
pub(super) struct State {
    pub(super) config: crate::config::MediaConfig,
    pub(super) avatar_hosts: BTreeSet<String>,
}

impl State {
    pub(super) fn allows_avatar(&self, value: &str) -> bool {
        super::avatars::valid_avatar_url(value)
            && url::Url::parse(value)
                .ok()
                .and_then(|url| url.host_str().map(str::to_owned))
                .is_some_and(|host| self.avatar_hosts.contains(&host))
    }
}

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Media {
    pub id: String,
    pub uploader: String,
    pub conversation: String,
    pub filename: String,
    #[serde(default)]
    pub hints: BTreeMap<String, String>,
    pub size: u64,
    pub content_type: String,
    pub object_key: String,
    pub ready: bool,
    pub created: u64,
    #[serde(default = "default_media_age")]
    pub max_age: i64,
    #[serde(default)]
    pub expires: Option<u64>,
    #[serde(default)]
    pub deleting: bool,
    #[serde(default)]
    pub deleted: bool,
    /// Sticker that a conversation copy was made from; never grants access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

pub(crate) fn timestamp() -> u64 {
    now()
}
fn default_media_age() -> i64 {
    7 * 86400
}
impl Media {
    pub(super) fn completed_record(&self, at: u64) -> Result<Self, EngineError> {
        let mut media = self.clone();
        media.expires = if media.max_age == -1 {
            None
        } else {
            Some(at.saturating_add(u64::try_from(media.max_age).map_err(|_| "Invalid media age.")?))
        };
        Ok(media)
    }
    pub(crate) fn final_key(&self) -> String {
        if self.conversation == "@avatar" {
            return format!("avatars/{}/{}", self.uploader, self.id);
        }
        if self.conversation == super::stickers::STICKERS {
            return format!("stickers/{}/{}", self.uploader, self.id);
        }
        let class = match self.max_age {
            -1 => "permanent".to_owned(),
            age if age % 86400 == 0 => format!("{}d", age / 86400),
            age => format!("{age}s"),
        };
        format!("media/{class}/{}", self.id)
    }
}
pub(super) enum MediaAccess {
    Read,
    Upload,
}
pub(crate) struct DownloadTicket {
    pub(crate) media: Media,
    principal: String,
}

impl Engine {
    pub(crate) fn configure_media(&mut self, config: crate::config::MediaConfig) {
        self.media.config = config;
    }

    #[cfg(test)]
    pub(crate) fn resolve_media_conversation(
        &mut self,
        user: &str,
        target: &str,
    ) -> Result<String, EngineError> {
        let Some(peer) = target.strip_prefix("@direct:") else {
            return Ok(target.into());
        };
        if !self.active(user) || !self.active(peer) || peer == user {
            return Err("Private recipient unavailable.".into());
        }
        let key = private_key(user, peer);
        let scope = self.private_scope(&key);
        self.require(Some(user), &scope, Action::Read)?;
        self.require(Some(user), &scope, Action::Send)?;
        self.require(Some(user), &scope, Action::MediaUpload)?;
        self.require_target_command(Some(user), &scope, "/tell")?;
        if let Some(chat) = self.data.private.get(&key) {
            return Ok(chat.id.clone());
        }
        self.require(Some(user), &Scope::Server, Action::Direct)?;
        let Scope::Private(id) = scope else {
            return Err("Invalid private scope.".into());
        };
        self.data.private.insert(
            key.clone(),
            PrivateChat {
                id: id.clone(),
                ..Default::default()
            },
        );
        Ok(id)
    }
    // The tentative private directory is visible only under the engine writer.
    // Storage commits it together with the reservation, after every quota check.
    #[cfg(test)]
    pub(crate) fn allocate_media_target(
        &mut self,
        user: &str,
        target: &str,
        filename: &str,
        size: u64,
        hints: BTreeMap<String, String>,
        preview: bool,
    ) -> Result<Option<Media>, EngineError> {
        let new_key = target
            .strip_prefix("@direct:")
            .map(|peer| private_key(user, peer))
            .filter(|key| !self.data.private.contains_key(key));
        let conversation = self.resolve_media_conversation(user, target)?;
        let result = if preview {
            crate::media::hint_headers(&hints)
                .map_err(EngineError::Invalid)
                .and_then(|_| {
                    self.validate_media_allocation(user, &conversation, filename, size)
                        .map(|_| None)
                })
        } else {
            self.allocate_media_with_hints(user, &conversation, filename, size, hints)
                .map(Some)
        };
        if let Some(key) = new_key {
            if preview || result.is_err() {
                self.data.private.remove(&key);
            } else {
                self.bot_index
                    .conversations
                    .insert(conversation, format!("@private:{key}"));
                self.read_model.advance();
                self.revision = self.revision.wrapping_add(1);
            }
        }
        result
    }
    pub(crate) fn media_allowance(
        &self,
        user: &str,
        scope: &Scope,
        size: u64,
        required_age: Option<i64>,
    ) -> Result<i64, EngineError> {
        self.require(Some(user), scope, Action::MediaUpload)?;
        if size == 0 || size > self.media.config.max_size {
            return Err("File exceeds the configured media size limit.".into());
        }
        let max_size = self
            .metadata_value(user, scope, "media.max_size")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(self.media.config.max_size)
            .min(self.media.config.max_size);
        let age = self
            .metadata_value(user, scope, "media.max_age")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(self.media.config.max_age);
        if size > max_size
            || required_age.is_some_and(|required| age != -1 && (required == -1 || required > age))
        {
            return Err("User metadata does not allow this media size or retention.".into());
        }
        Ok(age)
    }
    fn media_conversation_scope(&self, id: &str) -> Result<Scope, EngineError> {
        if self.data.rooms.values().any(|r| r.id == id) {
            Ok(Scope::Room(id.into()))
        } else if self.data.private.values().any(|c| c.id == id) {
            Ok(Scope::Private(id.into()))
        } else {
            Err(EngineError::NotFound("Conversation not found.".into()))
        }
    }
    #[cfg(test)]
    pub(super) fn authorize_media_text(
        &self,
        actor: Option<&str>,
        scope: &Scope,
        text: &str,
        previous: &str,
    ) -> Result<(), EngineError> {
        let previous = media_references(previous);
        for id in media_references(text).difference(&previous) {
            let user = actor.ok_or("Media uploads require an account.")?;
            self.require(Some(user), scope, Action::Read)?;
            self.require(Some(user), scope, Action::Send)?;
            let (media, referenced) = self.stored_media_with_reference(id)?;
            self.authorize_media_record(user, scope, &media, referenced)?;
        }
        Ok(())
    }

    pub(in crate::engine) fn authorize_media_record(
        &self,
        user: &str,
        scope: &Scope,
        media: &Media,
        referenced: bool,
    ) -> Result<(), EngineError> {
        self.require(Some(user), scope, Action::Read)?;
        self.require(Some(user), scope, Action::Send)?;
        let conversation = match scope {
            Scope::Room(id) | Scope::Private(id) => id,
            _ => return Err(EngineError::NotFound("Upload not found.".into())),
        };
        if media.conversation != *conversation
            || !media.ready
            || media.deleting
            || media.deleted
            || media.expires.is_some_and(|expiry| expiry <= now())
            || (self.principal_id(Some(user)) != Some(media.uploader.as_str()) && !referenced)
        {
            return Err(EngineError::NotFound("Upload not found.".into()));
        }
        self.media_allowance(user, scope, media.size, Some(media.max_age))?;
        Ok(())
    }

    pub(super) fn media_scope(
        &self,
        user: &str,
        id: &str,
        access: MediaAccess,
    ) -> Result<Scope, EngineError> {
        if !self.active(user) {
            return Err(EngineError::Forbidden("Account unavailable.".into()));
        }
        let scope = self.media_conversation_scope(id)?;
        self.require(Some(user), &scope, Action::Read)?;
        if matches!(access, MediaAccess::Upload) {
            self.require(Some(user), &scope, Action::Send)?;
            self.require(Some(user), &scope, Action::MediaUpload)?;
        }
        Ok(scope)
    }

    #[cfg(test)]
    pub(crate) fn validate_media_allocation(
        &self,
        user: &str,
        conversation: &str,
        filename: &str,
        size: u64,
    ) -> Result<i64, EngineError> {
        let scope = self.media_scope(user, conversation, MediaAccess::Upload)?;
        let max_age = self.media_allowance(user, &scope, size, None)?;
        if filename.is_empty()
            || filename.chars().count() > 200
            || filename.chars().any(char::is_control)
        {
            return Err("Invalid filename (maximum 200 characters).".into());
        }
        Ok(max_age)
    }
    #[cfg(test)]
    pub(crate) fn allocate_media(
        &self,
        user: &str,
        conversation: &str,
        filename: &str,
        size: u64,
    ) -> Result<Media, EngineError> {
        self.allocate_media_with_hints(user, conversation, filename, size, BTreeMap::new())
    }
    #[cfg(test)]
    pub(crate) fn allocate_media_with_hints(
        &self,
        user: &str,
        conversation: &str,
        filename: &str,
        size: u64,
        hints: BTreeMap<String, String>,
    ) -> Result<Media, EngineError> {
        crate::media::hint_headers(&hints).map_err(EngineError::Invalid)?;
        let max_age = self.validate_media_allocation(user, conversation, filename, size)?;
        let media = Media {
            hints,
            id: uuid::Uuid::new_v4().to_string(),
            uploader: self
                .data
                .users
                .get(user)
                .ok_or_else(|| EngineError::Forbidden("Account unavailable.".into()))?
                .id
                .clone(),
            conversation: conversation.into(),
            filename: filename.into(),
            size,
            content_type: "application/octet-stream".into(),
            object_key: String::new(),
            ready: false,
            created: now(),
            max_age,
            expires: None,
            deleting: false,
            deleted: false,
            source: None,
        };
        self.store_media_allocation(&media)?;
        Ok(media)
    }

    #[cfg(test)]
    pub(crate) fn media_owner(&self, user: &str, id: &str) -> Result<Media, EngineError> {
        let (media, attached) = self.stored_media_with_reference(id)?;
        self.authorize_media_owner(user, &media, attached)?;
        Ok(media)
    }
    pub(crate) fn authorize_media_owner(
        &self,
        user: &str,
        media: &Media,
        attached: bool,
    ) -> Result<(), EngineError> {
        if self
            .data
            .users
            .get(user)
            .is_none_or(|u| u.id != media.uploader)
        {
            return Err(EngineError::NotFound("Upload not found.".into()));
        }
        let scope = self.media_scope(user, &media.conversation, MediaAccess::Upload)?;
        self.media_allowance(user, &scope, media.size, Some(media.max_age))?;
        if media.deleting
            || media.deleted
            || media.expires.is_some_and(|t| t <= now())
            || (!attached && now().saturating_sub(media.created) >= self.media.config.pending_ttl)
        {
            return Err(EngineError::NotFound("Upload expired.".into()));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn finish_media(&self, user: &str, media: &Media) -> Result<(), EngineError> {
        let current = self.media_owner(user, &media.id)?;
        if current.ready {
            return Err(EngineError::Conflict("Upload already completed.".into()));
        }
        self.store_media_ready(media)
    }

    #[cfg(test)]
    pub(crate) fn prepare_media_download(
        &self,
        user: &str,
        id: &str,
    ) -> Result<DownloadTicket, EngineError> {
        let (media, attached) = self.stored_media_with_reference(id)?;
        self.prepare_media_download_record(user, media, attached)
    }
    pub(crate) fn prepare_media_download_record(
        &self,
        user: &str,
        media: Media,
        attached: bool,
    ) -> Result<DownloadTicket, EngineError> {
        self.authorize_media_read(user, &media, attached, false)?;
        let principal = self
            .principal_id(Some(user))
            .ok_or("Account unavailable.")?
            .into();
        Ok(DownloadTicket { media, principal })
    }
    #[cfg(test)]
    pub(crate) fn validate_media_download(
        &self,
        user: &str,
        ticket: &DownloadTicket,
    ) -> Result<(), EngineError> {
        let (current, attached) = self.stored_media_with_reference(&ticket.media.id)?;
        self.validate_media_download_record(user, ticket, &current, attached)
    }
    pub(crate) fn validate_media_download_record(
        &self,
        user: &str,
        ticket: &DownloadTicket,
        current: &Media,
        attached: bool,
    ) -> Result<(), EngineError> {
        if self.data.users.get(user).map(|account| &account.id) != Some(&ticket.principal)
            || ticket.media.expires.is_some_and(|expiry| expiry <= now())
        {
            return Err(EngineError::NotFound("Attachment not found.".into()));
        }
        self.authorize_media_read(user, current, attached, false)?;
        if current != &ticket.media {
            return Err(EngineError::Conflict(
                "Attachment changed; retry download.".into(),
            ));
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn media_info(&self, user: &str, id: &str) -> Result<Media, EngineError> {
        let (media, attached) = self.stored_media_with_reference(id)?;
        self.authorize_media_read(user, &media, attached, true)?;
        Ok(media)
    }
    #[cfg(test)]
    pub(crate) fn readable_media(&self, user: &str, id: &str) -> Result<Media, EngineError> {
        let (media, attached) = self.stored_media_with_reference(id)?;
        self.authorize_media_read(user, &media, attached, false)?;
        Ok(media)
    }
    pub(crate) fn authorize_media_read(
        &self,
        user: &str,
        media: &Media,
        attached: bool,
        include_expired: bool,
    ) -> Result<(), EngineError> {
        self.media_scope(user, &media.conversation, MediaAccess::Read)?;
        let expired = media.expires.is_some_and(|time| time <= now());
        if !attached
            || !(include_expired && expired)
                && (!media.ready || media.deleting || media.deleted || expired)
        {
            return Err(EngineError::NotFound("Attachment not found.".into()));
        }
        Ok(())
    }
    pub(crate) fn authorize_media_abandon(
        &self,
        user: &str,
        media: &Media,
    ) -> Result<(), EngineError> {
        if self.principal_id(Some(user)) != Some(media.uploader.as_str()) {
            return Err(EngineError::NotFound("Upload not found.".into()));
        }
        Ok(())
    }
}

// Conservative reference tracking also covers literal links and plain-text messages.
// IDs are canonical UUIDs; alternate encodings cannot grant storage access.
pub(super) fn media_references(text: &str) -> BTreeSet<String> {
    text.split("/api/media/")
        .skip(1)
        .filter_map(|part| {
            let (id, rest) = part.split_once('/')?;
            if !rest.starts_with("file") {
                return None;
            }
            let parsed = uuid::Uuid::parse_str(id).ok()?;
            (parsed.to_string() == id).then(|| id.to_owned())
        })
        .collect()
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    #[test]
    fn external_avatar_policy_is_independent_and_matches_exact_https_hosts() {
        let mut state = State::default();
        assert!(!state.allows_avatar("https://example.com/avatar.png"));
        state.avatar_hosts.insert("example.com".into());
        assert!(state.allows_avatar("https://EXAMPLE.com/avatar.png"));
        for value in [
            "http://example.com/a",
            "https://sub.example.com/a",
            "https://example.com.attacker.test/a",
            "https://user:secret@example.com/a",
            "data:image/png;base64,AA",
            "https://example.com/a\n",
        ] {
            assert!(!state.allows_avatar(value), "{value}");
        }
    }
}
