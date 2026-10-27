//! Owned browser message actions; lookup and persistence run outside the engine.
use super::*;
use authorization::{Action, Scope};
use message_write::PendingMutation;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Reply,
    Edit,
    React,
    Retract,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    #[tokio::test]
    async fn expired_retained_attachments_do_not_block_edits_or_reactions() {
        let mut engine = super::super::tests::engine();
        let conversation = engine.data.rooms["lobby"].id.clone();
        let mut media = engine
            .allocate_media("alice", &conversation, "photo.png", 8)
            .unwrap();
        media.ready = true;
        media.object_key = media.final_key();
        media.content_type = "image/png".into();
        engine.finish_media("alice", &media).unwrap();
        let text = format!("![photo](/api/media/{}/file)", media.id);
        engine.execute(Some("alice"), Some("lobby"), &text).unwrap();
        let message = super::super::tests::room_messages(&engine, "lobby")[0].clone();
        engine
            .store
            .db
            .lock()
            .unwrap()
            .execute("UPDATE media SET expires=1 WHERE id=?1", [&media.id])
            .unwrap();
        assert!(engine.readable_media("alice", &media.id).is_err());
        for command in [
            format!("/edit {} {text} retained", message.id),
            format!("/react {} 👍", message.id),
        ] {
            let plan = engine
                .browser_action_plan("alice", Some("lobby"), &command)
                .unwrap();
            let original = plan
                .history
                .locate_async(&plan.action.id)
                .await
                .unwrap()
                .unwrap()
                .1;
            let (pending, _) = engine
                .prepare_browser_action(
                    "alice",
                    Some("lobby"),
                    plan.action,
                    &conversation,
                    original,
                    false,
                )
                .unwrap();
            let pending = pending.unwrap();
            assert!(pending.media_records().await.unwrap().is_empty());
            let commit = pending.save(None).await.unwrap();
            engine.publish_message(pending, commit).unwrap();
        }
        let retained = engine.history("alice", "lobby").unwrap();
        assert_eq!(retained.messages[0].text, format!("{text} retained"));
        assert!(retained.messages[0].reactions.contains_key("👍"));
        assert_eq!(retained.messages[0].sequence, message.sequence);
    }
}
#[derive(Clone)]
pub(crate) struct BrowserAction {
    kind: Kind,
    pub(crate) id: String,
    value: String,
}
pub(crate) struct BrowserActionPlan {
    pub(crate) action: BrowserAction,
    pub(crate) conversation: Option<String>,
    pub(crate) history: std::sync::Arc<HistoryStore>,
}
impl BrowserAction {
    pub(crate) fn is_retraction(&self) -> bool {
        self.kind == Kind::Retract
    }
    fn command(&self) -> &'static str {
        match self.kind {
            Kind::Reply => "/reply",
            Kind::Edit => "/edit",
            Kind::React => "/react",
            Kind::Retract => "/retract",
        }
    }
}
impl Engine {
    pub(crate) fn browser_action_plan(
        &self,
        user: &str,
        view: Option<&str>,
        input: &str,
    ) -> Result<BrowserActionPlan, EngineError> {
        self.action_plan(Some(user), view, input)
    }
    pub(crate) fn action_plan(
        &self,
        actor: Option<&str>,
        view: Option<&str>,
        input: &str,
    ) -> Result<BrowserActionPlan, EngineError> {
        let input = input.trim();
        let command = input.split_whitespace().next().ok_or("Invalid command.")?;
        self.require_command(actor, view, command)?;
        let context = self.command_scope(actor, view)?;
        let kind = match command {
            "/reply" => Kind::Reply,
            "/edit" => Kind::Edit,
            "/react" => Kind::React,
            "/retract" => Kind::Retract,
            _ => return Err("Invalid message action.".into()),
        };
        if actor.is_none() && matches!(kind, Kind::Reply | Kind::React) {
            return Err("Message actions require a user account.".into());
        }
        let content = input
            .strip_prefix(command)
            .ok_or("Invalid command.")?
            .trim_start();
        let (id, value) = if kind == Kind::Retract {
            let parts: Vec<_> = content.split_whitespace().collect();
            if parts.len() != 1 {
                return Err("Usage: /retract message-id".into());
            }
            (parts[0], "")
        } else {
            content.split_once(char::is_whitespace).ok_or(
            "Usage: /react message-id reaction, /reply message-id message or /edit message-id message",
        )?
        };
        let value = value.trim();
        if value.is_empty() && kind != Kind::Retract {
            return Err("A reaction, reply or edit cannot be empty.".into());
        }
        if matches!(kind, Kind::Reply | Kind::Edit) && !has_message_content(value) {
            return Err("Messages cannot be blank.".into());
        }
        Ok(BrowserActionPlan {
            action: BrowserAction {
                kind,
                id: id.into(),
                value: value.into(),
            },
            conversation: match context {
                Scope::Room(id) | Scope::Private(id) => Some(id),
                _ => None,
            },
            history: self.history_store().clone(),
        })
    }
    fn action_target(
        &self,
        actor: Option<&str>,
        view: Option<&str>,
        action: &BrowserAction,
        conversation: &str,
    ) -> Result<(String, Scope, u64), EngineError> {
        let context = self.command_scope(actor, view)?;
        let target = if let Some(room) = view.filter(|view| !view.starts_with('@')) {
            room.to_owned()
        } else {
            let target = self
                .bot_index
                .conversations
                .get(conversation)
                .ok_or("Message not found in this conversation.")?;
            if let Some(key) = target.strip_prefix("@private:") {
                if !self.allows(actor, &self.private_scope(key), Action::Read)
                    || matches!(&context, Scope::Private(id) if id != conversation)
                {
                    return Err("Message not found in this conversation.".into());
                }
            } else if !matches!(action.kind, Kind::Edit | Kind::Retract)
                || !self.is_su(actor)
                || context != Scope::Server
            {
                return Err("Message not found in this conversation.".into());
            }
            target.clone()
        };
        let (scope, revision) = if let Some(key) = target.strip_prefix("@private:") {
            let chat = self
                .data
                .private
                .get(key)
                .ok_or("Message not found in this conversation.")?;
            (Scope::Private(chat.id.clone()), chat.revision)
        } else {
            let room = self.data.rooms.get(&target).ok_or("Room not found.")?;
            (Scope::Room(room.id.clone()), room.revision)
        };
        let permission = match action.kind {
            Kind::Reply => Action::Send,
            Kind::React => Action::React,
            Kind::Edit if self.allows(actor, &scope, Action::EditAny) => Action::EditAny,
            Kind::Edit => Action::EditOwn,
            Kind::Retract if self.allows(actor, &scope, Action::RetractAny) => Action::RetractAny,
            Kind::Retract => Action::RetractOwn,
        };
        self.require_message_action(actor, view, &scope, action.command(), permission)?;
        Ok((target, scope, revision))
    }
    pub(crate) fn authorize_browser_action(
        &self,
        user: &str,
        view: Option<&str>,
        action: &BrowserAction,
        conversation: &str,
    ) -> Result<(), EngineError> {
        self.action_target(Some(user), view, action, conversation)
            .map(|_| ())
    }
    pub(crate) fn prepare_browser_retraction(
        &self,
        user: &str,
        view: Option<&str>,
        action: &BrowserAction,
        conversation: &str,
        original: &Message,
    ) -> Result<PendingHistory, EngineError> {
        self.prepare_retraction(Some(user), view, action, conversation, original)
    }
    pub(crate) fn prepare_retraction(
        &self,
        actor: Option<&str>,
        view: Option<&str>,
        action: &BrowserAction,
        conversation: &str,
        original: &Message,
    ) -> Result<PendingHistory, EngineError> {
        let (_, scope, _) = self.action_target(actor, view, action, conversation)?;
        let id = match &scope {
            Scope::Room(id) | Scope::Private(id) => id,
            _ => return Err("Invalid message scope.".into()),
        };
        if !action.is_retraction() || id != conversation || original.id != action.id {
            return Err("Message not found in this conversation (it may have expired).".into());
        }
        if !self.allows(actor, &scope, Action::RetractAny)
            && self.principal_id(actor) != Some(original.author_id.as_str())
        {
            return Err(EngineError::Forbidden(
                "You can only delete your own messages.".into(),
            ));
        }
        self.prepare_history_write_for(
            actor,
            "/retract",
            &action.id,
            vec![(conversation.into(), original.sequence)],
            vec![],
        )
    }
    pub(crate) fn prepare_browser_action(
        &self,
        user: &str,
        view: Option<&str>,
        action: BrowserAction,
        conversation: &str,
        original: Message,
        markdown: bool,
    ) -> Result<(Option<PendingMessage>, String), EngineError> {
        self.prepare_action(Some(user), view, action, conversation, original, markdown)
    }
    pub(crate) fn prepare_action(
        &self,
        actor: Option<&str>,
        view: Option<&str>,
        action: BrowserAction,
        conversation: &str,
        mut original: Message,
        markdown: bool,
    ) -> Result<(Option<PendingMessage>, String), EngineError> {
        if actor.is_none() && matches!(action.kind, Kind::Reply | Kind::React) {
            return Err("Message actions require a user account.".into());
        }
        let user = actor.unwrap_or("console");
        let (target, scope, revision) = self.action_target(actor, view, &action, conversation)?;
        let target_id = match &scope {
            Scope::Room(id) | Scope::Private(id) => id,
            _ => return Err("Invalid message scope.".into()),
        };
        if target_id != conversation || original.id != action.id {
            return Err("Message not found in this conversation (it may have expired).".into());
        }
        let value = action.value.as_str();
        let (message, mutation, reply) = match action.kind {
            Kind::Retract => return Err("Retraction requires a history operation.".into()),
            Kind::React => {
                if value.chars().count() > crate::limits::MAX_REACTION_CHARS
                    || value.chars().any(char::is_control)
                {
                    return Err(
                        "Reactions support 1–128 characters without control characters.".into(),
                    );
                }
                let active = !original
                    .reactions
                    .get(value)
                    .is_some_and(|users| users.contains(user));
                if active {
                    if !original.reactions.contains_key(value) && original.reactions.len() >= 32 {
                        return Err(EngineError::unavailable(messages::ReactionError::Limit));
                    }
                    original
                        .reactions
                        .entry(value.into())
                        .or_default()
                        .insert(user.into());
                } else if let Some(users) = original.reactions.get_mut(value) {
                    users.remove(user);
                    if users.is_empty() {
                        original.reactions.remove(value);
                    }
                }
                (
                    original,
                    PendingMutation::Reaction {
                        active,
                        reaction: value.into(),
                        user: user.into(),
                    },
                    "Reaction updated.",
                )
            }
            Kind::Edit => {
                if value.chars().count() > crate::limits::MAX_MESSAGE_CHARS {
                    return Err("Messages support at most 4000 characters.".into());
                }
                if self.principal_id(actor) != Some(original.author_id.as_str())
                    && !self.allows(actor, &scope, Action::EditAny)
                {
                    return Err(EngineError::Forbidden(
                        "You can only edit your own messages.".into(),
                    ));
                }
                if original.text == value {
                    return Ok((None, "Message unchanged.".into()));
                }
                let previous = media::media_references(&original.text);
                if actor.is_none() && !media::media_references(value).is_subset(&previous) {
                    return Err("Media uploads require an account.".into());
                }
                let mut mentions = mentioned_names(value);
                if let Some(room) = self.data.rooms.get(&target) {
                    mentions.retain(|name| room.members.contains(name));
                } else {
                    mentions.retain(|name| {
                        name == &original.from || original.to.as_ref() == Some(name)
                    });
                }
                original.text = value.into();
                original.mentions = mentions;
                original.edited = true;
                (
                    original,
                    PendingMutation::Edit {
                        previous,
                        actor: self
                            .principal_id(actor)
                            .ok_or("Account unavailable.")?
                            .into(),
                    },
                    "Message edited.",
                )
            }
            Kind::Reply => {
                if value.chars().count() > crate::limits::MAX_MESSAGE_CHARS {
                    return Err("Messages support at most 4000 characters.".into());
                }
                let mut pending =
                    self.prepare_append_at(user, &target, scope, revision, value, markdown)?;
                pending.message.reply = Some(messages::reply_quote(&original));
                if target.starts_with("@private:") {
                    let peer = if original.from == user {
                        original
                            .to
                            .as_deref()
                            .ok_or("Private message has no recipient.")?
                    } else {
                        &original.from
                    };
                    if !self.active(peer) {
                        return Err(EngineError::NotFound("User not found.".into()));
                    }
                    pending.message.to = Some(peer.into());
                    pending.message.mentions = mentioned_names(value)
                        .into_iter()
                        .filter(|name| name == user || name == peer)
                        .collect();
                }
                return Ok((Some(pending), String::new()));
            }
        };
        self.pending_message_at(user, &target, scope, revision, message, mutation)
            .map(|pending| (Some(pending), reply.into()))
    }
}
