//! Owned upload reservations, including an unpublished first private directory.
use super::*;
use authorization::{Action, Scope};
use media::Media;

pub(crate) struct PendingMedia {
    store: MediaStore,
    pub(crate) media: Media,
    private: Option<(String, PrivateChat)>,
    quota: u64,
}
impl Engine {
    fn media_target(
        &self,
        user: &str,
        target: &str,
    ) -> Result<(Scope, Option<(String, PrivateChat)>), EngineError> {
        if let Some(peer) = target.strip_prefix("@direct:") {
            if !self.active(user) || !self.active(peer) || peer == user {
                return Err("Private recipient unavailable.".into());
            }
            let key = private_key(user, peer);
            let scope = self.private_scope(&key);
            self.require(Some(user), &scope, Action::Read)?;
            self.require(Some(user), &scope, Action::Send)?;
            self.require(Some(user), &scope, Action::MediaUpload)?;
            self.require_target_command(Some(user), &scope, "/tell")?;
            let private = if self.data.private.contains_key(&key) {
                None
            } else {
                self.require(Some(user), &Scope::Server, Action::Direct)?;
                let Scope::Private(id) = &scope else {
                    return Err("Invalid private scope.".into());
                };
                Some((
                    key,
                    PrivateChat {
                        id: id.clone(),
                        revision: 0,
                    },
                ))
            };
            Ok((scope, private))
        } else {
            Ok((
                self.media_scope(user, target, media::MediaAccess::Upload)?,
                None,
            ))
        }
    }
    fn check_media_request(
        &self,
        user: &str,
        scope: &Scope,
        filename: &str,
        size: u64,
        hints: &BTreeMap<String, String>,
    ) -> Result<i64, EngineError> {
        crate::media::hint_headers(hints).map_err(EngineError::Invalid)?;
        let max_age = self.media_allowance(user, scope, size, None)?;
        if filename.is_empty()
            || filename.chars().count() > 200
            || filename.chars().any(char::is_control)
        {
            return Err("Invalid filename (maximum 200 characters).".into());
        }
        Ok(max_age)
    }
    pub(crate) fn validate_media_target(
        &self,
        user: &str,
        target: &str,
        filename: &str,
        size: u64,
        hints: &BTreeMap<String, String>,
    ) -> Result<String, EngineError> {
        let (scope, _) = self.media_target(user, target)?;
        self.check_media_request(user, &scope, filename, size, hints)?;
        match scope {
            Scope::Room(id) | Scope::Private(id) => Ok(id),
            _ => Err("Invalid upload scope.".into()),
        }
    }
    pub(crate) fn prepare_media_allocation(
        &self,
        user: &str,
        target: &str,
        filename: &str,
        size: u64,
        hints: BTreeMap<String, String>,
    ) -> Result<PendingMedia, EngineError> {
        let (scope, private) = self.media_target(user, target)?;
        let max_age = self.check_media_request(user, &scope, filename, size, &hints)?;
        let quota = self
            .metadata_value(user, &Scope::Server, "media.user_quota")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(self.media.config.user_quota)
            .min(self.media.config.user_quota);
        let conversation = match scope {
            Scope::Room(id) | Scope::Private(id) => id,
            _ => return Err("Invalid upload scope.".into()),
        };
        Ok(PendingMedia {
            store: self.media_store(),
            private,
            quota,
            media: Media {
                id: uuid::Uuid::new_v4().to_string(),
                uploader: self
                    .principal_id(Some(user))
                    .ok_or("Account unavailable.")?
                    .into(),
                conversation,
                filename: filename.into(),
                hints,
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
            },
        })
    }
    pub(crate) fn publish_media_allocation(
        &mut self,
        pending: PendingMedia,
    ) -> Result<Media, EngineError> {
        if let Some((key, chat)) = pending.private {
            if self.data.private.contains_key(&key) {
                self.recovery_failed = true;
                return Err(EngineError::Unavailable(
                    "Committed upload directory changed; restart the service.".into(),
                ));
            }
            self.bot_index
                .conversations
                .insert(chat.id.clone(), format!("@private:{key}"));
            self.data.private.insert(key, chat);
            // Reservations remain silent. Invalidate routing/baselines so the
            // eventual first message is delivered even through warmed interests.
            self.read_model.advance();
            self.revision = self.revision.wrapping_add(1);
        }
        Ok(pending.media)
    }
}
impl PendingMedia {
    pub(crate) async fn save(&self) -> Result<(), EngineError> {
        let private = self
            .private
            .as_ref()
            .map(|(key, chat)| storage::private_json(chat).map(|json| (key.as_str(), json)))
            .transpose()?;
        self.store
            .database
            .media_write(&database::MediaWrite::Allocate(database::MediaAllocation {
                media: &self.media,
                quota: self.quota,
                pending_limit: self.store.config.pending_limit,
                rate_limit: self.store.config.allocations_per_minute,
                at: now(),
                private,
            }))
            .await?;
        Ok(())
    }
}
