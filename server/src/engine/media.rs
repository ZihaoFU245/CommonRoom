use super::*;
use authorization::{Action, Scope};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Media {
    pub id: String,
    pub uploader: String,
    pub conversation: String,
    pub filename: String,
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
        let id = self
            .principal_id(Some(user))
            .ok_or("Account unavailable.")?;
        let limits = self.authorization.media.get(id);
        let grants = [Scope::Server, scope.clone()]
            .into_iter()
            .flat_map(|s| limits.and_then(|m| m.get(&s)).into_iter().flatten());
        let mut ages: Vec<i64> = grants
            .filter(|g| size <= g.max_size.unwrap_or(self.media_config.max_size))
            .map(|g| g.max_age.unwrap_or(self.media_config.max_age))
            .collect();
        let private_default = matches!(scope, Scope::Private(pair) if pair.split_once(':').is_some_and(|(a,b)| a==id || b==id))
            && self
                .authorization
                .groups
                .get(id)
                .is_some_and(|groups| !groups.is_empty());
        let room_default = self
            .authorization
            .media_defaults
            .get(id)
            .is_some_and(|scopes| scopes.contains(scope))
            && limits.and_then(|m| m.get(scope)).is_none_or(Vec::is_empty);
        let private_default =
            private_default && limits.and_then(|m| m.get(scope)).is_none_or(Vec::is_empty);
        if room_default || private_default || self.is_su(Some(user)) {
            ages.push(self.media_config.max_age);
        }
        ages.into_iter()
            .filter(|age| {
                required_age
                    .is_none_or(|required| *age == -1 || (required != -1 && required <= *age))
            })
            .max_by_key(|age| if *age == -1 { i64::MAX } else { *age })
            .ok_or_else(|| "Media grant does not allow this size or retention.".into())
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

    pub(crate) fn media_scope(&self, user: &str, id: &str, write: bool) -> Result<(), String> {
        if !self.active(user) {
            return Err("Account unavailable.".into());
        }
        let scope = if self.data.rooms.values().any(|room| room.id == id) {
            Scope::Room(id.into())
        } else if self.data.private.values().any(|chat| chat.id == id) {
            Scope::Private(id.into())
        } else {
            return Err("Conversation not found.".into());
        };
        self.require(Some(user), &scope, Action::Read)?;
        if write {
            self.require(Some(user), &scope, Action::Send)?;
            self.require(Some(user), &scope, Action::MediaUpload)?;
        }
        Ok(())
    }

    pub(crate) fn validate_media_allocation(
        &self,
        user: &str,
        conversation: &str,
        filename: &str,
        size: u64,
    ) -> Result<i64, String> {
        self.media_scope(user, conversation, true)?;
        let max_age = self.media_allowance(
            user,
            &self.media_conversation_scope(conversation)?,
            size,
            None,
        )?;
        if filename.is_empty()
            || filename.chars().count() > 200
            || filename.chars().any(char::is_control)
        {
            return Err("Invalid filename (maximum 200 characters).".into());
        }
        Ok(max_age)
    }
    pub(crate) fn allocate_media(
        &self,
        user: &str,
        conversation: &str,
        filename: &str,
        size: u64,
    ) -> Result<Media, String> {
        let max_age = self.validate_media_allocation(user, conversation, filename, size)?;
        let media = Media {
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
        let media = self.stored_media(id)?;
        if self
            .data
            .users
            .get(user)
            .is_none_or(|u| u.id != media.uploader)
        {
            return Err("Upload not found.".into());
        }
        self.media_scope(user, &media.conversation, true)?;
        self.media_allowance(
            user,
            &self.media_conversation_scope(&media.conversation)?,
            media.size,
            Some(media.max_age),
        )?;
        if media.deleting
            || media.deleted
            || media.expires.is_some_and(|t| t <= now())
            || (!self.media_attached(id)?
                && now().saturating_sub(media.created) >= self.media_config.pending_ttl)
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
        let media = self.stored_media(id)?;
        self.media_scope(user, &media.conversation, false)?;
        if !media.ready
            || media.deleting
            || media.deleted
            || media.expires.is_some_and(|t| t <= now())
            || !self.media_attached(id)?
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
