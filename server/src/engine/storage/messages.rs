use super::*;
use dispatch::MessageMutation;

impl Engine {
    pub(in crate::engine) fn save_message_change_with_receipt(
        &mut self,
        change: &super::dispatch::MessageChange,
        receipt: Option<database::MessageReceipt<'_>>,
    ) -> Result<(), EngineError> {
        let (id, revision) = if change.private {
            self.data
                .private
                .get(&change.key)
                .map(|chat| (chat.id.clone(), chat.revision))
        } else {
            self.data
                .rooms
                .get(&change.key)
                .map(|room| (room.id.clone(), room.revision))
        }
        .ok_or_else(|| EngineError::NotFound("Conversation not found.".into()))?;
        if !matches!(change.mutation, MessageMutation::Append) && Some(revision) == change.revision
        {
            return Ok(());
        }
        let current = self
            .working_message(&id)
            .ok_or_else(|| EngineError::Internal("Missing working message.".into()))?;
        let new_conversation = if change.new_conversation() {
            let chat =
                self.data.private.get(&change.key).ok_or_else(|| {
                    EngineError::NotFound("Private conversation not found.".into())
                })?;
            Some(private_json(chat)?)
        } else {
            None
        };
        let mutation = match &change.mutation {
            MessageMutation::Append => database::MessageMutation::Append {
                retention: self.max_messages,
            },
            MessageMutation::Edit { attachments, actor } => database::MessageMutation::Edit {
                actor,
                reconcile_media: *attachments
                    != crate::engine::media::media_references(&current.text),
            },
            MessageMutation::Reaction => {
                let delta = self
                    .working
                    .as_ref()
                    .and_then(|row| row.reaction.as_ref())
                    .ok_or_else(|| EngineError::Internal("Missing reaction change.".into()))?;
                database::MessageMutation::Reaction {
                    active: delta.active,
                    reaction: &delta.reaction,
                    user: &delta.user,
                }
            }
        };
        let request = database::MessageWrite {
            conversation: &id,
            kind: if change.private { "private" } else { "room" },
            key: &change.key,
            revision,
            expected_revision: change.revision,
            message: current,
            mutation,
            new_conversation: new_conversation.as_deref(),
            receipt,
            at: now(),
        };
        let committed = self.store.db.save_message_blocking(&request)?;
        // Publish only the sequence/version returned after successful commit.
        self.data.next_sequence = self.data.next_sequence.max(committed.sequence);
        if let Some(working) = &mut self.working {
            working.message.sequence = committed.sequence;
        }
        let target_revision = if change.private {
            self.data
                .private
                .get_mut(&change.key)
                .map(|chat| &mut chat.revision)
        } else {
            self.data
                .rooms
                .get_mut(&change.key)
                .map(|room| &mut room.revision)
        };
        if let Some(revision) = target_revision {
            *revision = committed.revision;
        }
        self.changed_conversation = (!change.new_conversation()).then_some(committed.conversation);
        if change.new_conversation() {
            self.read_model.advance();
            self.bot_index
                .conversations
                .insert(id.clone(), format!("@private:{}", change.key));
        }
        self.revision = self.revision.wrapping_add(1);
        if change.new_conversation() {
            self.record_metadata(
                self.scope_notice([authorization::Scope::Private(id.clone())].into()),
            );
        }
        Ok(())
    }
}
