use super::*;
use authorization::{Action, Scope};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
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
}

pub(crate) fn timestamp() -> u64 {
    now()
}
fn default_media_age() -> i64 {
    7 * 86400
}
impl Media {
    pub(crate) fn final_key(&self) -> String {
        let class = match self.max_age {
            -1 => "permanent".to_owned(),
            age if age % 86400 == 0 => format!("{}d", age / 86400),
            age => format!("{age}s"),
        };
        format!("media/{class}/{}", self.id)
    }
}
enum MediaAccess {
    Read,
    Upload,
}
impl Engine {
    pub(crate) fn resolve_media_conversation(
        &mut self,
        user: &str,
        target: &str,
    ) -> Result<String, String> {
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
        if let Err(error) = self.save_empty_private(&key) {
            self.data.private.remove(&key);
            return Err(error);
        }
        Ok(id)
    }
    pub(crate) fn media_allowance(
        &self,
        user: &str,
        scope: &Scope,
        size: u64,
        required_age: Option<i64>,
    ) -> Result<i64, String> {
        self.require(Some(user), scope, Action::MediaUpload)?;
        if size == 0 || size > self.media_config.max_size {
            return Err("File exceeds the configured media size limit.".into());
        }
        let max_size = self
            .metadata_value(user, scope, "media.max_size")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(self.media_config.max_size)
            .min(self.media_config.max_size);
        let age = self
            .metadata_value(user, scope, "media.max_age")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(self.media_config.max_age);
        if size > max_size
            || required_age.is_some_and(|required| age != -1 && (required == -1 || required > age))
        {
            return Err("User metadata does not allow this media size or retention.".into());
        }
        Ok(age)
    }
    fn media_conversation_scope(&self, id: &str) -> Result<Scope, String> {
        if self.data.rooms.values().any(|r| r.id == id) {
            Ok(Scope::Room(id.into()))
        } else if self.data.private.values().any(|c| c.id == id) {
            Ok(Scope::Private(id.into()))
        } else {
            Err("Conversation not found.".into())
        }
    }
    pub(super) fn authorize_media_text(
        &self,
        actor: Option<&str>,
        scope: &Scope,
        text: &str,
        previous: &str,
    ) -> Result<(), String> {
        let previous = media_references(previous);
        for id in media_references(text).difference(&previous) {
            let media = self.stored_media(id)?;
            let user = actor.ok_or("Media uploads require an account.")?;
            self.require(Some(user), scope, Action::Read)?;
            self.require(Some(user), scope, Action::Send)?;
            self.media_allowance(user, scope, media.size, Some(media.max_age))?;
        }
        Ok(())
    }

    fn media_scope(&self, user: &str, id: &str, access: MediaAccess) -> Result<Scope, String> {
        if !self.active(user) {
            return Err("Account unavailable.".into());
        }
        let scope = self.media_conversation_scope(id)?;
        self.require(Some(user), &scope, Action::Read)?;
        if matches!(access, MediaAccess::Upload) {
            self.require(Some(user), &scope, Action::Send)?;
            self.require(Some(user), &scope, Action::MediaUpload)?;
        }
        Ok(scope)
    }

    pub(crate) fn validate_media_allocation(
        &self,
        user: &str,
        conversation: &str,
        filename: &str,
        size: u64,
    ) -> Result<i64, String> {
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
    ) -> Result<Media, String> {
        self.allocate_media_with_hints(user, conversation, filename, size, BTreeMap::new())
    }
    pub(crate) fn allocate_media_with_hints(
        &self,
        user: &str,
        conversation: &str,
        filename: &str,
        size: u64,
        hints: BTreeMap<String, String>,
    ) -> Result<Media, String> {
        crate::media::hint_headers(&hints)?;
        let max_age = self.validate_media_allocation(user, conversation, filename, size)?;
        let media = Media {
            hints,
            id: uuid::Uuid::new_v4().to_string(),
            uploader: self
                .data
                .users
                .get(user)
                .ok_or("Account unavailable.")?
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
        };
        self.store_media_allocation(&media)?;
        Ok(media)
    }

    pub(crate) fn media_owner(&self, user: &str, id: &str) -> Result<Media, String> {
        let (media, attached) = self.stored_media_with_reference(id)?;
        if self
            .data
            .users
            .get(user)
            .is_none_or(|u| u.id != media.uploader)
        {
            return Err("Upload not found.".into());
        }
        let scope = self.media_scope(user, &media.conversation, MediaAccess::Upload)?;
        self.media_allowance(user, &scope, media.size, Some(media.max_age))?;
        if media.deleting
            || media.deleted
            || media.expires.is_some_and(|t| t <= now())
            || (!attached && now().saturating_sub(media.created) >= self.media_config.pending_ttl)
        {
            return Err("Upload expired.".into());
        }
        Ok(media)
    }

    pub(crate) fn finish_media(&self, user: &str, media: &Media) -> Result<(), String> {
        let current = self.media_owner(user, &media.id)?;
        if current.ready {
            return Err("Upload already completed.".into());
        }
        self.store_media_ready(media)
    }

    pub(crate) fn readable_media(&self, user: &str, id: &str) -> Result<Media, String> {
        let (media, attached) = self.stored_media_with_reference(id)?;
        self.media_scope(user, &media.conversation, MediaAccess::Read)?;
        if !media.ready
            || media.deleting
            || media.deleted
            || media.expires.is_some_and(|t| t <= now())
            || !attached
        {
            return Err("Attachment not found.".into());
        }
        Ok(media)
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
