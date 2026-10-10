use super::*;
fn engine() -> Engine {
    let mut e = Engine::open(Path::new(":memory:")).unwrap();
    for (name, admin) in [("alice", true), ("bob", false), ("eve", false)] {
        e.provision(name, "test-hash".into(), admin, false).unwrap();
    }
    e.run(Some("alice"), None, "/new lobby").unwrap();
    e.run(Some("alice"), None, "/add bob lobby").unwrap();
    e.run(Some("alice"), None, "/add eve lobby").unwrap();
    e
}
#[test]
fn cleanup_respects_age_scope_permissions_and_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    {
        let mut e = Engine::open(&path).unwrap();
        e.provision("alice", "hash".into(), true, false).unwrap();
        e.provision("bob", "hash".into(), false, false).unwrap();
        e.run(Some("alice"), None, "/new one").unwrap();
        e.run(Some("alice"), None, "/new two").unwrap();
        for room in ["one", "two"] {
            e.run(Some("alice"), Some(room), "old").unwrap();
            e.data.rooms.get_mut(room).unwrap().messages[0].time = now() - 8 * 86400;
            e.run(Some("alice"), Some(room), "new").unwrap();
        }
        e.run(Some("alice"), None, "/tell bob old private").unwrap();
        e.data.private.get_mut("alice:bob").unwrap().messages[0].time = now() - 8 * 86400;
        e.run(Some("alice"), None, "/tell bob new private").unwrap();
        assert!(e.run(Some("bob"), None, "/clean 7d @all").is_err());
        for age in ["0d", "你好", "-1d", "99999999999999999999w"] {
            assert!(
                e.run(Some("alice"), None, &format!("/clean {age} @all"))
                    .is_err()
            );
        }
        e.run(Some("alice"), Some("one"), "/clean 7d").unwrap();
        assert_eq!(e.data.rooms["one"].messages.len(), 1);
        assert_eq!(e.data.rooms["two"].messages.len(), 2);
        assert_eq!(e.data.private["alice:bob"].messages.len(), 2);
        e.run(None, None, "/clean 7d @all").unwrap();
    }
    let e = Engine::open(&path).unwrap();
    assert_eq!(e.data.rooms["two"].messages[0].text, "new");
    assert_eq!(e.data.private["alice:bob"].messages[0].text, "new private");
    assert_eq!(e.data.private["alice:bob"].messages.len(), 1);
}
#[test]
fn password_change_keeps_current_session_and_rejects_stale_hashes() {
    let mut e = engine();
    e.login("current".into(), "bob", "test-hash", None).unwrap();
    e.login("other".into(), "bob", "test-hash", None).unwrap();
    e.change_password("bob", "test-hash", "new-hash".into(), "current")
        .unwrap();
    assert_eq!(e.session("current").as_deref(), Some("bob"));
    assert!(e.session("other").is_none());
    assert!(
        e.change_password("bob", "test-hash", "bad".into(), "current")
            .is_err()
    );
    assert_eq!(e.data.users["bob"].hash, "new-hash");
    let revision = e.revision;
    e.run(Some("bob"), None, "/help").unwrap();
    assert_eq!(e.revision, revision);
}
#[test]
fn private_contacts_come_only_from_the_users_retained_messages() {
    let mut e = engine();
    assert!(e.snapshot("alice").unwrap().private_peers.is_empty());
    e.run(Some("bob"), None, "/tell alice older").unwrap();
    for _ in 0..60 {
        e.run(Some("eve"), None, "/tell alice recent").unwrap();
    }
    let snapshot = e.snapshot("alice").unwrap();
    assert_eq!(snapshot.private_peers, vec!["bob", "eve"]);
    assert!(snapshot.direct.iter().any(|m| m.from == "bob"));
    assert_eq!(e.snapshot("bob").unwrap().private_peers, vec!["alice"]);
}
#[test]
fn private_rings_are_independent_and_read_positions_persist() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("chat.sqlite");
    let through;
    {
        let mut e = Engine::open(&path).unwrap();
        for name in ["alice", "bob", "eve"] {
            e.provision(name, "hash".into(), name == "alice", false)
                .unwrap();
        }
        e.set_limits(64, 64, 3).unwrap();
        e.run(Some("alice"), None, "/new room").unwrap();
        e.run(Some("alice"), None, "/add bob room").unwrap();
        for n in 0..5 {
            e.run(Some("alice"), None, &format!("/tell bob 私信 {n} 🙂"))
                .unwrap();
            e.run(Some("alice"), Some("room"), &format!("chat {n}"))
                .unwrap();
        }
        e.run(Some("alice"), None, "/tell eve independent").unwrap();
        assert_eq!(e.history("bob", "@direct:alice").unwrap().messages.len(), 3);
        assert_eq!(e.history("alice", "@direct:eve").unwrap().messages.len(), 1);
        assert_eq!(
            e.history("bob", "@direct:alice").unwrap().messages[0].text,
            "私信 2 🙂"
        );
        assert!(e.history("eve", "room").is_err());
        let dm = e.snapshot("bob").unwrap();
        assert_eq!(dm.unread["@direct:alice"].count, 3);
        assert_eq!(dm.unread["room"].count, 3);
        through = dm.unread["@direct:alice"].through;
        let revision = e.revision;
        assert!(e.mark_read("bob", "@direct:alice", through).unwrap());
        assert_eq!(
            e.revision, revision,
            "read writes must not rewrite message state"
        );
        assert!(!e.mark_read("bob", "@direct:alice", through).unwrap());
        assert!(e.mark_read("bob", "room", through).is_err());
        assert!(
            e.mark_read("eve", "room", dm.unread["room"].through)
                .is_err()
        );
        assert!(e.mark_read("bob", "@direct:alice", through + 100).is_err());
        assert_eq!(e.snapshot("bob").unwrap().unread["@direct:alice"].count, 0);
        assert_eq!(e.snapshot("alice").unwrap().unread["@direct:bob"].count, 0);
        let room_first = dm.unread["room"].first.unwrap();
        e.mark_read("bob", "room", room_first).unwrap();
        assert_eq!(e.snapshot("bob").unwrap().unread["room"].count, 2);
        e.mark_read("bob", "room", dm.unread["room"].through)
            .unwrap();
        e.mark_read("bob", "room", room_first).unwrap();
        assert_eq!(e.snapshot("bob").unwrap().unread["room"].count, 0);
    }
    let mut e = Engine::open(&path).unwrap();
    assert_eq!(e.snapshot("bob").unwrap().unread["@direct:alice"].count, 0);
    e.run(Some("alice"), None, "/tell bob after restart")
        .unwrap();
    assert_eq!(e.snapshot("bob").unwrap().unread["@direct:alice"].count, 1);
    assert!(
        e.snapshot("bob").unwrap().unread["@direct:alice"]
            .first
            .unwrap()
            > through
    );
    e.set_limits(64, 64, 1).unwrap();
    assert_eq!(e.history("bob", "@direct:alice").unwrap().messages.len(), 1);
    assert_eq!(e.history("alice", "@direct:eve").unwrap().messages.len(), 1);
}
#[test]
fn failed_read_and_private_eviction_writes_restore_state() {
    let mut e = engine();
    e.set_limits(64, 64, 1).unwrap();
    e.run(Some("alice"), None, "/tell bob retained").unwrap();
    let before = e.data.clone();
    let through = e.snapshot("bob").unwrap().unread["@direct:alice"].through;
    e.db.execute_batch("PRAGMA query_only=ON;").unwrap();
    assert!(e.mark_read("bob", "@direct:alice", through).is_err());
    assert_eq!(e.snapshot("bob").unwrap().unread["@direct:alice"].count, 1);
    assert!(e.run(Some("alice"), None, "/tell bob failed").is_err());
    assert!(e.data == before);
}
#[test]
fn configurable_limits_preserve_existing_accounts_and_rooms() {
    let mut e = Engine::open(Path::new(":memory:")).unwrap();
    e.set_limits(2, 1, 1000).unwrap();
    e.provision("alice", "hash".into(), true, false).unwrap();
    e.provision("bob", "hash".into(), false, false).unwrap();
    assert!(e.provision("eve", "hash".into(), false, false).is_err());
    e.run(Some("alice"), None, "/new one").unwrap();
    assert!(e.run(None, None, "/new two").is_err());
    e.set_limits(1, 1, 1000).unwrap();
    e.provision("bob", "replacement".into(), false, true)
        .unwrap();
    assert!(e.active("bob"));
    assert!(e.data.rooms.contains_key("one"));
    assert!(e.set_limits(0, 1, 1000).is_err());
}
#[test]
fn unicode_messages_round_trip_and_count_characters() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    let sample = "你好 日本語 한국어 مرحبا नमस्ते Привет שלום 🙂 e\u{301}\nsecond line";
    {
        let mut e = Engine::open(&path).unwrap();
        e.provision("alice", "hash".into(), true, false).unwrap();
        e.provision("bob", "hash".into(), false, false).unwrap();
        e.run(Some("alice"), None, "/new languages").unwrap();
        e.run(Some("alice"), Some("languages"), sample).unwrap();
        e.run(Some("alice"), None, &format!("/tell bob {sample}"))
            .unwrap();
        e.run(Some("alice"), Some("languages"), &"你".repeat(4000))
            .unwrap();
        e.run(
            Some("alice"),
            None,
            &format!("/tell bob {}", "🙂".repeat(4000)),
        )
        .unwrap();
        assert!(
            e.run(Some("alice"), Some("languages"), &"你".repeat(4001))
                .is_err()
        );
        assert!(
            e.run(
                Some("alice"),
                None,
                &format!("/tell bob {}", "🙂".repeat(4001))
            )
            .is_err()
        );
    }
    let e = Engine::open(&path).unwrap();
    assert_eq!(
        e.snapshot("alice").unwrap().rooms[0].messages[0].text,
        sample
    );
    assert_eq!(e.snapshot("bob").unwrap().direct[0].text, sample);
    assert_eq!(
        e.snapshot("bob").unwrap().direct[1].text.chars().count(),
        4000
    );
}
#[test]
fn new_accounts_have_no_rooms() {
    let mut e = Engine::open(Path::new(":memory:")).unwrap();
    e.provision("admin", "hash".into(), true, false).unwrap();
    e.provision("bob", "hash".into(), false, false).unwrap();
    assert!(e.data.rooms.is_empty());
    assert!(e.snapshot("bob").unwrap().rooms.is_empty());
    e.run(Some("admin"), None, "/new lobby").unwrap();
    e.provision("alice", "hash".into(), false, false).unwrap();
    assert!(e.snapshot("alice").unwrap().rooms.is_empty());
    assert!(e.run(Some("admin"), None, "/disable admin").is_err());
    e.run(Some("admin"), None, "/delete lobby").unwrap();
    assert!(e.data.rooms.is_empty());
}
#[test]
fn permissions_and_private_delivery() {
    let mut e = engine();
    assert!(e.run(Some("bob"), None, "/new secret").is_err());
    e.run(Some("alice"), None, "/new secret").unwrap();
    assert!(e.run(Some("bob"), None, "/join secret").is_err());
    e.run(Some("alice"), None, "/add bob secret").unwrap();
    e.run(Some("bob"), Some("secret"), "hello").unwrap();
    assert!(
        !e.snapshot("eve")
            .unwrap()
            .rooms
            .iter()
            .any(|r| r.name == "secret")
    );
    e.run(Some("bob"), None, "/tell alice private").unwrap();
    e.run(Some("bob"), None, "/tell eve other-private").unwrap();
    let history = e.run(Some("bob"), None, "/history 20 alice").unwrap().reply;
    assert!(history.contains("private"));
    assert!(!history.contains("other-private"));
    assert_eq!(e.snapshot("alice").unwrap().direct.len(), 1);
    assert_eq!(e.snapshot("eve").unwrap().direct.len(), 1);
    e.run(Some("alice"), None, "/kick bob secret").unwrap();
    assert!(e.run(Some("bob"), Some("secret"), "blocked").is_err());
    assert!(e.run(Some("eve"), None, "/disable bob").is_err());
}
#[test]
fn persists_accounts_and_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.sqlite");
    {
        let mut e = Engine::open(&path).unwrap();
        e.provision("alice", "hash".into(), true, false).unwrap();
        e.run(Some("alice"), None, "/new lobby").unwrap();
        e.run(Some("alice"), Some("lobby"), "retained").unwrap();
    }
    let e = Engine::open(&path).unwrap();
    assert!(e.active("alice"));
    assert_eq!(
        e.snapshot("alice").unwrap().rooms[0].messages[0].text,
        "retained"
    );
}
#[test]
fn bounded_history_and_disabled_users() {
    let mut e = engine();
    e.set_limits(64, 64, 200).unwrap();
    for _ in 0..210 {
        e.run(Some("bob"), Some("lobby"), "hello").unwrap();
    }
    assert_eq!(e.data.rooms["lobby"].messages.len(), 200);
    e.run(None, None, "/disable bob").unwrap();
    assert!(e.snapshot("bob").is_none());
}
#[test]
fn room_ring_keeps_newest_messages_across_restart_and_limit_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    {
        let mut e = Engine::open(&path).unwrap();
        e.set_limits(64, 64, 3).unwrap();
        e.run(None, None, "/new one").unwrap();
        e.run(None, None, "/new two").unwrap();
        e.run(None, Some("two"), "independent").unwrap();
        for n in 0..10 {
            e.run(None, Some("one"), &format!("消息 {n} 🙂")).unwrap();
        }
        let texts: Vec<_> = e.data.rooms["one"]
            .messages
            .iter()
            .map(|m| m.text.as_str())
            .collect();
        assert_eq!(texts, ["消息 7 🙂", "消息 8 🙂", "消息 9 🙂"]);
        assert_eq!(e.data.rooms["two"].messages.len(), 1);
        assert!(e.run(None, Some("one"), "/history 4").is_err());
        assert!(
            e.run(None, Some("one"), "/history")
                .unwrap()
                .reply
                .contains("消息 7")
        );
        // The on-disk representation stays a JSON array, compatible with older data folders.
        let raw: String =
            e.db.query_row("SELECT json FROM state", [], |r| r.get(0))
                .unwrap();
        assert!(
            serde_json::from_str::<serde_json::Value>(&raw).unwrap()["rooms"]["one"]["messages"]
                .is_array()
        );
    }
    {
        let mut e = Engine::open(&path).unwrap();
        assert_eq!(
            e.data.rooms["one"].messages.front().unwrap().text,
            "消息 7 🙂"
        );
        e.set_limits(64, 64, 1).unwrap();
        assert_eq!(e.data.rooms["one"].messages.len(), 1);
        assert_eq!(e.data.rooms["one"].messages[0].text, "消息 9 🙂");
        e.run(None, Some("one"), "newest").unwrap();
        assert_eq!(e.data.rooms["one"].messages[0].text, "newest");
        assert!(e.set_limits(64, 64, 0).is_err());
        assert_eq!(e.max_messages, 1);
    }
    let e = Engine::open(&path).unwrap();
    assert_eq!(e.data.rooms["one"].messages.len(), 1);
    assert_eq!(e.data.rooms["one"].messages[0].text, "newest");
}
#[test]
fn failed_writes_restore_evicted_messages_and_limits() {
    let mut e = engine();
    e.set_limits(64, 64, 2).unwrap();
    e.run(Some("alice"), Some("lobby"), "oldest").unwrap();
    e.run(Some("alice"), Some("lobby"), "latest").unwrap();
    let previous = e.data.clone();
    let revision = e.revision;
    e.db.execute_batch("PRAGMA query_only=ON;").unwrap();
    assert!(e.run(Some("alice"), Some("lobby"), "unsaved").is_err());
    assert!(e.data == previous);
    assert!(e.set_limits(1, 1, 1).is_err());
    assert!(e.data == previous);
    assert_eq!(e.max_messages, 2);
    assert_eq!(e.max_users, 64);
    assert_eq!(e.revision, revision);
}
#[test]
fn message_actions_are_scoped_toggle_and_preserve_reply_quotes() {
    let mut e = engine();
    e.run(
        Some("alice"),
        Some("lobby"),
        "你好 @bob! @eve and mail@alice.com",
    )
    .unwrap();
    let original = e.data.rooms["lobby"].messages.back().unwrap().clone();
    assert_eq!(
        original.mentions,
        BTreeSet::from(["bob".into(), "eve".into()])
    );
    let react = format!("/react {} 好👍", original.id);
    e.run(Some("bob"), Some("lobby"), &react).unwrap();
    e.run(Some("alice"), Some("lobby"), &react).unwrap();
    assert_eq!(e.data.rooms["lobby"].messages[0].reactions["好👍"].len(), 2);
    e.run(Some("bob"), Some("lobby"), &react).unwrap();
    assert_eq!(
        e.data.rooms["lobby"].messages[0].reactions["好👍"],
        BTreeSet::from(["alice".into()])
    );
    assert!(
        e.run(
            Some("bob"),
            Some("lobby"),
            &format!("/react {} {}", original.id, "a".repeat(129))
        )
        .is_err()
    );
    assert!(
        e.run(
            Some("bob"),
            Some("lobby"),
            &format!("/react {} bad\nreaction", original.id)
        )
        .is_err()
    );
    let long_reaction = "🙂".repeat(128);
    let command = format!("/react {} {long_reaction}", original.id);
    e.run(Some("bob"), Some("lobby"), &command).unwrap();
    assert!(e.data.rooms["lobby"].messages[0].reactions[&long_reaction].contains("bob"));
    e.run(Some("bob"), Some("lobby"), &command).unwrap();
    assert!(
        !e.data.rooms["lobby"].messages[0]
            .reactions
            .contains_key(&long_reaction)
    );
    e.set_limits(64, 64, 1).unwrap();
    e.run(
        Some("bob"),
        Some("lobby"),
        &format!("/reply {} @alice 回答", original.id),
    )
    .unwrap();
    let reply = &e.data.rooms["lobby"].messages[0];
    assert_eq!(reply.reply.as_ref().unwrap().id, original.id);
    assert_eq!(reply.reply.as_ref().unwrap().text, original.text);
    assert_eq!(reply.mentions, BTreeSet::from(["alice".into()]));
    assert!(e.run(Some("bob"), Some("lobby"), &react).is_err());
    e.run(Some("alice"), None, "/new secret").unwrap();
    e.run(Some("alice"), Some("secret"), "private-room")
        .unwrap();
    let id = e.data.rooms["secret"].messages[0].id.clone();
    for action in ["react", "reply"] {
        assert!(
            e.run(Some("eve"), Some("secret"), &format!("/{action} {id} nope"))
                .is_err()
        );
        assert!(
            e.run(Some("eve"), Some("lobby"), &format!("/{action} {id} nope"))
                .is_err()
        );
    }
    e.run(Some("alice"), None, "/tell bob @bob @eve secret DM")
        .unwrap();
    let dm = e.data.private["alice:bob"].messages.back().unwrap().clone();
    assert_eq!(dm.mentions, BTreeSet::from(["bob".into()]));
    assert!(
        e.run(Some("eve"), None, &format!("/react {} 👍", dm.id))
            .is_err()
    );
    assert!(
        e.run(Some("eve"), None, &format!("/reply {} stolen", dm.id))
            .is_err()
    );
    e.run(Some("bob"), None, &format!("/reply {} @alice 好", dm.id))
        .unwrap();
    assert_eq!(
        e.data.private["alice:bob"]
            .messages
            .back()
            .unwrap()
            .to
            .as_deref(),
        Some("alice")
    );
    assert!(e.snapshot("eve").unwrap().direct.is_empty());
}
#[test]
fn message_features_persist_and_rollback() {
    let legacy: Message =
        serde_json::from_str(r#"{"id":"old","from":"alice","to":null,"text":"legacy","time":1}"#)
            .unwrap();
    assert!(legacy.reply.is_none() && legacy.mentions.is_empty() && legacy.reactions.is_empty());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    {
        let mut e = Engine::open(&path).unwrap();
        e.provision("alice", "hash".into(), true, false).unwrap();
        e.run(Some("alice"), None, "/new room").unwrap();
        e.run(Some("alice"), Some("room"), "@alice 中文").unwrap();
        let id = e.data.rooms["room"].messages[0].id.clone();
        e.run(Some("alice"), Some("room"), &format!("/reply {id} reply"))
            .unwrap();
        e.run(Some("alice"), Some("room"), &format!("/react {id} 🙂"))
            .unwrap();
        let previous = e.data.clone();
        e.db.execute_batch("PRAGMA query_only=ON;").unwrap();
        assert!(
            e.run(Some("alice"), Some("room"), &format!("/react {id} 🙂"))
                .is_err()
        );
        assert!(e.data == previous);
    }
    let e = Engine::open(&path).unwrap();
    assert_eq!(
        e.data.rooms["room"].messages[0].reactions["🙂"],
        BTreeSet::from(["alice".into()])
    );
    assert!(e.data.rooms["room"].messages[1].reply.is_some());
    assert_eq!(
        e.db.query_row::<u32, _, _>("PRAGMA user_version", [], |r| r.get(0))
            .unwrap(),
        5
    );
}
#[test]
fn password_hashing() {
    let hash = hash_password("correct-horse").unwrap();
    assert!(verify_password("correct-horse", &hash));
    assert!(!verify_password("wrong", &hash));
    assert!(hash_password("ab").is_err());
    assert!(hash_password("你好吗").is_ok());
    assert!(hash_password("你好").is_err());
    let short_hash = hash_password("abc").unwrap();
    assert!(verify_password("abc", &short_hash));
    assert!(hash_password(&"a".repeat(129)).is_err());
}
#[test]
fn moving_folder_preserves_sessions_and_permissions() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let path = source.path().join("chat.sqlite");
    {
        let mut e = Engine::open(&path).unwrap();
        e.provision("alice", "hash".into(), true, false).unwrap();
        e.login("session-token".into(), "alice", "hash", None)
            .unwrap();
        e.run(Some("alice"), None, "/new team").unwrap();
        e.run(Some("alice"), Some("team"), "before migration")
            .unwrap();
        e.checkpoint().unwrap();
    }
    std::fs::copy(&path, destination.path().join("chat.sqlite")).unwrap();
    let mut e = Engine::open(&destination.path().join("chat.sqlite")).unwrap();
    assert_eq!(e.session("session-token"), Some("alice".into()));
    assert!(e.snapshot("alice").unwrap().admin);
    assert_eq!(e.data.rooms["team"].messages[0].text, "before migration");
    e.provision("alice", "new-hash".into(), false, true)
        .unwrap();
    assert!(e.session("session-token").is_none());
    assert!(e.login("new-token".into(), "alice", "hash", None).is_err());
    e.login("new-token".into(), "alice", "new-hash", None)
        .unwrap();
    e.logout("new-token").unwrap();
    assert!(e.session("new-token").is_none());
}
#[test]
fn rejects_two_servers_and_unknown_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    let e = Engine::open(&path).unwrap();
    assert!(Engine::open(&path).is_err());
    e.db.execute_batch("PRAGMA user_version=99;").unwrap();
    drop(e);
    assert!(Engine::open(&path).is_err());
}
#[test]
fn failed_storage_rolls_back_changes() {
    let mut e = engine();
    e.db.execute_batch("DROP TABLE state;").unwrap();
    assert!(e.run(Some("alice"), None, "/new lost").is_err());
    assert!(!e.data.rooms.contains_key("lost"));
}
#[test]
fn delete_user_checks_permissions_and_cleans_references() {
    let mut e = engine();
    for (actor, command) in [
        (Some("bob"), "/deleteuser eve"),
        (Some("missing"), "/deleteuser bob"),
        (Some("alice"), "/deleteuser alice"),
        (Some("alice"), "/deleteuser nobody"),
        (Some("alice"), "/deleteuser"),
        (None, "/deleteuser bob extra"),
    ] {
        let before = e.data.clone();
        assert!(e.run(actor, None, command).is_err());
        assert!(e.data == before);
    }
    assert!(
        !e.run(Some("bob"), None, "/help")
            .unwrap()
            .reply
            .contains("/deleteuser")
    );
    assert!(
        e.run(Some("alice"), None, "/help")
            .unwrap()
            .reply
            .contains("/deleteuser user")
    );
    e.run(Some("bob"), Some("lobby"), "old room message")
        .unwrap();
    let original = e.data.rooms["lobby"].messages[0].id.clone();
    e.run(
        Some("alice"),
        Some("lobby"),
        &format!("/reply {original} @bob reply"),
    )
    .unwrap();
    for (user, reaction) in [("bob", "👍"), ("alice", "👍"), ("bob", "😎")] {
        e.run(
            Some(user),
            Some("lobby"),
            &format!("/react {original} {reaction}"),
        )
        .unwrap();
    }
    for (user, text) in [
        ("alice", "/tell bob secret"),
        ("bob", "/tell eve another secret"),
        ("alice", "/tell eve unrelated"),
    ] {
        e.run(Some(user), None, text).unwrap();
    }
    for (token, user) in [
        ("bob-one", "bob"),
        ("bob-two", "bob"),
        ("alice-session", "alice"),
    ] {
        e.login(token.into(), user, "test-hash", None).unwrap();
    }
    let dm = e.snapshot("bob").unwrap().unread["@direct:alice"].through;
    e.mark_read("bob", "@direct:alice", dm).unwrap();
    e.mark_read("alice", "@direct:bob", dm).unwrap();
    e.run(Some("alice"), None, "/deleteuser bob").unwrap();
    assert!(!e.data.users.contains_key("bob"));
    assert!(e.session("bob-one").is_none() && e.session("bob-two").is_none());
    assert_eq!(e.session("alice-session").as_deref(), Some("alice"));
    assert!(!e.data.rooms["lobby"].members.contains("bob"));
    let messages = &e.data.rooms["lobby"].messages;
    assert_eq!(messages[0].from, "bob (deleted)");
    assert_eq!(messages[0].id, original);
    assert_eq!(messages[0].text, "old room message");
    assert_eq!(messages[1].reply.as_ref().unwrap().from, "bob (deleted)");
    assert!(!messages[1].mentions.contains("bob"));
    assert_eq!(
        messages[0].reactions["👍"],
        BTreeSet::from(["alice".into()])
    );
    assert!(!messages[0].reactions.contains_key("😎"));
    assert_eq!(e.data.private.len(), 1);
    assert_eq!(e.snapshot("alice").unwrap().private_peers, vec!["eve"]);
    assert!(e.history("alice", "@direct:bob").is_err());
    assert!(!e.read_positions.contains_key("bob"));
    assert!(
        e.read_positions
            .get("alice")
            .is_none_or(|p| !p.contains_key("dm:alice:bob"))
    );
    assert_eq!(e.db.query_row::<u64,_,_>("SELECT COUNT(*) FROM read_positions WHERE username='bob' OR conversation='dm:alice:bob'",[],|r|r.get(0)).unwrap(),0);
    e.provision("bob", "new-hash".into(), false, false).unwrap();
    let replacement = e.snapshot("bob").unwrap();
    assert!(
        replacement.rooms.is_empty()
            && replacement.direct.is_empty()
            && replacement.unread.is_empty()
    );
    assert!(e.login("stale".into(), "bob", "test-hash", None).is_err());
}
#[test]
fn delete_user_persists_and_stdin_can_replace_the_last_admin() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    {
        let mut e = Engine::open(&path).unwrap();
        e.set_limits(2, 2, 1000).unwrap();
        e.provision("alice", "hash".into(), true, false).unwrap();
        e.provision("bob", "hash".into(), false, false).unwrap();
        assert!(e.provision("eve", "hash".into(), false, false).is_err());
        e.run(None, None, "/disable bob").unwrap();
        e.run(Some("alice"), None, "/deleteuser bob").unwrap();
        e.provision("eve", "hash".into(), false, false).unwrap();
        e.run(None, None, "/deleteuser alice").unwrap();
        e.provision("alice", "replacement".into(), true, false)
            .unwrap();
    }
    let e = Engine::open(&path).unwrap();
    assert!(!e.data.users.contains_key("bob"));
    assert!(e.active("eve") && e.is_admin("alice"));
    assert_eq!(e.data.users["alice"].hash, "replacement");
}
#[test]
fn delete_user_rolls_back_database_and_cache_on_cleanup_failure() {
    let mut e = engine();
    e.run(Some("alice"), None, "/tell bob retained").unwrap();
    e.login("bob-token".into(), "bob", "test-hash", None)
        .unwrap();
    let through = e.snapshot("bob").unwrap().unread["@direct:alice"].through;
    e.mark_read("bob", "@direct:alice", through).unwrap();
    let before = e.data.clone();
    let positions = e.read_positions.clone();
    let revision = e.revision;
    let disk: String =
        e.db.query_row("SELECT json FROM state", [], |r| r.get(0))
            .unwrap();
    e.db.execute_batch("CREATE TRIGGER fail_cleanup BEFORE DELETE ON read_positions BEGIN SELECT RAISE(ABORT,'test cleanup failure'); END;").unwrap();
    assert!(e.run(Some("alice"), None, "/deleteuser bob").is_err());
    assert!(e.data == before);
    assert_eq!(e.read_positions, positions);
    assert_eq!(e.revision, revision);
    assert_eq!(e.session("bob-token").as_deref(), Some("bob"));
    assert_eq!(
        e.db.query_row::<String, _, _>("SELECT json FROM state", [], |r| r.get(0))
            .unwrap(),
        disk
    );
    assert_eq!(e.db.query_row::<u64,_,_>("SELECT sequence FROM read_positions WHERE username='bob' AND conversation='dm:alice:bob'",[],|r|r.get(0)).unwrap(),through);
    e.db.execute_batch("DROP TRIGGER fail_cleanup;").unwrap();
    e.run(Some("alice"), None, "/deleteuser bob").unwrap();
    assert!(e.session("bob-token").is_none());
}
#[test]
fn role_commands_are_authorized_and_not_chat_messages() {
    let mut e = engine();
    assert_eq!(
        e.run(Some("bob"), None, "/whoami").unwrap().reply,
        "Name: bob\nPermission: user"
    );
    assert!(e.run(Some("bob"), None, "/grant bob").is_err());
    assert!(e.run(Some("alice"), None, "/revoke alice").is_err());
    let help = e.run(Some("bob"), None, "/help").unwrap().reply;
    assert!(help.lines().count() >= 10);
    assert!(!help.contains("/grant"));
    assert!(!help.contains("/reset"));
    e.run(Some("alice"), None, "/grant bob").unwrap();
    assert!(e.snapshot("bob").unwrap().admin);
    assert!(
        e.snapshot("bob")
            .unwrap()
            .commands
            .iter()
            .any(|c| c.name == "/grant")
    );
    assert!(
        e.run(Some("bob"), None, "/whoami")
            .unwrap()
            .reply
            .contains("Permission: admin")
    );
    e.run(Some("bob"), None, "/new promoted").unwrap();
    e.run(Some("alice"), None, "/revoke bob").unwrap();
    assert!(e.run(Some("bob"), None, "/new denied").is_err());
    assert!(e.data.rooms["lobby"].messages.is_empty());
    assert!(e.run(Some("alice"), None, "/grant missing").is_err());
}
#[test]
fn history_tail_members_and_account_recovery() {
    let mut e = engine();
    for i in 0..80 {
        e.run(Some("bob"), Some("lobby"), &format!("message-{i}"))
            .unwrap();
    }
    let snapshot = e.snapshot("bob").unwrap();
    assert_eq!(snapshot.rooms[0].messages.len(), 50);
    assert_eq!(snapshot.rooms[0].messages[0].text, "message-30");
    assert_eq!(
        e.run(Some("bob"), Some("lobby"), "/history 60")
            .unwrap()
            .reply
            .lines()
            .count(),
        60
    );
    assert!(e.run(Some("bob"), Some("lobby"), "/history 1001").is_err());
    assert!(
        e.run(Some("bob"), Some("lobby"), "/members")
            .unwrap()
            .reply
            .contains("alice — admin")
    );
    e.run(Some("alice"), None, "/new private").unwrap();
    assert!(e.run(Some("bob"), Some("private"), "/history").is_err());
    assert!(
        e.run(Some("bob"), Some("lobby"), "/members private")
            .is_err()
    );
    e.run(Some("bob"), None, "/tell   alice   spaced text")
        .unwrap();
    assert_eq!(e.snapshot("alice").unwrap().direct[0].text, "spaced text");
    e.run(None, None, "/disable bob").unwrap();
    assert!(e.run(Some("eve"), None, "/enable bob").is_err());
    e.run(Some("alice"), None, "/enable bob").unwrap();
    assert!(e.active("bob"));
    assert!(e.snapshot("bob").unwrap().rooms.is_empty());
}
#[test]
fn granted_role_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    {
        let mut e = Engine::open(&path).unwrap();
        e.provision("alice", "hash".into(), true, false).unwrap();
        e.provision("bob", "hash".into(), false, false).unwrap();
        e.run(Some("alice"), None, "/grant bob").unwrap();
    }
    assert!(Engine::open(&path).unwrap().snapshot("bob").unwrap().admin);
}

