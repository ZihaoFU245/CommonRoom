use super::*;
fn builds(engine: &Engine) -> u64 {
    engine.read_model.builds.load(Ordering::Relaxed)
}
fn projections(engine: &Engine, user: &str) -> (serde_json::Value, serde_json::Value) {
    let browser = engine.snapshot_plan(user).unwrap().finish().unwrap().0;
    let bot = engine.bot_snapshot_plan(user).unwrap().finish().unwrap();
    let mut browser = serde_json::to_value(browser).unwrap();
    // An explicit cold-cache rebuild advances this opaque synchronization stamp.
    browser.as_object_mut().unwrap().remove("color_version");
    (browser, serde_json::to_value(bot).unwrap())
}
#[test]
fn temporary_sudo_projections_never_reuse_or_publish_persisted_authority_templates() {
    let mut engine = super::super::tests::engine();
    for permission in ["/sudo", "x:command.sudo"] {
        engine
            .execute(None, None, &format!("/grant bob @global {permission}"))
            .unwrap();
    }
    let original = projections(&engine, "bob");
    let stamp = engine.snapshot_stamp("bob", false).unwrap();
    let built = builds(&engine);
    for reject in [false, true] {
        let result = engine.with_command_authority(Some("bob"), None, true, |engine| {
            let elevated = projections(engine, "bob");
            assert!(
                elevated.0["groups"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|group| group == "su")
            );
            assert!(engine.snapshot_stamp("bob", false).unwrap() != stamp);
            if reject {
                Err("injected rejection".into())
            } else {
                Ok(())
            }
        });
        assert_eq!(result.is_err(), reject);
        assert_eq!(projections(&engine, "bob"), original);
        assert!(engine.snapshot_stamp("bob", false).unwrap() == stamp);
        assert_eq!(builds(&engine), built);
    }
}
#[test]
fn messages_sessions_presence_and_reads_reuse_metadata_and_refresh_exact_content() {
    let mut engine = super::super::tests::engine();
    let before = projections(&engine, "bob");
    assert_eq!(builds(&engine), 2);
    let metadata = engine.read_model.revision;
    engine
        .execute(Some("alice"), Some("lobby"), "new content")
        .unwrap();
    engine
        .login("cookie".into(), "bob", "test-hash", None)
        .unwrap();
    engine.connect("alice");
    let latest = projections(&engine, "bob");
    assert_eq!(builds(&engine), 2);
    assert_eq!(engine.read_model.revision, metadata);
    assert_ne!(before, latest);
    assert_eq!(latest.0["rooms"][0]["messages"][0]["text"], "new content");
    assert_eq!(latest.0["unread"]["lobby"]["count"], 1);
    assert_eq!(latest.1["conversations"][0]["unread"]["count"], 1);
    let through = engine.data.next_sequence;
    engine.mark_read("bob", "lobby", through).unwrap();
    let read = projections(&engine, "bob");
    assert_eq!(read.0["unread"]["lobby"]["count"], 0);
    assert_eq!(read.1["conversations"][0]["unread"]["count"], 0);
    assert_eq!(builds(&engine), 2);
    // Compare reused projections with a completely cold authorized build.
    engine.read_model.advance();
    assert_eq!(projections(&engine, "bob"), read);
    assert_eq!(builds(&engine), 4);
}
#[test]
fn policy_renames_profiles_private_creation_and_recovery_invalidate_templates() {
    let mut engine = super::super::tests::engine();
    projections(&engine, "bob");
    engine
        .execute(None, None, "/revoke bob user #lobby")
        .unwrap();
    let denied = projections(&engine, "bob");
    assert!(denied.0["rooms"].as_array().unwrap().is_empty());
    assert!(denied.1["conversations"].as_array().unwrap().is_empty());
    assert_eq!(builds(&engine), 4);
    engine
        .execute(None, None, "/grant bob user #lobby")
        .unwrap();
    engine
        .execute(None, None, "/rename room lobby renamed")
        .unwrap();
    engine
        .execute(None, None, "/rename user bob robert")
        .unwrap();
    let renamed = projections(&engine, "robert");
    assert_eq!(renamed.0["username"], "robert");
    assert_eq!(renamed.0["rooms"][0]["name"], "renamed");
    assert_eq!(renamed.1["conversations"][0]["view"], "renamed");
    engine
        .execute(Some("alice"), None, "/tell robert direct")
        .unwrap();
    let direct = projections(&engine, "robert");
    assert_eq!(direct.0["direct"][0]["text"], "direct");
    assert_eq!(direct.1["conversations"].as_array().unwrap().len(), 2);
    engine.set_avatar_hosts(["example.com".into()].into());
    engine
        .set_avatar("alice", Some("https://example.com/a".into()), false)
        .unwrap();
    assert_eq!(
        projections(&engine, "robert").0["avatars"]["alice"],
        "https://example.com/a"
    );
    engine.set_avatar_hosts(BTreeSet::new());
    assert!(
        projections(&engine, "robert").0["avatars"]
            .get("alice")
            .is_none()
    );
    let built = builds(&engine);
    engine.restore_command_state(engine.revision, None).unwrap();
    projections(&engine, "robert");
    assert_eq!(builds(&engine), built + 2);
    engine.execute(None, None, "/disable robert").unwrap();
    assert!(engine.snapshot_plan("robert").is_none());
    assert!(engine.bot_snapshot_plan("robert").is_err());
}
#[test]
fn metadata_cache_bounds_profiles_bytes_and_replacement_without_retained_history() {
    let engine = super::super::tests::engine();
    let plan = engine.snapshot_plan("bob").unwrap();
    assert!(
        plan.snapshot
            .rooms
            .iter()
            .all(|room| room.messages.is_empty())
    );
    assert!(plan.snapshot.unread.is_empty());
    let mut cache = Cache::default();
    for index in 0..MAX_PROFILES + 1 {
        cache.insert(
            (index.to_string(), false),
            Template::Browser(Box::new(plan.clone())),
            1024,
        );
    }
    assert_eq!(cache.entries.len(), MAX_PROFILES);
    assert!(cache.get(&("0".into(), false)).is_none());
    cache.insert(
        ("large".into(), false),
        Template::Browser(Box::new(plan.clone())),
        MAX_BYTES,
    );
    assert_eq!(cache.entries.len(), 1);
    assert_eq!(cache.bytes, MAX_BYTES);
    cache.insert(
        ("large".into(), false),
        Template::Browser(Box::new(plan.clone())),
        1024,
    );
    assert_eq!(cache.bytes, 1024);
    cache.insert(
        ("oversize".into(), false),
        Template::Browser(Box::new(plan)),
        MAX_BYTES + 1,
    );
    assert_eq!(cache.entries.len(), 1);
}
#[test]
fn profile_edits_reuse_authorized_templates_and_match_cold_snapshots() {
    let mut engine = super::super::tests::engine();
    engine.set_avatar_hosts(["example.com".into()].into());
    projections(&engine, "bob");
    let built = builds(&engine);
    engine
        .set_avatar("alice", Some("https://example.com/first".into()), false)
        .unwrap();
    let updated = projections(&engine, "bob");
    assert_eq!(updated.0["avatars"]["alice"], "https://example.com/first");
    assert_eq!(
        builds(&engine),
        built,
        "profile writes must not rebuild authority templates"
    );
    engine
        .execute(
            None,
            None,
            "/user set @all avatar https://example.com/bulk @global",
        )
        .unwrap();
    let bulk = projections(&engine, "bob");
    assert_eq!(bulk.0["avatars"]["alice"], "https://example.com/bulk");
    assert_eq!(builds(&engine), built);
    engine.read_model.advance();
    assert_eq!(projections(&engine, "bob"), bulk);
    engine.execute(None, None, "/room kick bob #lobby").unwrap();
    assert!(
        projections(&engine, "bob").0["rooms"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
