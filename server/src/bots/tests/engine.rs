use super::*;

fn fixture() -> Engine {
    let mut e = Engine::open(Path::new(":memory:")).unwrap();
    e.provision("admin", "hash".into(), true, false).unwrap();
    e.provision("human", "hash".into(), false, false).unwrap();
    e.execute(Some("admin"), None, "/new room support").unwrap();
    e.execute(None, None, "/new room secret").unwrap();
    e
}
fn new_bot(e: &mut Engine) {
    e.execute(Some("admin"), Some("support"), "/bot new helper")
        .unwrap();
}

#[test]
fn bot_sends_reject_blank_content_without_mutating_history() {
    let mut e = fixture();
    new_bot(&mut e);
    let sequence = e.data.next_sequence;
    for text in ["", " \n\t", "\u{fffc}", "\u{200b}\u{200d}", "\u{2800}"] {
        let error = e.send_literal("helper", "support", text).err().unwrap();
        assert_eq!(error.code, crate::bots::error::Code::InvalidRequest);
        assert_eq!(e.data.next_sequence, sequence);
    }
    assert!(e.history("helper", "support").unwrap().messages.is_empty());
    let text = "👩\u{200d}💻";
    assert_eq!(
        e.send_literal("helper", "support", text).unwrap().text,
        text
    );
}
fn token(e: &mut Engine, name: &str, label: &str) -> String {
    e.execute(
        Some("admin"),
        None,
        &format!("/bot token new {name} {label}"),
    )
    .unwrap()
    .lines()
    .last()
    .unwrap()
    .into()
}

#[test]
fn explicit_scope_and_safe_defaults_are_enforced() {
    let mut e = fixture();
    let before = e.data.clone();
    for view in [None, Some("@command"), Some("@direct:human")] {
        assert!(e.execute(Some("admin"), view, "/bot new helper").is_err());
        assert!(e.data == before);
    }
    assert!(
        e.execute(Some("admin"), Some("support"), "/bot new helper #secret")
            .is_err()
    );
    new_bot(&mut e);
    assert!(e.data.rooms["support"].members.contains("helper"));
    assert!(e.groups(Some("helper")).is_empty());
    let snapshot = e.snapshot("helper").unwrap();
    assert_eq!(snapshot.rooms.len(), 1);
    assert_eq!(snapshot.rooms[0].name, "support");
    assert!(snapshot.users.is_empty());
    assert!(snapshot.direct.is_empty());
    let permitted: BTreeSet<_> = snapshot.rooms[0].commands.iter().map(|c| c.name).collect();
    assert_eq!(permitted, DEFAULT_COMMANDS.iter().copied().collect());
    for command in [
        "/passwd old new",
        "/rename changed",
        "/clean 1s",
        "/bot ls",
        "/retract missing",
        "/tell human hi",
    ] {
        assert!(
            e.execute(Some("helper"), Some("support"), command).is_err(),
            "{command}"
        );
    }
    assert!(e.history("helper", "secret").is_err());
    assert!(e.execute(Some("helper"), Some("secret"), "no").is_err());
    assert!(
        e.execute(Some("helper"), Some("support"), "/man reply")
            .unwrap()
            .contains("/reply")
    );
    assert!(
        !e.execute(Some("helper"), Some("support"), "/help")
            .unwrap()
            .contains("/retract")
    );
    assert!(e.execute(None, None, "/bot new worldwide @global").is_ok());
    assert!(e.allows(
        Some("worldwide"),
        &e.room_scope("secret").unwrap(),
        Action::Read
    ));
}

#[test]
fn mentions_replies_reactions_and_literal_commands_are_ordinary_messages() {
    let mut e = fixture();
    new_bot(&mut e);
    e.execute(
        Some("admin"),
        Some("support"),
        "@helper can you help? @outsider",
    )
    .unwrap();
    let history = e.history("helper", "support").unwrap();
    let original = &history.messages[0];
    assert!(original.mentions.contains("helper"));
    assert!(!original.mentions.contains("outsider"));
    let id = original.id.clone();
    e.execute(
        Some("helper"),
        Some("support"),
        &format!("/reply {id} Certainly"),
    )
    .unwrap();
    e.execute(Some("helper"), Some("support"), &format!("/react {id} 👍"))
        .unwrap();
    let message = e
        .send_literal("helper", "support", "/rm user human")
        .unwrap();
    assert_eq!(message.text, "/rm user human");
    assert_eq!(message.author_id, e.data.users["helper"].id);
    assert!(e.data.users.contains_key("human"));
    let page = e.history("helper", "support").unwrap();
    assert!(page.messages[0].reactions["👍"].contains("helper"));
    assert_eq!(page.messages[1].reply.as_ref().unwrap().id, id);
    e.mark_read("helper", "support", page.messages[0].sequence)
        .unwrap();
}