#[test]
fn online_presence_counts_tabs_and_excludes_disabled_accounts() {
    let mut e = engine();
    let revision = e.revision;
    e.connect("bob");
    e.connect("bob");
    e.connect("eve");
    assert_eq!(e.snapshot("alice").unwrap().online, vec!["bob", "eve"]);
    e.disconnect("bob");
    assert_eq!(e.snapshot("alice").unwrap().online, vec!["bob", "eve"]);
    e.disconnect("bob");
    e.disconnect("bob");
    assert_eq!(e.snapshot("alice").unwrap().online, vec!["eve"]);
    assert_eq!(e.revision, revision);
    e.run(Some("alice"), None, "/disable eve").unwrap();
    assert!(e.snapshot("alice").unwrap().online.is_empty());
}

#[test]
fn retract_enforces_ownership_membership_and_private_visibility() {
    let mut e = engine();
    e.run(Some("bob"), Some("lobby"), "room original").unwrap();
    let id = e.data.rooms["lobby"].messages[0].id.clone();
    println!("DEBUG quoting id={id}");
    let command = format!("/retract {id}");
    // Even administrators cannot retract another author's message.
    assert!(e.run(Some("alice"), Some("lobby"), &command).is_err());
    e.run(Some("alice"), Some("lobby"), &format!("/reply {id} reply"))
        .unwrap();
    e.run(Some("alice"), None, "/kick bob lobby").unwrap();
    assert!(e.run(Some("bob"), Some("lobby"), &command).is_err());
    e.run(Some("alice"), None, "/add bob lobby").unwrap();
    let revision = e.data.rooms["lobby"].revision;
    e.run(Some("bob"), Some("lobby"), &command).unwrap();
    assert_eq!(e.data.rooms["lobby"].revision, revision + 1);
    assert_eq!(e.data.rooms["lobby"].messages.len(), 1);
    assert!(e.data.rooms["lobby"].messages[0].reply.is_none());
    assert!(e.run(Some("bob"), Some("lobby"), &command).is_err());
    assert_eq!(e.snapshot("alice").unwrap().unread["lobby"].count, 0);

    e.run(Some("bob"), None, "/tell alice private original")
        .unwrap();
    let id = e.data.private["alice:bob"].messages[0].id.clone();
    let command = format!("/retract {id}");
    assert!(e.run(Some("eve"), None, &command).is_err());
    assert!(e.run(Some("alice"), None, &command).is_err());
    assert!(e.run(Some("bob"), Some("lobby"), &command).is_err());
    e.run(Some("alice"), None, &format!("/reply {id} reply"))
        .unwrap();
    e.run(Some("bob"), None, &command).unwrap();
    assert_eq!(e.data.private["alice:bob"].messages.len(), 1);
    assert!(e.data.private["alice:bob"].messages[0].reply.is_none());
}

