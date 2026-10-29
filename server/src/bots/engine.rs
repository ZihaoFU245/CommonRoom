//! Stable-ID chat operations. Authorization and persistence share the engine lock.
use super::*;
#[cfg(test)]
use crate::bots::models::SendResult;
use crate::bots::{
    error::{Code, Error},
    models,
};
use authorization::{Action, Scope};

pub(crate) struct Target {
    pub view: String,
    pub scope: Scope,
    pub revision: u64,
}
#[cfg(test)]
pub(crate) struct PreparedSend {
    revision: u64,
    account: String,
    digest: String,
    reply: Option<Reply>,
    replay: Option<Message>,
}
impl Engine {
    pub(crate) fn bot_command_peers(
        &self,
        user: &str,
        id: &str,
    ) -> Result<Vec<(String, String)>, EngineError> {
        let target = self
            .bot_target(user, id, Action::Read)
            .map_err(EngineError::from)?;
        if !self.allows(Some(user), &target.scope, Action::Send)
            || !self.allows(Some(user), &target.scope, Action::Members)
        {
            return Ok(Vec::new());
        }
        let members: Vec<_> = target.view.strip_prefix("@private:").map_or_else(
            || {
                self.data
                    .rooms
                    .get(&target.view)
                    .map(|room| room.members.iter().cloned().collect())
                    .unwrap_or_default()
            },
            |pair| pair.split(':').map(str::to_owned).collect(),
        );
        Ok(members
            .into_iter()
            .filter_map(|name| {
                let account = self.data.users.get(&name)?;
                (self.data.bots.contains_key(&account.id)
                    && !account.disabled
                    && self.bot_target(&name, id, Action::Read).is_ok()
                    && self.bot_target(&name, id, Action::Send).is_ok())
                .then(|| (account.id.clone(), name))
            })
            .collect())
    }
    pub(crate) fn bot_target(&self, user: &str, id: &str, action: Action) -> Result<Target, Error> {
        if !models::valid_id(id) {
            return Err(Error::new(Code::InvalidRequest, "Invalid conversation ID."));
        }
        if !self.active(user) {
            return Err(Error::new(Code::Unauthorized, "Account unavailable."));
        }
        let view = self
            .bot_index
            .conversations
            .get(id)
            .ok_or_else(|| Error::new(Code::NotFound, "Conversation not found."))?;
        let target = if let Some(key) = view.strip_prefix("@private:") {
            let chat = self
                .data
                .private
                .get(key)
                .filter(|c| c.id == id)
                .ok_or_else(|| Error::new(Code::NotFound, "Conversation not found."))?;
            Target {
                view: view.clone(),
                scope: self.private_scope(key),
                revision: chat.revision,
            }
        } else {
            let room = self
                .data
                .rooms
                .get(view)
                .filter(|r| r.id == id)
                .ok_or_else(|| Error::new(Code::NotFound, "Conversation not found."))?;
            Target {
                view: view.clone(),
                scope: Scope::Room(id.into()),
                revision: room.revision,
            }
        };
        if !self.allows(Some(user), &target.scope, action) {
            return Err(Error::new(
                Code::Forbidden,
                "Conversation permission denied.",
            ));
        }
        Ok(target)
    }
    #[cfg(test)]
    pub(crate) fn bot_snapshot(&self, user: &str) -> Result<models::Snapshot, Error> {
        self.bot_snapshot_plan(user)?.finish()
    }
    pub(crate) fn bot_snapshot_plan(
        &self,
        user: &str,
    ) -> Result<super::snapshot::BotSnapshotPlan, Error> {
        let account = self
            .data
            .users
            .get(user)
            .filter(|a| !a.disabled)
            .ok_or_else(|| Error::new(Code::Unauthorized, "Account unavailable."))?;
        if let Some(mut plan) = self.read_model.bot(&account.id) {
            for conversation in &mut plan.snapshot.conversations {
                let (revision, read) = self
                    .conversation_position(user, &conversation.id)
                    .ok_or_else(|| Error::storage("Conversation metadata unavailable."))?;
                conversation.content_revision = revision;
                plan.cursors.insert(conversation.id.clone(), read);
            }
            return Ok(plan);
        }
        self.ensure_memory().map_err(Error::storage)?;
        let mut conversations = Vec::new();
        let mut cursors = BTreeMap::new();
        for (id, view) in self
            .data
            .rooms
            .iter()
            .map(|(view, r)| (&r.id, view.clone()))
            .chain(self.data.private.iter().map(|(key, c)| {
                (
                    &c.id,
                    private_peer(key, user).map_or_else(
                        || format!("@private:{key}"),
                        |peer| format!("@direct:{peer}"),
                    ),
                )
            }))
        {
            let Some(internal_view) = self.bot_index.conversations.get(id) else {
                continue;
            };
            let scope = self
                .command_scope(Some(user), Some(internal_view))
                .map_err(Error::storage)?;
            let rights = self.effective(Some(user), &scope);
            if !rights.has(Action::Read) {
                continue;
            }
            let target = Target {
                view: internal_view.clone(),
                scope,
                revision: self
                    .data
                    .rooms
                    .get(internal_view)
                    .map(|r| r.revision)
                    .or_else(|| {
                        internal_view
                            .strip_prefix("@private:")
                            .and_then(|key| self.data.private.get(key).map(|c| c.revision))
                    })
                    .ok_or_else(|| Error::storage("Missing conversation metadata."))?,
            };
            let send = rights.has(Action::Send);
            let command = |name, action| rights.command(name) && rights.has(action);
            let members = if rights.has(Action::Members) {
                self.data.rooms.get(&view).map_or_else(
                    || {
                        target
                            .view
                            .strip_prefix("@private:")
                            .map(|pair| pair.split(':').map(str::to_owned).collect())
                            .unwrap_or_default()
                    },
                    |r| r.members.iter().cloned().collect(),
                )
            } else {
                Vec::new()
            };
            let key = target.view.strip_prefix("@private:").map_or_else(
                || format!("room:{}", target.view),
                |pair| format!("dm:{pair}"),
            );
            cursors.insert(id.clone(), self.read_position(user, &key));
            conversations.push(models::Conversation {
                id: id.clone(),
                view: view.clone(),
                members,
                permissions: models::Permissions {
                    send,
                    reply: command("/reply", Action::Send),
                    react: command("/react", Action::React),
                },
                content_revision: target.revision,
                unread: None,
            });
        }
        let plan = super::snapshot::BotSnapshotPlan {
            history: self.history_store().clone(),
            cursors,
            snapshot: models::Snapshot {
                kind: "snapshot",
                policy_revision: self.data.policy.revision,
                server_instance: self.instance.clone(),
                credential: serde_json::Value::Null,
                account: models::Account {
                    id: account.id.clone(),
                    username: user.into(),
                },
                conversations,
            },
        };
        self.read_model.remember_bot(account.id.clone(), &plan);
        Ok(plan)
    }
    #[cfg(test)]
    pub(crate) fn issue_bot_credential(
        &mut self,
        context: &crate::bots::auth::Context,
        input: models::CredentialInput,
    ) -> Result<serde_json::Value, Error> {
        let (result, before) = self.apply_bot_credential(context, input)?;
        if let Err(error) = self.save_bot_metadata(&before) {
            before.restore(&mut self.data);
            return Err(Error::storage(error));
        }
        Ok(result)
    }
    pub(crate) fn prepare_bot_credential(
        &mut self,
        context: &crate::bots::auth::Context,
        input: models::CredentialInput,
    ) -> Result<(serde_json::Value, metadata_write::PendingMetadata), Error> {
        let dirty = self.metadata_dirty;
        let (result, before) = self.apply_bot_credential(context, input)?;
        let pending = self.capture_metadata(&before, vec![]);
        before.restore(&mut self.data);
        self.metadata_dirty = dirty;
        pending
            .map(|pending| (result, pending))
            .map_err(Error::storage)
    }
    fn authorize_worker_request(
        &self,
        user: &str,
        input: &models::CredentialInput,
    ) -> Result<(), Error> {
        if input.operations.contains("media_upload")
            && !["read", "send"]
                .iter()
                .all(|op| input.operations.contains(*op))
        {
            return Err(Error::new(
                Code::InvalidRequest,
                "Media upload requires read and send operations.",
            ));
        }
        if input.conversations.is_some()
            && input
                .operations
                .iter()
                .any(|op| matches!(op.as_str(), "commands" | "create"))
        {
            return Err(Error::new(
                Code::Forbidden,
                "Commands and conversation creation require an explicit account-wide worker scope (conversations:null).",
            ));
        }
        let targets = match &input.conversations {
            Some(ids) => ids
                .iter()
                .map(|id| self.bot_target(user, id, Action::Read))
                .collect::<Result<Vec<_>, _>>()?,
            None => self
                .bot_index
                .conversations
                .keys()
                .filter_map(|id| self.bot_target(user, id, Action::Read).ok())
                .collect(),
        };
        for operation in &input.operations {
            if operation == "profile" {
                continue;
            } // Own appearance is self-service.
            if operation == "create" {
                let room = self.allows(Some(user), &Scope::Server, Action::CreateRoom)
                    && self
                        .require_target_command(Some(user), &Scope::Server, "/new")
                        .is_ok();
                let direct = self.allows(Some(user), &Scope::Server, Action::Direct)
                    && self
                        .data
                        .users
                        .keys()
                        .filter(|peer| peer.as_str() != user)
                        .any(|peer| {
                            let scope = self.private_scope(&private_key(user, peer));
                            self.allows(Some(user), &scope, Action::Send)
                                && self
                                    .require_target_command(Some(user), &scope, "/tell")
                                    .is_ok()
                        });
                if room || direct {
                    continue;
                }
                return Err(Error::new(
                    Code::Forbidden,
                    "Account cannot create conversations.",
                ));
            }
            let allowed = |target: &Target| {
                let rights = self.effective(Some(user), &target.scope);
                match operation.as_str() {
                    "read" | "read_ack" | "config" => rights.has(Action::Read),
                    "send" | "activity" => rights.has(Action::Read) && rights.has(Action::Send),
                    "react" => {
                        rights.has(Action::Read)
                            && rights.has(Action::React)
                            && rights.command("/react")
                    }
                    "media_upload" => {
                        rights.has(Action::Read)
                            && rights.has(Action::Send)
                            && rights.has(Action::MediaUpload)
                    }
                    "commands" => self
                        .commands_for(Some(user), Some(&target.view))
                        .iter()
                        .any(|command| crate::bots::supported_command(command.name)),
                    _ => false,
                }
            };
            // Explicit scopes promise every requested operation on every listed
            // conversation. Null delegates the account's live grants, so an
            // operation must be possible somewhere, never broader than those grants.
            let permitted = match &input.conversations {
                Some(_) => targets.iter().all(allowed),
                None => targets.iter().any(allowed),
            };
            if !permitted {
                return Err(Error::new(
                    Code::Forbidden,
                    format!("Account permissions cannot satisfy worker operation {operation}."),
                ));
            }
        }
        Ok(())
    }
    fn apply_bot_credential(
        &mut self,
        context: &crate::bots::auth::Context,
        input: models::CredentialInput,
    ) -> Result<(serde_json::Value, MetadataChange), Error> {
        let user = context.issuer(self)?;
        if !valid_name(&input.label)
            || input.expires <= now()
            || input.expires > now().saturating_add(86400)
            || input.operations.is_empty()
            || input
                .operations
                .iter()
                .any(|op| !models::OPERATIONS.contains(&op.as_str()))
            || input
                .conversations
                .as_ref()
                .is_some_and(|ids| ids.iter().any(|id| !models::valid_id(id)))
        {
            return Err(Error::new(
                Code::InvalidRequest,
                "Use valid scopes, operations and an expiry within 24 hours.",
            ));
        }
        self.authorize_worker_request(&user, &input)?;
        let before = MetadataChange::keyed(
            &self.data,
            changes::DirtyKeys {
                bots: [context.account.clone()].into(),
                ..Default::default()
            },
        );
        let registration = self
            .data
            .bots
            .get_mut(&context.account)
            .ok_or_else(|| Error::new(Code::Unauthorized, "Registration gone."))?;
        if registration.tokens.len() >= 8
            || registration.tokens.values().any(|c| c.label == input.label)
        {
            return Err(Error::new(
                Code::InvalidRequest,
                "Credential limit or duplicate label.",
            ));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let secret = format!(
            "bot_{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        registration.tokens.insert(
            id.clone(),
            models::Credential {
                kind: models::CredentialKind::Worker,
                label: input.label,
                digest: crate::bots::digest(&secret),
                created: now(),
                expires: Some(input.expires),
                conversations: input.conversations,
                operations: Some(input.operations),
            },
        );
        Ok((
            serde_json::json!({"id":id,"token":secret,"expires":input.expires}),
            before,
        ))
    }
    pub(crate) fn prepare_bot_conversation(
        &mut self,
        user: &str,
        input: models::ConversationInput,
    ) -> Result<(String, Option<metadata_write::PendingMetadata>), Error> {
        match input {
            models::ConversationInput::Room { name } => {
                if !valid_name(&name) {
                    return Err(Error::new(Code::InvalidRequest, "Invalid room name."));
                }
                self.require(Some(user), &Scope::Server, Action::CreateRoom)?;
                self.require_target_command(Some(user), &Scope::Server, "/new")?;
                if self.data.rooms.contains_key(&name) {
                    return Err(Error::new(Code::Conflict, "Room already exists."));
                }
                let (_, pending) =
                    self.prepare_metadata_command(Some(user), None, &format!("/new room {name}"))?;
                let pending = pending.ok_or_else(|| Error::storage("Missing room creation"))?;
                let id = pending
                    .room_id(&name)
                    .ok_or_else(|| Error::storage("Missing room"))?
                    .to_owned();
                Ok((id, Some(pending)))
            }
            models::ConversationInput::Direct { username } => {
                if !valid_name(&username) {
                    return Err(Error::new(Code::InvalidRequest, "Invalid username."));
                }
                if !self.active(&username) {
                    return Err(Error::new(Code::NotFound, "Account not found."));
                }
                let key = private_key(user, &username);
                let scope = self.private_scope(&key);
                self.require_target_command(Some(user), &scope, "/tell")?;
                self.require(Some(user), &scope, Action::Send)?;
                if let Some(chat) = self.data.private.get(&key) {
                    return Ok((chat.id.clone(), None));
                }
                self.require(Some(user), &Scope::Server, Action::Direct)?;
                let Scope::Private(id) = scope else {
                    return Err(Error::storage("Missing private scope"));
                };
                let before = MetadataChange::keyed(
                    &self.data,
                    changes::DirtyKeys {
                        private: [key.clone()].into(),
                        ..Default::default()
                    },
                );
                self.data.private.insert(
                    key,
                    PrivateChat {
                        id: id.clone(),
                        ..Default::default()
                    },
                );
                let pending = self.capture_metadata(&before, vec![]);
                before.restore(&mut self.data);
                pending
                    .map(|pending| (id, Some(pending)))
                    .map_err(Error::storage)
            }
        }
    }
    #[cfg(test)]
    pub(crate) fn bot_read(&mut self, user: &str, id: &str, through: u64) -> Result<bool, Error> {
        let retained = self
            .history_store()
            .contains_sequence(id, through)
            .map_err(Error::storage)?;
        self.bot_read_checked(user, id, through, retained)
    }
    #[cfg(test)]
    pub(crate) fn bot_read_checked(
        &mut self,
        user: &str,
        id: &str,
        through: u64,
        retained: bool,
    ) -> Result<bool, Error> {
        let target = self.bot_target(user, id, Action::Read)?;
        let key = target.view.strip_prefix("@private:").map_or_else(
            || format!("room:{}", target.view),
            |pair| format!("dm:{pair}"),
        );
        if self.read_position(user, &key) >= through {
            return Ok(false);
        }
        if !retained {
            return Err(Error::new(
                Code::MessageGone,
                "Read position is no longer retained.",
            ));
        }
        // Preconditions are checked above; any remaining failure is storage failure.
        self.advance_read_position(user, &key, through)
            .map_err(Error::storage)
    }
    #[cfg(test)]
    pub(crate) fn bot_react(
        &mut self,
        user: &str,
        id: &str,
        message_id: &str,
        reaction: &str,
        active: bool,
    ) -> Result<bool, Error> {
        let original = self
            .history_store()
            .locate(message_id)
            .map_err(Error::storage)?
            .filter(|(conversation, _)| conversation == id)
            .map(|(_, m)| m)
            .ok_or_else(|| Error::new(Code::NotFound, "Message not found in this conversation."))?;
        self.bot_react_loaded(user, id, original, reaction, active)
    }
    #[cfg(test)]
    pub(crate) fn bot_react_loaded(
        &mut self,
        user: &str,
        id: &str,
        original: Message,
        reaction: &str,
        active: bool,
    ) -> Result<bool, Error> {
        let message_id = original.id.clone();
        if !models::valid_id(&message_id) {
            return Err(Error::new(Code::InvalidRequest, "Invalid message ID."));
        }
        let target = self.bot_target(user, id, Action::Read)?;
        if self
            .require_message_action(
                Some(user),
                Some(&target.view),
                &target.scope,
                "/react",
                Action::React,
            )
            .is_err()
        {
            return Err(Error::new(Code::Forbidden, "Reaction permission denied."));
        }
        // Load only the affected row, then share mutation/rollback with browser toggles.
        self.working = Some(messages::WorkingMessage {
            reaction: None,
            conversation: id.into(),
            message: original,
        });
        let change = self
            .capture_message_change(Some(user), &target.view, Some(&message_id), true)
            .ok_or_else(|| Error::storage("Missing reaction rollback"))?;
        self.changed_conversation = None;
        let result = self
            .update_reaction(&target.scope, user, &message_id, reaction, Some(active))
            .map_err(|error| match error {
                messages::ReactionError::Limit => {
                    Error::new(Code::InvalidRequest, "At most 32 reactions per message.")
                }
                messages::ReactionError::NotFound => Error::storage(error),
            });
        let result = self.finish_message_change(change, result, None, Error::storage);
        self.clear_working_messages();
        result
    }
    pub(in crate::engine) fn validate_bot_send(
        &self,
        user: &str,
        input: &models::MessageInput,
    ) -> Result<Target, Error> {
        let text = input.text.trim();
        if !has_message_content(text)
            || input.text.chars().count() > crate::limits::MAX_MESSAGE_CHARS
        {
            return Err(Error::new(
                Code::InvalidRequest,
                "Messages support 1–4000 characters.",
            ));
        }
        if input
            .reply_to
            .as_ref()
            .is_some_and(|id| !models::valid_id(id))
        {
            return Err(Error::new(Code::InvalidRequest, "Invalid reply target ID."));
        }
        let target = self.bot_target(user, &input.conversation_id, Action::Send)?;
        if !self.allows(Some(user), &target.scope, Action::Read) {
            return Err(Error::new(
                Code::Forbidden,
                "Read permission required for replies and keyed sends.",
            ));
        }
        if input.reply_to.is_some()
            && self
                .require_message_action(
                    Some(user),
                    Some(&target.view),
                    &target.scope,
                    "/reply",
                    Action::Send,
                )
                .is_err()
        {
            return Err(Error::new(Code::Forbidden, "Reply permission denied."));
        }
        Ok(target)
    }
    #[cfg(test)]
    pub fn send_bot_message(
        &mut self,
        user: &str,
        input: &models::MessageInput,
        key: Option<&str>,
    ) -> Result<SendResult, Error> {
        let prepared = self.prepare_bot_send(user, input, key)?;
        self.commit_bot_send(user, input, key, prepared)
    }
    #[cfg(test)]
    pub(crate) fn prepare_bot_send(
        &self,
        user: &str,
        input: &models::MessageInput,
        key: Option<&str>,
    ) -> Result<PreparedSend, Error> {
        let target = self.validate_bot_send(user, input)?;
        let account_id = self
            .data
            .users
            .get(user)
            .ok_or_else(|| Error::new(Code::Unauthorized, "Account unavailable."))?
            .id
            .clone();
        let digest = models::message_digest(input)?;
        if let Some(key) = key {
            if let Some(receipt) = self.bot_receipt(&account_id, key).map_err(Error::storage)? {
                if receipt.digest != digest {
                    return Err(Error::new(
                        Code::IdempotencyConflict,
                        "Key already used for another message.",
                    ));
                }
                let page = self
                    .history_store()
                    .page(
                        &input.conversation_id,
                        target.revision,
                        HistoryRange {
                            start: Some(receipt.sequence),
                            end: Some(receipt.sequence),
                            limit: Some(1),
                            ..Default::default()
                        },
                        "",
                    )
                    .map_err(Error::storage)?;
                let message = page.messages.first().cloned().ok_or_else(|| {
                    Error::new(Code::MessageGone, "Original message is no longer retained.")
                })?;
                return Ok(PreparedSend {
                    revision: target.revision,
                    account: account_id,
                    digest,
                    reply: None,
                    replay: Some(message),
                });
            }
            self.admit_bot_receipt(&account_id)?;
        }
        let reply = input
            .reply_to
            .as_ref()
            .map(|id| {
                self.history_store()
                    .locate(id)
                    .map_err(Error::storage)
                    .and_then(|original| {
                        original
                            .filter(|(conversation, _)| conversation == &input.conversation_id)
                            .map(|(_, m)| messages::reply_quote(&m))
                            .ok_or_else(|| {
                                Error::new(
                                    Code::NotFound,
                                    "Reply target not found in this conversation.",
                                )
                            })
                    })
            })
            .transpose()?;
        Ok(PreparedSend {
            revision: target.revision,
            account: account_id,
            digest,
            reply,
            replay: None,
        })
    }
    #[cfg(test)]
    pub(crate) fn commit_bot_send(
        &mut self,
        user: &str,
        input: &models::MessageInput,
        key: Option<&str>,
        prepared: PreparedSend,
    ) -> Result<SendResult, Error> {
        let target = self.validate_bot_send(user, input)?;
        let account_id = self
            .data
            .users
            .get(user)
            .ok_or_else(|| Error::new(Code::Unauthorized, "Account unavailable."))?
            .id
            .clone();
        if account_id != prepared.account {
            return Err(Error::new(
                Code::Conflict,
                "Conversation changed; retry send.",
            ));
        }
        if let Some(key) = key {
            if let Some(receipt) = self.bot_receipt(&account_id, key).map_err(Error::storage)? {
                if receipt.digest != prepared.digest {
                    return Err(Error::new(
                        Code::IdempotencyConflict,
                        "Key already used for another message.",
                    ));
                }
                // Another admitted request may have committed this receipt after
                // preparation. Re-read the retained original under current access
                // rather than returning a scheduling-dependent conflict.
                let page = self
                    .history_store()
                    .page_checked(
                        &input.conversation_id,
                        target.revision,
                        HistoryRange {
                            start: Some(receipt.sequence),
                            end: Some(receipt.sequence),
                            limit: Some(1),
                            ..Default::default()
                        },
                        "",
                    )
                    .map_err(Error::storage)?;
                let message = page.messages.first().cloned().ok_or_else(|| {
                    Error::new(Code::MessageGone, "Original message is no longer retained.")
                })?;
                return Ok(SendResult {
                    message,
                    replayed: true,
                });
            }
            if prepared.replay.is_some() {
                return Err(Error::new(Code::Conflict, "Receipt expired; retry send."));
            }
            self.admit_bot_receipt(&account_id)?;
        }
        if target.revision != prepared.revision {
            return Err(Error::new(
                Code::Conflict,
                "Conversation changed; retry send.",
            ));
        }
        self.changed_conversation = None;
        let digest = prepared.digest;
        let reply = prepared.reply;
        let text = input.text.trim();
        let change = self
            .capture_message_change(Some(user), &target.view, None, false)
            .ok_or_else(|| Error::storage("Missing mutation context"))?;
        let result: Result<_, EngineError> = (|| {
            self.append_text(Some(user), &target.view, text, input.markdown, reply)?;
            let message = self
                .working_message(&input.conversation_id)
                .cloned()
                .ok_or_else(|| EngineError::Internal("Missing committed message.".into()))?;
            Ok(message)
        })();
        let receipt = result.as_ref().ok().and_then(|_| {
            key.map(|key| database::MessageReceipt {
                account_id: &account_id,
                key,
                digest: &digest,
                created: now(),
            })
        });
        let result = self
            .finish_message_change(change, result, receipt, std::convert::identity)
            .and_then(|mut message| {
                message.sequence = self
                    .working_message(&input.conversation_id)
                    .ok_or_else(|| EngineError::Internal("Missing committed message.".into()))?
                    .sequence;
                Ok(SendResult {
                    message,
                    replayed: false,
                })
            })
            .map_err(Error::from);
        self.clear_working_messages();
        result
    }
    #[cfg(test)]
    pub fn send_literal(&mut self, user: &str, view: &str, text: &str) -> Result<Message, Error> {
        let (_, id, _) = self.conversation(user, view).map_err(Error::storage)?;
        self.send_bot_message(
            user,
            &models::MessageInput {
                conversation_id: id,
                text: text.into(),
                reply_to: None,
                markdown: false,
            },
            None,
        )
        .map(|r| r.message)
    }
}