#[test]
fn only_global_admins_manage_connections_and_su_targets_stay_protected() {
    let mut e = fixture();
    e.execute(None, None, "/grant human admin #support")
        .unwrap();
    assert!(
        e.execute(Some("human"), Some("support"), "/bot new denied")
            .is_err()
    );
    e.execute(None, None, "/grant human #support /bot").unwrap();
    assert!(
        e.execute(Some("human"), Some("support"), "/bot ls")
            .is_err()
    );
    assert!(
        e.commands_for(Some("human"), Some("support"))
            .iter()
            .any(|c| c.name == "/bot")
    );
    new_bot(&mut e);
    e.execute(None, None, "/grant helper su").unwrap();
    for command in [
        "/bot token new helper protected",
        "/bot token ls helper",
        "/bot rm helper @global",
    ] {
        assert!(e.execute(Some("admin"), None, command).is_err());
    }
    e.execute(None, None, "/disable helper").unwrap();
    assert!(
        e.execute(Some("admin"), None, "/bot token new helper protected")
            .is_err()
    );
    assert!(
        e.execute(None, None, "/bot token new helper recovery")
            .is_ok()
    );
}

#[test]
fn credentials_follow_identity_and_revoke_disable_delete_fail_closed() {
    let mut e = fixture();
    new_bot(&mut e);
    let secret = token(&mut e, "helper", "runner");
    assert_eq!(e.bot_identity(&secret).as_deref(), Some("helper"));
    assert!(e.bot_identity("invalid").is_none());
    assert!(!serde_json::to_string(&e.data).unwrap().contains(&secret));
    let listing = e
        .execute(Some("admin"), None, "/bot token ls helper")
        .unwrap();
    assert!(!listing.contains(&secret));
    assert!(!listing.contains(&crate::bots::digest(&secret)));
    assert!(
        e.execute(Some("admin"), None, "/bot token new helper runner")
            .is_err()
    );
    let token_id = e.data.bots[&e.data.users["helper"].id]
        .tokens
        .keys()
        .next()
        .unwrap()
        .clone();
    e.execute(None, None, "/rename helper renamed").unwrap();
    assert_eq!(e.bot_identity(&secret).as_deref(), Some("renamed"));
    e.execute(Some("admin"), None, "/disable renamed").unwrap();
    assert!(e.bot_identity(&secret).is_none());
    e.execute(Some("admin"), None, "/enable renamed").unwrap();
    assert!(e.bot_identity(&secret).is_some());
    e.execute(
        Some("admin"),
        None,
        &format!("/bot token rm renamed {token_id}"),
    )
    .unwrap();
    assert!(e.bot_identity(&secret).is_none());
    let replacement = token(&mut e, "renamed", "replacement");
    assert!(
        e.execute(Some("admin"), Some("support"), "/bot rm renamed")
            .is_err()
    );
    e.execute(Some("admin"), None, "/rm user renamed").unwrap();
    assert!(e.data.bots.is_empty());
    e.execute(Some("admin"), Some("support"), "/bot new renamed")
        .unwrap();
    assert!(e.bot_identity(&replacement).is_none());
}

#[test]
fn room_scoped_grants_can_be_revoked_and_other_grants_added_normally() {
    let mut e = fixture();
    new_bot(&mut e);
    e.execute(None, None, "/revoke helper #support /react")
        .unwrap();
    assert!(
        !e.commands_for(Some("helper"), Some("support"))
            .iter()
            .any(|c| c.name == "/react")
    );
    e.execute(None, None, "/revoke helper #support r:message.read")
        .unwrap();
    assert!(e.history("helper", "support").is_err());
    assert!(e.snapshot("helper").unwrap().rooms.is_empty());
    e.execute(None, None, "/grant helper #secret r:message.read")
        .unwrap();
    e.execute(None, None, "/grant helper #secret /history")
        .unwrap();
    assert!(e.history("helper", "secret").is_ok());
    assert!(
        e.execute(Some("helper"), Some("secret"), "still denied")
            .is_err()
    );
}

