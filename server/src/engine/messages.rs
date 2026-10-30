use super::*;
use authorization::{Action, Scope};

pub(super) enum ReactionError {
    Limit,
}
impl std::fmt::Display for ReactionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Limit => "This message already has 32 different reactions.",
        })
    }
}

pub(super) fn reply_quote(original: &Message) -> Reply {
    Reply {
        id: original.id.clone(),
        from: original.from.clone(),
        text: original.text.chars().take(160).collect(),
        sequence: original.sequence,
    }
}

impl Engine {
    pub(super) fn require_message_action(
        &self,
        actor: Option<&str>,
        view: Option<&str>,
        scope: &Scope,
        command: &str,
        action: Action,
    ) -> Result<(), EngineError> {
        self.require_command(actor, view, command)?;
        self.require_target_command(actor, scope, command)?;
        self.require(actor, scope, Action::Read)?;
        self.require(actor, scope, action)
    }
    #[cfg(test)]
    pub(super) fn message(
        &self,
        actor: Option<&str>,
        to: Option<&str>,
        text: &str,
        markdown: bool,
    ) -> Result<Message, EngineError> {
        if !has_message_content(text) {
            return Err("Messages cannot be blank.".into());
        }
        let mut message = new_message(actor.unwrap_or("console"), to, text);
        message.markdown = markdown;
        message.author_id = actor
            .and_then(|name| self.data.users.get(name))
            .map_or_else(|| "console".into(), |u| u.id.clone());
        // The database assigns the sequence in the append transaction.
        message.sequence = 0;
        Ok(message)
    }
}
