use super::*;

// Bounded 100-row maintenance batches; the caller's transaction covers all batches.
pub(super) fn rewrite_account_messages(
    db: &Connection,
    name: &str,
    new_name: Option<&str>,
) -> Result<(), EngineError> {
    let replacement = new_name
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{name} (deleted)"));
    let mut after = 0u64;
    loop {
        let mut query=db.prepare("SELECT m.conversation,m.json FROM message_accounts a JOIN messages m ON m.conversation=a.conversation AND m.sequence=a.sequence WHERE a.name=?2 AND a.sequence>?1 ORDER BY a.sequence LIMIT 100").map_err(EngineError::from)?;
        let rows = query
            .query_map(params![after, name], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(EngineError::from)?;
        let batch = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(EngineError::from)?;
        if batch.is_empty() {
            break;
        }
        for (id, json) in batch {
            let mut m: Message = serde_json::from_str(&json).map_err(EngineError::from)?;
            after = m.sequence;
            let mut changed = false;
            if m.from == name {
                m.from = replacement.clone();
                changed = true;
            }
            if new_name.is_some() && m.to.as_deref() == Some(name) {
                m.to = new_name.map(str::to_owned);
                changed = true;
            }
            if let Some(reply) = &mut m.reply
                && reply.from == name
            {
                reply.from = replacement.clone();
                changed = true;
            }
            if m.mentions.remove(name) {
                if let Some(new) = new_name {
                    m.mentions.insert(new.into());
                }
                changed = true;
            }
            if changed {
                put_message(db, &id, &m)?;
            }
        }
    }
    if let Some(new) = new_name {
        db.execute(
            "UPDATE OR IGNORE reactions SET username=?1 WHERE username=?2",
            params![new, name],
        )
        .map_err(EngineError::from)?;
    }
    db.execute("DELETE FROM reactions WHERE username=?1", [name])
        .map_err(EngineError::from)?;
    Ok(())
}

pub(super) fn account_conversations(
    db: &Connection,
    name: &str,
) -> Result<BTreeSet<String>, EngineError> {
    let mut query = db.prepare("SELECT conversation FROM message_accounts WHERE name=?1 UNION SELECT conversation FROM reactions WHERE username=?1").map_err(EngineError::from)?;
    query
        .query_map([name], |r| r.get(0))
        .map_err(EngineError::from)?
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(EngineError::from)
}

impl Engine {
    pub(in crate::engine) fn account_reference_ids(
        &self,
        name: &str,
    ) -> Result<BTreeSet<String>, EngineError> {
        let db = self.store.db.lock().map_err(EngineError::unavailable)?;
        account_conversations(&db, name)
    }
    /// Rename account state and every affected read cursor in one transaction.
    pub(in crate::engine) fn store_account_rename(
        &mut self,
        name: &str,
        new_name: &str,
        conversations: &BTreeMap<String, String>,
        affected: &BTreeSet<String>,
        before: &MetadataChange,
    ) -> Result<(), EngineError> {
        let writer = self.store.db.clone();
        let mut db = writer.lock().map_err(EngineError::unavailable)?;
        let transaction = db.savepoint().map_err(EngineError::from)?;
        // A reused name must never inherit orphaned cursors from an old account.
        transaction
            .execute(
                "DELETE FROM read_positions WHERE username=?1",
                params![new_name],
            )
            .map_err(EngineError::from)?;
        transaction
            .execute(
                "UPDATE read_positions SET username=?1 WHERE username=?2",
                params![new_name, name],
            )
            .map_err(EngineError::from)?;
        for (old, new) in conversations {
            transaction
                .execute(
                    "DELETE FROM read_positions WHERE conversation=?1",
                    params![new],
                )
                .map_err(EngineError::from)?;
            transaction
                .execute(
                    "UPDATE read_positions SET conversation=?1 WHERE conversation=?2",
                    params![new, old],
                )
                .map_err(EngineError::from)?;
        }
        rewrite_account_messages(&transaction, name, Some(new_name))?;
        for (id, revision) in self
            .data
            .rooms
            .values_mut()
            .map(|r| (&r.id, &mut r.revision))
            .chain(
                self.data
                    .private
                    .values_mut()
                    .map(|c| (&c.id, &mut c.revision)),
            )
        {
            if affected.contains(id) {
                *revision = revision.wrapping_add(1);
            }
        }
        write_delta_view(&transaction, &self.data, before.view())?;
        transaction.commit().map_err(EngineError::from)
    }
    /// Account deletion and its cursor cleanup must commit atomically.
    pub(in crate::engine) fn store_account_deletion(
        &mut self,
        name: &str,
        removed: &BTreeSet<String>,
        before: &MetadataChange,
    ) -> Result<(), EngineError> {
        let writer = self.store.db.clone();
        let mut db = writer.lock().map_err(EngineError::unavailable)?;
        let transaction = db.savepoint().map_err(EngineError::from)?;
        rewrite_account_messages(&transaction, name, None)?;
        write_delta_view(&transaction, &self.data, before.view())?;
        transaction
            .execute(
                "DELETE FROM read_positions WHERE username=?1",
                params![name],
            )
            .map_err(EngineError::from)?;
        for key in removed {
            transaction
                .execute(
                    "DELETE FROM read_positions WHERE conversation=?1",
                    params![key],
                )
                .map_err(EngineError::from)?;
        }
        transaction.commit().map_err(EngineError::from)
    }
}