#[test]
fn failed_writes_roll_back_creation_credentials_and_messages() {
    let mut e = fixture();
    let previous = e.data.clone();
    e.store
        .db
        .lock()
        .unwrap()
        .execute_batch("PRAGMA query_only=ON")
        .unwrap();
    assert!(
        e.execute(Some("admin"), Some("support"), "/bot new helper")
            .is_err()
    );
    assert!(e.data == previous);
    e.store
        .db
        .lock()
        .unwrap()
        .execute_batch("PRAGMA query_only=OFF")
        .unwrap();
    new_bot(&mut e);
    let secret = token(&mut e, "helper", "runner");
    let previous = e.data.clone();
    e.store
        .db
        .lock()
        .unwrap()
        .execute_batch("PRAGMA query_only=ON")
        .unwrap();
    assert!(
        e.execute(Some("admin"), None, "/bot token new helper failed")
            .is_err()
    );
    assert!(
        e.execute(Some("admin"), None, "/bot owner helper human")
            .is_err()
    );
    assert!(
        e.send_literal("helper", "support", "not committed")
            .is_err()
    );
    assert!(e.data == previous);
    assert!(e.bot_identity(&secret).is_some());
    let token_id = e.data.bots[&e.data.users["helper"].id]
        .tokens
        .keys()
        .next()
        .unwrap()
        .clone();
    assert!(
        e.execute(
            Some("admin"),
            None,
            &format!("/bot token rm helper {token_id}")
        )
        .is_err()
    );
    assert!(
        e.execute(Some("admin"), None, "/bot rm helper @global")
            .is_err()
    );
    assert!(e.data == previous);
}

#[test]
fn registrations_and_credentials_survive_current_schema_restart() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("chat.sqlite");
    let secret;
    {
        let mut e = Engine::open(&path).unwrap();
        e.execute(None, None, "/new room support").unwrap();
        e.execute(None, Some("support"), "/bot new helper").unwrap();
        e.provision("owner", "hash".into(), false, false).unwrap();
        e.execute(None, None, "/bot owner helper owner").unwrap();
        secret = e
            .execute(None, None, "/bot token new helper runner")
            .unwrap()
            .lines()
            .last()
            .unwrap()
            .to_owned();
        e.send_literal("helper", "support", "preserved").unwrap();
    }
    let e = Engine::open(&path).unwrap();
    assert_eq!(e.bot_identity(&secret).as_deref(), Some("helper"));
    assert_eq!(
        e.bot_owner(&e.data.users["helper"].id).as_deref(),
        Some("owner")
    );
    assert_eq!(
        e.history("helper", "support").unwrap().messages[0].text,
        "preserved"
    );
    assert_eq!(
        e.store
            .db
            .lock()
            .unwrap()
            .query_row::<u32, _, _>("PRAGMA user_version", [], |r| r.get(0))
            .unwrap(),
        crate::engine::database::SQLITE_SCHEMA_VERSION
    );
}

#[test]
fn input_budget_is_shared_across_credentials_for_one_account() {
    let mut e = fixture();
    new_bot(&mut e);
    let a = token(&mut e, "helper", "a");
    let b = token(&mut e, "helper", "b");
    for _ in 0..15 {
        e.admit_bot_input(&a).unwrap();
        e.admit_bot_input(&b).unwrap();
    }
    assert!(e.admit_bot_input(&a).unwrap_err().starts_with("Slow down;"));
    assert!(e.admit_bot_input(&b).is_err());
}

#[test]
fn provisioning_and_credential_limits_fail_without_partial_changes() {
    let mut e = fixture();
    e.set_limits(2, 64, 1000).unwrap();
    let previous = e.data.clone();
    assert!(
        e.execute(Some("admin"), Some("support"), "/bot new helper")
            .is_err()
    );
    assert!(e.data == previous);
    e.set_limits(64, 64, 1000).unwrap();
    new_bot(&mut e);
    for i in 0..8 {
        token(&mut e, "helper", &format!("runner-{i}"));
    }
    let previous = e.data.clone();
    assert!(
        e.execute(Some("admin"), None, "/bot token new helper overflow")
            .is_err()
    );
    assert!(e.data == previous);
}

