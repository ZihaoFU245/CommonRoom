use super::*;
#[test]
fn completing_an_old_avatar_does_not_restore_a_replaced_profile() {
    let mut engine = super::super::tests::engine();
    let media = ready(&engine, "alice");
    engine.finish_avatar("alice", &media).unwrap();
    engine.set_avatar("alice", None, false).unwrap();
    let current = engine.stored_media(&media.id).unwrap();
    assert!(
        engine
            .prepare_avatar_completion("alice", media, &current)
            .unwrap()
            .is_none()
    );
    assert!(!engine.avatar_directory().contains_key("alice"));
}
fn ready(engine: &Engine, user: &str) -> Media {
    let mut media = engine.allocate_avatar(user, 64).unwrap();
    media.ready = true;
    media.object_key = media.final_key();
    media.content_type = "image/png".into();
    media
}
#[test]
fn profile_frames_match_live_directory_member_and_historical_author_visibility() {
    let mut e = super::super::tests::engine();
    e.set_avatar_hosts(["example.com".into()].into());
    let owner = e.active_account_id("alice").unwrap().to_owned();
    e.set_avatar("alice", Some("https://example.com/a.png".into()), false)
        .unwrap();
    assert!(
        e.avatar_plan("bob", &owner)
            .unwrap()
            .finish()
            .unwrap()
            .is_some()
    );
    e.execute(None, None, "/revoke bob user @global").unwrap();
    assert!(
        e.avatar_plan("bob", &owner)
            .unwrap()
            .finish()
            .unwrap()
            .is_some()
    );
    e.execute(None, None, "/revoke bob user #lobby").unwrap();
    e.execute(None, None, "/grant bob #lobby r:message.read")
        .unwrap();
    assert!(
        e.avatar_plan("bob", &owner)
            .unwrap()
            .finish()
            .unwrap()
            .is_none()
    );
    e.execute(Some("alice"), Some("lobby"), "Retained author")
        .unwrap();
    let value = e
        .avatar_plan("bob", &owner)
        .unwrap()
        .finish()
        .unwrap()
        .unwrap();
    assert_eq!(
        value,
        serde_json::json!({"kind":"avatar","username":"alice","url":"https://example.com/a.png"})
    );
    assert!(value.get("messages").is_none());
    e.set_avatar("alice", None, false).unwrap();
    assert!(
        e.avatar_plan("bob", &owner)
            .unwrap()
            .finish()
            .unwrap()
            .unwrap()["url"]
            .is_null()
    );
    e.execute(None, None, "/clean -1 #lobby").unwrap();
    assert!(
        e.avatar_plan("bob", &owner)
            .unwrap()
            .finish()
            .unwrap()
            .is_none()
    );
    e.execute(None, None, "/user disable alice").unwrap();
    assert!(e.avatar_plan("bob", &owner).is_none());
}
#[test]
fn external_avatar_hosts_are_denied_by_default_and_filtered_after_policy_changes() {
    let mut e = super::super::tests::engine();
    assert!(
        e.set_avatar("bob", Some("https://example.com/a.png".into()), false)
            .is_err()
    );
    e.media.avatar_hosts.insert("example.com".into());
    e.set_avatar("bob", Some("https://example.com/a.png".into()), false)
        .unwrap();
    for value in [
        "https://tracker.example/a",
        "https://example.com.evil/a",
        "https://evil.example.com/a",
        "https://example.com@evil.test/a",
    ] {
        assert!(e.set_avatar("bob", Some(value.into()), false).is_err());
    }
    assert!(e.snapshot("alice").unwrap().avatars.contains_key("bob"));
    e.set_avatar_hosts(BTreeSet::new());
    assert!(!e.snapshot("alice").unwrap().avatars.contains_key("bob"));
}
#[test]
fn self_service_url_validation_remove_and_metadata_isolation() {
    let mut e = super::super::tests::engine();
    e.media
        .avatar_hosts
        .extend(["example.com".into(), "www.gravatar.com".into()]);
    let before = e.data.users["alice"].clone();
    for url in [
        "http://example.com/a.png",
        "javascript:alert(1)",
        "data:image/png;base64,AA",
        "https://a:b@example.com/a",
        "https://example.com/\n",
        "/api/avatars/forged/file",
    ] {
        assert!(
            e.set_avatar("bob", Some(url.into()), false).is_err(),
            "{url}"
        );
    }
    assert!(e.set_avatar("missing", None, false).is_err());
    e.set_avatar(
        "bob",
        Some("https://www.gravatar.com/avatar/example?s=128".into()),
        false,
    )
    .unwrap();
    assert_eq!(
        e.snapshot("alice").unwrap().avatars["bob"],
        "https://www.gravatar.com/avatar/example?s=128"
    );
    assert!(e.data.users["alice"] == before);
    assert!(
        e.execute(
            Some("bob"),
            None,
            "/user set alice avatar https://example.com/a"
        )
        .is_err()
    );
    e.set_avatar("bob", None, false).unwrap();
    assert!(!e.snapshot("alice").unwrap().avatars.contains_key("bob"));
}
#[test]
fn avatar_upload_ownership_type_limits_and_cleanup_lifecycle() {
    let mut e = super::super::tests::engine();
    e.media
        .avatar_hosts
        .extend(["example.com".into(), "www.gravatar.com".into()]);
    for size in [0, MAX_AVATAR_SIZE + 1] {
        assert!(e.allocate_avatar("bob", size).is_err());
    }
    assert_eq!(MAX_AVATAR_SIZE, 65_536);
    {
        let boundary_engine = super::super::tests::engine();
        let boundary = boundary_engine
            .allocate_avatar("bob", MAX_AVATAR_SIZE)
            .unwrap();
        assert_eq!(boundary.size, MAX_AVATAR_SIZE);
    }
    e.media.config.pending_limit = 1;
    let mut media = ready(&e, "bob");
    assert!(
        media
            .object_key
            .starts_with(&format!("avatars/{}/", e.data.users["bob"].id))
    );
    assert!(e.avatar_owner("alice", &media.id).is_err());
    assert!(e.readable_avatar("bob", &media.id).is_err());
    media.content_type = "text/html".into();
    assert!(e.finish_avatar("bob", &media).is_err());
    media.content_type = "image/png".into();
    e.finish_avatar("bob", &media).unwrap();
    assert!(e.readable_avatar("bob", &media.id).is_ok());
    let next = e.allocate_avatar("bob", 64).unwrap();
    e.abandon_media("bob", &next.id).unwrap();
    e.expired_media().unwrap();
    e.remove_media_record(&next.id).unwrap();
    e.prepare_media_cleanup().unwrap();
    assert!(e.expired_media().unwrap().is_empty());
    assert!(e.readable_avatar("bob", &media.id).is_ok());
    e.store
        .db
        .lock()
        .unwrap()
        .execute("UPDATE media SET unreferenced=0,created=0", [])
        .unwrap();
    assert!(e.expired_media().unwrap().is_empty());
    e.set_avatar("bob", Some("https://example.com/new.png".into()), false)
        .unwrap();
    assert!(e.readable_avatar("bob", &media.id).is_err());
    assert_eq!(e.expired_media().unwrap().len(), 1);
}
#[test]
fn avatar_persists_across_restart_rename_and_failed_writes_roll_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    let id;
    {
        let mut e = Engine::open(&path).unwrap();
        e.provision("bob", "hash".into(), false, false).unwrap();
        let media = ready(&e, "bob");
        id = media.id.clone();
        e.finish_avatar("bob", &media).unwrap();
        let before = e.data.clone();
        let revision = e.revision;
        e.store.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_avatar BEFORE UPDATE ON entities WHEN OLD.kind='account' BEGIN SELECT RAISE(FAIL,'injected'); END;").unwrap();
        assert!(e.set_avatar("bob", None, false).is_err());
        assert!(e.data == before);
        assert_eq!(e.revision, revision);
        e.store
            .db
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER reject_avatar;")
            .unwrap();
        e.execute(None, None, "/user rename bob robert").unwrap();
    }
    let e = Engine::open(&path).unwrap();
    e.prepare_media_cleanup().unwrap();
    assert_eq!(
        e.snapshot("robert").unwrap().avatars["robert"],
        avatar_path(&id)
    );
    assert!(e.readable_avatar("robert", &id).is_ok());
    assert!(e.expired_media().unwrap().is_empty());
}
#[test]
fn avatar_projection_does_not_reveal_hidden_directory_identities() {
    let mut e = Engine::open(Path::new(":memory:")).unwrap();
    e.media.avatar_hosts.insert("example.com".into());
    for name in ["alice", "bob"] {
        e.provision(name, "hash".into(), false, false).unwrap();
    }
    e.set_avatar("alice", Some("https://example.com/a.png".into()), false)
        .unwrap();
    e.set_avatar("bob", Some("https://example.com/b.png".into()), false)
        .unwrap();
    e.execute(None, None, "/revoke bob user @global").unwrap();
    let snapshot = e.snapshot("bob").unwrap();
    assert!(snapshot.users.is_empty());
    assert_eq!(snapshot.avatars.len(), 1);
    assert!(snapshot.avatars.contains_key("bob"));
}
#[test]
fn allocation_replays_preserve_quota_and_reject_other_owners_or_payloads() {
    let e = super::super::tests::engine();
    let id = uuid::Uuid::new_v4().to_string();
    let first = e.allocate_avatar_id("bob", 64, &id).unwrap();
    let repeated = e.allocate_avatar_id("bob", 64, &id).unwrap();
    assert_eq!(first.id, repeated.id);
    assert!(e.allocate_avatar_id("alice", 64, &id).is_err());
    assert!(e.allocate_avatar_id("bob", 65, &id).is_err());
    assert!(e.allocate_avatar_id("bob", 64, "bad").is_err());
    assert_eq!(
        e.store
            .db
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM media WHERE id=?1", [&id], |r| r
                .get::<_, u64>(0))
            .unwrap(),
        1
    );
    e.abandon_media("bob", &id).unwrap();
    assert!(e.allocate_avatar_id("bob", 64, &id).is_err());
}
#[test]
fn managed_avatar_reads_follow_visibility_and_disabled_accounts_keep_bytes() {
    let mut e = Engine::open(Path::new(":memory:")).unwrap();
    for name in ["alice", "bob"] {
        e.provision(name, "hash".into(), false, false).unwrap();
    }
    let media = ready(&e, "alice");
    e.finish_avatar("alice", &media).unwrap();
    assert!(e.readable_avatar("bob", &media.id).is_ok());
    e.execute(None, None, "/revoke bob user @global").unwrap();
    assert!(e.readable_avatar("bob", &media.id).is_err());
    assert!(e.readable_avatar("alice", &media.id).is_ok());
    e.execute(None, None, "/room new shared").unwrap();
    for name in ["alice", "bob"] {
        e.execute(None, None, &format!("/room add {name} #shared"))
            .unwrap();
    }
    assert!(e.readable_avatar("bob", &media.id).is_ok());
    e.execute(None, None, "/revoke bob user #shared").unwrap();
    e.execute(None, None, "/grant bob #shared r:message.read")
        .unwrap();
    e.execute(Some("alice"), Some("shared"), "Retained author")
        .unwrap();
    assert!(e.snapshot("bob").unwrap().rooms[0].members.is_empty());
    assert!(e.readable_avatar("bob", &media.id).is_ok());
    e.execute(None, None, "/clean -1 #shared").unwrap();
    assert!(e.readable_avatar("bob", &media.id).is_err());
    e.execute(None, None, "/room kick bob #shared").unwrap();
    assert!(e.readable_avatar("bob", &media.id).is_err());
    e.execute(None, None, "/user disable alice").unwrap();
    assert!(e.readable_avatar("alice", &media.id).is_err());
    e.prepare_media_cleanup().unwrap();
    assert!(e.expired_media().unwrap().is_empty());
    e.execute(None, None, "/user enable alice").unwrap();
    assert!(e.readable_avatar("alice", &media.id).is_ok());
}
#[test]
fn expired_reservations_and_replaced_accounts_cannot_complete() {
    let mut e = super::super::tests::engine();
    e.media
        .avatar_hosts
        .extend(["example.com".into(), "www.gravatar.com".into()]);
    let media = ready(&e, "bob");
    e.store
        .db
        .lock()
        .unwrap()
        .execute(
            "UPDATE media SET json=json_set(json,'$.created',0) WHERE id=?1",
            [&media.id],
        )
        .unwrap();
    assert!(e.finish_avatar("bob", &media).is_err());
    let other = ready(&e, "eve");
    e.execute(None, None, "/user delete eve").unwrap();
    e.provision("eve", "hash".into(), false, false).unwrap();
    assert!(e.finish_avatar("eve", &other).is_err());
}
