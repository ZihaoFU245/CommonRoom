//! Owned administrative metadata deltas; preparation restores memory before I/O.
use super::*;
use database::{MetadataOp, MetadataWrite};

pub(crate) enum MetadataPreparation {
    Ready(String, Option<Box<PendingMetadata>>),
    Rename(super::accounts::AccountRenameRead),
}

pub(crate) struct PendingMetadata {
    database: database::Handle,
    before: MetadataChange,
    after: MetadataChange,
    operations: Vec<MetadataOp<'static>>,
    audit: Vec<authorization::Audit>,
    authorization: authorization::CompiledPolicy,
    notice: MetadataNotice,
    removed_rooms: Vec<String>,
    renamed_cursors: Vec<(String, String)>,
    removed_accounts: Vec<(String, String)>,
    renamed_accounts: Vec<(String, String)>,
    removed_cursors: BTreeSet<String>,
}
impl PendingMetadata {
    pub(super) fn room_id(&self, name: &str) -> Option<&str> {
        self.after
            .view()
            .rooms?
            .get(name)
            .map(|room| room.id.as_str())
    }

    pub(super) fn account_rename(mut self, old: String, new: String) -> Self {
        self.operations.insert(
            0,
            MetadataOp::RewriteAccount {
                old: old.clone(),
                new: Some(new.clone()),
            },
        );
        self.operations.insert(
            0,
            MetadataOp::RenameAccountCursors {
                old: old.clone(),
                new: new.clone(),
            },
        );
        self.renamed_accounts.push((old, new));
        self
    }

    pub(super) fn account_deletion(
        mut self,
        name: String,
        id: String,
        removed: BTreeSet<String>,
    ) -> Self {
        self.operations.insert(
            0,
            MetadataOp::RewriteAccount {
                old: name.clone(),
                new: None,
            },
        );
        self.operations
            .push(MetadataOp::RemoveAccountCursors { user: name.clone() });
        for conversation in &removed {
            self.operations.push(MetadataOp::RemoveConversationCursors {
                conversation: conversation.clone(),
            });
        }
        self.removed_accounts.push((name, id));
        self.removed_cursors = removed;
        self
    }

