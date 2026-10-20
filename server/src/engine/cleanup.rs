//! Explicit, authorized history cleanup and read-only previews.
use super::*;
use authorization::Scope;

pub(super) const USAGE: &str = "/clean age scope [--dry-run]";

struct CleanupPlan {
    scope: String,
    age: String,
    cutoff: u64,
    preview: bool,
    targets: Vec<(Scope, usize, usize)>,
}
impl CleanupPlan {
    fn report(&self) -> String {
        let messages: usize = self.targets.iter().map(|(_, count, _)| count).sum();
        let references: usize = self.targets.iter().map(|(_, _, count)| count).sum();
        format!(
            "## {}\n\n- Scope: `{}`\n- Older than: `{}`\n- Conversations checked: {}\n- Messages {}: {}\n- Attachment references {}: {}\n\n- {}",
            if self.preview {
                "Cleanup preview"
            } else {
                "Cleanup complete"
            },
            self.scope,
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
impl Engine {
    fn cleanup_plan(&self, context: &CommandContext<'_>) -> Result<CleanupPlan, String> {
        let parts = context.parts;
        let preview = match parts {
            ["/clean", _, _] => false,
            ["/clean", _, _, "--dry-run"] => true,
            _ => return Err(format!("Usage: {USAGE}")),
        };
        let age = parse_age(parts[1])?;
        let cutoff = now()
            .checked_sub(age)
            .ok_or("Age is older than the clock permits.")?;
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
        let mut counts = Vec::with_capacity(targets.len());
        for target in targets {
            let id = match &target {
                Scope::Room(id) | Scope::Private(id) => id,
                _ => return Err("Invalid cleanup scope.".into()),
            };
            let (messages, references) = self.history_store.read(|db| db.query_row(
                "SELECT (SELECT count(*) FROM messages WHERE conversation=?1 AND json_extract(json,'$.time')<?2), (SELECT count(*) FROM message_media AS refs JOIN messages AS m ON m.conversation=refs.conversation AND m.sequence=refs.sequence WHERE m.conversation=?1 AND json_extract(m.json,'$.time')<?2)",
                params![id, cutoff], |row| Ok((row.get::<_, usize>(0)?, row.get::<_, usize>(1)?))
            ).map_err(|error| error.to_string()))?;
            counts.push((target, messages, references));
        }
        Ok(CleanupPlan {
            scope: scope.into(),
            age: parts[1].into(),
            cutoff,
            preview,
            targets: counts,
        })
    }
    pub(super) fn preview_cleanup(&self, context: &CommandContext<'_>) -> Result<String, String> {
        let plan = self.cleanup_plan(context)?;
        if !plan.preview {
            return Err(format!("Usage: {USAGE}"));
        }
        Ok(plan.report())
    }
    pub(super) fn apply_cleanup(&mut self, context: &CommandContext<'_>) -> Result<String, String> {
        let plan = self.cleanup_plan(context)?;
        let report = plan.report();
        if plan.preview {
            return Ok(report);
        }
        for (target, count, _) in &plan.targets {
            if *count == 0 {
                continue;
            }
            let id = match target {
                Scope::Room(id) | Scope::Private(id) => id,
                _ => return Err("Invalid cleanup scope.".into()),
            };
            self.pending_cleanups.push((id.clone(), plan.cutoff));
            if let Some(room) = self.data.rooms.values_mut().find(|room| &room.id == id) {
                room.revision = room.revision.wrapping_add(1);
            }
            if let Some(chat) = self.data.private.values_mut().find(|chat| &chat.id == id) {
                chat.revision = chat.revision.wrapping_add(1);
            }
        }
        self.audit(
            context.actor,
            "/clean",
            &format!("{} {}", plan.age, plan.scope),
        );
        Ok(report)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::engine::tests::{engine, private_messages, room_messages};

    fn age_first(e: &Engine, id: &str) {
        e.db.lock().unwrap().execute("UPDATE messages SET json=json_set(json,'$.time',?2) WHERE conversation=?1 AND sequence=(SELECT min(sequence) FROM messages WHERE conversation=?1)", params![id, now()-8*86400]).unwrap();
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
    fn cleanup_requires_an_explicit_scope_in_every_view() {
        let mut e = engine();
        seed_private(&mut e);
        e.execute(None, None, "/grant alice su").unwrap();
        let before = e.data.clone();
        let revision = e.revision;
        let writes = e.db.lock().unwrap().total_changes();
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
                assert_eq!(e.db.lock().unwrap().total_changes(), writes);
            }
        }
    }
    #[test]
    fn private_cleanup_supports_peer_and_pair_targets_with_read_only_preview() {
        let mut e = engine();
        seed_private(&mut e);
        for permission in ["x:history.clean 7d", "/clean"] {
            e.execute(
                None,
                None,
                &format!("/grant bob @private:bob:eve {permission}"),
            )
            .unwrap();
        }
        let before = e.data.clone();
        let revision = e.revision;
        let writes = e.db.lock().unwrap().total_changes();
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
            assert_eq!(e.db.lock().unwrap().total_changes(), writes);
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
        e.db.lock().unwrap().execute_batch("CREATE TRIGGER deny_clean BEFORE DELETE ON messages WHEN json_extract(OLD.json,'$.text')='old private' BEGIN SELECT RAISE(ABORT,'injected cleanup failure'); END;").unwrap();
        assert!(e.execute(None, None, "/clean 7d @global").is_err());
        assert_eq!(e.revision, revision);
        assert!(e.data == before);
        assert_eq!(room_messages(&e, "lobby").len(), 1);
        assert_eq!(private_messages(&e, "alice:eve").len(), 1);
        e.db.lock()
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
