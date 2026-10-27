//! Owned authorized query plan: no engine borrow survives disk I/O or serialization.
use super::*;
use futures_util::{StreamExt as _, stream};
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
    pub(super) colors: super::colors::ColorDirectory,
}
impl SnapshotPlan {
    #[cfg(test)]
    pub(crate) fn finish(self) -> Result<(Snapshot, bool), EngineError> {
        database::block_on_storage(self.finish_async())?
    }
    pub(crate) async fn finish_async(mut self) -> Result<(Snapshot, bool), EngineError> {
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
        let requests: Vec<_> = self
            .jobs
            .iter()
            .map(|c| (c.id.as_str(), c.read, c.revision))
            .collect();
        let unreads = self
            .history
            .unread_batch_async(&self.snapshot.username, &requests)
            .await?;
        let page_jobs: Vec<_> = self
            .jobs
            .clone()
            .into_iter()
            .map(|conversation| {
                let history = self.history.clone();
                async move {
                    history
                        .page_checked_async(
                            &conversation.id,
                            conversation.revision,
                            HistoryRange {
                                limit: Some(VISIBLE_HISTORY),
                                ..Default::default()
                            },
                            "",
                        )
                        .await
                }
            })
            .collect();
        let pages = stream::iter(page_jobs).buffered(4);
        futures_util::pin_mut!(pages);
        for (conversation, unread) in self.jobs.iter().zip(unreads) {
            if tail_budget > 0 {
                match pages
                    .next()
                    .await
                    .ok_or_else(|| EngineError::Internal("Missing snapshot page.".into()))?
                {
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
            match unread {
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
        self.history.ensure_available_async().await?;
        let mut visible: BTreeSet<&str> = self
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
        visible.extend(super::colors::message_names(&self.snapshot.direct));
        for room in &self.snapshot.rooms {
            visible.extend(super::colors::message_names(&room.messages));
        }
        self.snapshot.colors = self
            .colors
            .global
            .into_iter()
            .filter(|(name, _)| visible.contains(name.as_str()))
            .collect();
        // A room override also reveals room access, so it is limited to names
        // this room already discloses: visible members, tail authors and the viewer.
        for room in &mut self.snapshot.rooms {
            let Some(overrides) = self.colors.rooms.remove(&room.id) else {
                continue;
            };
            let names = super::colors::message_names(&room.messages);
            room.colors = overrides
                .into_iter()
                .filter(|(name, _)| {
                    *name == self.snapshot.username
                        || room.members.contains(name)
                        || names.contains(name.as_str())
                })
                .collect();
        }
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
    #[cfg(test)]
    pub(crate) fn finish(self) -> Result<crate::bots::models::Snapshot, crate::bots::error::Error> {
        database::block_on_storage(self.finish_async())
            .map_err(crate::bots::error::Error::storage)?
    }
    pub(crate) async fn finish_async(
        mut self,
    ) -> Result<crate::bots::models::Snapshot, crate::bots::error::Error> {
        use crate::bots::error::Error;
        let requests: Vec<_> = self
            .snapshot
            .conversations
            .iter()
            .map(|c| {
                (
                    c.id.as_str(),
                    self.cursors.get(&c.id).copied().unwrap_or(0),
                    c.content_revision,
                )
            })
            .collect();
        let unreads = self
            .history
            .unread_batch_async(&self.snapshot.account.username, &requests)
            .await
            .map_err(Error::storage)?;
        for (conversation, unread) in self.snapshot.conversations.iter_mut().zip(unreads) {
            conversation.unread = unread.map_err(|_| {
                tracing::warn!(conversation_id=%conversation.id,"Bot conversation unread metadata unavailable");
            }).ok();
        }
        self.history
            .ensure_available_async()
            .await
            .map_err(Error::storage)?;
        Ok(self.snapshot)
    }
}

/// A conversation notification owns its authorization result and read coordinates.
pub(crate) struct ConversationPlan {
    pub(super) colors: super::colors::ColorDirectory,
    pub(super) color_version: String,
    pub(super) user: String,
    pub(super) conversation: Conversation,
    pub(super) history: Arc<HistoryStore>,
}
impl ConversationPlan {
    pub(crate) fn id(&self) -> &str {
        &self.conversation.id
    }
    #[cfg(test)]
    pub(crate) fn finish(self) -> Result<serde_json::Value, EngineError> {
        database::block_on_storage(self.finish_async())?
    }
    pub(crate) async fn finish_async(self) -> Result<serde_json::Value, EngineError> {
        let job = self.conversation;
        let page = self
            .history
            .page_checked_async(
                &job.id,
                job.revision,
                HistoryRange {
                    limit: Some(VISIBLE_HISTORY),
                    ..Default::default()
                },
                &job.view,
            )
            .await?;
        let unread = self
            .history
            .unread_batch_async(&self.user, &[(&job.id, job.read, job.revision)])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| EngineError::Internal("Missing unread result.".into()))??;
        let mut frame = serde_json::json!({"kind":"conversation","view":job.view,"id":job.id,"messages":page.messages,"unread":unread});
        if !self.color_version.is_empty() {
            frame["name_colors"] = serde_json::to_value(self.colors.for_messages(
                self.color_version,
                Some(&job.id),
                &page.messages,
            ))?;
        }
        Ok(frame)
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
    #[cfg(test)]
    pub(crate) fn finish(self) -> Result<BTreeMap<String, Unread>, EngineError> {
        database::block_on_storage(self.finish_async())?
    }
    pub(crate) async fn finish_async(self) -> Result<BTreeMap<String, Unread>, EngineError> {
        let mut unread = BTreeMap::new();
        let requests: Vec<_> = self
            .conversations
            .iter()
            .map(|c| (c.id.as_str(), c.read, c.revision))
            .collect();
        let results = self
            .history
            .unread_batch_async(&self.user, &requests)
            .await?;
        for (job, result) in self.conversations.into_iter().zip(results) {
            unread.insert(job.view, result?);
        }
        self.history.ensure_available_async().await?;
        Ok(unread)
    }
}
