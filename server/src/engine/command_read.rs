//! Owned command queries; database work runs after metadata guards are released.
use super::*;
use authorization::Scope;

pub(crate) enum CommandRead {
    Text(String),
    History(HistoryCommand),
    Preview(cleanup::CleanupPlan),
}
impl CommandRead {
    pub(crate) fn ids(&self) -> BTreeSet<String> {
        match self {
            Self::Text(_) => BTreeSet::new(),
            Self::History(plan) => [plan.id.clone()].into(),
            Self::Preview(plan) => plan.ids().into_iter().collect(),
        }
    }
    pub(crate) async fn finish_async(self) -> Result<String, EngineError> {
        match self {
            Self::Text(text) => Ok(text),
            Self::History(plan) => plan.finish_async().await,
            Self::Preview(mut plan) => {
                plan.count().await?;
                Ok(plan.report())
            }
        }
    }
    #[cfg(test)]
    pub(super) fn finish(self) -> Result<String, EngineError> {
        match self {
            Self::Text(text) => Ok(text),
            plan => database::block_on_storage(plan.finish_async())?,
        }
    }
}
pub(crate) struct HistoryCommand {
    history: std::sync::Arc<HistoryStore>,
    id: String,
    revision: u64,
    range: HistoryRange,
    view: String,
}
impl HistoryCommand {
    async fn finish_async(self) -> Result<String, EngineError> {
        let page = self
            .history
            .page_checked_async(&self.id, self.revision, self.range, &self.view)
            .await
            .map_err(EngineError::from)?;
        Ok(Self::render(&page, self.range))
    }
    #[cfg(test)]
    pub(super) fn finish(self) -> Result<String, EngineError> {
        database::block_on_storage(self.finish_async())?
    }
    fn render(page: &History, range: HistoryRange) -> String {
        let text = page
            .messages
            .iter()
            .map(|m| {
                format!(
                    "[{}] {} {}{}: {}",
                    m.sequence,
                    m.time,
                    m.from,
                    m.to.as_ref()
                        .map(|to| format!(" → {to}"))
                        .unwrap_or_default(),
                    m.text
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        if text.is_empty() {
            "No messages in this range.".into()
        } else if page.has_after
            && range
                .end
                .zip(page.messages.last())
                .is_some_and(|(end, message)| message.sequence < end)
        {
            format!("{text}\nMore history available; continue after the last sequence.")
        } else {
            text
        }
    }
}
impl Engine {
    pub(super) fn history_command_plan(
        &self,
        context: &CommandContext<'_>,
    ) -> Result<HistoryCommand, EngineError> {
        let actor = context.actor;
        let parts = context.parts;
        let usage = "Usage: /history [start end] or /history #room";
        let (view, range) = match parts {
            [_] => (
                context
                    .view
                    .ok_or("Select a conversation or use /history #room.")?,
                history::HistoryRange::default(),
            ),
            [_, target] if target.starts_with('#') && target.len() > 1 => (
                target.strip_prefix('#').ok_or(usage)?,
                history::HistoryRange::default(),
            ),
            [_, start, end] => (
                context
                    .view
                    .ok_or("Select a conversation for sequence ranges.")?,
                history::HistoryRange {
                    start: Some(start.parse().map_err(|_| usage)?),
                    end: Some(end.parse().map_err(|_| usage)?),
                    ..Default::default()
                },
            ),
            _ => return Err(usage.into()),
        };
        let (id, revision) = if let Some(user) = actor {
            let (_, id, revision) = self.conversation(user, view)?;
            (id, revision)
        } else if let Some(key) = view.strip_prefix("@private:") {
            let chat = self
                .data
                .private
                .get(key)
                .ok_or_else(|| EngineError::NotFound("Conversation not found.".into()))?;
            (chat.id.clone(), chat.revision)
        } else {
            let room = self
                .data
                .rooms
                .get(view)
                .ok_or_else(|| EngineError::NotFound("Room not found.".into()))?;
            (room.id.clone(), room.revision)
        };
        let scope = if view.starts_with('@') {
            Scope::Private(id.clone())
        } else {
            Scope::Room(id.clone())
        };
        self.require_target_command(actor, &scope, context.command)?;
        Ok(HistoryCommand {
            history: self.history_store().clone(),
            id,
            revision,
            range,
            view: view.into(),
        })
    }
}