fn input(e: &Engine, view: &str, text: &str) -> crate::bots::models::MessageInput {
    crate::bots::models::MessageInput {
        conversation_id: e.conversation("helper", view).unwrap().1,
        text: text.into(),
        reply_to: None,
        markdown: true,
    }
}

#[test]
fn mixed_browser_and_bot_sends_preserve_quotes_retention_and_shared_cursors() {
    use crate::bots::error::Code;
    let mut e = fixture();
    new_bot(&mut e);
    e.set_limits(64, 64, 2).unwrap();
    e.execute(Some("admin"), Some("support"), "@helper @human question")
        .unwrap();
    let question = e.history("helper", "support").unwrap().messages[0].clone();
    assert_eq!(question.mentions, BTreeSet::from(["helper".into()]));
    let mut request = input(&e, "support", "**@admin @human answer**");
    request.reply_to = Some(question.id.clone());
    let answer = e
        .send_bot_message("helper", &request, Some("mixed"))
        .unwrap()
        .message;
    assert_eq!(answer.mentions, BTreeSet::from(["admin".into()]));
    assert_eq!(answer.reply.as_ref().unwrap().id, question.id);
    assert!(answer.markdown);
    assert!(
        e.bot_read("helper", &request.conversation_id, answer.sequence)
            .unwrap()
    );

    e.execute_formatted(
        Some("admin"),
        Some("support"),
        &format!("/reply {} @helper received", answer.id),
        true,
    )
    .unwrap();
    let page = e.history("helper", "support").unwrap();
    assert_eq!(page.messages.len(), 2);
    let reply = page.messages.last().unwrap();
    assert_eq!(reply.reply.as_ref().unwrap().text, answer.text);
    assert_eq!(reply.mentions, BTreeSet::from(["helper".into()]));
    assert!(reply.markdown);
    assert!(e.mark_read("helper", "support", reply.sequence).unwrap());
    assert_eq!(
        e.bot_snapshot("helper").unwrap().conversations[0]
            .unread
            .as_ref()
            .unwrap()
            .count,
        0
    );
    assert!(
        !e.bot_read("helper", &request.conversation_id, question.sequence)
            .unwrap()
    );
    // Browser validation and frozen bot retry semantics intentionally differ here.
    assert!(e.mark_read("helper", "support", question.sequence).is_err());
    e.execute(Some("admin"), Some("support"), "next").unwrap();
    assert_eq!(
        e.send_bot_message("helper", &request, Some("mixed"))
            .err()
            .unwrap()
            .code,
        Code::MessageGone
    );
    assert!(e.working.is_none());
}