#[test]
fn retract_persists_and_failed_writes_restore_message_and_quotes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    {
        let mut e = Engine::open(&path).unwrap();
        e.provision("alice", "hash".into(), true, false).unwrap();
        e.provision("bob", "hash".into(), false, false).unwrap();
        e.run(Some("alice"), None, "/new room").unwrap();
        for room in [Some("room"), None] {
            e.run(
                Some("alice"),
                room,
                if room.is_some() {
                    "original"
                } else {
                    "/tell bob original"
                },
            )
            .unwrap();
            let id = if room.is_some() {
                e.data.rooms["room"].messages[0].id.clone()
            } else {
                e.data.private["alice:bob"].messages[0].id.clone()
            };
            e.run(Some("alice"), room, &format!("/reply {id} reply"))
                .unwrap();
            let before = e.data.clone();
            let revision = e.revision;
            e.db.execute_batch("PRAGMA query_only=ON;").unwrap();
            assert!(
                e.run(Some("alice"), room, &format!("/retract {id}"))
                    .is_err()
            );
            if e.data != before {
                println!(
                    "DIFF before={}",
                    serde_json::to_string(&before).unwrap_or_default()
                );
                println!(
                    "DIFF after ={}",
                    serde_json::to_string(&e.data).unwrap_or_default()
                );
            }
            assert!(e.data == before);
            assert_eq!(e.revision, revision);
            e.db.execute_batch("PRAGMA query_only=OFF;").unwrap();
            e.run(Some("alice"), room, &format!("/retract {id}"))
                .unwrap();
        }
        e.connect("alice");
    }
    let e = Engine::open(&path).unwrap();
    assert_eq!(e.data.rooms["room"].messages.len(), 1);
    assert!(e.data.rooms["room"].messages[0].reply.is_none());
    assert_eq!(e.data.private["alice:bob"].messages.len(), 1);
    assert!(e.data.private["alice:bob"].messages[0].reply.is_none());
    assert!(e.snapshot("alice").unwrap().online.is_empty());
}

