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
        e.execute(Some("admin"), Some("support"), "/bot new helper secret")
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
    e.execute(None, None, "/grant human admin support").unwrap();
    assert!(
        e.execute(Some("human"), Some("support"), "/bot new denied")
            .is_err()
    );
    e.execute(None, None, "/grant human support /bot").unwrap();
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
    e.execute(None, None, "/revoke helper support /react")
        .unwrap();
    assert!(
        !e.commands_for(Some("helper"), Some("support"))
            .iter()
            .any(|c| c.name == "/react")
    );
    e.execute(None, None, "/revoke helper support r:message.read")
        .unwrap();
    assert!(e.history("helper", "support").is_err());
    assert!(e.snapshot("helper").unwrap().rooms.is_empty());
    e.execute(None, None, "/grant helper secret r:message.read")
        .unwrap();
    e.execute(None, None, "/grant helper secret /history")
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
    e.db.lock()
        .unwrap()
        .execute_batch("PRAGMA query_only=ON")
        .unwrap();
    assert!(
        e.execute(Some("admin"), Some("support"), "/bot new helper")
            .is_err()
    );
    assert!(e.data == previous);
    e.db.lock()
        .unwrap()
        .execute_batch("PRAGMA query_only=OFF")
        .unwrap();
    new_bot(&mut e);
    let secret = token(&mut e, "helper", "runner");
    let previous = e.data.clone();
    e.db.lock()
        .unwrap()
        .execute_batch("PRAGMA query_only=ON")
        .unwrap();
    assert!(
        e.execute(Some("admin"), None, "/bot token new helper failed")
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
fn registrations_and_credentials_survive_restart_and_v7_upgrade_is_atomic() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("chat.sqlite");
    let secret;
    {
        let mut e = Engine::open(&path).unwrap();
        e.execute(None, None, "/new room support").unwrap();
        e.execute(None, Some("support"), "/bot new helper").unwrap();
        secret = e
            .execute(None, None, "/bot token new helper runner")
            .unwrap()
            .lines()
            .last()
            .unwrap()
            .to_owned();
        e.send_literal("helper", "support", "preserved").unwrap();
        e.db.lock().unwrap().execute_batch("PRAGMA user_version=7; CREATE TRIGGER fail_upgrade BEFORE UPDATE ON state BEGIN SELECT RAISE(FAIL,'injected'); END;").unwrap();
    }
    assert!(Engine::open(&path).is_err());
    let db = Connection::open(&path).unwrap();
    assert_eq!(
        db.query_row::<u32, _, _>("PRAGMA user_version", [], |r| r.get(0))
            .unwrap(),
        7
    );
    db.execute_batch("DROP TRIGGER fail_upgrade").unwrap();
    drop(db);
    let e = Engine::open(&path).unwrap();
    assert_eq!(e.bot_identity(&secret).as_deref(), Some("helper"));
    assert_eq!(
        e.history("helper", "support").unwrap().messages[0].text,
        "preserved"
    );
    assert_eq!(
        e.db.lock()
            .unwrap()
            .query_row::<u32, _, _>("PRAGMA user_version", [], |r| r.get(0))
            .unwrap(),
        8
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

#[test]
fn legacy_v7_without_bot_metadata_preserves_accounts_sessions_policy_and_history() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("chat.sqlite");
    let previous;
    {
        let mut e = Engine::open(&path).unwrap();
        e.provision("admin", "hash".into(), true, false).unwrap();
        e.execute(Some("admin"), None, "/new room support").unwrap();
        e.execute(Some("admin"), Some("support"), "legacy history")
            .unwrap();
        e.login("legacy-session".into(), "admin", "hash", None)
            .unwrap();
        e.mark_read("admin", "support", 1).unwrap();
        previous = e.data.clone();
        e.db.lock()
            .unwrap()
            .execute_batch(
                "UPDATE state SET json=json_remove(json,'$.bots'); PRAGMA user_version=7;",
            )
            .unwrap();
    }
    let e = Engine::open(&path).unwrap();
    assert!(e.data == previous);
    assert_eq!(e.session("legacy-session").as_deref(), Some("admin"));
    assert_eq!(
        e.history("admin", "support").unwrap().messages[0].text,
        "legacy history"
    );
    assert_eq!(e.unreads("admin").unwrap()["support"].count, 0);
}

#[test]
fn runtime_config_uses_existing_command_and_account_action_grants() {
    let mut e = fixture();
    new_bot(&mut e);
    e.execute(None, Some("support"), "/bot new other").unwrap();
    for write in [false, true] {
        assert!(
            e.authorize_bot_config(Some("admin"), None, "helper", write)
                .is_ok()
        );
        assert!(
            e.authorize_bot_config(Some("human"), None, "helper", write)
                .is_err()
        );
    }
    e.execute(None, None, "/grant human @account:helper r:bot.config")
        .unwrap();
    assert!(
        e.authorize_bot_config(Some("human"), None, "helper", false)
            .is_err(),
        "action alone does not grant command"
    );
    e.execute(None, None, "/grant human @account:helper /bot")
        .unwrap();
    assert!(
        e.authorize_bot_config(Some("human"), None, "helper", false)
            .is_ok()
    );
    assert!(
        e.authorize_bot_config(Some("human"), None, "helper", true)
            .is_err()
    );
    assert!(
        e.authorize_bot_config(Some("human"), None, "other", false)
            .is_err()
    );
    assert!(
        e.authorize_bot_config(Some("human"), Some("support"), "helper", false)
            .is_err(),
        "context gate still applies"
    );
    e.execute(None, None, "/grant human support /bot").unwrap();
    assert!(
        e.authorize_bot_config(Some("human"), Some("support"), "helper", false)
            .is_ok()
    );
    e.execute(None, None, "/grant human @account:helper w:bot.config")
        .unwrap();
    assert!(
        e.authorize_bot_config(Some("human"), None, "helper", true)
            .is_ok()
    );
    assert!(e.execute(Some("human"), None, "/bot ls @global").is_err());
    assert!(
        e.execute(Some("human"), None, "/bot token new helper stolen")
            .is_err()
    );
    e.execute(None, None, "/revoke human @account:helper w:bot.config")
        .unwrap();
    assert!(
        e.authorize_bot_config(Some("human"), None, "helper", true)
            .is_err()
    );
    e.execute(None, None, "/grant helper su").unwrap();
    assert!(
        e.authorize_bot_config(Some("admin"), None, "helper", false)
            .unwrap_err()
            .contains("Only su")
    );
    e.execute(None, None, "/disable helper").unwrap();
    assert!(e.authorize_bot_config(None, None, "helper", false).is_err());
}
