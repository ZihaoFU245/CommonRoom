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
impl Engine {
    pub(crate) fn bot_target(&self, user: &str, id: &str, action: Action) -> Result<Target, Error> {
        if !models::valid_id(id) {
            return Err(Error::new(Code::InvalidRequest, "Invalid conversation ID."));
        }
        if !self.active(user) {
            return Err(Error::new(Code::Unauthorized, "Account unavailable."));
        }
        let target = self
            .data
            .rooms
            .iter()
            .find(|(_, r)| r.id == id)
            .map(|(view, r)| Target {
                view: view.clone(),
                scope: Scope::Room(id.into()),
                revision: r.revision,
            })
            .or_else(|| {
                self.data
                    .private
                    .iter()
                    .find(|(_, c)| c.id == id)
                    .map(|(key, c)| Target {
                        view: format!("@private:{key}"),
                        scope: self.private_scope(key),
                        revision: c.revision,
                    })
            })
            .ok_or_else(|| Error::new(Code::NotFound, "Conversation not found."))?;
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
        let unread = self.unreads(user).map_err(Error::storage)?;
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
            let target = match self.bot_target(user, id, Action::Read) {
                Ok(target) => target,
                Err(_) => continue,
            };
            let send = self.allows(Some(user), &target.scope, Action::Send);
            let command = |name, action| {
                self.require_message_action(
                    Some(user),
                    Some(&target.view),
                    &target.scope,
                    name,
                    action,
                )
                .is_ok()
            };
            let members = if self.allows(Some(user), &target.scope, Action::Members) {
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
                unread: unread
                    .get(&view)
                    .cloned()
                    .ok_or_else(|| Error::storage("Missing unread metadata"))?,
            });
        }
        Ok(models::Snapshot {
            kind: "snapshot",
            account: models::Account {
                id: account.id.clone(),
                username: user.into(),
            },
            conversations,
        })
    }
    pub(crate) fn bot_read(&mut self, user: &str, id: &str, through: u64) -> Result<bool, Error> {
        let target = self.bot_target(user, id, Action::Read)?;
        let key = target.view.strip_prefix("@private:").map_or_else(
            || format!("room:{}", target.view),
            |pair| format!("dm:{pair}"),
        );
        if self.read_position(user, &key) >= through {
            return Ok(false);
        }
        if !self
            .history_store
            .contains_sequence(id, through)
            .map_err(Error::storage)?
        {
            return Err(Error::new(
                Code::MessageGone,
                "Read position is no longer retained.",
            ));
        }
        // Preconditions are checked above; any remaining failure is storage failure.
        self.advance_read_position(user, &key, through)
            .map_err(Error::storage)
    }
    pub(crate) fn bot_react(
        &mut self,
        user: &str,
        id: &str,
        message_id: &str,
        reaction: &str,
        active: bool,
    ) -> Result<bool, Error> {
        if !models::valid_id(message_id) {
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
        let original = self
            .history_store
            .locate(message_id)
            .map_err(Error::storage)?
            .filter(|(conversation, _)| conversation == id)
            .map(|(_, m)| m)
            .ok_or_else(|| Error::new(Code::NotFound, "Message not found in this conversation."))?;
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
            .capture_message_change(&target.view, Some(message_id), true)
            .ok_or_else(|| Error::storage("Missing reaction rollback"))?;
        self.changed_conversation = None;
        let result = self
            .update_reaction(&target.scope, user, message_id, reaction, Some(active))
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
    pub fn send_bot_message(
        &mut self,
        user: &str,
        input: &models::MessageInput,
        key: Option<&str>,
    ) -> Result<SendResult, Error> {
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
        self.changed_conversation = None;
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
                let message = page.messages.into_iter().next().ok_or_else(|| {
                    Error::new(Code::MessageGone, "Original message is no longer retained.")
                })?;
                return Ok(SendResult {
                    message,
                    replayed: true,
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