#[test]
fn debug_is_admin_only_and_read_only() {
    let mut e = engine();
    let revision = e.revision;
    for mode in ["on", "off"] {
        let command = format!("/debug {mode}");
        assert_eq!(
            e.execute(Some("alice"), None, &command).unwrap(),
            format!("Debug {mode}.")
        );
        assert!(e.execute(Some("bob"), None, &command).is_err());
        assert!(e.execute(None, None, &command).is_err());
    }
    for command in ["/debug", "/debug yes", "/debug on extra"] {
        assert!(e.execute(Some("alice"), None, command).is_err());
    }
    assert_eq!(e.revision, revision);
    assert!(
        crate::commands::available(true, false)
            .iter()
            .any(|c| c.name == "/debug")
    );
    assert!(
        !crate::commands::available(false, false)
            .iter()
            .any(|c| c.name == "/debug")
    );
}

#[test]
fn su_retracts_any_room_or_private_message_and_admin_retracts_own() {
    let mut e = engine();
    assert_eq!(
        e.execute(None, None, "/whoami").unwrap(),
        "Name: su\nPermission: su"
    );
    assert!(
        crate::commands::available(true, true)
            .iter()
            .any(|c| c.name == "/retract")
    );
    for room in [Some("lobby"), None] {
        e.execute(
            Some("bob"),
            room,
            if room.is_some() {
                "original"
            } else {
                "/tell alice original"
            },
        )
        .unwrap();
        let id = if room.is_some() {
            e.data.rooms["lobby"].messages[0].id.clone()
        } else {
            e.data.private["alice:bob"].messages[0].id.clone()
        };
        e.execute(Some("alice"), room, &format!("/reply {id} reply"))
            .unwrap();
        assert!(
            e.execute(Some("alice"), room, &format!("/retract {id}"))
                .is_err()
        );
        let before = e.data.clone();
        e.db.execute_batch("PRAGMA query_only=ON;").unwrap();
        assert!(e.execute(None, None, &format!("/retract {id}")).is_err());
        assert!(e.data == before);
        e.db.execute_batch("PRAGMA query_only=OFF;").unwrap();
        e.execute(None, None, &format!("/retract {id}")).unwrap();
        let messages = if room.is_some() {
            &e.data.rooms["lobby"].messages
        } else {
            &e.data.private["alice:bob"].messages
        };
        assert_eq!(messages.len(), 1);
        assert!(messages[0].reply.is_none());
        let reply_id = messages[0].id.clone();
        e.execute(Some("alice"), room, &format!("/retract {reply_id}"))
            .unwrap();
    }
    assert!(e.execute(None, None, "/retract missing").is_err());
}
/// Provision an agent owned by `owner` and invite it to `room`.
fn agent_engine(room: &str, owner: &str, reply: AgentReply) -> Engine {
    let mut e = engine();
    e.run(Some(owner), None, "/agent helper sk-test-secret")
        .unwrap();
    e.run(Some(owner), None, &format!("/add helper {room}"))
        .unwrap();
    if reply != AgentReply::default() {
        e.run(Some(owner), None, "/agent-reply auto").unwrap();
    }
    e
}
#[test]
fn agent_accounts_are_roles_that_cannot_log_in_or_act_as_admins() {
    let mut e = engine();
    e.run(Some("alice"), None, "/agent helper sk-test-secret")
        .unwrap();
    let account = e.data.users["helper"].clone();
    assert!(account.is_agent() && !account.is_human() && !account.disabled);
    assert!(!account.id.is_empty(), "an agent is an account with an id");
    assert_eq!(account.reply, AgentReply::Mention);
    assert_eq!(account.api_key, "sk-test-secret");
    // Ownership is a policy grant, not a field on the account.
    assert_eq!(e.agent_owner_name(&account.id).as_deref(), Some("alice"));
    // An agent holds the least authority that lets it answer: it may read and
    // write in its conversations, and it holds no command at all.
    let scope = crate::engine::authorization::Scope::Server;
    let rights = e.allowance(Some("helper"), &scope);
    assert!(rights.contains(&"r:message.read".to_string()), "{rights:?}");
    assert!(
        rights.contains(&"w:message.create".to_string()),
        "{rights:?}"
    );
    assert!(rights.contains(&"r:member.list".to_string()), "{rights:?}");
    assert!(
        !rights.iter().any(|p| p.starts_with("x:")),
        "an agent holds no management action: {rights:?}"
    );
    assert!(
        !rights
            .iter()
            .any(|p| p == "w:message.react" || p == "w:message.retract.own"),
        "an agent answers messages, it does not manage them: {rights:?}"
    );
    assert!(
        e.commands_for(Some("helper"), None).is_empty(),
        "an agent holds no command"
    );
    // Agents hold no password, so no password can match one.
    assert!(!verify_password("", &account.hash));
    assert!(account.hash.is_empty());
    assert!(e.login("token".into(), "helper", "", None).is_err());
    assert!(e.session("token").is_none());
    // An agent is rejected wherever administrator permission is required.
    assert!(!e.is_admin("helper"));
    assert!(e.run(Some("helper"), None, "/new sneaky").is_err());
    assert!(e.run(Some("helper"), None, "/clean 7d @all").is_err());
    assert!(e.run(Some("helper"), None, "/users").is_err());
    // An agent cannot be handed a group, so the last-admin guard never has to
    // consider one.
    assert!(e.run(Some("alice"), None, "/grant helper admin").is_err());
    assert!(e.run(Some("alice"), None, "/grant helper su").is_err());
    assert_eq!(e.role("helper"), "agent");
    assert_eq!(e.role("alice"), "admin");
    assert_eq!(e.role("bob"), "user");
    assert!(e.snapshot("alice").unwrap().roles["helper"] == "agent");
}
#[test]
fn agent_keys_stay_out_of_snapshots_history_help_and_errors() {
    let mut e = agent_engine("lobby", "alice", AgentReply::Mention);
    let secret = "sk-test-secret";
    let snapshot = serde_json::to_string(&e.snapshot("alice").unwrap()).unwrap();
    assert!(!snapshot.contains(secret));
    assert!(!snapshot.contains("api_key"));
    assert!(!crate::commands::help(true, false).contains(secret));
    e.run(Some("alice"), Some("lobby"), "hello @helper")
        .unwrap();
    for message in &e.data.rooms["lobby"].messages {
        assert!(!message.text.contains(secret));
    }
    // Usage and validation errors never echo the supplied credential.
    let denied = e
        .run(Some("bob"), None, "/agent-key another-sk-secret")
        .map(|execution| execution.reply);
    assert!(denied.is_err(), "bob owns no agent and may not set a key");
    assert!(!denied.unwrap_or_default().contains("another-sk-secret"));
    let usage = e
        .run(Some("bob"), None, "/agent-key")
        .map(|execution| execution.reply);
    assert!(usage.is_err(), "a key command needs a key argument");
    assert!(!usage.unwrap_or_default().contains("another-sk-secret"));
    assert_eq!(e.data.users["helper"].api_key, secret);
}
#[test]
fn agent_creation_requires_a_name_and_a_key() {
    let mut e = engine();
    for command in [
        "/agent",
        "/agent helper",
        "/agent helper sk-key extra",
        "/agent bad/name sk-key",
        "/agent helper sk key",
        "/agent helper \"quoted\"",
    ] {
        assert!(e.run(Some("alice"), None, command).is_err(), "{command}");
    }
    assert!(e.data.users.is_empty() || !e.data.users.contains_key("helper"));
    // Any signed-in account may create an agent and becomes its owner.
    e.run(Some("bob"), None, "/agent helper sk-key").unwrap();
    assert!(e.owns_any_agent("bob"));
    assert!(
        e.run(Some("alice"), None, "/agent helper sk-other")
            .is_err(),
        "names are unique"
    );
    assert!(
        e.run(Some("alice"), None, "/agent helper sk-other")
            .is_err()
    );
    // Console input has no owning account.
    assert!(e.run(None, None, "/agent console-agent sk-key").is_err());
}
#[test]
fn owners_and_admins_configure_agents_within_their_scope() {
    let mut e = engine();
    e.run(Some("bob"), None, "/agent helper sk-first").unwrap();
    assert!(e.owns_any_agent("bob"));
    // Web search starts off and needs its own key.
    assert!(!e.data.users["helper"].search);
    assert!(e.data.users["helper"].search_key.is_empty());
    assert!(
        e.run(Some("bob"), None, "/agent-search on").is_err(),
        "search cannot be enabled before a key exists"
    );
    assert!(
        e.run(Some("bob"), None, "/agent-search-key tvly-dev-bob")
            .is_ok()
    );
    assert_eq!(e.data.users["helper"].search_key, "tvly-dev-bob");
    assert!(
        !e.data.users["helper"].search,
        "setting a key does not enable search"
    );
    e.run(Some("bob"), None, "/agent-search on").unwrap();
    assert!(e.data.users["helper"].search);
    e.run(Some("bob"), None, "/agent-search off").unwrap();
    assert!(!e.data.users["helper"].search);
    assert!(e.run(Some("bob"), None, "/agent-search maybe").is_err());
    // The source policy starts at auto and is validated like a reply mode.
    assert_eq!(e.data.users["helper"].sources, SourceMode::Auto);
    e.run(Some("bob"), None, "/agent-sources never").unwrap();
    assert_eq!(e.data.users["helper"].sources, SourceMode::Never);
    e.run(Some("bob"), None, "/agent-sources ALWAYS").unwrap();
    assert_eq!(e.data.users["helper"].sources, SourceMode::Always);
    assert!(e.run(Some("bob"), None, "/agent-sources maybe").is_err());
    assert_eq!(
        e.data.users["helper"].sources,
        SourceMode::Always,
        "a rejected value changes nothing"
    );
    assert!(e.run(Some("eve"), None, "/agent-sources never").is_err());
    e.run(Some("bob"), None, "/agent-sources auto").unwrap();
    // The owner edits its own agent.
    e.run(Some("bob"), None, "/agent-key sk-second").unwrap();
    assert_eq!(e.data.users["helper"].api_key, "sk-second");
    e.run(Some("bob"), None, "/agent-reply AUTO").unwrap();
    assert_eq!(e.data.users["helper"].reply, AgentReply::Auto);
    e.run(Some("bob"), None, "/agent-reply mention").unwrap();
    assert_eq!(e.data.users["helper"].reply, AgentReply::Mention);
    assert!(e.run(Some("bob"), None, "/agent-reply silent").is_err());
    assert_eq!(e.data.users["helper"].reply, AgentReply::Mention);
    // Other users may not touch it; an admin may.
    assert!(e.run(Some("eve"), None, "/agent-key sk-stolen").is_err());
    assert!(e.run(Some("eve"), None, "/agent-reply auto").is_err());
    assert!(e.run(Some("eve"), None, "/agent-search on").is_err());
    assert!(
        e.run(Some("eve"), None, "/agent-search-key tvly-dev-stolen")
            .is_err()
    );
    assert!(e.run(Some("eve"), None, "/agent-prompt be rude").is_err());
    assert_eq!(e.data.users["helper"].search_key, "tvly-dev-bob");
    assert_eq!(e.data.users["helper"].api_key, "sk-second");
    e.run(Some("alice"), None, "/agent-key sk-admin helper")
        .unwrap();
    assert_eq!(e.data.users["helper"].api_key, "sk-admin");
    e.run(Some("alice"), None, "/agent-reply auto helper")
        .unwrap();
    assert_eq!(e.data.users["helper"].reply, AgentReply::Auto);
    assert!(
        e.run(Some("alice"), None, "/agent-key sk-x missing")
            .is_err(),
        "an admin cannot configure a name that is not an agent"
    );
    // Non-agents and missing agents are rejected.
    assert!(
        e.run(Some("alice"), None, "/agent-name renamed bob")
            .is_err()
    );
    e.run(Some("bob"), None, "/agent-remove helper").unwrap();
    assert!(!e.data.users.contains_key("helper"));
    // A name that is not an agent is never a silent fallback to the caller's
    // own agent, and a rename never fabricates one.
    assert!(e.run(Some("bob"), None, "/agent-remove other").is_err());
    assert!(
        e.run(Some("bob"), None, "/agent-name other renamed")
            .is_err()
    );
    assert!(e.data.users.is_empty() || !e.data.users.contains_key("other"));
}
#[test]
fn search_credentials_travel_only_for_agents_that_may_search() {
    let mut e = agent_engine("lobby", "alice", AgentReply::Auto);
    // Disabled search means no search credential reaches the caller at all.
    e.run(Some("alice"), None, "/agent-search-key tvly-dev-alice")
        .unwrap();
    let job = e.run(Some("bob"), Some("lobby"), "hello").unwrap();
    assert_eq!(job.agents.len(), 1);
    assert!(!job.agents[0].search);
    assert!(
        job.agents[0].search_key.is_empty(),
        "a disabled search never carries the credential"
    );
    // Enabled search carries it, and the key stays out of every client view.
    e.run(Some("alice"), None, "/agent-search on").unwrap();
    let job = e.run(Some("bob"), Some("lobby"), "hello again").unwrap();
    assert!(job.agents[0].search);
    assert_eq!(job.agents[0].search_key, "tvly-dev-alice");
    let snapshot = serde_json::to_string(&e.snapshot("alice").unwrap()).unwrap();
    assert!(!snapshot.contains("tvly-dev-alice") && !snapshot.contains("search_key"));
    assert!(!format!("{:?}", job.agents[0]).contains("tvly-dev-alice"));
    assert_eq!(
        crate::commands::help(true, false)
            .matches("/agent-search")
            .count(),
        2,
        "help lists the search toggle and its key command"
    );
}
#[test]
fn personalities_are_free_text_and_never_replace_the_base_rules() {
    let mut e = engine();
    e.run(Some("bob"), None, "/agent helper sk-key").unwrap();
    // A new agent has no personality, and asking shows that.
    assert!(e.data.users["helper"].prompt.is_empty());
    let shown = e.run(Some("bob"), None, "/agent-prompt").unwrap().reply;
    assert!(shown.contains("no personality yet"), "{shown}");
    // The text may contain spaces, punctuation, emoji and newlines, and it is
    // only trimmed of surrounding whitespace.
    let persona = "你是一条大肥鱼 🐟\n每句话都要有 emoji ✨ 而且要提到水";
    e.run(Some("bob"), None, &format!("/agent-prompt {persona}"))
        .unwrap();
    assert_eq!(e.data.users["helper"].prompt, persona);
    let shown = e.run(Some("bob"), None, "/agent-prompt").unwrap().reply;
    assert!(shown.contains("你是一条大肥鱼 🐟"), "{shown}");
    // The base rules survive, and the persona is added after them.
    let prompt = agent_system_prompt("helper", "room:lobby", false, persona);
    assert!(prompt.contains("You are helper"));
    assert!(prompt.contains("cannot search the web"));
    assert!(prompt.contains("你是一条大肥鱼 🐟"));
    assert!(
        prompt.find("You are helper") < prompt.find("你是一条大肥鱼 🐟"),
        "the persona never replaces the rules"
    );
    assert_eq!(
        agent_system_prompt("helper", "room:lobby", false, "   "),
        agent_system_prompt("helper", "room:lobby", false, ""),
        "blank personality text is the same as none"
    );
    // `-` clears it, and an owner with several agents may name the target.
    e.run(Some("bob"), None, "/agent-prompt -").unwrap();
    assert!(e.data.users["helper"].prompt.is_empty());
    // A bare agent name reads that agent instead of becoming its personality,
    // because agent names are unique accounts.
    e.run(Some("bob"), None, "/agent-prompt helper").unwrap();
    assert!(
        e.data.users["helper"].prompt.is_empty(),
        "naming one agent does not write its name as a personality"
    );
    e.run(Some("bob"), None, "/agent-prompt 写诗 writer")
        .unwrap();
    assert_eq!(e.data.users["helper"].prompt, "写诗 writer");
    e.run(Some("bob"), None, "/agent writer sk-two").unwrap();
    e.run(Some("bob"), None, "/agent-prompt 只写诗 writer")
        .unwrap();
    assert_eq!(e.data.users["writer"].prompt, "只写诗");
    assert_eq!(
        e.data.users["helper"].prompt, "写诗 writer",
        "an unnamed target with several agents is an error, not a guess"
    );
    // An oversized or multi-line personality is bounded, not truncated.
    let too_long = "字".repeat(4001);
    assert!(
        e.run(
            Some("bob"),
            None,
            &format!("/agent-prompt {too_long} helper")
        )
        .is_err()
    );
    e.run(
        Some("bob"),
        None,
        &format!("/agent-prompt {} helper", "字".repeat(4000)),
    )
    .unwrap();
    assert_eq!(e.data.users["helper"].prompt.chars().count(), 4000);
    // A multi-line personality survives the command pipeline intact.
    let two_lines = "第一条 ✨\n第二条 🐟";
    e.run(
        Some("bob"),
        None,
        &format!("/agent-prompt {two_lines} helper"),
    )
    .unwrap();
    assert_eq!(e.data.users["helper"].prompt, two_lines);
    // An unnamed command with several owned agents asks for the name rather
    // than guessing which personality to change.
    let ambiguous = e
        .run(Some("bob"), None, "/agent-prompt 随便")
        .map(|e| e.reply);
    assert!(ambiguous.is_err(), "an ambiguous target is refused");
    assert_eq!(e.data.users["helper"].prompt, two_lines);
    assert_eq!(e.data.users["writer"].prompt, "只写诗");
    // `/agent prompt ...` is the same command as `/agent-prompt`, since a bare
    // `/agent <name> <key>` already owns the two-word form.
    e.run(Some("bob"), None, "/agent prompt 只说鱼话 writer")
        .unwrap();
    assert_eq!(e.data.users["writer"].prompt, "只说鱼话");
    let shown = e
        .run(Some("bob"), None, "/agent prompt writer")
        .unwrap()
        .reply;
    assert!(shown.contains("只说鱼话"), "{shown}");
    // `/agent` with a name that is not `prompt` still creates an agent.
    e.run(Some("bob"), None, "/agent second sk-three").unwrap();
    assert!(e.data.users["second"].is_agent());
    // A personality the owner did not write can still be cleared by an admin.
    e.run(Some("alice"), None, "/agent-prompt - writer")
        .unwrap();
    assert!(e.data.users["writer"].prompt.is_empty());
    // The personality reaches the job that produces the answer.
    e.run(Some("bob"), None, "/agent-reply auto helper")
        .unwrap();
    e.run(Some("alice"), None, "/add helper lobby").unwrap();
    let asked = e.run(Some("bob"), Some("lobby"), "hello").unwrap();
    assert_eq!(asked.agents.len(), 1);
    assert_eq!(
        asked.agents[0].prompt, two_lines,
        "the stored personality reaches the job"
    );
    let snapshot = serde_json::to_string(&e.snapshot("bob").unwrap()).unwrap();
    assert!(
        snapshot.contains("第一条 ✨"),
        "the owner can read the personality they wrote"
    );
}
#[test]
fn provider_settings_are_validated_and_resolve_to_an_endpoint() {
    let mut e = engine();
    e.run(Some("bob"), None, "/agent helper sk-key").unwrap();
    // An agent starts on the built-in provider and model.
    let provider = resolve_provider("", "", "").unwrap();
    assert_eq!(provider.name, "deepseek");
    assert_eq!(provider.model, DEFAULT_MODEL);
    assert_eq!(
        provider.endpoint(),
        "https://api.deepseek.com/chat/completions"
    );
    // Switching provider keeps the key and records the canonical name.
    e.run(Some("bob"), None, "/agent-provider OpenRouter")
        .unwrap();
    assert_eq!(
        e.data.users["helper"].provider, "openrouter",
        "the stored name is canonical"
    );
    e.run(Some("bob"), None, "/agent-model z-ai/glm-4.6")
        .unwrap();
    assert_eq!(e.data.users["helper"].model, "z-ai/glm-4.6");
    e.run(Some("alice"), None, "/add helper lobby").unwrap();
    e.run(Some("alice"), None, "/agent-reply auto helper")
        .unwrap();
    let asked = e.run(Some("bob"), Some("lobby"), "hi").unwrap();
    assert_eq!(asked.agents.len(), 1);
    assert_eq!(asked.agents[0].provider, "openrouter");
    assert_eq!(asked.agents[0].model, "z-ai/glm-4.6");
    assert_eq!(
        resolve_provider(
            &asked.agents[0].provider,
            &asked.agents[0].base_url,
            &asked.agents[0].model
        )
        .unwrap()
        .endpoint(),
        "https://openrouter.ai/api/v1/chat/completions"
    );
    // A gateway is named by URL, and every documented spelling normalises.
    for spelling in [
        "https://gateway.example.com/v1",
        "https://gateway.example.com/v1/",
        "https://gateway.example.com/v1/chat/completions",
    ] {
        e.run(
            Some("bob"),
            None,
            &format!("/agent-base-url {spelling} helper"),
        )
        .unwrap();
        assert_eq!(
            e.data.users["helper"].base_url, "https://gateway.example.com/v1",
            "{spelling}"
        );
    }
    // The provider command refuses a URL, which has its own command.
    assert!(
        e.run(
            Some("bob"),
            None,
            "/agent-provider https://openrouter.ai/api/v1 helper"
        )
        .is_err()
    );
    // Invalid values are refused and change nothing.
    for bad in [
        "/agent-provider notaprovider helper",
        "/agent-base-url http://plain.example.com helper",
        "/agent-base-url gateway.example.com helper",
        "/agent-model has space helper",
        "/agent-model bad;chars helper",
    ] {
        assert!(e.run(Some("bob"), None, bad).is_err(), "{bad}");
    }
    assert_eq!(
        e.data.users["helper"].base_url,
        "https://gateway.example.com/v1"
    );
    assert_eq!(e.data.users["helper"].model, "z-ai/glm-4.6");
    // `-` returns each setting to its default.
    e.run(Some("bob"), None, "/agent-base-url - helper")
        .unwrap();
    e.run(Some("bob"), None, "/agent-model default helper")
        .unwrap();
    e.run(Some("bob"), None, "/agent-provider - helper")
        .unwrap();
    assert!(e.data.users["helper"].base_url.is_empty());
    assert!(e.data.users["helper"].model.is_empty());
    assert!(e.data.users["helper"].provider.is_empty());
    // Only the owner or an admin may change the provider.
    assert!(
        e.run(Some("eve"), None, "/agent-provider openai helper")
            .is_err()
    );
    assert!(
        e.run(Some("eve"), None, "/agent-model gpt-4.1 helper")
            .is_err()
    );
    // The summary reports the selection without any credential.
    e.run(Some("bob"), None, "/agent-provider openrouter helper")
        .unwrap();
    let summary = e
        .run(Some("bob"), None, "/agent-config helper")
        .unwrap()
        .reply;
    assert!(summary.contains("openrouter · deepseek-flash"), "{summary}");
    assert!(summary.contains("provider key: set"), "{summary}");
    assert!(summary.contains("search key: missing"), "{summary}");
    assert!(
        !summary.contains("sk-key"),
        "no credential in the summary: {summary}"
    );
    // Every known provider resolves, so the registry and resolver agree.
    for (name, root) in KNOWN_PROVIDERS {
        let provider = resolve_provider(name, "", "").unwrap();
        assert_eq!(provider.name, *name);
        assert_eq!(provider.base_url, *root);
        assert!(provider.endpoint().ends_with("/chat/completions"));
    }
}
#[test]
fn rename_carries_membership_history_mentions_replies_and_reactions() {
    let mut e = engine();
    e.run(Some("alice"), None, "/agent helper sk-key").unwrap();
    e.run(Some("alice"), None, "/add helper lobby").unwrap();
    e.run(Some("alice"), Some("lobby"), "hello @helper")
        .unwrap();
    let answer = e
        .insert_agent_reply("helper", "room:lobby", "my answer")
        .unwrap();
    // A person replies to the agent's retained answer and reacts to it.
    e.run(
        Some("bob"),
        Some("lobby"),
        &format!("/reply {answer} thanks"),
    )
    .unwrap();
    e.run(Some("bob"), Some("lobby"), &format!("/react {answer} 👍"))
        .unwrap();
    e.run(Some("bob"), None, "/tell helper @helper private")
        .unwrap();
    e.insert_agent_reply("helper", "dm:bob:helper", "private answer")
        .unwrap();
    // The agent records a reaction of its own before the rename.
    e.data
        .rooms
        .get_mut("lobby")
        .unwrap()
        .messages
        .iter_mut()
        .find(|message| message.id == answer)
        .unwrap()
        .reactions
        .entry("🎉".into())
        .or_default()
        .insert("helper".into());
    e.run(Some("alice"), None, "/agent-name assistant").unwrap();
    assert!(!e.data.users.contains_key("helper"));
    assert!(e.data.users["assistant"].is_agent());
    {
        let room = &e.data.rooms["lobby"];
        assert!(
            room.members.contains("assistant") && !room.members.contains("helper"),
            "membership follows the new name"
        );
        assert_eq!(room.messages[1].from, "assistant");
        assert_eq!(room.messages[1].text, "my answer");
        assert_eq!(room.messages[1].id, answer);
        // The quote and the mention in retained history follow the new name.
        assert_eq!(
            room.messages[2]
                .reply
                .as_ref()
                .map(|quote| quote.from.as_str()),
            Some("assistant")
        );
        assert!(
            room.messages[2].mentions.is_empty(),
            "only the mentioned account is recorded"
        );
        assert_eq!(
            room.messages[0].mentions,
            BTreeSet::from(["assistant".to_string()])
        );
        assert_eq!(
            room.messages[1].reactions["👍"],
            BTreeSet::from(["bob".to_string()]),
            "a human reaction is unchanged by the rename"
        );
    }
    // A reaction the agent itself recorded follows the rename.
    assert_eq!(
        e.data.rooms["lobby"].messages[1].reactions["🎉"],
        BTreeSet::from(["assistant".to_string()])
    );
    assert!(
        !e.data.rooms["lobby"].messages[1]
            .reactions
            .contains_key("helper")
    );
    assert_eq!(e.data.rooms["lobby"].messages.len(), 3);
    // The private conversation moved too.
    assert!(e.data.private.contains_key("assistant:bob"));
    assert!(!e.data.private.contains_key("bob:helper"));
    let private = &e.data.private["assistant:bob"];
    assert!(private.messages.iter().all(|m| m.from != "helper"));
    assert_eq!(private.messages[1].text, "private answer");
    // A rename onto an existing account is refused.
    assert!(e.run(Some("alice"), None, "/agent-name bob").is_err());
    assert!(e.data.users.contains_key("assistant"));
    // The renamed agent still answers its new mention.
    let queued = e.run(Some("bob"), Some("lobby"), "hey @assistant").unwrap();
    assert_eq!(queued.agents.len(), 1);
    assert_eq!(queued.agents[0].name, "assistant");
}
#[test]
fn mentions_trigger_agents_and_other_messages_do_not() {
    let mut e = agent_engine("lobby", "alice", AgentReply::Mention);
    let quiet = e
        .run(Some("bob"), Some("lobby"), "no mention here")
        .unwrap();
    assert!(quiet.agents.is_empty());
    assert!(e.agent_queue.is_empty());
    // A mention of a name outside the room does not trigger the agent.
    let outside = e
        .run(Some("bob"), Some("lobby"), "@helper elsewhere")
        .unwrap();
    assert_eq!(outside.agents.len(), 1);
    // Mentions are recorded only for room members, so remove it and retry.
    e.run(Some("alice"), None, "/kick helper lobby").unwrap();
    let kicked = e.run(Some("bob"), Some("lobby"), "@helper gone").unwrap();
    assert!(kicked.agents.is_empty());
    e.run(Some("alice"), None, "/add helper lobby").unwrap();
    // An agent is never answered by an agent, with or without a mention, so
    // two agents cannot hold an unbounded conversation.
    e.run(Some("alice"), None, "/agent buddy sk-buddy").unwrap();
    e.run(Some("alice"), None, "/add buddy lobby").unwrap();
    let cross = e
        .run(Some("buddy"), Some("lobby"), "hello @helper")
        .unwrap();
    assert!(cross.agents.is_empty(), "a mention does not cross agents");
    // Alice owns both agents, so the command must name its target.
    assert!(e.run(Some("alice"), None, "/agent-reply auto").is_err());
    e.run(Some("alice"), None, "/agent-reply auto helper")
        .unwrap();
    assert_eq!(
        e.data.users["helper"].reply,
        AgentReply::Auto,
        "a named agent is configured even when the owner has several"
    );
    let auto = e.run(Some("buddy"), Some("lobby"), "morning").unwrap();
    assert!(auto.agents.is_empty(), "auto mode does not cross agents");
    // A person mentioning the same agent still gets an answer.
    assert_eq!(
        e.run(Some("bob"), Some("lobby"), "hello @helper")
            .unwrap()
            .agents
            .len(),
        1
    );
}
#[test]
fn auto_mode_answers_every_message_and_private_messages_too() {
    let mut e = agent_engine("lobby", "alice", AgentReply::Auto);
    let room = e.run(Some("bob"), Some("lobby"), "morning").unwrap();
    assert_eq!(room.agents.len(), 1);
    let job = &room.agents[0];
    assert_eq!(job.name, "helper");
    assert_eq!(job.api_key, "sk-test-secret");
    assert_eq!(job.view, "room:lobby");
    assert_eq!(
        job.trigger,
        e.data.rooms["lobby"].messages.back().unwrap().id
    );
    // The agent receives the conversation as context.
    assert!(job.context.iter().any(|m| m.text == "morning"));
    // Retained context is bounded.
    for n in 0..40 {
        e.run(Some("bob"), Some("lobby"), &format!("message {n}"))
            .unwrap();
    }
    let latest = e.run(Some("bob"), Some("lobby"), "last").unwrap();
    assert_eq!(latest.agents[0].context.len(), AGENT_CONTEXT);
    // Private conversations trigger agents as well.
    let direct = e.run(Some("bob"), None, "/tell helper hello").unwrap();
    assert_eq!(direct.agents.len(), 1);
    assert_eq!(direct.agents[0].view, "dm:bob:helper");
    // Auto activity needs no mention.
    assert!(
        e.agent_queue
            .iter()
            .all(|job| job.name == "helper" && !job.api_key.is_empty())
    );
}
#[test]
fn failed_provider_messages_never_become_conversation_context() {
    let mut e = agent_engine("lobby", "alice", AgentReply::Auto);
    // A failed call is posted with the error marker.
    let failure = format!("{AGENT_ERROR_PREFIX} The model returned an empty reply.");
    e.insert_agent_reply("helper", "room:lobby", &failure)
        .unwrap();
    let asked = e.run(Some("bob"), Some("lobby"), "real question").unwrap();
    assert_eq!(asked.agents.len(), 1);
    let context = &asked.agents[0].context;
    assert!(
        context
            .iter()
            .any(|message| message.text == "real question"),
        "the question is context"
    );
    assert!(
        !context
            .iter()
            .any(|message| message.text.starts_with(AGENT_ERROR_PREFIX)),
        "a provider fault is never replayed to the model as agent speech"
    );
    // The stored failure stays visible to people.
    assert!(
        e.data.rooms["lobby"]
            .messages
            .iter()
            .any(|message| message.text == failure)
    );
}
#[test]
fn disabled_and_keyless_agents_never_receive_work() {
    let mut e = agent_engine("lobby", "alice", AgentReply::Auto);
    e.run(Some("alice"), None, "/disable helper").unwrap();
    assert!(
        e.run(Some("bob"), Some("lobby"), "quiet")
            .unwrap()
            .agents
            .is_empty()
    );
    assert!(!e.data.rooms["lobby"].members.contains("helper"));
    e.run(Some("alice"), None, "/enable helper").unwrap();
    e.run(Some("alice"), None, "/add helper lobby").unwrap();
    assert_eq!(
        e.run(Some("bob"), Some("lobby"), "loud")
            .unwrap()
            .agents
            .len(),
        1
    );
    e.data.users.get_mut("helper").unwrap().api_key.clear();
    assert!(
        e.run(Some("bob"), Some("lobby"), "silent")
            .unwrap()
            .agents
            .is_empty()
    );
}
#[test]
fn one_pending_reply_per_agent_and_replies_are_persisted_once() {
    let mut e = agent_engine("lobby", "alice", AgentReply::Auto);
    e.connect("helper");
    let first = e.run(Some("bob"), Some("lobby"), "one").unwrap();
    assert_eq!(first.agents.len(), 1);
    assert!(e.claim_agent("helper"));
    // The websocket layer refuses a second concurrent claim.
    assert!(!e.claim_agent("helper"));
    let second = e.run(Some("bob"), Some("lobby"), "two").unwrap();
    assert!(second.agents.is_empty(), "a busy agent is not queued twice");
    e.release_agent("helper");
    let id = e
        .insert_agent_reply("helper", "room:lobby", "the answer 你好")
        .unwrap();
    assert_eq!(
        e.data.rooms["lobby"].messages.back().unwrap().text,
        "the answer 你好"
    );
    assert_eq!(e.data.rooms["lobby"].messages.back().unwrap().id, id);
    assert_eq!(
        e.data.rooms["lobby"].messages.back().unwrap().from,
        "helper"
    );
    // Only "one" produced an answer: the second claim was refused.
    assert_eq!(e.snapshot("bob").unwrap().unread["lobby"].count, 1);
    // Agent answers are ordinary unread messages for the room's people.
    assert_eq!(
        e.data.rooms["lobby"]
            .messages
            .iter()
            .filter(|message| message.from != "bob")
            .count(),
        1
    );
    // A retracted or evicted membership no longer accepts replies.
    e.run(Some("alice"), None, "/kick helper lobby").unwrap();
    assert!(
        e.insert_agent_reply("helper", "room:lobby", "still here")
            .is_err()
    );
    assert!(
        e.insert_agent_reply("helper", "room:missing", "nowhere")
            .is_err()
    );
    assert!(
        e.insert_agent_reply("bob", "room:lobby", "not an agent")
            .is_err()
    );
    assert!(e.insert_agent_reply("helper", "room:lobby", "   ").is_err());
    assert!(
        e.insert_agent_reply("helper", "room:lobby", &"你".repeat(4001))
            .is_err()
    );
}
#[test]
fn agent_replies_persist_across_restart_and_roll_back_on_write_failure() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    {
        let mut e = Engine::open(&path).unwrap();
        e.provision("alice", "hash".into(), true, false).unwrap();
        e.run(Some("alice"), None, "/new lobby").unwrap();
        e.run(Some("alice"), None, "/agent helper sk-persisted")
            .unwrap();
        e.run(Some("alice"), None, "/add helper lobby").unwrap();
        e.insert_agent_reply("helper", "room:lobby", "restart me")
            .unwrap();
        let before = e.data.clone();
        e.db.execute_batch("PRAGMA query_only=ON;").unwrap();
        assert!(
            e.insert_agent_reply("helper", "room:lobby", "unsaved")
                .is_err()
        );
        assert!(e.data == before);
        e.db.execute_batch("PRAGMA query_only=OFF;").unwrap();
    }
    {
        let e = Engine::open(&path).unwrap();
        let account = &e.data.users["helper"];
        assert!(account.is_agent() && account.api_key == "sk-persisted");
        assert_eq!(account.reply, AgentReply::Mention);
        assert_eq!(
            e.data.rooms["lobby"].messages.back().unwrap().text,
            "restart me"
        );
        assert!(e.data.rooms["lobby"].members.contains("helper"));
    }
}
#[test]
fn accounts_saved_before_agents_load_as_humans_with_defaults() {
    let legacy: Account =
        serde_json::from_str(r#"{"hash":"hash","admin":true,"disabled":false}"#).unwrap();
    assert!(legacy.is_human() && !legacy.is_agent());
    assert!(legacy.api_key.is_empty());
    assert_eq!(legacy.reply, AgentReply::Mention);
    let agent: Account = serde_json::from_str(
        r#"{"hash":"","admin":false,"disabled":false,"agent":true,"api_key":"sk","reply":"auto","owner":"alice"}"#,
    )
    .unwrap();
    assert!(agent.is_agent() && agent.reply == AgentReply::Auto);
    assert!(
        serde_json::from_str::<Account>(
            r#"{"hash":"","admin":false,"disabled":false,"agent":true,"reply":"always"}"#
        )
        .is_err()
    );
}
#[test]
fn help_lists_agent_commands_only_for_the_owner_or_admins() {
    let mut e = engine();
    let plain = e.run(Some("bob"), None, "/help").unwrap().reply;
    assert!(!plain.contains("/agent-key") && !plain.contains("/agent-reply"));
    e.run(Some("bob"), None, "/agent helper sk-key").unwrap();
    let owner = e.run(Some("bob"), None, "/help").unwrap().reply;
    assert!(owner.contains("/agent-key api-key [agent-name]"));
    assert!(owner.contains("/agent-reply [auto|mention] [agent-name]"));
    assert!(owner.contains("/agent-name new-name [agent-name]"));
    assert!(
        e.snapshot("bob")
            .unwrap()
            .commands
            .iter()
            .any(|c| c.name == "/agent-reply")
    );
    assert!(
        !e.snapshot("eve")
            .unwrap()
            .commands
            .iter()
            .any(|c| c.name == "/agent-reply")
    );
    assert!(
        e.snapshot("alice")
            .unwrap()
            .commands
            .iter()
            .any(|c| c.name == "/agent-key")
    );
    // The console never advertises owner-scoped agent configuration.
    assert!(!crate::commands::help(true, true).contains("/agent-key"));
}
#[test]
fn users_and_members_report_the_agent_role() {
    let mut e = agent_engine("lobby", "alice", AgentReply::Mention);
    let users = e.run(Some("alice"), None, "/users").unwrap().reply;
    assert!(users.contains("helper — agent"));
    assert!(users.contains("alice — admin"));
    assert!(users.contains("bob — user"));
    let members = e
        .run(Some("alice"), Some("lobby"), "/members")
        .unwrap()
        .reply;
    assert!(members.contains("helper — agent"));
}
#[test]
fn agent_system_prompt_states_identity_and_place() {
    let room = agent_system_prompt("helper", "room:lobby", false, "");
    assert!(room.contains("You are helper") && room.contains("#lobby"));
    assert!(
        room.contains("cannot search the web"),
        "the agent is told what it cannot do"
    );
    let searching = agent_system_prompt("helper", "room:lobby", true, "");
    assert!(
        searching.contains("can search the web") && !searching.contains("cannot search the web"),
        "the capability matches the configured setting"
    );
    let direct = agent_system_prompt("helper", "dm:bob:helper", false, "");
    assert!(direct.contains("bob and helper"));
}
