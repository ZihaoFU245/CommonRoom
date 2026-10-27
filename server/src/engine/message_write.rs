//! Owned message state: prepare and publish touch memory; persistence is async.
use super::*;
use crate::bots::{
    error::{Code, Error},
    models::{self, SendResult},
};

pub(crate) struct PendingMessage {
    pub(crate) database: database::Handle,
    pub(crate) history: std::sync::Arc<HistoryStore>,
    pub(crate) message: Message,
    pub(crate) conversation: String,
    pub(crate) scope: authorization::Scope,
    actor: String,
    key: String,
    private: bool,
    revision: u64,
    mutation: PendingMutation,
    retention: usize,
    new_conversation: Option<String>,
}
pub(super) enum PendingMutation {
    Append,
    Edit {
        previous: BTreeSet<String>,
        actor: String,
    },
    Reaction {
        active: bool,
        reaction: String,
        user: String,
    },
}
impl Engine {
    pub(crate) fn prepare_append(
        &self,
        user: &str,
        input: &models::MessageInput,
    ) -> Result<PendingMessage, Error> {
        let target = self.validate_bot_send(user, input)?;
        self.prepare_append_at(
            user,
            &target.view,
            target.scope,
            target.revision,
            input.text.trim(),
            input.markdown,
        )
        .map_err(Into::into)
    }
    pub(crate) fn prepare_browser_message(
        &self,
        user: &str,
        view: Option<&str>,
        text: &str,
        markdown: bool,
    ) -> Result<PendingMessage, EngineError> {
        let text = text.trim();
        crate::limits::validate_input(text).map_err(EngineError::Invalid)?;
        if text.split_whitespace().next() == Some("/tell") {
            return self.prepare_browser_tell(user, view, text, markdown);
        }
        if !self.active(user) {
            return Err(EngineError::Forbidden("Account unavailable.".into()));
        }
        if text.chars().count() > crate::limits::MAX_MESSAGE_CHARS {
            return Err("Messages support at most 4000 characters.".into());
        }
        let view = view.ok_or("Select a room first.")?;
        let (scope, revision) = self.browser_text_target(user, view)?;
        if !has_message_content(text) {
            return Err("Messages cannot be blank.".into());
        }
        // Plain browser messages require Send only. Check Read before loading
        // attachment records so denied readers cannot probe stored uploads.
        if !media::media_references(text).is_empty() {
            self.require(Some(user), &scope, authorization::Action::Read)?;
        }
        self.prepare_append_at(user, view, scope, revision, text, markdown)
    }
    pub(crate) fn browser_message_conversation(
        &self,
        user: &str,
        view: Option<&str>,
        input: &str,
    ) -> Result<String, EngineError> {
        let scope = if input.split_whitespace().next() == Some("/tell") {
            self.require_command(Some(user), view, "/tell")?;
            let (recipient, _) = tell_content(input)?;
            self.browser_tell_target(user, recipient)?.1
        } else {
            self.browser_text_target(user, view.ok_or("Select a room first.")?)?
                .0
        };
        match scope {
            authorization::Scope::Room(id) | authorization::Scope::Private(id) => Ok(id),
            _ => Err("Invalid message scope.".into()),
        }
    }
    fn browser_tell_target(
        &self,
        user: &str,
        recipient: &str,
    ) -> Result<(String, authorization::Scope, Option<u64>), EngineError> {
        use authorization::{Action, Scope};
        if !self.active(recipient) {
            return Err(EngineError::NotFound("User not found.".into()));
        }
        let key = private_key(user, recipient);
        let scope = self.private_scope(&key);
        self.require_target_command(Some(user), &scope, "/tell")?;
        let revision = self.data.private.get(&key).map(|chat| chat.revision);
        if revision.is_none() {
            self.require(Some(user), &Scope::Server, Action::Direct)?;
        }
        self.require(Some(user), &scope, Action::Send)?;
        Ok((key, scope, revision))
    }
    fn prepare_browser_tell(
        &self,
        user: &str,
        view: Option<&str>,
        input: &str,
        markdown: bool,
    ) -> Result<PendingMessage, EngineError> {
        self.require_command(Some(user), view, "/tell")?;
        let (recipient, text) = tell_content(input)?;
        let (key, scope, revision) = self.browser_tell_target(user, recipient)?;
        if !has_message_content(text) {
            return Err("Messages cannot be blank.".into());
        }
        if !media::media_references(text).is_empty() {
            self.require(Some(user), &scope, authorization::Action::Read)?;
        }
        let mut pending = self.prepare_append_at(
            user,
            &format!("@private:{key}"),
            scope,
            revision.unwrap_or(0),
            text,
            markdown,
        )?;
        if revision.is_none() {
            pending.new_conversation = Some(storage::private_json(&PrivateChat {
                id: pending.conversation.clone(),
                revision: 0,
            })?);
        }
        Ok(pending)
    }
    fn browser_text_target(
        &self,
        user: &str,
        view: &str,
    ) -> Result<(authorization::Scope, u64), EngineError> {
        if let Some(key) = view.strip_prefix("@private:") {
            let scope = self.private_scope(key);
            self.require(Some(user), &scope, authorization::Action::Send)?;
            let chat =
                self.data.private.get(key).ok_or_else(|| {
                    EngineError::NotFound("Private conversation not found.".into())
                })?;
            Ok((scope, chat.revision))
        } else {
            let scope = self.room_scope(view)?;
            self.require(Some(user), &scope, authorization::Action::Send)?;
            Ok((scope, self.data.rooms[view].revision))
        }
    }
    pub(super) fn prepare_append_at(
        &self,
        user: &str,
        view: &str,
        scope: authorization::Scope,
        revision: u64,
        text: &str,
        markdown: bool,
    ) -> Result<PendingMessage, EngineError> {
        let conversation = match &scope {
            authorization::Scope::Room(id) | authorization::Scope::Private(id) => id.clone(),
            _ => return Err("Invalid message scope.".into()),
        };
        let private = view.starts_with("@private:");
        let key = view.strip_prefix("@private:").unwrap_or(view).to_owned();
        let recipient = if private {
            let (_, fallback) = key.split_once(':').ok_or("Invalid private conversation.")?;
            Some(private_peer(&key, user).unwrap_or(fallback))
        } else {
            None
        };
        let mut message = new_message(user, recipient, text);
        message.author_id = self
            .data
            .users
            .get(user)
            .ok_or_else(|| EngineError::Forbidden("Account unavailable.".into()))?
            .id
            .clone();
        message.markdown = markdown;
        if private {
            message.private_id.clone_from(&conversation);
            message
                .mentions
                .retain(|name| key.split(':').any(|participant| participant == name));
        } else {
            let room = self
                .data
                .rooms
                .get(&key)
                .ok_or_else(|| EngineError::NotFound("Conversation not found.".into()))?;
            message.mentions.retain(|name| room.members.contains(name));
        }
        self.pending_message_at(
            user,
            view,
            scope,
            revision,
            message,
            PendingMutation::Append,
        )
    }
    pub(super) fn pending_message_at(
        &self,
        actor: &str,
        view: &str,
        scope: authorization::Scope,
        revision: u64,
        message: Message,
        mutation: PendingMutation,
    ) -> Result<PendingMessage, EngineError> {
        let conversation = match &scope {
            authorization::Scope::Room(id) | authorization::Scope::Private(id) => id.clone(),
            _ => return Err("Invalid message scope.".into()),
        };
        Ok(PendingMessage {
            database: self.store.db.clone(),
            history: self.history_store().clone(),
            message,
            conversation,
            scope,
            actor: actor.into(),
            key: view.strip_prefix("@private:").unwrap_or(view).to_owned(),
            private: view.starts_with("@private:"),
            revision,
            mutation,
            retention: self.max_messages,
            new_conversation: None,
        })
    }
    pub(crate) fn prepare_reaction(
        &self,
        user: &str,
        id: &str,
        mut message: Message,
        reaction: &str,
        active: bool,
    ) -> Result<Option<PendingMessage>, Error> {
        let target = self.bot_target(user, id, authorization::Action::Read)?;
        self.require_message_action(
            Some(user),
            Some(&target.view),
            &target.scope,
            "/react",
            authorization::Action::React,
        )
        .map_err(|_| Error::new(Code::Forbidden, "Reaction permission denied."))?;
        let present = message
            .reactions
            .get(reaction)
            .is_some_and(|users| users.contains(user));
        if present == active {
            return Ok(None);
        }
        if active && !message.reactions.contains_key(reaction) && message.reactions.len() >= 32 {
            return Err(Error::new(
                Code::InvalidRequest,
                "At most 32 reactions per message.",
            ));
        }
        if active {
            message
                .reactions
                .entry(reaction.into())
                .or_default()
                .insert(user.into());
        } else if let Some(users) = message.reactions.get_mut(reaction) {
            users.remove(user);
            if users.is_empty() {
                message.reactions.remove(reaction);
            }
        }
        self.pending_message_at(
            user,
            &target.view,
            target.scope,
            target.revision,
            message,
            PendingMutation::Reaction {
                active,
                reaction: reaction.into(),
                user: user.into(),
            },
        )
        .map(Some)
        .map_err(Into::into)
    }
    pub(crate) fn validate_append_media(
        &self,
        append: &PendingMessage,
        records: &[(media::Media, bool)],
    ) -> Result<(), EngineError> {
        for (media, referenced) in records {
            self.authorize_media_record(&append.actor, &append.scope, media, *referenced)?;
        }
        Ok(())
    }
    pub(crate) fn publish_message(
        &mut self,
        mut append: PendingMessage,
        commit: database::MessageCommit,
    ) -> Result<SendResult, EngineError> {
        let created = append.new_conversation.is_some();
        if created {
            if !append.private
                || self.data.private.contains_key(&append.key)
                || commit.conversation != append.conversation
            {
                self.recovery_failed = true;
                return Err(EngineError::Unavailable(
                    "Committed conversation identity changed; recovery required, retry shortly."
                        .into(),
                ));
            }
            self.data.private.insert(
                append.key.clone(),
                PrivateChat {
                    id: commit.conversation.clone(),
                    revision: commit.revision,
                },
            );
            self.bot_index.conversations.insert(
                commit.conversation.clone(),
                format!("@private:{}", append.key),
            );
            self.read_model.advance();
        }
        let target = if append.private {
            self.data
                .private
                .get_mut(&append.key)
                .filter(|chat| chat.id == commit.conversation)
                .map(|chat| &mut chat.revision)
        } else {
            self.data
                .rooms
                .get_mut(&append.key)
                .filter(|room| room.id == commit.conversation)
                .map(|room| &mut room.revision)
        };
        let Some(revision) = target else {
            self.recovery_failed = true;
            return Err(EngineError::Unavailable(
                "Committed conversation unavailable; recovery required, retry shortly.".into(),
            ));
        };
        *revision = (*revision).max(commit.revision);
        self.data.next_sequence = self.data.next_sequence.max(commit.sequence);
        self.revision = self.revision.wrapping_add(1);
        self.changed_conversation = (!created).then_some(commit.conversation.clone());
        if created {
            self.record_metadata(
                self.scope_notice([authorization::Scope::Private(commit.conversation)].into()),
            );
        }
        append.message.sequence = commit.sequence;
        Ok(SendResult {
            message: append.message,
            replayed: false,
        })
    }
}
impl PendingMessage {
    pub(crate) fn creates_conversation(&self) -> bool {
        self.new_conversation.is_some()
    }
    pub(crate) async fn replay(
        &self,
        key: &str,
        digest: &str,
    ) -> Result<Option<SendResult>, Error> {
        let receipt = self
            .database
            .message_receipt(&database::ReceiptRead {
                account: &self.message.author_id,
                key,
                after: now().saturating_sub(models::RECEIPT_TTL),
            })
            .await
            .map_err(Error::storage)?;
        let Some(receipt) = receipt else {
            return Ok(None);
        };
        if receipt.digest != digest {
            return Err(Error::new(
                Code::IdempotencyConflict,
                "Key already used for another message.",
            ));
        }
        let page = self
            .history
            .page_checked_async(
                &self.conversation,
                self.revision,
                HistoryRange {
                    start: Some(receipt.sequence),
                    end: Some(receipt.sequence),
                    limit: Some(1),
                    ..Default::default()
                },
                "",
            )
            .await
            .map_err(Error::storage)?;
        let message = page.messages.first().cloned().ok_or_else(|| {
            Error::new(Code::MessageGone, "Original message is no longer retained.")
        })?;
        Ok(Some(SendResult {
            message,
            replayed: true,
        }))
    }
    pub(crate) async fn resolve_reply(&mut self, id: Option<&str>) -> Result<(), Error> {
        if let Some(id) = id {
            let original = self
                .history
                .locate_async(id)
                .await
                .map_err(Error::storage)?
                .filter(|(conversation, _)| conversation == &self.conversation)
                .ok_or_else(|| {
                    Error::new(
                        Code::NotFound,
                        "Reply target not found in this conversation.",
                    )
                })?;
            self.message.reply = Some(messages::reply_quote(&original.1));
        }
        Ok(())
    }
    pub(crate) async fn media_records(&self) -> Result<Vec<(media::Media, bool)>, Error> {
        let mut records = Vec::new();
        if matches!(self.mutation, PendingMutation::Reaction { .. }) {
            return Ok(records);
        }
        let references = media::media_references(&self.message.text);
        let previous = match &self.mutation {
            PendingMutation::Edit { previous, .. } => previous.clone(),
            _ => BTreeSet::new(),
        };
        for id in references.difference(&previous) {
            records.push(
                self.database
                    .stored_media(id)
                    .await
                    .map_err(|cause| Error::from(EngineError::from(cause)))?,
            );
        }
        Ok(records)
    }
    pub(super) fn write<'a>(
        &'a self,
        receipt: Option<(&'a str, &'a str)>,
    ) -> database::Result<database::MessageWrite<'a>> {
        Ok(database::MessageWrite {
            conversation: &self.conversation,
            kind: if self.private { "private" } else { "room" },
            key: &self.key,
            revision: self
                .revision
                .checked_add(1)
                .ok_or(database::Error::Conversion(
                    "Conversation revision exhausted",
                ))?,
            expected_revision: self.new_conversation.is_none().then_some(self.revision),
            message: &self.message,
            mutation: match &self.mutation {
                PendingMutation::Append => database::MessageMutation::Append {
                    retention: self.retention,
                },
                PendingMutation::Edit { previous, actor } => database::MessageMutation::Edit {
                    reconcile_media: *previous != media::media_references(&self.message.text),
                    actor,
                },
                PendingMutation::Reaction {
                    active,
                    reaction,
                    user,
                } => database::MessageMutation::Reaction {
                    active: *active,
                    reaction,
                    user,
                },
            },
            new_conversation: self.new_conversation.as_deref(),
            receipt: receipt.map(|(key, digest)| database::MessageReceipt {
                account_id: &self.message.author_id,
                key,
                digest,
                created: now(),
            }),
            at: now(),
        })
    }
    pub(crate) async fn save(
        &self,
        receipt: Option<(&str, &str)>,
    ) -> database::Result<database::MessageCommit> {
        self.database.save_message(&self.write(receipt)?).await
    }
}

fn tell_content(input: &str) -> Result<(&str, &str), EngineError> {
    let content = input
        .trim()
        .strip_prefix("/tell")
        .ok_or("Invalid command.")?
        .trim_start();
    let (recipient, text) = content
        .split_once(char::is_whitespace)
        .ok_or("Usage: /tell user message")?;
    let text = text.trim();
    if text.is_empty() {
        return Err("Usage: /tell user message".into());
    }
    if text.chars().count() > crate::limits::MAX_MESSAGE_CHARS {
        return Err("Messages support at most 4000 characters.".into());
    }
    Ok((recipient, text))
}
