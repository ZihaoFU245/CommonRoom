//! Explicit, authorized history cleanup and read-only previews.
use super::*;
use authorization::Scope;

pub(super) const USAGE: &str = "/clean age scope [--dry-run]";

pub(crate) struct CleanupPlan {
    scope: String,
    age: String,
    cutoff: u64,
    pub(crate) preview: bool,
    history: std::sync::Arc<HistoryStore>,
    targets: Vec<(Scope, usize, usize)>,
}
impl CleanupPlan {
    pub(crate) fn report(&self) -> String {
        let messages: usize = self.targets.iter().map(|(_, count, _)| count).sum();
        let references: usize = self.targets.iter().map(|(_, _, count)| count).sum();
        format!(
            "## {}\n\n- Scope: `{}`\n- {}: `{}`\n- Conversations checked: {}\n- Messages {}: {}\n- Attachment references {}: {}\n\n- {}",
            if self.preview {
                "Cleanup preview"
            } else {
                "Cleanup complete"
            },
            self.scope,
            if self.age == "all history" {
                "History"
            } else {
                "Older than"
            },
            self.age,
            self.targets.len(),
            if self.preview { "matching" } else { "deleted" },
            messages,
            if self.preview { "affected" } else { "removed" },
            references,
            if self.preview {
                "Preview only; nothing was changed."
            } else {
                "Unreferenced files are cleaned asynchronously; upload quota is released after confirmed storage deletion."
            }
        )
    }
}
impl CleanupPlan {
    pub(crate) fn ids(&self) -> Vec<String> {
        let mut ids = self
            .targets
            .iter()
            .filter_map(|(scope, _, _)| match scope {
                Scope::Room(id) | Scope::Private(id) => Some(id.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        ids.sort();
        ids
    }
    pub(crate) fn matched_ids(&self) -> Vec<String> {
        self.targets
            .iter()
            .filter(|(_, count, _)| *count > 0)
            .filter_map(|(scope, _, _)| match scope {
                Scope::Room(id) | Scope::Private(id) => Some(id.clone()),
                _ => None,
            })
            .collect()
    }
    pub(crate) fn has_matches(&self) -> bool {
        self.targets.iter().any(|(_, count, _)| *count > 0)
    }
    fn set_counts(&mut self, totals: BTreeMap<String, (usize, usize)>) {
        for (scope, count, references) in &mut self.targets {
            if let Scope::Room(id) | Scope::Private(id) = scope {
                (*count, *references) = totals.get(id).copied().unwrap_or_default();
            }
        }
    }
    pub(crate) async fn count(&mut self) -> Result<(), EngineError> {
        let totals = self
            .history
            .cleanup_counts_async(&self.ids(), self.cutoff)
            .await?;
        self.set_counts(totals);
        Ok(())
    }
}
impl Engine {
    pub(crate) fn cleanup_stamp(
        &self,
        plan: &CleanupPlan,
    ) -> Result<Vec<(String, u64)>, EngineError> {
        plan.ids()
            .into_iter()
            .map(|id| {
                let view = self
                    .bot_index
                    .conversations
                    .get(&id)
                    .ok_or("Cleanup target unavailable.")?;
                let revision = if let Some(key) = view.strip_prefix("@private:") {
                    self.data.private.get(key).map(|chat| chat.revision)
                } else {
                    self.data.rooms.get(view).map(|room| room.revision)
                }
                .ok_or("Cleanup target unavailable.")?;
                Ok((id, revision))
            })
            .collect()
    }
    pub(super) fn resolve_cleanup_plan(
        &self,
        context: &CommandContext<'_>,
    ) -> Result<CleanupPlan, EngineError> {
        let parts = context.parts;
        let preview = match parts {
            ["/clean", _, _] => false,
            ["/clean", _, _, "--dry-run"] => true,
            _ => return Err(EngineError::Invalid(format!("Usage: {USAGE}"))),
        };
        // Zero is an internal all-history request; normal durations stay positive.
        let age = if parts[1] == "-1" {
            0
        } else {
            parse_age(parts[1])?
        };
        let cutoff = if age == 0 {
            u64::try_from(i64::MAX).map_err(|_| "Invalid clock range.")?
        } else {
            now()
                .checked_sub(age)
                .ok_or("Age is older than the clock permits.")?
        };
        let value = parts[2];
        let scope = if value == "@all" { "@global" } else { value };
        let private_error = "Private conversation not found or access denied.";
        let targets =
            match scope {
                "@global" | "@private" => self
                    .data
                    .private
                    .values()
                    .map(|chat| Scope::Private(chat.id.clone()))
                    .chain(
                        self.data
                            .rooms
                            .values()
                            .filter(|_| scope == "@global")
                            .map(|room| Scope::Room(room.id.clone())),
                    )
                    .collect::<Vec<_>>(),
                value if value.starts_with('#') => vec![self.room_scope(&value[1..])?],
                value if value.starts_with("@direct:") => {
                    let actor = context
                        .actor
                        .ok_or("Use @private:a:b from stdin; @direct:user requires an account.")?;
                    let key = private_key(actor, &value[8..]);
                    let chat = self.data.private.get(&key).ok_or(private_error)?;
                    vec![Scope::Private(chat.id.clone())]
                }
                value if value.starts_with("@private:") => {
                    vec![self.parse_scope(value).map_err(|_| private_error)?]
                }
                _ => return Err(
                    "Cleanup scope must be #room, @direct:user, @private:a:b, @private or @global."
                        .into(),
                ),
            };
        // Check all resources before counting or scheduling any deletions.
        for target in &targets {
            let result = self
                .require_target_command(context.actor, target, "/clean")
                .and_then(|()| self.require_clean(context.actor, target, age));
            if result.is_err() && matches!(target, Scope::Private(_)) && !self.is_su(context.actor)
            {
                return Err(private_error.into());
            }
            result?;
        }
        if targets.is_empty() {
            self.require_target_command(context.actor, &Scope::Server, "/clean")?;
            self.require_clean(context.actor, &Scope::Server, age)?;
        }
        Ok(CleanupPlan {
            scope: scope.into(),
            age: if age == 0 {
                "all history".into()
            } else {
                parts[1].into()
            },
            cutoff,
            preview,
            history: self.history_store().clone(),
            targets: targets.into_iter().map(|scope| (scope, 0, 0)).collect(),
        })
    }
    pub(crate) fn prepare_browser_cleanup(
        &self,
        user: &str,
        view: Option<&str>,
        input: &str,
    ) -> Result<CleanupPlan, EngineError> {
        self.prepare_cleanup(Some(user), view, input)
    }
    pub(crate) fn prepare_cleanup(
        &self,
        actor: Option<&str>,
        view: Option<&str>,
        input: &str,
    ) -> Result<CleanupPlan, EngineError> {
        crate::commands::validate_room_arguments(input).map_err(EngineError::Invalid)?;
        self.require_command(actor, view, "/clean")?;
        let parts = input.split_whitespace().collect::<Vec<_>>();
        let context = CommandContext {
            actor,
            command: "/clean",
            room: view.filter(|v| !v.starts_with('@')),
            parts: &parts,
            view,
            scope: self.command_scope(actor, view)?,
            author: actor.unwrap_or("console"),
        };
        self.resolve_cleanup_plan(&context)
    }
    pub(crate) fn prepare_history_cleanup(
        &self,
        user: &str,
        plan: &CleanupPlan,
    ) -> Result<Option<PendingHistory>, EngineError> {
        self.prepare_cleanup_write(Some(user), plan)
    }
    pub(crate) fn prepare_cleanup_write(
        &self,
        actor: Option<&str>,
        plan: &CleanupPlan,
    ) -> Result<Option<PendingHistory>, EngineError> {
        if plan.preview || !plan.has_matches() {
            return Ok(None);
        }
        let cleanups = plan
            .targets
            .iter()
            .filter(|(_, count, _)| *count > 0)
            .map(|(scope, _, _)| {
                let id = match scope {
                    Scope::Room(id) | Scope::Private(id) => id,
                    _ => return Err("Invalid cleanup scope."),
                };
                Ok((id.clone(), plan.cutoff))
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.prepare_history_write_for(
            actor,
            "/clean",
            &format!("{} {}", plan.age, plan.scope),
            vec![],
            cleanups,
        )
        .map(Some)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::engine::tests::{engine, private_messages, room_messages};

    fn age_first(e: &Engine, id: &str) {
        e.store.db.lock().unwrap().execute("UPDATE messages SET json=json_set(json,'$.time',?2) WHERE conversation=?1 AND sequence=(SELECT min(sequence) FROM messages WHERE conversation=?1)", params![id, now()-8*86400]).unwrap();
    }
    fn seed_private(e: &mut Engine) {
        e.execute(Some("bob"), None, "/tell eve old private")
            .unwrap();
        age_first(e, &e.data.private["bob:eve"].id);
        e.execute(Some("bob"), None, "/tell eve new private")
            .unwrap();
        e.execute(Some("alice"), None, "/tell eve other pair")
            .unwrap();
        age_first(e, &e.data.private["alice:eve"].id);
    }
    #[test]
    fn cleanup_metadata_defaults_follow_live_roles_and_ownership() {
        let mut e = engine();
        let room = e.room_scope("lobby").unwrap();
        assert_eq!(e.cleanup_limit("alice", &Scope::Server).unwrap().0, -1);
        assert_eq!(e.cleanup_limit("bob", &room).unwrap().0, 0);
        e.execute(None, None, "/room new owned").unwrap();
        e.execute(None, None, "/room owner bob #owned").unwrap();
        let owned = e.room_scope("owned").unwrap();
        assert_eq!(e.cleanup_limit("bob", &owned).unwrap(), (-1, "role".into()));
        e.execute(Some("bob"), Some("owned"), "/clean -1 #owned --dry-run")
            .unwrap();
        e.execute(None, None, "/room owner eve #owned").unwrap();
        assert_eq!(e.cleanup_limit("bob", &owned).unwrap().0, 0);
        assert_eq!(e.cleanup_limit("eve", &owned).unwrap().0, -1);
        assert!(e.data.users["bob"].metadata.is_empty());
        e.execute(None, None, "/grant bob admin").unwrap();
        assert_eq!(e.cleanup_limit("bob", &Scope::Server).unwrap().0, -1);
        e.execute(None, None, "/revoke bob admin").unwrap();
        assert_eq!(e.cleanup_limit("bob", &Scope::Server).unwrap().0, 0);
    }

    #[test]
    fn cleanup_metadata_limits_require_grants_and_resolve_room_over_global() {
        let mut e = engine();
        for permission in ["/clean", "x:history.clean"] {
            e.execute(None, None, &format!("/grant bob #lobby {permission}"))
                .unwrap();
        }
        assert!(
            e.execute(Some("bob"), Some("lobby"), "/clean 7d #lobby --dry-run")
                .unwrap_err()
                .contains("disabled")
        );
        e.execute(None, None, "/user set bob history.clean_min_age 7d @global")
            .unwrap();
        assert!(
            e.execute(Some("bob"), Some("lobby"), "/clean 1d #lobby")
                .is_err()
        );
        assert!(
            e.execute(Some("bob"), Some("lobby"), "/clean -1 #lobby --dry-run")
                .is_err()
        );
        e.execute(Some("bob"), Some("lobby"), "/clean 7d #lobby --dry-run")
            .unwrap();
        e.execute(None, None, "/user set bob history.clean_min_age -1 #lobby")
            .unwrap();
        e.execute(Some("bob"), Some("lobby"), "/clean -1 #lobby --dry-run")
            .unwrap();
        e.execute(
            None,
            None,
            "/user set bob history.clean_min_age @unset #lobby",
        )
        .unwrap();
        assert_eq!(
            e.cleanup_limit("bob", &e.room_scope("lobby").unwrap())
                .unwrap(),
            (604800, "@global".into())
        );
        e.execute(None, None, "/user set bob history.clean_min_age 0 #lobby")
            .unwrap();
        assert!(
            e.execute(Some("bob"), Some("lobby"), "/clean 30d #lobby")
                .is_err()
        );
        for input in [
            "/grant bob #lobby x:history.clean 7d",
            "/revoke bob #lobby x:history.clean 7d",
            "/user set bob history.clean_min_age -2 #lobby",
            "/user set bob history.clean_min_age 0d #lobby",
            "/user set bob history.clean_min_age abc #lobby",
            "/user set bob history.clean_min_age 18446744073709551615w #lobby",
            "/user set bob history.unknown @unset #lobby",
        ] {
            let before = e.data.clone();
            assert!(e.execute(None, None, input).is_err(), "{input}");
            assert!(e.data == before);
        }
        e.execute(None, None, "/user set bob history.clean_min_age -1 #lobby")
            .unwrap();
        e.execute(None, None, "/revoke bob #lobby x:history.clean")
            .unwrap();
        assert!(
            e.execute(Some("bob"), Some("lobby"), "/clean -1 #lobby")
                .is_err()
        );
    }

    #[test]
    fn sudo_bypasses_cleanup_metadata_and_both_command_and_action_grants_temporarily() {
        let mut e = engine();
        seed_private(&mut e);
        e.execute(Some("eve"), Some("lobby"), "room message")
            .unwrap();
        for permission in ["/sudo", "x:command.sudo"] {
            e.execute(None, None, &format!("/grant bob @global {permission}"))
                .unwrap();
        }
        e.execute(None, None, "/user set bob history.clean_min_age 0 @global")
            .unwrap();
        e.execute(None, None, "/user set bob history.clean_min_age 0 #lobby")
            .unwrap();
        let scope = e.room_scope("lobby").unwrap();
        assert!(!e.effective(Some("bob"), &scope).command("/clean"));
        assert!(!e.allows(Some("bob"), &scope, authorization::Action::Clean));
        assert!(
            e.execute(Some("bob"), Some("lobby"), "/clean -1 #lobby")
                .is_err()
        );
        let before = e.data.clone();
        let writes = e.store.db.lock().unwrap().total_changes();
        e.execute(
            Some("bob"),
            Some("lobby"),
            "/sudo /clean -1 @global --dry-run",
        )
        .unwrap();
        assert!(e.data == before);
        assert_eq!(e.store.db.lock().unwrap().total_changes(), writes);
        assert!(!e.is_su(Some("bob")));
        e.execute(Some("bob"), Some("lobby"), "/sudo /clean -1 @global")
            .unwrap();
        assert!(room_messages(&e, "lobby").is_empty());
        assert!(private_messages(&e, "bob:eve").is_empty());
        assert!(private_messages(&e, "alice:eve").is_empty());
        assert_eq!(
            e.data.users["bob"].metadata["@global"]["history.clean_min_age"],
            "0"
        );
        assert!(!e.is_su(Some("bob")));
        assert!(!e.effective(Some("bob"), &scope).command("/clean"));
        assert!(
            e.execute(Some("bob"), Some("lobby"), "/sudo /clean -1")
                .is_err()
        );
        assert!(!e.is_su(Some("bob")));
        e.execute(None, None, "/revoke bob @global x:command.sudo")
            .unwrap();
        assert!(
            e.execute(Some("bob"), Some("lobby"), "/sudo /clean -1 #lobby")
                .is_err()
        );
    }

    #[test]
    fn bulk_cleanup_rejects_a_metadata_limit_before_deleting_any_target() {
        let mut e = engine();
        seed_private(&mut e);
        e.execute(Some("alice"), Some("lobby"), "keep room")
            .unwrap();
        e.execute(None, None, "/user set alice history.clean_min_age 0 #lobby")
            .unwrap();
        let before = e.data.clone();
        for suffix in ["", " --dry-run"] {
            assert!(
                e.execute(
                    Some("alice"),
                    Some("lobby"),
                    &format!("/clean -1 @global{suffix}")
                )
                .is_err()
            );
            assert!(e.data == before);
            assert_eq!(private_messages(&e, "bob:eve").len(), 2);
            assert_eq!(room_messages(&e, "lobby").len(), 1);
        }
    }

    #[test]
    fn cleanup_requires_an_explicit_scope_in_every_view() {
        let mut e = engine();
        seed_private(&mut e);
        e.execute(None, None, "/grant alice su").unwrap();
        let before = e.data.clone();
        let revision = e.revision;
        let writes = e.store.db.lock().unwrap().total_changes();
        for view in [None, Some("@command"), Some("lobby"), Some("@direct:eve")] {
            for command in [
                "/clean",
                "/clean 7d",
                "/clean 7d --dry-run",
                "/clean 7d lobby",
                "/clean 7d @account:bob",
                "/clean 7d #lobby --unknown",
            ] {
                assert!(
                    e.execute(Some("alice"), view, command).is_err(),
                    "{view:?}: {command}"
                );
                assert!(e.data == before);
                assert_eq!(e.revision, revision);
                assert_eq!(e.store.db.lock().unwrap().total_changes(), writes);
            }
        }
    }
    #[test]
    fn private_cleanup_supports_peer_and_pair_targets_with_read_only_preview() {
        let mut e = engine();
        seed_private(&mut e);
        for permission in ["x:history.clean", "/clean"] {
            e.execute(
                None,
                None,
                &format!("/grant bob @private:bob:eve {permission}"),
            )
            .unwrap();
        }
        e.execute(None, None, "/user set bob history.clean_min_age 7d @global")
            .unwrap();
        let before = e.data.clone();
        let revision = e.revision;
        let writes = e.store.db.lock().unwrap().total_changes();
        for command in [
            "/clean 7d @direct:eve --dry-run",
            "/clean 7d @private:bob:eve --dry-run",
        ] {
            let output = e.read_command(Some("bob"), None, command).unwrap().unwrap();
            assert!(output.starts_with("## Cleanup preview"));
            assert!(output.contains("- Messages matching: 1"));
            assert!(
                e.execute(Some("bob"), Some("@direct:eve"), command)
                    .unwrap()
                    .contains("nothing was changed")
            );
            assert!(e.data == before);
            assert_eq!(e.revision, revision);
            assert_eq!(e.store.db.lock().unwrap().total_changes(), writes);
        }
        assert!(
            e.execute(Some("bob"), None, "/clean 1d @direct:eve --dry-run")
                .is_err()
        );
        assert!(
            e.execute(Some("bob"), None, "/clean 1d @direct:eve")
                .is_err()
        );
        let result = e
            .execute(Some("bob"), None, "/clean 7d @direct:eve")
            .unwrap();
        assert!(result.contains("- Messages deleted: 1"));
        assert_eq!(private_messages(&e, "bob:eve")[0].text, "new private");
        assert_eq!(private_messages(&e, "bob:eve").len(), 1);
        assert_eq!(private_messages(&e, "alice:eve").len(), 1);
        e.execute(None, None, "/clean 7d @private:alice:eve")
            .unwrap();
        assert!(private_messages(&e, "alice:eve").is_empty());
    }
    #[test]
    fn private_targets_do_not_create_chats_or_disclose_unauthorized_pairs() {
        let mut e = engine();
        seed_private(&mut e);
        e.execute(None, None, "/grant bob @global /clean").unwrap();
        let before = e.data.clone();
        let denied = e
            .execute(Some("bob"), None, "/clean 7d @private:alice:eve --dry-run")
            .unwrap_err();
        for target in ["@direct:missing", "@private:alice:missing"] {
            assert_eq!(
                e.execute(Some("bob"), None, &format!("/clean 7d {target} --dry-run"))
                    .unwrap_err(),
                denied
            );
        }
        assert!(
            e.execute(None, None, "/clean 7d @direct:eve")
                .unwrap_err()
                .contains("requires an account")
        );
        assert!(e.data == before);
    }
    #[test]
    fn bulk_cleanup_authorizes_all_targets_and_rolls_back_on_storage_failure() {
        let mut e = engine();
        seed_private(&mut e);
        e.execute(Some("alice"), Some("lobby"), "old room").unwrap();
        age_first(&e, &e.data.rooms["lobby"].id);
        for permission in ["x:history.clean", "/clean"] {
            for scope in ["#lobby", "@private:bob:eve"] {
                e.execute(None, None, &format!("/grant bob {scope} {permission}"))
                    .unwrap();
            }
        }
        let before = e.data.clone();
        for command in [
            "/clean 7d @global",
            "/clean 7d @global --dry-run",
            "/clean 7d @private",
        ] {
            assert!(e.execute(Some("bob"), None, command).is_err());
            assert!(e.data == before);
            assert_eq!(room_messages(&e, "lobby").len(), 1);
            assert_eq!(private_messages(&e, "bob:eve").len(), 2);
        }
        let revision = e.revision;
        e.store.db.lock().unwrap().execute_batch("CREATE TRIGGER deny_clean BEFORE DELETE ON messages WHEN json_extract(OLD.json,'$.text')='old private' BEGIN SELECT RAISE(ABORT,'injected cleanup failure'); END;").unwrap();
        assert!(e.execute(None, None, "/clean 7d @global").is_err());
        assert_eq!(e.revision, revision);
        assert!(e.data == before);
        assert_eq!(room_messages(&e, "lobby").len(), 1);
        assert_eq!(private_messages(&e, "alice:eve").len(), 1);
        e.store
            .db
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER deny_clean;")
            .unwrap();
        e.execute(None, None, "/clean 7d @private").unwrap();
        assert_eq!(room_messages(&e, "lobby").len(), 1);
        assert_eq!(private_messages(&e, "bob:eve").len(), 1);
        assert!(private_messages(&e, "alice:eve").is_empty());
    }
    #[test]
    fn preview_counts_attachment_references_and_cleanup_preserves_shared_media() {
        let mut e = engine();
        let id = e.data.rooms["lobby"].id.clone();
        let mut media = e.allocate_media("bob", &id, "photo.png", 8).unwrap();
        media.ready = true;
        media.object_key = media.final_key();
        media.content_type = "image/png".into();
        e.finish_media("bob", &media).unwrap();
        let link = format!("![photo](https://example.com/api/media/{}/file)", media.id);
        e.execute(Some("bob"), Some("lobby"), &link).unwrap();
        age_first(&e, &id);
        e.execute(Some("bob"), Some("lobby"), &link).unwrap();
        let report = e
            .execute(Some("alice"), Some("lobby"), "/clean 7d #lobby --dry-run")
            .unwrap();
        assert!(report.contains("- Attachment references affected: 1"));
        let report = e
            .execute(Some("alice"), Some("lobby"), "/clean 7d #lobby")
            .unwrap();
        assert!(report.contains("- Attachment references removed: 1"));
        assert!(report.contains("quota is released after confirmed storage deletion"));
        assert!(e.readable_media("bob", &media.id).is_ok());
        assert_eq!(room_messages(&e, "lobby").len(), 1);
    }
}