#[test]
fn browser_toggles_and_bot_desired_reactions_share_limits_and_failure_rollback() {
    use crate::bots::error::Code;
    let mut e = fixture();
    new_bot(&mut e);
    let request = input(&e, "support", "react here");
    let message = e
        .send_bot_message("helper", &request, Some("reactions"))
        .unwrap()
        .message;
    let id = &request.conversation_id;
    for n in 0..32 {
        let reaction = format!("r{n}");
        if n % 2 == 0 {
            e.execute(
                Some("helper"),
                Some("support"),
                &format!("/react {} {reaction}", message.id),
            )
            .unwrap();
        } else {
            assert!(
                e.bot_react("helper", id, &message.id, &reaction, true)
                    .unwrap()
            );
        }
    }
    let before = e.data.clone();
    let revision = e.revision;
    assert!(!e.bot_react("helper", id, &message.id, "r0", true).unwrap());
    assert_eq!(e.revision, revision);
    assert_eq!(
        e.bot_react("helper", id, &message.id, "extra", true)
            .err()
            .unwrap()
            .code,
        Code::InvalidRequest
    );
    assert!(
        e.execute(
            Some("helper"),
            Some("support"),
            &format!("/react {} extra", message.id)
        )
        .is_err()
    );
    assert!(e.data == before);

    e.store
        .db
        .lock()
        .unwrap()
        .execute_batch("PRAGMA query_only=ON")
        .unwrap();
    assert_eq!(
        e.bot_react("helper", id, &message.id, "r0", false)
            .err()
            .unwrap()
            .code,
        Code::Unavailable
    );
    assert!(
        e.execute(
            Some("helper"),
            Some("support"),
            &format!("/react {} r0", message.id)
        )
        .is_err()
    );
    assert!(e.data == before);
    assert_eq!(e.revision, revision);
    e.store
        .db
        .lock()
        .unwrap()
        .execute_batch("PRAGMA query_only=OFF")
        .unwrap();
    assert!(e.bot_react("helper", id, &message.id, "r0", false).unwrap());
    assert!(!e.bot_react("helper", id, &message.id, "r0", false).unwrap());
    e.execute(
        Some("helper"),
        Some("support"),
        &format!("/react {} r0", message.id),
    )
    .unwrap();
    let page = e.history("helper", "support").unwrap();
    assert_eq!(page.messages[0].reactions.len(), 32);
    assert!(page.messages[0].reactions["r0"].contains("helper"));
    assert!(e.working.is_none());
}
#[test]
fn stable_targets_cannot_follow_recreated_names_and_receipts_survive_restart_and_rename() {
    use crate::bots::error::Code;
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("chat.sqlite");
    let original;
    let request;
    {
        let mut e = Engine::open(&path).unwrap();
        e.execute(None, None, "/new room support").unwrap();
        e.execute(None, Some("support"), "/bot new helper").unwrap();
        request = input(&e, "support", "**answer**");
        original = e
            .send_bot_message("helper", &request, Some("a"))
            .unwrap()
            .message
            .id;
        e.execute(None, None, "/rename room support renamed")
            .unwrap();
        e.execute(None, None, "/rename helper renamedbot").unwrap();
    }
    let mut e = Engine::open(&path).unwrap();
    let replay = e
        .send_bot_message("renamedbot", &request, Some("a"))
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.message.id, original);
    assert_eq!(replay.message.from, "renamedbot");
    e.execute(None, None, "/rm room renamed").unwrap();
    e.execute(None, None, "/new room support").unwrap();
    for action in ["r:message.read", "w:message.create"] {
        e.execute(None, None, &format!("/grant renamedbot #support {action}"))
            .unwrap();
    }
    assert_eq!(
        e.send_bot_message("renamedbot", &request, Some("fresh"))
            .err()
            .unwrap()
            .code,
        Code::NotFound
    );
    let mut replacement = request.clone();
    replacement.conversation_id = e.data.rooms["support"].id.clone();
    assert_eq!(
        e.send_bot_message("renamedbot", &replacement, Some("a"))
            .err()
            .unwrap()
            .code,
        Code::IdempotencyConflict
    );
    assert!(
        e.history("renamedbot", "support")
            .unwrap()
            .messages
            .is_empty()
    );
    e.execute(None, None, "/bot rm renamedbot @global").unwrap();
    assert_eq!(
        e.store
            .db
            .lock()
            .unwrap()
            .query_row::<usize, _, _>("SELECT count(*) FROM bot_receipts", [], |r| r.get(0))
            .unwrap(),
        0
    );
}
#[test]
fn reply_receipts_and_failed_cursor_writes_have_typed_atomic_outcomes() {
    use crate::bots::error::Code;
    let mut e = fixture();
    new_bot(&mut e);
    e.execute(Some("admin"), Some("support"), "question")
        .unwrap();
    let question = e.history("helper", "support").unwrap().messages[0].clone();
    let mut request = input(&e, "support", "/literal answer");
    request.reply_to = Some(question.id.clone());
    let previous = e.data.clone();
    e.store.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_receipt BEFORE INSERT ON bot_receipts BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert_eq!(
        e.send_bot_message("helper", &request, Some("a"))
            .err()
            .unwrap()
            .code,
        Code::Unavailable
    );
    assert!(e.data == previous);
    assert_eq!(e.history("helper", "support").unwrap().messages.len(), 1);
    e.store
        .db
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_receipt;")
        .unwrap();
    let sent = e.send_bot_message("helper", &request, Some("a")).unwrap();
    assert_eq!(sent.message.reply.unwrap().id, question.id);
    assert!(sent.message.markdown);
    assert!(
        e.send_bot_message("helper", &request, Some("a"))
            .unwrap()
            .replayed
    );
    e.execute(None, None, "/revoke helper #support /reply")
        .unwrap();
    assert_eq!(
        e.send_bot_message("helper", &request, Some("a"))
            .err()
            .unwrap()
            .code,
        Code::Forbidden
    );
    e.execute(None, None, "/grant helper #support /reply")
        .unwrap();
    e.store
        .db
        .lock()
        .unwrap()
        .execute_batch("PRAGMA query_only=ON")
        .unwrap();
    assert_eq!(
        e.bot_read("helper", &request.conversation_id, question.sequence)
            .err()
            .unwrap()
            .code,
        Code::Unavailable
    );
    e.store
        .db
        .lock()
        .unwrap()
        .execute_batch("PRAGMA query_only=OFF")
        .unwrap();
    assert!(
        e.bot_read("helper", &request.conversation_id, question.sequence)
            .unwrap()
    );
    e.execute(None, Some("support"), &format!("/retract {}", question.id))
        .unwrap();
    assert!(
        !e.bot_read("helper", &request.conversation_id, question.sequence)
            .unwrap()
    );
}
#[test]
fn live_receipts_are_not_evicted_and_expired_keys_can_be_reused() {
    use crate::bots::{
        error::Code,
        models::{RECEIPT_LIMIT, RECEIPT_TTL},
    };
    let mut e = fixture();
    new_bot(&mut e);
    let request = input(&e, "support", "answer");
    e.send_bot_message("helper", &request, Some("a")).unwrap();
    e.store
        .db
        .lock()
        .unwrap()
        .execute("UPDATE bot_receipts SET created=?1", [now() - RECEIPT_TTL])
        .unwrap();
    assert!(
        !e.send_bot_message("helper", &request, Some("a"))
            .unwrap()
            .replayed
    );
    let account = e.data.users["helper"].id.clone();
    for i in 1..RECEIPT_LIMIT {
        e.store
            .db
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO bot_receipts VALUES(?1,?2,'digest','conversation',1,?3)",
                params![account, format!("key-{i}"), now()],
            )
            .unwrap();
    }
    assert_eq!(
        e.send_bot_message("helper", &request, Some("new"))
            .err()
            .unwrap()
            .code,
        Code::IdempotencyCapacity
    );
    assert!(
        e.send_bot_message("helper", &request, Some("a"))
            .unwrap()
            .replayed
    );
}
#[test]
fn private_ids_preserve_participants_and_cross_conversation_replies_are_rejected() {
    use crate::bots::error::Code;
    let mut e = fixture();
    new_bot(&mut e);
    e.execute(Some("admin"), None, "/tell helper question")
        .unwrap();
    for permission in ["r:message.read", "w:message.create", "/reply"] {
        e.execute(
            None,
            None,
            &format!("/grant helper @private:admin:helper {permission}"),
        )
        .unwrap();
    }
    let mut request = input(&e, "@direct:admin", "private answer");
    let sent = e.send_bot_message("helper", &request, Some("a")).unwrap();
    assert_eq!(sent.message.to.as_deref(), Some("admin"));
    assert_eq!(sent.message.private_id, request.conversation_id);
    request.reply_to = Some(sent.message.id);
    assert!(
        e.send_bot_message("helper", &request, Some("b"))
            .unwrap()
            .message
            .reply
            .is_some()
    );
    request.conversation_id = e.data.rooms["support"].id.clone();
    assert_eq!(
        e.send_bot_message("helper", &request, Some("c"))
            .err()
            .unwrap()
            .code,
        Code::NotFound
    );
}

