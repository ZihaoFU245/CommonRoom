use super::*;

pub(in crate::engine) const CLEANUP_COUNTS_SQL: crate::engine::database::Sql<'static> = sql(
    "SELECT targets.value, count(DISTINCT m.sequence), count(refs.media_id) FROM json_each(?1) AS targets LEFT JOIN messages AS m ON m.conversation=targets.value AND json_extract(m.json,'$.time')<?2 LEFT JOIN message_media AS refs ON refs.conversation=m.conversation AND refs.sequence=m.sequence GROUP BY targets.value",
    "SELECT targets.value, count(DISTINCT m.sequence), count(refs.media_id) FROM jsonb_array_elements_text($1::text::jsonb) AS targets LEFT JOIN messages AS m ON m.conversation=targets.value AND ((m.projection #>> '{time}'))::bigint<$2 LEFT JOIN message_media AS refs ON refs.conversation=m.conversation AND refs.sequence=m.sequence GROUP BY targets.value",
);

pub(super) fn write_history_deletions(
    tx: &Connection,
    deletes: &[(String, u64)],
    cleanups: &[(String, u64)],
) -> Result<(), EngineError> {
    for (id, sequence) in deletes {
        let message_id:String=tx.query_row(sql("SELECT json_extract(json,'$.id') FROM messages WHERE conversation=?1 AND sequence=?2", "SELECT (projection #>> '{id}') FROM messages WHERE conversation=$1 AND sequence=$2"),params![id,sequence],|r|r.get(0)).map_err(EngineError::from)?;
        if tx.is_postgres() {
            scrub_postgres_replies(tx, id, &message_id)?;
        } else {
            tx.execute("UPDATE messages SET json=json_set(json,'$.reply',NULL) WHERE conversation=?1 AND json_extract(json,'$.reply.id')=?2",params![id,message_id])?;
        }
        tx.execute(
            sql(
                "DELETE FROM messages WHERE conversation=?1 AND sequence=?2",
                "DELETE FROM messages WHERE conversation=$1 AND sequence=$2",
            ),
            params![id, sequence],
        )
        .map_err(EngineError::from)?;
    }
    for (id, cutoff) in cleanups {
        tx.execute(
            sql("DELETE FROM messages WHERE conversation=?1 AND json_extract(json,'$.time')<?2", "DELETE FROM messages WHERE conversation=$1 AND ((projection #>> '{time}'))::bigint<$2"),
            params![id, cutoff],
        )
        .map_err(EngineError::from)?;
    }
    Ok(())
}

// PostgreSQL cannot parse escaped NULs inside arbitrary message text as JSONB.
// Seek via the safe reply projection, then rewrite bounded payload batches in Rust.
fn scrub_postgres_replies(
    db: &Connection,
    conversation: &str,
    target: &str,
) -> Result<(), EngineError> {
    let mut after = 0u64;
    loop {
        let mut query=db.prepare_cached("SELECT json FROM messages WHERE conversation=$1 AND (projection #>> '{reply,id}')=$2 AND sequence>$3 ORDER BY sequence LIMIT 100")?;
        let batch = query
            .query_map(params![conversation, target, after], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        if batch.is_empty() {
            break;
        }
        for raw in batch {
            let mut message: Message = serde_json::from_str(&raw)?;
            after = message.sequence;
            message.reply = None;
            put_message(db, conversation, &message)?;
        }
    }
    Ok(())
}

impl Engine {
    /// One indexed range per target, batched on a single reader connection.
    /// The existing messages_time expression index covers the cutoff predicate.
    pub(in crate::engine) fn cleanup_counts(
        &self,
        ids: &[String],
        cutoff: u64,
    ) -> Result<BTreeMap<String, (usize, usize)>, EngineError> {
        self.history_store().read(|db| {
            let mut totals = BTreeMap::new();
            let mut query = db
                .prepare_cached(CLEANUP_COUNTS_SQL)
                .map_err(EngineError::unavailable)?;
            for batch in ids.chunks(256) {
                let json = serde_json::to_string(batch).map_err(EngineError::unavailable)?;
                let rows = query
                    .query_map(params![json, cutoff], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            (row.get::<_, usize>(1)?, row.get::<_, usize>(2)?),
                        ))
                    })
                    .map_err(EngineError::unavailable)?;
                for row in rows {
                    let (id, counts) = row.map_err(EngineError::unavailable)?;
                    totals.insert(id, counts);
                }
            }
            Ok(totals)
        })
    }
    pub(in crate::engine) fn save_history_maintenance(&mut self) -> Result<(), EngineError> {
        if self.store.pending_deletes.is_empty() && self.store.pending_cleanups.is_empty() {
            return Ok(());
        }
        let ids: BTreeSet<_> = self
            .store
            .pending_deletes
            .iter()
            .chain(self.store.pending_cleanups.iter())
            .map(|(id, _)| id.clone())
            .collect();
        let scopes = ids
            .iter()
            .filter_map(|id| {
                self.bot_index.conversations.get(id).map(|view| {
                    if view.starts_with("@private:") {
                        authorization::Scope::Private(id.clone())
                    } else {
                        authorization::Scope::Room(id.clone())
                    }
                })
            })
            .collect();
        let notice = self.scope_notice(scopes);
        let writer = self.store.db.clone();
        let mut db = writer.lock().map_err(EngineError::unavailable)?;
        let tx = db.savepoint().map_err(EngineError::from)?;
        write_history_deletions(
            &tx,
            &self.store.pending_deletes,
            &self.store.pending_cleanups,
        )?;
        for (kind, key, id, revision) in self
            .data
            .rooms
            .iter()
            .map(|(key, r)| ("room", key, &r.id, r.revision))
            .chain(
                self.data
                    .private
                    .iter()
                    .map(|(key, c)| ("private", key, &c.id, c.revision)),
            )
        {
            if ids.contains(id) {
                tx.execute(
                    sql(
                        "UPDATE entities SET revision=?3 WHERE kind=?1 AND key=?2",
                        "UPDATE entities SET revision=$3 WHERE kind=$1 AND key=$2",
                    ),
                    params![kind, key, revision],
                )
                .map_err(EngineError::from)?;
            }
        }
        write_audit(&tx, &self.data)?;
        tx.commit().map_err(EngineError::from)?;
        self.revision = self.revision.wrapping_add(1);
        self.record_metadata(notice);
        if ids.len() == 1 {
            self.changed_conversation = ids.into_iter().next();
        }
        Ok(())
    }
}
