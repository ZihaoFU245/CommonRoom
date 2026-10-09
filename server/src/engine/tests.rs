use super::*;
fn engine() -> Engine {
    let mut e = Engine::open(Path::new(":memory:")).unwrap();
    for (name, admin) in [("alice", true), ("bob", false), ("eve", false)] {
        e.provision(name, "test-hash".into(), admin, false).unwrap();
    }
    e.execute(Some("alice"), None, "/new lobby").unwrap();
    e.execute(Some("alice"), None, "/add bob lobby").unwrap();
    e.execute(Some("alice"), None, "/add eve lobby").unwrap();
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
        e.execute(Some("alice"), None, "/new one").unwrap();
        e.execute(Some("alice"), None, "/new two").unwrap();
        for room in ["one", "two"] {
            e.execute(Some("alice"), Some(room), "old").unwrap();
            e.data.rooms.get_mut(room).unwrap().messages[0].time = now() - 8 * 86400;
            e.execute(Some("alice"), Some(room), "new").unwrap();
        }
        e.execute(Some("alice"), None, "/tell bob old private")
            .unwrap();
        e.data.private.get_mut("alice:bob").unwrap().messages[0].time = now() - 8 * 86400;
        e.execute(Some("alice"), None, "/tell bob new private")
            .unwrap();
        assert!(e.execute(Some("bob"), None, "/clean 7d @all").is_err());
        for age in ["0d", "你好", "-1d", "99999999999999999999w"] {
            assert!(
                e.execute(Some("alice"), None, &format!("/clean {age} @all"))
                    .is_err()
            );
        }
        e.execute(Some("alice"), Some("one"), "/clean 7d").unwrap();
        assert_eq!(e.data.rooms["one"].messages.len(), 1);
        assert_eq!(e.data.rooms["two"].messages.len(), 2);
        assert_eq!(e.data.private["alice:bob"].messages.len(), 2);
        e.execute(None, None, "/clean 7d @all").unwrap();
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
    e.execute(Some("bob"), None, "/help").unwrap();
    assert_eq!(e.revision, revision);
}
#[test]
fn private_contacts_come_only_from_the_users_retained_messages() {
    let mut e = engine();
    assert!(e.snapshot("alice").unwrap().private_peers.is_empty());
    e.execute(Some("bob"), None, "/tell alice older").unwrap();
    for _ in 0..60 {
        e.execute(Some("eve"), None, "/tell alice recent").unwrap();
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
        e.execute(Some("alice"), None, "/new room").unwrap();
        e.execute(Some("alice"), None, "/add bob room").unwrap();
        for n in 0..5 {
            e.execute(Some("alice"), None, &format!("/tell bob 私信 {n} 🙂"))
                .unwrap();
            e.execute(Some("alice"), Some("room"), &format!("chat {n}"))
                .unwrap();
        }
        e.execute(Some("alice"), None, "/tell eve independent")
            .unwrap();
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
    e.execute(Some("alice"), None, "/tell bob after restart")
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
    e.execute(Some("alice"), None, "/tell bob retained")
        .unwrap();
    let before = e.data.clone();
    let through = e.snapshot("bob").unwrap().unread["@direct:alice"].through;
    e.db.execute_batch("PRAGMA query_only=ON;").unwrap();
    assert!(e.mark_read("bob", "@direct:alice", through).is_err());
    assert_eq!(e.snapshot("bob").unwrap().unread["@direct:alice"].count, 1);
    assert!(e.execute(Some("alice"), None, "/tell bob failed").is_err());
    assert!(e.data == before);
}
#[test]
fn configurable_limits_preserve_existing_accounts_and_rooms() {
    let mut e = Engine::open(Path::new(":memory:")).unwrap();
    e.set_limits(2, 1, 1000).unwrap();
    e.provision("alice", "hash".into(), true, false).unwrap();
    e.provision("bob", "hash".into(), false, false).unwrap();
    assert!(e.provision("eve", "hash".into(), false, false).is_err());
    e.execute(Some("alice"), None, "/new one").unwrap();
    assert!(e.execute(None, None, "/new two").is_err());
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
        e.execute(Some("alice"), None, "/new languages").unwrap();
        e.execute(Some("alice"), Some("languages"), sample).unwrap();
        e.execute(Some("alice"), None, &format!("/tell bob {sample}"))
            .unwrap();
        e.execute(Some("alice"), Some("languages"), &"你".repeat(4000))
            .unwrap();
        e.execute(
            Some("alice"),
            None,
            &format!("/tell bob {}", "🙂".repeat(4000)),
        )
        .unwrap();
        assert!(
            e.execute(Some("alice"), Some("languages"), &"你".repeat(4001))
                .is_err()
        );
        assert!(
            e.execute(
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
    e.execute(Some("admin"), None, "/new lobby").unwrap();
    e.provision("alice", "hash".into(), false, false).unwrap();
    assert!(e.snapshot("alice").unwrap().rooms.is_empty());
    assert!(e.execute(Some("admin"), None, "/disable admin").is_err());
    e.execute(Some("admin"), None, "/delete lobby").unwrap();
    assert!(e.data.rooms.is_empty());
}
#[test]
fn permissions_and_private_delivery() {
    let mut e = engine();
    assert!(e.execute(Some("bob"), None, "/new secret").is_err());
    e.execute(Some("alice"), None, "/new secret").unwrap();
    assert!(e.execute(Some("bob"), None, "/join secret").is_err());
    e.execute(Some("alice"), None, "/add bob secret").unwrap();
    e.execute(Some("bob"), Some("secret"), "hello").unwrap();
    assert!(
        !e.snapshot("eve")
            .unwrap()
            .rooms
            .iter()
            .any(|r| r.name == "secret")
    );
    e.execute(Some("bob"), None, "/tell alice private").unwrap();
    e.execute(Some("bob"), None, "/tell eve other-private")
        .unwrap();
    let history = e.execute(Some("bob"), None, "/history 20 alice").unwrap();
    assert!(history.contains("private"));
    assert!(!history.contains("other-private"));
    assert_eq!(e.snapshot("alice").unwrap().direct.len(), 1);
    assert_eq!(e.snapshot("eve").unwrap().direct.len(), 1);
    e.execute(Some("alice"), None, "/kick bob secret").unwrap();
    assert!(e.execute(Some("bob"), Some("secret"), "blocked").is_err());
    assert!(e.execute(Some("eve"), None, "/disable bob").is_err());
}
#[test]
fn persists_accounts_and_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.sqlite");
    {
        let mut e = Engine::open(&path).unwrap();
        e.provision("alice", "hash".into(), true, false).unwrap();
        e.execute(Some("alice"), None, "/new lobby").unwrap();
        e.execute(Some("alice"), Some("lobby"), "retained").unwrap();
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
        e.execute(Some("bob"), Some("lobby"), "hello").unwrap();
    }
    assert_eq!(e.data.rooms["lobby"].messages.len(), 200);
    e.execute(None, None, "/disable bob").unwrap();
    assert!(e.snapshot("bob").is_none());
}
#[test]
fn room_ring_keeps_newest_messages_across_restart_and_limit_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    {
        let mut e = Engine::open(&path).unwrap();
        e.set_limits(64, 64, 3).unwrap();
        e.execute(None, None, "/new one").unwrap();
        e.execute(None, None, "/new two").unwrap();
        e.execute(None, Some("two"), "independent").unwrap();
        for n in 0..10 {
            e.execute(None, Some("one"), &format!("消息 {n} 🙂"))
                .unwrap();
        }
        let texts: Vec<_> = e.data.rooms["one"]
            .messages
            .iter()
            .map(|m| m.text.as_str())
            .collect();
        assert_eq!(texts, ["消息 7 🙂", "消息 8 🙂", "消息 9 🙂"]);
        assert_eq!(e.data.rooms["two"].messages.len(), 1);
        assert!(e.execute(None, Some("one"), "/history 4").is_err());
        assert!(
            e.execute(None, Some("one"), "/history")
                .unwrap()
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
        e.execute(None, Some("one"), "newest").unwrap();
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
    e.execute(Some("alice"), Some("lobby"), "oldest").unwrap();
    e.execute(Some("alice"), Some("lobby"), "latest").unwrap();
    let previous = e.data.clone();
    let revision = e.revision;
    e.db.execute_batch("PRAGMA query_only=ON;").unwrap();
    assert!(e.execute(Some("alice"), Some("lobby"), "unsaved").is_err());
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
    e.execute(
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
    e.execute(Some("bob"), Some("lobby"), &react).unwrap();
    e.execute(Some("alice"), Some("lobby"), &react).unwrap();
    assert_eq!(e.data.rooms["lobby"].messages[0].reactions["好👍"].len(), 2);
    e.execute(Some("bob"), Some("lobby"), &react).unwrap();
    assert_eq!(
        e.data.rooms["lobby"].messages[0].reactions["好👍"],
        BTreeSet::from(["alice".into()])
    );
    assert!(
        e.execute(
            Some("bob"),
            Some("lobby"),
            &format!("/react {} {}", original.id, "a".repeat(129))
        )
        .is_err()
    );
    assert!(
        e.execute(
            Some("bob"),
            Some("lobby"),
            &format!("/react {} bad\nreaction", original.id)
        )
        .is_err()
    );
    let long_reaction = "🙂".repeat(128);
    let command = format!("/react {} {long_reaction}", original.id);
    e.execute(Some("bob"), Some("lobby"), &command).unwrap();
    assert!(e.data.rooms["lobby"].messages[0].reactions[&long_reaction].contains("bob"));
    e.execute(Some("bob"), Some("lobby"), &command).unwrap();
    assert!(
        !e.data.rooms["lobby"].messages[0]
            .reactions
            .contains_key(&long_reaction)
    );
    e.set_limits(64, 64, 1).unwrap();
    e.execute(
        Some("bob"),
        Some("lobby"),
        &format!("/reply {} @alice 回答", original.id),
    )
    .unwrap();
    let reply = &e.data.rooms["lobby"].messages[0];
    assert_eq!(reply.reply.as_ref().unwrap().id, original.id);
    assert_eq!(reply.reply.as_ref().unwrap().text, original.text);
    assert_eq!(reply.mentions, BTreeSet::from(["alice".into()]));
    assert!(e.execute(Some("bob"), Some("lobby"), &react).is_err());
    e.execute(Some("alice"), None, "/new secret").unwrap();
    e.execute(Some("alice"), Some("secret"), "private-room")
        .unwrap();
    let id = e.data.rooms["secret"].messages[0].id.clone();
    for action in ["react", "reply"] {
        assert!(
            e.execute(Some("eve"), Some("secret"), &format!("/{action} {id} nope"))
                .is_err()
        );
        assert!(
            e.execute(Some("eve"), Some("lobby"), &format!("/{action} {id} nope"))
                .is_err()
        );
    }
    e.execute(Some("alice"), None, "/tell bob @bob @eve secret DM")
        .unwrap();
    let dm = e.data.private["alice:bob"].messages.back().unwrap().clone();
    assert_eq!(dm.mentions, BTreeSet::from(["bob".into()]));
    assert!(
        e.execute(Some("eve"), None, &format!("/react {} 👍", dm.id))
            .is_err()
    );
    assert!(
        e.execute(Some("eve"), None, &format!("/reply {} stolen", dm.id))
            .is_err()
    );
    e.execute(Some("bob"), None, &format!("/reply {} @alice 好", dm.id))
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
fn schema_one_migration_keeps_accounts_history_and_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    {
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE state(id INTEGER PRIMARY KEY, json TEXT NOT NULL); PRAGMA user_version=1;").unwrap();
        let legacy = serde_json::json!({
            "users":{"alice":{"hash":"hash", "admin":true, "disabled":false}},
            "rooms":{"room":{"members":["alice"], "messages":[{"id":"old", "from":"alice", "to":null, "text":"旧消息", "time":1}]}},
            "direct":[], "sessions":{"token":{"username":"alice", "expires":now()+43200}}
        });
        db.execute(
            "INSERT INTO state(id,json) VALUES(1,?1)",
            params![legacy.to_string()],
        )
        .unwrap();
    }
    let e = Engine::open(&path).unwrap();
    assert!(e.is_admin("alice"));
    assert_eq!(e.session("token"), Some("alice".into()));
    assert_eq!(e.data.rooms["room"].messages[0].text, "旧消息");
    assert!(e.data.rooms["room"].messages[0].reactions.is_empty());
    assert!(e.data.rooms["room"].messages[0].reply.is_none());
}
#[test]
fn schema_two_migrates_private_pairs_without_losing_features() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("chat.sqlite");
    {
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE state(id INTEGER PRIMARY KEY, json TEXT NOT NULL); PRAGMA user_version=2;").unwrap();
        let account = serde_json::json!({"hash":"hash","admin":false,"disabled":false});
        let legacy = serde_json::json!({
            "users":{"alice":account,"bob":account,"eve":account}, "rooms":{},
            "direct":[
                {"id":"dm1","from":"alice","to":"bob","text":"你好","time":1,"reactions":{"🙂":["bob"]},"mentions":["bob"]},
                {"id":"dm2","from":"bob","to":"alice","text":"reply","time":2,"reply":{"id":"dm1","from":"alice","text":"你好"}},
                {"id":"dm3","from":"alice","to":"eve","text":"independent","time":3}
            ],"sessions":{"token":{"username":"bob","expires":now()+43200}}
        });
        db.execute(
            "INSERT INTO state VALUES(1,?1)",
            params![legacy.to_string()],
        )
        .unwrap();
    }
    {
        let mut e = Engine::open(&path).unwrap();
        assert!(e.data.direct.is_empty());
        assert_eq!(e.session("token").as_deref(), Some("bob"));
        let messages = e.history("bob", "@direct:alice").unwrap().messages;
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].id, "dm1");
        assert!(messages[0].reactions["🙂"].contains("bob"));
        assert!(messages[0].mentions.contains("bob"));
        assert_eq!(messages[1].reply.as_ref().unwrap().id, "dm1");
        assert!(messages[0].sequence < messages[1].sequence);
        assert_eq!(e.snapshot("bob").unwrap().unread["@direct:alice"].count, 0);
        e.execute(Some("alice"), None, "/tell bob new").unwrap();
        assert_eq!(e.snapshot("bob").unwrap().unread["@direct:alice"].count, 1);
    }
    let e = Engine::open(&path).unwrap();
    assert_eq!(e.history("bob", "@direct:alice").unwrap().messages.len(), 3);
    assert_eq!(e.snapshot("bob").unwrap().unread["@direct:alice"].count, 1);
    assert_eq!(
        e.db.query_row::<u32, _, _>("PRAGMA user_version", [], |r| r.get(0))
            .unwrap(),
        3
    );
}
#[test]
fn message_features_migrate_persist_and_rollback() {
    let legacy: Message =
        serde_json::from_str(r#"{"id":"old","from":"alice","to":null,"text":"legacy","time":1}"#)
            .unwrap();
    assert!(legacy.reply.is_none() && legacy.mentions.is_empty() && legacy.reactions.is_empty());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    {
        let mut e = Engine::open(&path).unwrap();
        e.provision("alice", "hash".into(), true, false).unwrap();
        e.execute(Some("alice"), None, "/new room").unwrap();
        e.execute(Some("alice"), Some("room"), "@alice 中文")
            .unwrap();
        let id = e.data.rooms["room"].messages[0].id.clone();
        e.execute(Some("alice"), Some("room"), &format!("/reply {id} reply"))
            .unwrap();
        e.execute(Some("alice"), Some("room"), &format!("/react {id} 🙂"))
            .unwrap();
        let previous = e.data.clone();
        e.db.execute_batch("PRAGMA query_only=ON;").unwrap();
        assert!(
            e.execute(Some("alice"), Some("room"), &format!("/react {id} 🙂"))
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
        3
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
        e.execute(Some("alice"), None, "/new team").unwrap();
        e.execute(Some("alice"), Some("team"), "before migration")
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
    assert!(e.execute(Some("alice"), None, "/new lost").is_err());
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
        assert!(e.execute(actor, None, command).is_err());
        assert!(e.data == before);
    }
    assert!(
        !e.execute(Some("bob"), None, "/help")
            .unwrap()
            .contains("/deleteuser")
    );
    assert!(
        e.execute(Some("alice"), None, "/help")
            .unwrap()
            .contains("/deleteuser user")
    );
    e.execute(Some("bob"), Some("lobby"), "old room message")
        .unwrap();
    let original = e.data.rooms["lobby"].messages[0].id.clone();
    e.execute(
        Some("alice"),
        Some("lobby"),
        &format!("/reply {original} @bob reply"),
    )
    .unwrap();
    for (user, reaction) in [("bob", "👍"), ("alice", "👍"), ("bob", "😎")] {
        e.execute(
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
        e.execute(Some(user), None, text).unwrap();
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
    e.execute(Some("alice"), None, "/deleteuser bob").unwrap();
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
        e.execute(None, None, "/disable bob").unwrap();
        e.execute(Some("alice"), None, "/deleteuser bob").unwrap();
        e.provision("eve", "hash".into(), false, false).unwrap();
        e.execute(None, None, "/deleteuser alice").unwrap();
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
    e.execute(Some("alice"), None, "/tell bob retained")
        .unwrap();
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
    assert!(e.execute(Some("alice"), None, "/deleteuser bob").is_err());
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
    e.execute(Some("alice"), None, "/deleteuser bob").unwrap();
    assert!(e.session("bob-token").is_none());
}
#[test]
fn role_commands_are_authorized_and_not_chat_messages() {
    let mut e = engine();
    assert_eq!(
        e.execute(Some("bob"), None, "/whoami").unwrap(),
        "Name: bob\nPermission: user"
    );
    assert!(e.execute(Some("bob"), None, "/grant bob").is_err());
    assert!(e.execute(Some("alice"), None, "/revoke alice").is_err());
    let help = e.execute(Some("bob"), None, "/help").unwrap();
    assert!(help.lines().count() >= 10);
    assert!(!help.contains("/grant"));
    assert!(!help.contains("/reset"));
    e.execute(Some("alice"), None, "/grant bob").unwrap();
    assert!(e.snapshot("bob").unwrap().admin);
    assert!(
        e.snapshot("bob")
            .unwrap()
            .commands
            .iter()
            .any(|c| c.name == "/grant")
    );
    assert!(
        e.execute(Some("bob"), None, "/whoami")
            .unwrap()
            .contains("Permission: admin")
    );
    e.execute(Some("bob"), None, "/new promoted").unwrap();
    e.execute(Some("alice"), None, "/revoke bob").unwrap();
    assert!(e.execute(Some("bob"), None, "/new denied").is_err());
    assert!(e.data.rooms["lobby"].messages.is_empty());
    assert!(e.execute(Some("alice"), None, "/grant missing").is_err());
}
#[test]
fn history_tail_members_and_account_recovery() {
    let mut e = engine();
    for i in 0..80 {
        e.execute(Some("bob"), Some("lobby"), &format!("message-{i}"))
            .unwrap();
    }
    let snapshot = e.snapshot("bob").unwrap();
    assert_eq!(snapshot.rooms[0].messages.len(), 50);
    assert_eq!(snapshot.rooms[0].messages[0].text, "message-30");
    assert_eq!(
        e.execute(Some("bob"), Some("lobby"), "/history 60")
            .unwrap()
            .lines()
            .count(),
        60
    );
    assert!(
        e.execute(Some("bob"), Some("lobby"), "/history 1001")
            .is_err()
    );
    assert!(
        e.execute(Some("bob"), Some("lobby"), "/members")
            .unwrap()
            .contains("alice — admin")
    );
    e.execute(Some("alice"), None, "/new private").unwrap();
    assert!(e.execute(Some("bob"), Some("private"), "/history").is_err());
    assert!(
        e.execute(Some("bob"), Some("lobby"), "/members private")
            .is_err()
    );
    e.execute(Some("bob"), None, "/tell   alice   spaced text")
        .unwrap();
    assert_eq!(e.snapshot("alice").unwrap().direct[0].text, "spaced text");
    e.execute(None, None, "/disable bob").unwrap();
    assert!(e.execute(Some("eve"), None, "/enable bob").is_err());
    e.execute(Some("alice"), None, "/enable bob").unwrap();
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
        e.execute(Some("alice"), None, "/grant bob").unwrap();
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
    e.execute(Some("alice"), None, "/disable eve").unwrap();
    assert!(e.snapshot("alice").unwrap().online.is_empty());
}

#[test]
fn retract_enforces_ownership_membership_and_private_visibility() {
    let mut e = engine();
    e.execute(Some("bob"), Some("lobby"), "room original")
        .unwrap();
    let id = e.data.rooms["lobby"].messages[0].id.clone();
    let command = format!("/retract {id}");
    // Even administrators cannot retract another author's message.
    assert!(e.execute(Some("alice"), Some("lobby"), &command).is_err());
    e.execute(Some("alice"), Some("lobby"), &format!("/reply {id} reply"))
        .unwrap();
    e.execute(Some("alice"), None, "/kick bob lobby").unwrap();
    assert!(e.execute(Some("bob"), Some("lobby"), &command).is_err());
    e.execute(Some("alice"), None, "/add bob lobby").unwrap();
    let revision = e.data.rooms["lobby"].revision;
    e.execute(Some("bob"), Some("lobby"), &command).unwrap();
    assert_eq!(e.data.rooms["lobby"].revision, revision + 1);
    assert_eq!(e.data.rooms["lobby"].messages.len(), 1);
    assert!(e.data.rooms["lobby"].messages[0].reply.is_none());
    assert!(e.execute(Some("bob"), Some("lobby"), &command).is_err());
    assert_eq!(e.snapshot("alice").unwrap().unread["lobby"].count, 0);

    e.execute(Some("bob"), None, "/tell alice private original")
        .unwrap();
    let id = e.data.private["alice:bob"].messages[0].id.clone();
    let command = format!("/retract {id}");
    assert!(e.execute(Some("eve"), None, &command).is_err());
    assert!(e.execute(Some("alice"), None, &command).is_err());
    assert!(e.execute(Some("bob"), Some("lobby"), &command).is_err());
    e.execute(Some("alice"), None, &format!("/reply {id} reply"))
        .unwrap();
    e.execute(Some("bob"), None, &command).unwrap();
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
        e.execute(Some("alice"), None, "/new room").unwrap();
        for room in [Some("room"), None] {
            e.execute(
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
            e.execute(Some("alice"), room, &format!("/reply {id} reply"))
                .unwrap();
            let before = e.data.clone();
            let revision = e.revision;
            e.db.execute_batch("PRAGMA query_only=ON;").unwrap();
            assert!(
                e.execute(Some("alice"), room, &format!("/retract {id}"))
                    .is_err()
            );
            assert!(e.data == before);
            assert_eq!(e.revision, revision);
            e.db.execute_batch("PRAGMA query_only=OFF;").unwrap();
            e.execute(Some("alice"), room, &format!("/retract {id}"))
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