#[test]
fn prepared_bot_send_rechecks_revision_permissions_and_concurrent_receipts() {
    let mut e = fixture();
    new_bot(&mut e);
    let input = crate::bots::models::MessageInput {
        conversation_id: e.data.rooms["support"].id.clone(),
        text: "prepared".into(),
        reply_to: None,
        markdown: false,
    };
    let prepared = e.prepare_bot_send("helper", &input, Some("same")).unwrap();
    e.send_bot_message("helper", &input, Some("same")).unwrap();
    let replay = e
        .commit_bot_send("helper", &input, Some("same"), prepared)
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(e.history("helper", "support").unwrap().messages.len(), 1);
    let stale = e.prepare_bot_send("helper", &input, Some("stale")).unwrap();
    e.send_bot_message("helper", &input, Some("different"))
        .unwrap();
    assert_eq!(
        e.commit_bot_send("helper", &input, Some("stale"), stale)
            .err()
            .unwrap()
            .code,
        crate::bots::error::Code::Conflict
    );
    let prepared = e.prepare_bot_send("helper", &input, Some("next")).unwrap();
    e.execute(None, None, "/revoke helper #support w:message.create")
        .unwrap();
    assert_eq!(
        e.commit_bot_send("helper", &input, Some("next"), prepared)
            .err()
            .unwrap()
            .code,
        crate::bots::error::Code::Forbidden
    );
}
