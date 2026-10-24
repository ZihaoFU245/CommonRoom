use super::*;
use dispatch::MessageMutation;

// Borrow the payload during serialization; reactions are independent rows.
#[derive(serde::Serialize)]
struct MessageRecord<'a> {
    id: &'a str,
    from: &'a str,
    author_id: &'a str,
    markdown: bool,
    edited: bool,
    private_id: &'a str,
    to: &'a Option<String>,
    text: &'a str,
    time: u64,
    sequence: u64,
    reply: &'a Option<Reply>,
    mentions: &'a BTreeSet<String>,
}
pub(super) fn put_message(
    db: &Connection,
    key: &str,
    message: &Message,
) -> Result<(), EngineError> {
    let record = MessageRecord {
        id: &message.id,
        from: &message.from,
        author_id: &message.author_id,
        markdown: message.markdown,
        edited: message.edited,
        private_id: &message.private_id,
        to: &message.to,
        text: &message.text,
        time: message.time,
        sequence: message.sequence,
        reply: &message.reply,
        mentions: &message.mentions,
    };
    let json = serde_json::to_string(&record).map_err(EngineError::from)?;
    db.prepare_cached("INSERT INTO messages VALUES(?1,?2,?3) ON CONFLICT(conversation,sequence) DO UPDATE SET json=excluded.json WHERE json!=excluded.json")?.execute(params![key,message.sequence,json]).map_err(EngineError::from)?;
    Ok(())
}
impl Engine {
    pub(in crate::engine) fn save_message_change_with_receipt(
        &mut self,
        change: &super::dispatch::MessageChange,
        receipt: Option<&crate::bots::models::Receipt>,
    ) -> Result<(), EngineError> {
        let (id, revision) = if change.private {
            self.data
                .private
                .get(&change.key)
                .map(|chat| (&chat.id, chat.revision))
        } else {
            self.data
                .rooms
                .get(&change.key)
                .map(|room| (&room.id, room.revision))
        }
        .ok_or_else(|| EngineError::NotFound("Conversation not found.".into()))?;
        if !matches!(change.mutation, MessageMutation::Append) && Some(revision) == change.revision
        {
            return Ok(());
        }
        let current = self
            .working_message(id)
            .ok_or_else(|| EngineError::Internal("Missing working message.".into()))?;
        let writer = self.store.db.clone();
        let mut db = writer.lock().map_err(EngineError::unavailable)?;
        let tx = db.savepoint().map_err(EngineError::from)?;
        if let MessageMutation::Reaction = &change.mutation {
            let delta = self
                .working
                .as_ref()
                .and_then(|row| row.reaction.as_ref())
                .ok_or_else(|| EngineError::Internal("Missing reaction change.".into()))?;
            if delta.active {
                tx.prepare_cached("INSERT INTO reactions VALUES(?1,?2,?3,?4)")?
                    .execute(params![id, current.sequence, delta.reaction, delta.user])?;
            } else {
                tx.prepare_cached("DELETE FROM reactions WHERE conversation=?1 AND sequence=?2 AND reaction=?3 AND username=?4")?.execute(params![id, current.sequence, delta.reaction, delta.user])?;
            }
        } else {
            put_message(&tx, id, current)?;
            if let MessageMutation::Edit { attachments } = &change.mutation {
                if *attachments != crate::engine::media::media_references(&current.text) {
                    media::reconcile(&tx, id, current)?;
                }
            } else {
                media::reconcile_insert(&tx, id, current)?;
            }
        }
        if matches!(change.mutation, MessageMutation::Append) {
            retention::prune(&tx, id, self.max_messages)?;
            tx.execute(
                "UPDATE counters SET sequence=?1 WHERE id=1",
                params![self.data.next_sequence],
            )
            .map_err(EngineError::from)?;
        }
        let kind = if change.private { "private" } else { "room" };
        if change.new_conversation() {
            let chat =
                self.data.private.get(&change.key).ok_or_else(|| {
                    EngineError::NotFound("Private conversation not found.".into())
                })?;
            put_entity(&tx, kind, &change.key, &private_metadata(chat))?;
        }
        tx.execute(
            "UPDATE entities SET revision=?3 WHERE kind=?1 AND key=?2",
            params![kind, change.key, revision],
        )
        .map_err(EngineError::from)?;
        if let Some(receipt) = receipt {
            tx.execute(
                "DELETE FROM bot_receipts WHERE created<=?1",
                [receipt
                    .created
                    .saturating_sub(crate::bots::models::RECEIPT_TTL)],
            )
            .map_err(EngineError::from)?;
            tx.execute(
                "INSERT INTO bot_receipts VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    receipt.account_id,
                    receipt.key,
                    receipt.digest,
                    receipt.conversation,
                    receipt.sequence,
                    receipt.created
                ],
            )
            .map_err(EngineError::from)?;
        }
        tx.commit().map_err(EngineError::from)?;
        self.changed_conversation = (!change.new_conversation()).then(|| id.clone());
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