    pub(crate) async fn save(mut self) -> Result<Self, EngineError> {
        let mut batch = MetadataWrite::default();
        batch
            .operations
            .extend(std::mem::take(&mut self.operations));
        for audit in &self.audit {
            batch.operations.push(MetadataOp::AppendAudit(audit));
        }
        self.database.save_metadata(&batch).await?;
        drop(batch);
        Ok(self)
    }
}
pub(crate) fn native_metadata_command(input: &str) -> bool {
    let routed = crate::commands::route(input);
    let parts = routed.split_whitespace().collect::<Vec<_>>();
    matches!(
        parts.as_slice(),
        [
            "/enable"
                | "/disable"
                | "/grant"
                | "/revoke"
                | "/add"
                | "/kick"
                | "/join"
                | "/leave"
                | "/owner",
            ..
        ] | ["/new", "room", ..]
            | ["/rm", "room" | "group" | "user", ..]
            | ["/user", "set" | "reset", ..]
            | ["/rename", ..]
            | ["/bot", ..]
    )
}
impl Engine {
    pub(crate) fn prepare_metadata(
        &mut self,
        actor: Option<&str>,
        view: Option<&str>,
        input: &str,
    ) -> Result<MetadataPreparation, EngineError> {
        let routed = crate::commands::route(input);
        let parts = routed.split_whitespace().collect::<Vec<_>>();
        if parts.first() == Some(&"/rename") && parts.get(1) != Some(&"room") {
            return match self.account_rename_read(actor, view, input)? {
                Some(read) => Ok(MetadataPreparation::Rename(read)),
                None => Ok(MetadataPreparation::Ready(
                    "Username unchanged.".into(),
                    None,
                )),
            };
        }
        self.prepare_metadata_command(actor, view, input)
            .map(|(reply, pending)| MetadataPreparation::Ready(reply, pending.map(Box::new)))
    }
    pub(super) fn prepare_metadata_command(
        &mut self,
        actor: Option<&str>,
        view: Option<&str>,
        input: &str,
    ) -> Result<(String, Option<PendingMetadata>), EngineError> {
        if !native_metadata_command(input) {
            return Err("Command requires a different storage operation.".into());
        }
        let routed = crate::commands::route(input);
        let parts = routed.split_whitespace().collect::<Vec<_>>();
        if parts.first() == Some(&"/bot") {
            crate::limits::validate_input(input).map_err(EngineError::Invalid)?;
            crate::commands::validate_room_arguments(input).map_err(EngineError::Invalid)?;
            self.require_bot_management(actor, view)?;
            if let [_, "rm", name, "@global"] = parts.as_slice() {
                self.registered_id(name)?;
                self.require_target_command(actor, &authorization::Scope::Server, "/bot")?;
                return self
                    .prepare_account_deletion(actor, name)
                    .map(|(reply, pending)| (reply, Some(pending)));
            }
            return self.prepare_bot_command(actor, view, &parts);
        }
        if parts.starts_with(&["/rm", "user"]) {
            crate::limits::validate_input(input).map_err(EngineError::Invalid)?;
            let command = input.split_whitespace().next().unwrap_or("");
            self.require_command(actor, view, command)?;
            require_len(&parts, 3, "/user delete who")?;
            let scope =
                self.account_target(actor, parts[2], authorization::Action::DeleteAccount)?;
            self.require_target_command(actor, &scope, command)?;
            return self
                .prepare_account_deletion(actor, parts[2])
                .map(|(reply, pending)| (reply, Some(pending)));
        }
        let account_plan = if parts.first() == Some(&"/user") {
            Some(self.prepare_account_metadata(actor, view, input)?)
        } else {
            None
        };
        let room_plan = if parts.starts_with(&["/rename", "room"]) {
            crate::limits::validate_input(input).map_err(EngineError::Invalid)?;
            crate::commands::validate_room_arguments(input).map_err(EngineError::Invalid)?;
            let command = input.split_whitespace().next().unwrap_or("");
            self.require_command(actor, view, command)?;
            let [_, _, name, new_name] = parts.as_slice() else {
                return Err(EngineError::Invalid(format!(
                    "Usage: {}",
                    crate::commands::RENAME_USAGE
                )));
            };
            let Some(plan) = self.room_rename_plan(actor, name, new_name, command)? else {
                return Ok(("Room name unchanged.".into(), None));
            };
            Some(plan)
        } else {
            None
        };
        let dirty = self.metadata_dirty;
        self.metadata_dirty = false;
        let (reply, before, renamed_cursors) = if let Some(plan) = room_plan {
            let (reply, before, old, new) = self.apply_room_rename(actor, plan);
            (Ok(reply), before, vec![(old, new)])
        } else if let Some(plan) = account_plan {
            let (reply, before) = self.apply_account_metadata(actor, plan);
            (Ok(reply), before, vec![])
        } else {
            let before = self.command_metadata_change(view, &parts);
            (self.apply(actor, view, input, false), before, vec![])
        };
        let result = (|| {
            let reply = reply?;
            if !self.metadata_dirty {
                return Ok((reply, None));
            }
            if self.data.policy.assignments != before.policy.assignments
                || self.data.policy.grants != before.policy.grants
                || self.data.policy.media_revocations != before.policy.media_revocations
            {
                self.data.policy.revision = before.policy.revision.wrapping_add(1);
            }
            self.capture_metadata(&before, renamed_cursors)
                .map(|pending| (reply, Some(pending)))
        })();
        before.restore(&mut self.data);
        self.metadata_dirty = dirty;
        result
    }
    pub(super) fn capture_metadata(
        &self,
        before: &MetadataChange,
        renamed_cursors: Vec<(String, String)>,
    ) -> Result<PendingMetadata, EngineError> {
        let authorization =
            authorization::CompiledPolicy::compile(&self.data).map_err(EngineError::unavailable)?;
        let notice = MetadataNotice::between(
            &self.data,
            before.view(),
            &self.authorization,
            &authorization,
        );
        let removed_rooms = before.removed_rooms(&self.data);
        let mut operations = self.metadata_operations(before)?;
        for (old, new) in &renamed_cursors {
            operations.push(MetadataOp::RenameConversationCursors {
                old: old.clone(),
                new: new.clone(),
            });
        }
        for room in &removed_rooms {
            operations.push(MetadataOp::RemoveConversationCursors {
                conversation: format!("room:{room}"),
            });
        }
        let previous_audit = before
            .policy
            .audit
            .iter()
            .map(|event| &event.id)
            .collect::<BTreeSet<_>>();
        let audit = self
            .data
            .policy
            .audit
            .iter()
            .filter(|audit| !previous_audit.contains(&audit.id))
            .cloned()
            .collect();
        let after = before.capture_current(&self.data)?;
        Ok(PendingMetadata {
            database: self.store.db.clone(),
            before: before.clone(),
            after,
            operations,
            audit,
            authorization,
            notice,
            removed_rooms,
            renamed_cursors,
            removed_accounts: vec![],
            renamed_accounts: vec![],
            removed_cursors: BTreeSet::new(),
        })
    }
    pub(crate) fn publish_metadata(&mut self, pending: PendingMetadata) -> Result<(), EngineError> {
        if !pending.before.unchanged(&self.data) {
            self.recovery_failed = true;
            return Err(EngineError::Unavailable(
                "Committed metadata state changed; restart the service.".into(),
            ));
        }
        // Metadata publication never allocates or overwrites the append counter.
        let sequence = self.data.next_sequence;
        pending.after.restore(&mut self.data);
        self.data.next_sequence = sequence;
        for (old, new) in pending.renamed_accounts {
            self.store.read_positions.remove(&new);
            if let Some(positions) = self.store.read_positions.remove(&old) {
                self.store.read_positions.insert(new, positions);
            }
        }
        for (old, new) in pending.renamed_cursors {
            for positions in self.store.read_positions.values_mut() {
                positions.remove(&new);
                if let Some(sequence) = positions.remove(&old) {
                    positions.insert(new.clone(), sequence);
                }
            }
        }
        for room in pending.removed_rooms {
            let key = format!("room:{room}");
            for positions in self.store.read_positions.values_mut() {
                positions.remove(&key);
            }
        }
        for (name, id) in pending.removed_accounts {
            self.read_revisions.remove(&id);
            self.store.read_positions.remove(&name);
        }
        if !pending.removed_cursors.is_empty() {
            for positions in self.store.read_positions.values_mut() {
                positions.retain(|view, _| !pending.removed_cursors.contains(view));
            }
        }
        self.working = None;
        self.authorization = pending.authorization;
        self.bot_index = crate::bots::models::Index::build(&self.data);
        self.advance_colors();
        self.finish_metadata_commit(pending.notice);
        Ok(())
    }
}
