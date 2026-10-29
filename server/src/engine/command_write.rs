//! Native command preparation owns data; asynchronous continuations never retain
//! a borrowed engine or temporary command authority.
use super::*;

pub(crate) enum NativeCommand {
    Ready(String, PendingCommand),
    Read(CommandRead),
    System(crate::system::Command),
    Metadata(MetadataPreparation),
    Action(BrowserActionPlan),
    Cleanup(cleanup::CleanupPlan),
}
impl NativeCommand {
    pub(crate) fn ordering_ids(&self) -> Vec<String> {
        match self {
            Self::Ready(_, PendingCommand::Message(pending)) => vec![pending.conversation.clone()],
            Self::Action(plan) => plan.conversation.iter().cloned().collect(),
            Self::Cleanup(plan) => plan.ids(),
            _ => vec![],
        }
    }
    pub(crate) fn audited(&self) -> bool {
        matches!(self, Self::Action(plan) if plan.action.is_retraction())
            || matches!(self, Self::Cleanup(plan) if !plan.preview)
    }
}
impl Engine {
    pub(crate) fn reject_command(
        &self,
        actor: Option<&str>,
        view: Option<&str>,
        input: &str,
    ) -> Result<String, EngineError> {
        crate::limits::validate_input(input).map_err(EngineError::Invalid)?;
        crate::commands::validate_room_arguments(input).map_err(EngineError::Invalid)?;
        let command = input.split_whitespace().next().unwrap_or("");
        self.require_command(actor, view, command)?;
        if command == "/reset" {
            Err("Account provisioning is console-only.".into())
        } else {
            Err("Unknown command. Try /help.".into())
        }
    }

    pub(crate) fn prepare_native_command(
        &mut self,
        user: &str,
        view: &str,
        input: &str,
        markdown: bool,
    ) -> Result<NativeCommand, EngineError> {
        if let Some(read) = self.prepare_read_command(Some(user), Some(view), input) {
            return read.map(NativeCommand::Read);
        }
        match input.split_whitespace().next() {
            Some("/tell") => {
                let pending = self.prepare_browser_message(user, Some(view), input, markdown)?;
                let reply = format!(
                    "Private message sent to {}.",
                    pending
                        .message
                        .to
                        .as_deref()
                        .ok_or("Private message has no recipient.")?
                );
                Ok(NativeCommand::Ready(
                    reply,
                    PendingCommand::Message(Box::new(pending)),
                ))
            }
            Some("/reply" | "/edit" | "/react" | "/retract") => {
                let plan = self.browser_action_plan(user, Some(view), input)?;
                if let Some(id) = &plan.conversation {
                    self.authorize_browser_action(user, Some(view), &plan.action, id)?;
                }
                Ok(NativeCommand::Action(plan))
            }
            Some("/clean") => self
                .prepare_browser_cleanup(user, Some(view), input)
                .map(NativeCommand::Cleanup),
            _ => self
                .prepare_metadata(Some(user), Some(view), input)
                .map(NativeCommand::Metadata),
        }
    }
    pub(crate) fn prepare_native_action(
        &self,
        user: &str,
        view: &str,
        plan: BrowserActionPlan,
        original: Message,
        conversation: &str,
        markdown: bool,
    ) -> Result<(String, PendingCommand), EngineError> {
        if plan.action.is_retraction() {
            self.prepare_browser_retraction(user, Some(view), &plan.action, conversation, &original)
                .map(|pending| ("Message deleted.".into(), PendingCommand::History(pending)))
        } else {
            self.prepare_browser_action(
                user,
                Some(view),
                plan.action,
                conversation,
                original,
                markdown,
            )
            .map(|(pending, reply)| {
                (
                    reply,
                    pending.map_or(PendingCommand::None, |pending| {
                        PendingCommand::Message(Box::new(pending))
                    }),
                )
            })
        }
    }
}
