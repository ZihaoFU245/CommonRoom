//! Owned authorized query plan: no engine borrow survives disk I/O or serialization.
use super::*;
use std::sync::Arc;

#[derive(Clone)]
pub(super) struct Conversation {
    pub(super) view: String,
    pub(super) id: String,
    pub(super) revision: u64,
    pub(super) read: u64,
    pub(super) direct: bool,
}
#[derive(Clone)]
pub(crate) struct SnapshotPlan {
    pub(super) snapshot: Snapshot,
    pub(super) jobs: Vec<Conversation>,
    pub(super) history: Arc<HistoryStore>,
    pub(super) avatars: BTreeMap<String, String>,
}
impl SnapshotPlan {
    pub(crate) fn finish(mut self) -> Result<(Snapshot, bool), EngineError> {
        self.history.ensure_available()?;
        let mut complete = true;
        let mut tail_budget = 1000usize;
        let mut byte_budget = 4 * 1024 * 1024usize;
        let room_indices: BTreeMap<_, _> = self
            .snapshot
            .rooms
            .iter()
            .enumerate()
            .map(|(index, room)| (room.id.clone(), index))
            .collect();
        for conversation in &self.jobs {
            if tail_budget > 0 {
                match self.history.page_checked(
                    &conversation.id,
                    conversation.revision,
                    HistoryRange {
                        limit: Some(VISIBLE_HISTORY),
                        ..Default::default()
                    },
                    "",
                ) {
                    Ok(page) => {
                        let messages = page
                            .messages
                            .bounded_tail(&mut tail_budget, &mut byte_budget);
                        if conversation.direct {
                            self.snapshot.direct.extend(messages.iter().cloned());
                        } else if let Some(index) = room_indices.get(&conversation.id) {
                            self.snapshot.rooms[*index].messages = messages;
                        }
                    }
                    Err(_) => {
                        tracing::warn!(conversation_id=%conversation.id,"Conversation history unavailable; snapshot tail omitted.");
                        complete = false;
                    }
                }
            }
            match self.history.unread(
                &conversation.id,
                &self.snapshot.username,
                conversation.read,
                conversation.revision,
            ) {
                Ok(unread) => {
                    self.snapshot
                        .unread
                        .insert(conversation.view.clone(), unread);
                }
                Err(_) => {
                    tracing::warn!(conversation_id=%conversation.id,"Conversation unread data unavailable; snapshot entry omitted.");
                    complete = false;
                }
            }
        }
        self.snapshot.direct.sort_by_key(|message| message.sequence);
        self.history.ensure_available()?;
        let visible: BTreeSet<&str> = self
            .snapshot
            .users
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(self.snapshot.username.as_str()))
            .chain(self.snapshot.private_peers.iter().map(String::as_str))
            .chain(
                self.snapshot
                    .direct
                    .iter()
                    .map(|message| message.from.as_str()),
            )
            .chain(
                self.snapshot
                    .rooms
                    .iter()
                    .flat_map(|room| room.members.iter().map(String::as_str)),
            )
            .chain(
                self.snapshot
                    .rooms
                    .iter()
                    .flat_map(|room| room.messages.iter().map(|message| message.from.as_str())),
            )
            .collect();
        self.snapshot.avatars = self
            .avatars
            .into_iter()
            .filter(|(name, _)| visible.contains(name.as_str()))
            .collect();
        Ok((self.snapshot, complete))
    }
}

#[derive(Clone)]
pub(crate) struct BotSnapshotPlan {
    pub(crate) snapshot: crate::bots::models::Snapshot,
    pub(super) cursors: BTreeMap<String, u64>,
    pub(super) history: Arc<HistoryStore>,
}
impl BotSnapshotPlan {
    pub(crate) fn finish(
        mut self,
    ) -> Result<crate::bots::models::Snapshot, crate::bots::error::Error> {
        use crate::bots::error::Error;
        self.history.ensure_available().map_err(Error::storage)?;
        for conversation in &mut self.snapshot.conversations {
            conversation.unread=self.history.unread(&conversation.id,&self.snapshot.account.username,self.cursors.get(&conversation.id).copied().unwrap_or(0),conversation.content_revision).map_err(|_| {
                tracing::warn!(conversation_id=%conversation.id,"Bot conversation unread metadata unavailable");
            }).ok();
        }
        self.history.ensure_available().map_err(Error::storage)?;
        Ok(self.snapshot)
    }
}

/// A conversation notification owns its authorization result and read coordinates.
pub(crate) struct ConversationPlan {
    pub(super) user: String,
    pub(super) conversation: Conversation,
    pub(super) history: Arc<HistoryStore>,
}
impl ConversationPlan {
    pub(crate) fn id(&self) -> &str {
        &self.conversation.id
    }
    pub(crate) fn finish(self) -> Result<serde_json::Value, EngineError> {
        let job = self.conversation;
        let page = self.history.page_checked(
            &job.id,
            job.revision,
            HistoryRange {
                limit: Some(VISIBLE_HISTORY),
                ..Default::default()
            },
            &job.view,
        )?;
        let unread = self
            .history
            .unread(&job.id, &self.user, job.read, job.revision)?;
        Ok(
            serde_json::json!({"kind":"conversation","view":job.view,"id":job.id,"messages":page.messages,"unread":unread}),
        )
    }
}
pub(crate) struct UnreadPlan {
    pub(super) user: String,
    pub(super) conversations: Vec<Conversation>,
    pub(super) history: Arc<HistoryStore>,
}
impl UnreadPlan {
    pub(crate) fn ids(&self) -> impl Iterator<Item = &str> {
        self.conversations.iter().map(|job| job.id.as_str())
    }
    pub(crate) fn finish(self) -> Result<BTreeMap<String, Unread>, EngineError> {
        self.history.ensure_available()?;
        let mut unread = BTreeMap::new();
        for job in self.conversations {
            unread.insert(
                job.view,
                self.history
                    .unread(&job.id, &self.user, job.read, job.revision)?,
            );
        }
        self.history.ensure_available()?;
        Ok(unread)
    }
}
