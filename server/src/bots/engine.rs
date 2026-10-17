//! Stable-ID chat operations. Authorization and persistence share the engine lock.
use super::*;
use crate::bots::{
    error::{Code, Error},
    models::{self, Receipt, SendResult},
};
use authorization::{Action, Scope};

pub(crate) struct Target {
    pub view: String,
    pub scope: Scope,
    pub revision: u64,
}
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
    ) -> Result<Vec<(String, String)>, String> {
        let target = self
            .bot_target(user, id, Action::Read)
            .map_err(|e| e.to_string())?;
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
    pub(crate) fn bot_snapshot(&self, user: &str) -> Result<models::Snapshot, Error> {
        let account = self
            .data
            .users
            .get(user)
            .filter(|a| !a.disabled)
            .ok_or_else(|| Error::new(Code::Unauthorized, "Account unavailable."))?;
        self.history_store
            .ensure_available()
            .map_err(Error::storage)?;
        let mut conversations = Vec::new();
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
                unread: {
                    let key = target.view.strip_prefix("@private:").map_or_else(|| format!("room:{}", target.view), |pair| format!("dm:{pair}"));
                    self.history_store.unread(id, user, self.read_position(user, &key), target.revision).map_err(|_| {
                        tracing::warn!(conversation_id = %id, "Bot conversation unread metadata unavailable");
                    }).ok()
                },
            });
        }
        Ok(models::Snapshot {
            kind: "snapshot",
            credential: serde_json::Value::Null,
            account: models::Account {
                id: account.id.clone(),
                username: user.into(),
            },
            conversations,
        })
    }
    pub(crate) fn issue_bot_credential(
        &mut self,
        context: &crate::bots::auth::Context,
        input: models::CredentialInput,
    ) -> Result<serde_json::Value, Error> {
        context.permits(self, "credentials", None)?;
        if context.metadata(self)?.conversations.is_some() {
            return Err(Error::new(
                Code::Forbidden,
                "Only an account-wide provisioning credential may issue tokens.",
            ));
        }
        if !valid_name(&input.label)
            || input.expires <= now()
            || input.expires > now().saturating_add(86400)
            || input.operations.is_empty()
            || input
                .operations
                .iter()
                .any(|op| !models::OPERATIONS.contains(&op.as_str()))
            || input.conversations.iter().any(|id| !models::valid_id(id))
        {
            return Err(Error::new(
                Code::InvalidRequest,
                "Use valid scopes, operations and an expiry within 24 hours.",
            ));
        }
        let user = context.user(self)?;
        for id in &input.conversations {
            self.bot_target(&user, id, Action::Read)?;
        }
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
                label: input.label,
                digest: crate::bots::digest(&secret),
                created: now(),
                expires: Some(input.expires),
                conversations: Some(input.conversations),
                operations: Some(input.operations),
            },
        );
        if let Err(error) = self.save_bot_metadata() {
            if let Some(r) = self.data.bots.get_mut(&context.account) {
                r.tokens.remove(&id);
            }
            return Err(Error::storage(error));
        }
        Ok(serde_json::json!({"id":id,"token":secret,"expires":input.expires}))
    }
    pub(crate) fn create_bot_conversation(
        &mut self,
        user: &str,
        input: models::ConversationInput,
    ) -> Result<String, Error> {
        match input {
            models::ConversationInput::Room { name } => {
                if !valid_name(&name) {
                    return Err(Error::new(Code::InvalidRequest, "Invalid room name."));
                }
                self.require(Some(user), &Scope::Server, Action::CreateRoom)
                    .map_err(|e| Error::new(Code::Forbidden, e))?;
                self.require_target_command(Some(user), &Scope::Server, "/new")
                    .map_err(|e| Error::new(Code::Forbidden, e))?;
                if self.data.rooms.contains_key(&name) {
                    return Err(Error::new(Code::Conflict, "Room already exists."));
                }
                self.execute(Some(user), None, &format!("/new room {name}"))
                    .map_err(Error::storage)?;
                self.data
                    .rooms
                    .get(&name)
                    .map(|r| r.id.clone())
                    .ok_or_else(|| Error::storage("Missing room"))
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
                self.require_target_command(Some(user), &scope, "/tell")
                    .map_err(|e| Error::new(Code::Forbidden, e))?;
                self.require(Some(user), &scope, Action::Send)
                    .map_err(|e| Error::new(Code::Forbidden, e))?;
                if let Some(c) = self.data.private.get(&key) {
                    return Ok(c.id.clone());
                }
                self.require(Some(user), &Scope::Server, Action::Direct)
                    .map_err(|e| Error::new(Code::Forbidden, e))?;
                let Scope::Private(id) = scope else {
                    return Err(Error::storage("Missing private scope"));
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
                    return Err(Error::storage(error));
                }
                Ok(id)
            }
        }
    }
    #[cfg(test)]
    pub(crate) fn bot_read(&mut self, user: &str, id: &str, through: u64) -> Result<bool, Error> {
        let retained = self
            .history_store
            .contains_sequence(id, through)
            .map_err(Error::storage)?;
        self.bot_read_checked(user, id, through, retained)
    }
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
            .history_store
            .locate(message_id)
            .map_err(Error::storage)?
            .filter(|(conversation, _)| conversation == id)
            .map(|(_, m)| m)
            .ok_or_else(|| Error::new(Code::NotFound, "Message not found in this conversation."))?;
        self.bot_react_loaded(user, id, original, reaction, active)
    }
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
        let messages = if let Some(pair) = target.view.strip_prefix("@private:") {
            self.data.private.get_mut(pair).map(|c| &mut c.messages)
        } else {
            self.data
                .rooms
                .get_mut(&target.view)
                .map(|r| &mut r.messages)
        }
        .ok_or_else(|| Error::storage("Missing conversation"))?;
        messages.push_back(original);
        let change = self
            .capture_message_change(&target.view, Some(&message_id), true)
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
    fn validate_bot_send(&self, user: &str, input: &models::MessageInput) -> Result<Target, Error> {
        let text = input.text.trim();
        if text.is_empty() || input.text.chars().count() > 4000 {
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
    pub(crate) fn prepare_bot_send(
        &self,
        user: &str,
        input: &models::MessageInput,
        key: Option<&str>,
    ) -> Result<PreparedSend, Error> {
        let target = self.validate_bot_send(user, input)?;
        let text = input.text.trim();
        let account_id = self
            .data
            .users
            .get(user)
            .ok_or_else(|| Error::new(Code::Unauthorized, "Account unavailable."))?
            .id
            .clone();
        // Tuple order is fixed even if serde_json map-order features change.
        let canonical = serde_json::to_string(&(
            &input.conversation_id,
            text,
            &input.reply_to,
            input.markdown,
        ))
        .map_err(Error::storage)?;
        let digest = crate::bots::digest(&canonical);
        if let Some(key) = key {
            if let Some(receipt) = self.bot_receipt(&account_id, key).map_err(Error::storage)? {
                if receipt.digest != digest {
                    return Err(Error::new(
                        Code::IdempotencyConflict,
                        "Key already used for another message.",
                    ));
                }
                let page = self
                    .history_store
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
                self.history_store
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
        if account_id != prepared.account || target.revision != prepared.revision {
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
                if let Some(message) = prepared.replay.filter(|m| m.sequence == receipt.sequence) {
                    return Ok(SendResult {
                        message,
                        replayed: true,
                    });
                }
                return Err(Error::new(Code::Conflict, "Receipt changed; retry send."));
            }
            if prepared.replay.is_some() {
                return Err(Error::new(Code::Conflict, "Receipt expired; retry send."));
            }
            self.admit_bot_receipt(&account_id)?;
        }
        self.changed_conversation = None;
        let digest = prepared.digest;
        let reply = prepared.reply;
        let text = input.text.trim();
        let change = self
            .capture_message_change(&target.view, None, false)
            .ok_or_else(|| Error::storage("Missing mutation context"))?;
        let result = (|| {
            self.append_text(Some(user), &target.view, text, input.markdown, reply)?;
            let message = if let Some(pair) = target.view.strip_prefix("@private:") {
                self.data.private.get(pair).and_then(|c| c.messages.back())
            } else {
                self.data
                    .rooms
                    .get(&target.view)
                    .and_then(|r| r.messages.back())
            }
            .cloned()
            .ok_or_else(|| "Missing message".to_owned())?;
            Ok(message)
        })();
        let receipt = result.as_ref().ok().and_then(|message| {
            key.map(|key| Receipt {
                account_id,
                key: key.into(),
                digest,
                conversation: input.conversation_id.clone(),
                sequence: message.sequence,
                created: now(),
            })
        });
        let result = self
            .finish_message_change(change, result, receipt.as_ref(), std::convert::identity)
            .map(|message| SendResult {
                message,
                replayed: false,
            })
            .map_err(Error::storage);
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
