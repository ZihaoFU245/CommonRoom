use super::*;
pub(super) fn engine() -> Engine {
    let mut e = Engine::open(Path::new(":memory:")).unwrap();
    for (name, admin) in [("alice", true), ("bob", false), ("eve", false)] {
        e.provision(name, "test-hash".into(), admin, false).unwrap();
    }
    e.execute(Some("alice"), None, "/new room lobby").unwrap();
    e.execute(Some("alice"), None, "/add bob #lobby").unwrap();
    e.execute(Some("alice"), None, "/add eve #lobby").unwrap();
    e
}
pub(super) fn persisted_messages(e: &Engine, id: &str) -> VecDeque<Message> {
    let db = e.db.lock().unwrap();
    let mut query = db
        .prepare("SELECT json FROM messages WHERE conversation=?1 ORDER BY sequence")
        .unwrap();
    let rows = query.query_map([id], |r| r.get::<_, String>(0)).unwrap();
    rows.map(|row| {
        let mut message: Message = serde_json::from_str(&row.unwrap()).unwrap();
        super::history::load_reactions(&db, id, &mut message).unwrap();
        message
    })
    .collect()
}
pub(super) fn room_messages(e: &Engine, name: &str) -> VecDeque<Message> {
    persisted_messages(e, &e.data.rooms[name].id)
}
pub(super) fn private_messages(e: &Engine, key: &str) -> VecDeque<Message> {
    persisted_messages(e, &e.data.private[key].id)
}

fn age_first(e: &Engine, id: &str) {
    e.db.lock().unwrap().execute("UPDATE messages SET json=json_set(json,'$.time',?2) WHERE conversation=?1 AND sequence=(SELECT min(sequence) FROM messages WHERE conversation=?1)",params![id,now()-8*86400]).unwrap();
}
#[test]
fn cleanup_respects_age_scope_permissions_and_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    {
        let mut e = Engine::open(&path).unwrap();
        e.provision("alice", "hash".into(), true, false).unwrap();
        e.provision("bob", "hash".into(), false, false).unwrap();
        e.execute(Some("alice"), None, "/new room one").unwrap();
        e.execute(Some("alice"), None, "/new room two").unwrap();
        for room in ["one", "two"] {
            e.execute(Some("alice"), Some(room), "old").unwrap();
            age_first(&e, &e.data.rooms[room].id);
            e.execute(Some("alice"), Some(room), "new").unwrap();
        }
        e.execute(Some("alice"), None, "/tell bob old private")
            .unwrap();
        age_first(&e, &e.data.private["alice:bob"].id);
        e.execute(Some("alice"), None, "/tell bob new private")
            .unwrap();
        assert!(e.execute(Some("bob"), None, "/clean 7d @all").is_err());
        for age in ["0d", "你好", "-1d", "99999999999999999999w"] {
            assert!(
                e.execute(Some("alice"), None, &format!("/clean {age} @all"))
                    .is_err()
            );
        }
        e.execute(Some("alice"), Some("one"), "/clean 7d #one")
            .unwrap();
        assert_eq!(room_messages(&e, "one").len(), 1);
        assert_eq!(room_messages(&e, "two").len(), 2);
        assert_eq!(private_messages(&e, "alice:bob").len(), 2);
        e.execute(None, None, "/clean 7d @all").unwrap();
    }
    let e = Engine::open(&path).unwrap();
    assert_eq!(room_messages(&e, "two")[0].text, "new");
    assert_eq!(private_messages(&e, "alice:bob")[0].text, "new private");
    assert_eq!(private_messages(&e, "alice:bob").len(), 1);
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
        e.execute(Some("alice"), None, "/new room room").unwrap();
        e.execute(Some("alice"), None, "/add bob #room").unwrap();
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
    e.db.lock()
        .unwrap()
        .execute_batch("PRAGMA query_only=ON;")
        .unwrap();
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
    e.execute(Some("alice"), None, "/new room one").unwrap();
    assert!(e.execute(None, None, "/new room two").is_err());
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
        e.execute(Some("alice"), None, "/new room languages")
            .unwrap();
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
    e.execute(Some("admin"), None, "/new room lobby").unwrap();
    e.provision("alice", "hash".into(), false, false).unwrap();
    assert!(e.snapshot("alice").unwrap().rooms.is_empty());
    assert!(e.execute(Some("admin"), None, "/disable admin").is_err());
    e.execute(Some("admin"), None, "/rm room lobby").unwrap();
    assert!(e.data.rooms.is_empty());
}
#[test]
fn permissions_and_private_delivery() {
    let mut e = engine();
    assert!(e.execute(Some("bob"), None, "/new room secret").is_err());
    e.execute(Some("alice"), None, "/new room secret").unwrap();
    assert!(e.execute(Some("bob"), None, "/join #secret").is_err());
    e.execute(Some("alice"), None, "/add bob #secret").unwrap();
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
    let history = e
        .execute(Some("bob"), Some("@direct:alice"), "/history 1 100")
        .unwrap();
    assert!(history.contains("private"));
    assert!(!history.contains("other-private"));
    assert_eq!(e.snapshot("alice").unwrap().direct.len(), 1);
    assert_eq!(e.snapshot("eve").unwrap().direct.len(), 1);
    e.execute(Some("alice"), None, "/kick bob #secret").unwrap();
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
        e.execute(Some("alice"), None, "/new room lobby").unwrap();
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
    assert_eq!(room_messages(&e, "lobby").len(), 200);
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
        e.execute(None, None, "/new room one").unwrap();
        e.execute(None, None, "/new room two").unwrap();
        e.execute(None, Some("two"), "independent").unwrap();
        for n in 0..10 {
            e.execute(None, Some("one"), &format!("消息 {n} 🙂"))
                .unwrap();
        }
        let texts: Vec<_> = room_messages(&e, "one")
            .iter()
            .map(|m| m.text.clone())
            .collect();
        assert_eq!(texts, ["消息 7 🙂", "消息 8 🙂", "消息 9 🙂"]);
        assert_eq!(room_messages(&e, "two").len(), 1);
        assert!(e.execute(None, Some("one"), "/history 4").is_err());
        assert!(
            e.execute(None, Some("one"), "/history")
                .unwrap()
                .contains("消息 7")
        );
        // Each retained message has its own indexed row.
        assert_eq!(
            e.db.lock()
                .unwrap()
                .query_row::<usize, _, _>("SELECT count(*) FROM messages", [], |r| r.get(0))
                .unwrap(),
            4
        );
    }
    {
        let mut e = Engine::open(&path).unwrap();
        assert_eq!(room_messages(&e, "one").front().unwrap().text, "消息 7 🙂");
        e.set_limits(64, 64, 1).unwrap();
        assert_eq!(room_messages(&e, "one").len(), 1);
        assert_eq!(room_messages(&e, "one")[0].text, "消息 9 🙂");
        e.execute(None, Some("one"), "newest").unwrap();
        assert_eq!(room_messages(&e, "one")[0].text, "newest");
        assert!(e.set_limits(64, 64, 0).is_err());
        assert_eq!(e.max_messages, 1);
    }
    let e = Engine::open(&path).unwrap();
    assert_eq!(room_messages(&e, "one").len(), 1);
    assert_eq!(room_messages(&e, "one")[0].text, "newest");
}
#[test]
fn failed_writes_restore_evicted_messages_and_limits() {
    let mut e = engine();
    e.set_limits(64, 64, 2).unwrap();
    e.execute(Some("alice"), Some("lobby"), "oldest").unwrap();
    e.execute(Some("alice"), Some("lobby"), "latest").unwrap();
    let previous = e.data.clone();
    let revision = e.revision;
    e.db.lock()
        .unwrap()
        .execute_batch("PRAGMA query_only=ON;")
        .unwrap();
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
    let original = room_messages(&e, "lobby").back().unwrap().clone();
    assert_eq!(
        original.mentions,
        BTreeSet::from(["bob".into(), "eve".into()])
    );
    let react = format!("/react {} 好👍", original.id);
    e.execute(Some("bob"), Some("lobby"), &react).unwrap();
    e.execute(Some("alice"), Some("lobby"), &react).unwrap();
    assert_eq!(room_messages(&e, "lobby")[0].reactions["好👍"].len(), 2);
    e.execute(Some("bob"), Some("lobby"), &react).unwrap();
    assert_eq!(
        room_messages(&e, "lobby")[0].reactions["好👍"],
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
    assert!(room_messages(&e, "lobby")[0].reactions[&long_reaction].contains("bob"));
    e.execute(Some("bob"), Some("lobby"), &command).unwrap();
    assert!(
        !room_messages(&e, "lobby")[0]
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
    let reply = &room_messages(&e, "lobby")[0];
    assert_eq!(reply.reply.as_ref().unwrap().id, original.id);
    assert_eq!(reply.reply.as_ref().unwrap().text, original.text);
    assert_eq!(reply.mentions, BTreeSet::from(["alice".into()]));
    assert!(e.execute(Some("bob"), Some("lobby"), &react).is_err());
    e.execute(Some("alice"), None, "/new room secret").unwrap();
    e.execute(Some("alice"), Some("secret"), "private-room")
        .unwrap();
    let id = room_messages(&e, "secret")[0].id.clone();
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
    let dm = private_messages(&e, "alice:bob").back().unwrap().clone();
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
        private_messages(&e, "alice:bob")
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
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    {
        let mut e = Engine::open(&path).unwrap();
        e.provision("alice", "hash".into(), true, false).unwrap();
        e.execute(Some("alice"), None, "/new room room").unwrap();
        e.execute(Some("alice"), Some("room"), "@alice 中文")
            .unwrap();
        let id = room_messages(&e, "room")[0].id.clone();
        e.execute(Some("alice"), Some("room"), &format!("/reply {id} reply"))
            .unwrap();
        e.execute(Some("alice"), Some("room"), &format!("/react {id} 🙂"))
            .unwrap();
        let previous = e.data.clone();
        e.db.lock()
            .unwrap()
            .execute_batch("PRAGMA query_only=ON;")
            .unwrap();
        assert!(
            e.execute(Some("alice"), Some("room"), &format!("/react {id} 🙂"))
                .is_err()
        );
        assert!(e.data == previous);
    }
    let e = Engine::open(&path).unwrap();
    assert_eq!(
        room_messages(&e, "room")[0].reactions["🙂"],
        BTreeSet::from(["alice".into()])
    );
    assert!(room_messages(&e, "room")[1].reply.is_some());
    assert_eq!(
        e.db.lock()
            .unwrap()
            .query_row::<u32, _, _>("PRAGMA user_version", [], |r| r.get(0))
            .unwrap(),
        17
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
        e.execute(Some("alice"), None, "/new room team").unwrap();
        e.execute(Some("alice"), Some("team"), "before moving")
            .unwrap();
        e.checkpoint().unwrap();
    }
    std::fs::copy(&path, destination.path().join("chat.sqlite")).unwrap();
    let mut e = Engine::open(&destination.path().join("chat.sqlite")).unwrap();
    assert_eq!(e.session("session-token"), Some("alice".into()));
    assert!(e.snapshot("alice").unwrap().admin);
    assert_eq!(room_messages(&e, "team")[0].text, "before moving");
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
    e.db.lock()
        .unwrap()
        .execute_batch("PRAGMA user_version=99;")
        .unwrap();
    drop(e);
    assert!(Engine::open(&path).is_err());
}
#[test]
fn failed_storage_rolls_back_changes() {
    let mut e = engine();
    e.db.lock()
        .unwrap()
        .execute_batch("DROP TABLE state;")
        .unwrap();
    assert!(e.execute(Some("alice"), None, "/new room lost").is_err());
    assert!(!e.data.rooms.contains_key("lost"));
}
#[test]
fn delete_user_checks_permissions_and_cleans_references() {
    let mut e = engine();
    for (actor, command) in [
        (Some("bob"), "/rm user eve"),
        (Some("missing"), "/rm user bob"),
        (Some("alice"), "/rm user alice"),
        (Some("alice"), "/rm user nobody"),
        (Some("alice"), "/rm user"),
        (None, "/rm user bob extra"),
    ] {
        let before = e.data.clone();
        assert!(e.execute(actor, None, command).is_err());
        assert!(e.data == before);
    }
    assert!(
        !e.execute(Some("bob"), None, "/help")
            .unwrap()
            .contains("/rm user")
    );
    assert!(
        e.execute(Some("alice"), None, "/help")
            .unwrap()
            .contains("delete who")
    );
    e.execute(Some("bob"), Some("lobby"), "old room message")
        .unwrap();
    let original = room_messages(&e, "lobby")[0].id.clone();
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
    e.execute(Some("alice"), None, "/rm user bob").unwrap();
    assert!(!e.data.users.contains_key("bob"));
    assert!(e.session("bob-one").is_none() && e.session("bob-two").is_none());
    assert_eq!(e.session("alice-session").as_deref(), Some("alice"));
    assert!(!e.data.rooms["lobby"].members.contains("bob"));
    let messages = &room_messages(&e, "lobby");
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
    assert_eq!(e.db.lock().unwrap().query_row::<u64,_,_>("SELECT COUNT(*) FROM read_positions WHERE username='bob' OR conversation='dm:alice:bob'",[],|r|r.get(0)).unwrap(),0);
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
        e.execute(Some("alice"), None, "/rm user bob").unwrap();
        e.provision("eve", "hash".into(), false, false).unwrap();
        e.execute(None, None, "/rm user alice").unwrap();
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
        e.db.lock()
            .unwrap()
            .query_row("SELECT json FROM state", [], |r| r.get(0))
            .unwrap();
    e.db.lock().unwrap().execute_batch("CREATE TRIGGER fail_cleanup BEFORE DELETE ON read_positions BEGIN SELECT RAISE(ABORT,'test cleanup failure'); END;").unwrap();
    assert!(e.execute(Some("alice"), None, "/rm user bob").is_err());
    assert!(e.data == before);
    assert_eq!(e.read_positions, positions);
    assert_eq!(e.revision, revision);
    assert_eq!(e.session("bob-token").as_deref(), Some("bob"));
    assert_eq!(
        e.db.lock()
            .unwrap()
            .query_row::<String, _, _>("SELECT json FROM state", [], |r| r.get(0))
            .unwrap(),
        disk
    );
    assert_eq!(e.db.lock().unwrap().query_row::<u64,_,_>("SELECT sequence FROM read_positions WHERE username='bob' AND conversation='dm:alice:bob'",[],|r|r.get(0)).unwrap(),through);
    e.db.lock()
        .unwrap()
        .execute_batch("DROP TRIGGER fail_cleanup;")
        .unwrap();
    e.execute(Some("alice"), None, "/rm user bob").unwrap();
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
    e.execute(Some("bob"), None, "/new room promoted").unwrap();
    e.execute(Some("alice"), None, "/revoke bob").unwrap();
    assert!(e.execute(Some("bob"), None, "/new room denied").is_err());
    assert!(room_messages(&e, "lobby").is_empty());
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
        e.execute(Some("bob"), Some("lobby"), "/history 21 80")
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
    e.execute(Some("alice"), None, "/new room private").unwrap();
    assert!(e.execute(Some("bob"), Some("private"), "/history").is_err());
    assert!(
        e.execute(Some("bob"), Some("lobby"), "/members #private")
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
    assert!(e.connect("bob"));
    assert!(!e.connect("bob"));
    assert!(e.connect("eve"));
    assert_eq!(e.snapshot("alice").unwrap().online, vec!["bob", "eve"]);
    assert!(!e.disconnect("bob"));
    assert_eq!(e.snapshot("alice").unwrap().online, vec!["bob", "eve"]);
    assert!(e.disconnect("bob"));
    assert!(!e.disconnect("bob"));
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
    let id = room_messages(&e, "lobby")[0].id.clone();
    let command = format!("/retract {id}");
    // Even administrators cannot retract another author's message.
    assert!(e.execute(Some("alice"), Some("lobby"), &command).is_err());
    e.execute(Some("alice"), Some("lobby"), &format!("/reply {id} reply"))
        .unwrap();
    e.execute(Some("alice"), None, "/kick bob #lobby").unwrap();
    assert!(e.execute(Some("bob"), Some("lobby"), &command).is_err());
    e.execute(Some("alice"), None, "/add bob #lobby").unwrap();
    let revision = e.data.rooms["lobby"].revision;
    e.execute(Some("bob"), Some("lobby"), &command).unwrap();
    assert_eq!(e.data.rooms["lobby"].revision, revision + 1);
    assert_eq!(room_messages(&e, "lobby").len(), 1);
    assert!(room_messages(&e, "lobby")[0].reply.is_none());
    assert!(e.execute(Some("bob"), Some("lobby"), &command).is_err());
    assert_eq!(e.snapshot("alice").unwrap().unread["lobby"].count, 0);

    e.execute(Some("bob"), None, "/tell alice private original")
        .unwrap();
    let id = private_messages(&e, "alice:bob")[0].id.clone();
    let command = format!("/retract {id}");
    assert!(e.execute(Some("eve"), None, &command).is_err());
    assert!(e.execute(Some("alice"), None, &command).is_err());
    assert!(e.execute(Some("bob"), Some("lobby"), &command).is_err());
    e.execute(Some("alice"), None, &format!("/reply {id} reply"))
        .unwrap();
    e.execute(Some("bob"), None, &command).unwrap();
    assert_eq!(private_messages(&e, "alice:bob").len(), 1);
    assert!(private_messages(&e, "alice:bob")[0].reply.is_none());
}

#[test]
fn retract_persists_and_failed_writes_restore_message_and_quotes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    {
        let mut e = Engine::open(&path).unwrap();
        e.provision("alice", "hash".into(), true, false).unwrap();
        e.provision("bob", "hash".into(), false, false).unwrap();
        e.execute(Some("alice"), None, "/new room room").unwrap();
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
                room_messages(&e, "room")[0].id.clone()
            } else {
                private_messages(&e, "alice:bob")[0].id.clone()
            };
            e.execute(Some("alice"), room, &format!("/reply {id} reply"))
                .unwrap();
            let before = e.data.clone();
            let revision = e.revision;
            e.db.lock()
                .unwrap()
                .execute_batch("PRAGMA query_only=ON;")
                .unwrap();
            assert!(
                e.execute(Some("alice"), room, &format!("/retract {id}"))
                    .is_err()
            );
            assert!(e.data == before);
            assert_eq!(e.revision, revision);
            e.db.lock()
                .unwrap()
                .execute_batch("PRAGMA query_only=OFF;")
                .unwrap();
            e.execute(Some("alice"), room, &format!("/retract {id}"))
                .unwrap();
        }
        e.connect("alice");
    }
    let e = Engine::open(&path).unwrap();
    assert_eq!(room_messages(&e, "room").len(), 1);
    assert!(room_messages(&e, "room")[0].reply.is_none());
    assert_eq!(private_messages(&e, "alice:bob").len(), 1);
    assert!(private_messages(&e, "alice:bob")[0].reply.is_none());
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
            room_messages(&e, "lobby")[0].id.clone()
        } else {
            private_messages(&e, "alice:bob")[0].id.clone()
        };
        e.execute(Some("alice"), room, &format!("/reply {id} reply"))
            .unwrap();
        assert!(
            e.execute(Some("alice"), room, &format!("/retract {id}"))
                .is_err()
        );
        let before = e.data.clone();
        e.db.lock()
            .unwrap()
            .execute_batch("PRAGMA query_only=ON;")
            .unwrap();
        assert!(e.execute(None, None, &format!("/retract {id}")).is_err());
        assert!(e.data == before);
        e.db.lock()
            .unwrap()
            .execute_batch("PRAGMA query_only=OFF;")
            .unwrap();
        e.execute(None, None, &format!("/retract {id}")).unwrap();
        let messages = if room.is_some() {
            &room_messages(&e, "lobby")
        } else {
            &private_messages(&e, "alice:bob")
        };
        assert_eq!(messages.len(), 1);
        assert!(messages[0].reply.is_none());
        let reply_id = messages[0].id.clone();
        e.execute(Some("alice"), room, &format!("/retract {reply_id}"))
            .unwrap();
    }
    assert!(e.execute(None, None, "/retract missing").is_err());
}

#[test]
fn old_and_unversioned_databases_are_rejected_without_modification() {
    for version in [0, 1, 2, 3, 4, 5, 6, 7, 8, 15, 99] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite");
        {
            let db = Connection::open(&path).unwrap();
            db.execute_batch(&format!("CREATE TABLE state(id INTEGER PRIMARY KEY,json TEXT); PRAGMA user_version={version};")).unwrap();
            db.execute("INSERT INTO state VALUES(1,'untouched')", [])
                .unwrap();
        }
        assert!(Engine::open(&path).is_err(), "schema {version}");
        let db = Connection::open(&path).unwrap();
        assert_eq!(
            db.query_row::<u32, _, _>("PRAGMA user_version", [], |r| r.get(0))
                .unwrap(),
            version
        );
        assert_eq!(
            db.query_row::<String, _, _>("SELECT json FROM state", [], |r| r.get(0))
                .unwrap(),
            "untouched"
        );
        assert_eq!(
            db.query_row::<usize, _, _>(
                "SELECT count(*) FROM sqlite_master WHERE type='table'",
                [],
                |r| r.get(0)
            )
            .unwrap(),
            1
        );
    }
}

fn stored_data(e: &Engine) -> Data {
    let raw: String =
        e.db.lock()
            .unwrap()
            .query_row("SELECT json FROM state", [], |r| r.get(0))
            .unwrap();
    let mut data: Data = serde_json::from_str(&raw).unwrap();
    super::storage::load_entities(&e.db.lock().unwrap(), &mut data).unwrap();
    data
}

#[test]
fn hot_mutations_write_constant_rows_and_leave_unrelated_history_untouched() {
    let mut e = engine();
    e.set_limits(64, 64, 1).unwrap();
    e.execute(Some("alice"), Some("lobby"), "room retained")
        .unwrap();
    e.execute(Some("alice"), None, "/tell bob private retained")
        .unwrap();
    let room_id = e.data.rooms["lobby"].id.clone();
    e.db.lock().unwrap().execute_batch(&format!("CREATE TRIGGER untouched_room BEFORE UPDATE ON messages WHEN OLD.conversation='{room_id}' BEGIN SELECT RAISE(ABORT,'unrelated history'); END; CREATE TRIGGER untouched_state BEFORE UPDATE ON state BEGIN SELECT RAISE(ABORT,'unrelated policy'); END;")).unwrap();
    let changes = e.db.lock().unwrap().total_changes();
    e.execute(Some("alice"), None, "/tell bob new private")
        .unwrap();
    assert_eq!(e.db.lock().unwrap().total_changes() - changes, 8); // insert/evict, reverse-name references, counter, revision
    let id = private_messages(&e, "alice:bob")[0].id.clone();
    e.db.lock().unwrap().execute_batch("CREATE TRIGGER untouched_payload BEFORE UPDATE ON messages BEGIN SELECT RAISE(ABORT,'reaction rewrote payload'); END;").unwrap();
    let changes = e.db.lock().unwrap().total_changes();
    e.execute(Some("alice"), None, &format!("/react {id} 🙂"))
        .unwrap();
    assert_eq!(e.db.lock().unwrap().total_changes() - changes, 2); // membership and revision
    assert!(stored_data(&e) == e.data);
    let changes = e.db.lock().unwrap().total_changes();
    let sequence = private_messages(&e, "alice:bob")[0].sequence;
    e.mark_read("bob", "@direct:alice", sequence).unwrap();
    e.execute(Some("alice"), None, "/help").unwrap();
    assert_eq!(e.db.lock().unwrap().total_changes() - changes, 1);
}

#[test]
fn hot_transaction_failures_restore_disk_memory_sequences_and_evictions() {
    for private in [false, true] {
        for stage in ["insert", "delete", "counter", "revision", "reaction"] {
            let mut e = engine();
            e.set_limits(64, 64, 1).unwrap();
            let view = if private { None } else { Some("lobby") };
            let send = if private {
                "/tell bob retained"
            } else {
                "retained"
            };
            e.execute(Some("alice"), view, send).unwrap();
            let id = if private {
                private_messages(&e, "alice:bob")[0].id.clone()
            } else {
                room_messages(&e, "lobby")[0].id.clone()
            }
            .clone();
            let before = e.data.clone();
            let revision = e.revision;
            let trigger = match stage {
                "insert" => "BEFORE INSERT ON messages",
                "delete" => "BEFORE DELETE ON messages",
                "counter" => "BEFORE UPDATE ON counters",
                "revision" => "BEFORE UPDATE ON entities",
                _ => "BEFORE INSERT ON reactions",
            };
            e.db.lock().unwrap().execute_batch(&format!("CREATE TRIGGER reject_hot {trigger} BEGIN SELECT RAISE(ABORT,'injected failure'); END;")).unwrap();
            let input = if stage == "reaction" {
                format!("/react {id} ok")
            } else {
                send.into()
            };
            assert!(e.execute(Some("alice"), view, &input).is_err(), "{stage}");
            assert!(e.data == before, "{stage}");
            assert!(stored_data(&e) == before, "{stage}");
            assert_eq!(e.revision, revision);
        }
    }
    let mut e = engine();
    let before = e.data.clone();
    e.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_new BEFORE INSERT ON entities BEGIN SELECT RAISE(ABORT,'new chat failure'); END;").unwrap();
    assert!(e.execute(Some("alice"), None, "/tell bob first").is_err());
    assert!(e.data == before);
    assert!(stored_data(&e) == before);
}

#[test]
fn history_has_only_current_range_and_explicit_hash_room_forms() {
    let mut e = engine();
    e.set_limits(64, 64, 200).unwrap();
    for index in 1..=120 {
        e.execute(Some("bob"), Some("lobby"), &format!("history item {index}"))
            .unwrap();
    }
    for command in ["/history", "/history #lobby"] {
        let output = e.execute(Some("bob"), Some("lobby"), command).unwrap();
        assert_eq!(output.lines().count(), 100);
        assert!(output.starts_with("[21]"));
        assert!(output.contains("history item 120"));
    }
    let output = e
        .execute(Some("bob"), Some("lobby"), "/history 10 12")
        .unwrap();
    assert_eq!(output.lines().count(), 3);
    assert!(output.starts_with("[10]"));
    for input in [
        "/history lobby",
        "/history #",
        "/history 1",
        "/history 1 2 lobby",
        "/history 1 2 #lobby",
        "/history #lobby 1 2",
        "/history @direct:alice",
    ] {
        assert!(
            e.execute(Some("bob"), Some("lobby"), input).is_err(),
            "{input}"
        );
    }
    assert!(e.execute(Some("bob"), None, "/history").is_err());
    assert!(e.execute(Some("bob"), None, "/history 1 2").is_err());
    e.execute(Some("alice"), None, "/new room hidden").unwrap();
    e.execute(Some("alice"), Some("hidden"), "protected history")
        .unwrap();
    assert!(
        e.execute(Some("bob"), Some("lobby"), "/history #hidden")
            .is_err()
    );
    for permission in ["/sudo", "x:command.sudo"] {
        e.execute(None, None, &format!("/grant bob @global {permission}"))
            .unwrap();
    }
    let before = e.data.clone();
    let output = e
        .execute(Some("bob"), None, "/sudo /history #hidden")
        .unwrap();
    assert!(output.contains("protected history"));
    assert!(e.data == before);
    assert!(
        e.execute(Some("bob"), Some("lobby"), "/history #hidden")
            .is_err()
    );
    assert!(
        !e.snapshot("bob")
            .unwrap()
            .rooms
            .iter()
            .any(|room| room.name == "hidden")
    );
    assert!(
        e.execute(None, None, "/history #hidden")
            .unwrap()
            .contains("protected history")
    );
}

#[test]
fn current_schema_validation_failures_do_not_rewrite_metadata() {
    for corruption in ["cursor", "policy", "missing_metadata"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite");
        let original;
        {
            let mut e = Engine::open(&path).unwrap();
            e.provision("alice", "hash".into(), true, false).unwrap();
            match corruption {
                "cursor" => {
                    e.db.lock()
                        .unwrap()
                        .execute_batch("INSERT INTO read_positions VALUES('alice','room',-1)")
                        .unwrap();
                }
                "policy" => {
                    e.data.policy.grants.push(authorization::Grant {
                        owner: false,
                        subject: authorization::Subject::Account(e.data.users["alice"].id.clone()),
                        scope: authorization::Scope::Server,
                        permissions: BTreeSet::from(["/delete".into()]),
                    });
                    e.db.lock()
                        .unwrap()
                        .execute(
                            "UPDATE state SET json=?1",
                            params![serde_json::to_string(&e.data).unwrap()],
                        )
                        .unwrap();
                }
                "missing_metadata" => {
                    e.db.lock()
                        .unwrap()
                        .execute_batch("UPDATE state SET json=json_remove(json,'$.bots')")
                        .unwrap();
                }
                _ => {}
            }
            original =
                e.db.lock()
                    .unwrap()
                    .query_row::<String, _, _>("SELECT json FROM state", [], |r| r.get(0))
                    .unwrap();
        }
        assert!(Engine::open(&path).is_err(), "{corruption}");
        let db = Connection::open(&path).unwrap();
        assert_eq!(
            db.query_row::<String, _, _>("SELECT json FROM state", [], |r| r.get(0))
                .unwrap(),
            original
        );
        assert_eq!(
            db.query_row::<u32, _, _>("PRAGMA user_version", [], |r| r.get(0))
                .unwrap(),
            17
        );
    }
}

#[test]
fn current_schema_restart_preserves_policy_sessions_and_cursors_without_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    let previous;
    let cursors;
    {
        let mut e = Engine::open(&path).unwrap();
        e.provision("alice", "hash".into(), true, false).unwrap();
        e.execute(Some("alice"), None, "/new room team").unwrap();
        e.execute(Some("alice"), Some("team"), "preserved").unwrap();
        e.mark_read("alice", "team", 1).unwrap();
        e.login("session".into(), "alice", "hash", None).unwrap();
        previous = e.data.clone();
        cursors = e.read_positions.clone();
        e.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_state_write BEFORE UPDATE ON state BEGIN SELECT RAISE(ABORT,'startup must not rewrite state'); END;").unwrap();
    }
    let e = Engine::open(&path).unwrap();
    assert!(e.data == previous);
    assert_eq!(e.read_positions, cursors);
    assert_eq!(e.session("session").as_deref(), Some("alice"));
    assert_eq!(e.revision, 0);
    assert_eq!(room_messages(&e, "team")[0].text, "preserved");
}

#[test]
fn markdown_is_per_message_persisted_and_does_not_bypass_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    {
        let mut e = Engine::open(&path).unwrap();
        e.provision("alice", "hash".into(), true, false).unwrap();
        e.provision("bob", "hash".into(), false, false).unwrap();
        e.execute(Some("alice"), None, "/new room lobby").unwrap();
        e.execute_formatted(Some("alice"), Some("lobby"), "**bold**", true)
            .unwrap();
        let first = room_messages(&e, "lobby")[0].clone();
        assert!(first.markdown);
        assert_eq!(first.text, "**bold**");
        e.execute_formatted(
            Some("alice"),
            Some("lobby"),
            &format!("/reply {} *reply*", first.id),
            true,
        )
        .unwrap();
        e.execute(Some("alice"), Some("lobby"), "**plain**")
            .unwrap();
        e.execute_formatted(Some("alice"), None, "/tell bob **private**", true)
            .unwrap();
        let private = private_messages(&e, "alice:bob")[0].clone();
        e.execute_formatted(
            Some("bob"),
            Some("@direct:alice"),
            &format!("/reply {} *reply*", private.id),
            true,
        )
        .unwrap();
        assert!(
            e.execute_formatted(Some("bob"), Some("lobby"), "**denied**", true)
                .is_err()
        );
        // Old JSON without the optional field remains readable without a schema migration.
        let mut legacy = serde_json::to_value(&first).unwrap();
        legacy.as_object_mut().unwrap().remove("markdown");
        assert!(!serde_json::from_value::<Message>(legacy).unwrap().markdown);
    }
    let e = Engine::open(&path).unwrap();
    let messages = room_messages(&e, "lobby");
    assert_eq!(messages.len(), 3);
    assert!(messages[0].markdown);
    assert!(messages[1].markdown);
    assert!(!messages[2].markdown);
    assert!(private_messages(&e, "alice:bob").iter().all(|m| m.markdown));
    assert!(e.snapshot("bob").unwrap().direct.iter().all(|m| m.markdown));
}

#[test]
fn edits_preserve_identity_format_reactions_quotes_and_read_positions() {
    let mut e = engine();
    e.execute_formatted(Some("bob"), Some("lobby"), "**original** @eve", true)
        .unwrap();
    let original = room_messages(&e, "lobby")[0].clone();
    e.execute(
        Some("alice"),
        Some("lobby"),
        &format!("/react {} 👍", original.id),
    )
    .unwrap();
    e.execute(
        Some("alice"),
        Some("lobby"),
        &format!("/reply {} reply", original.id),
    )
    .unwrap();
    let before = room_messages(&e, "lobby");
    e.mark_read("alice", "lobby", before[1].sequence).unwrap();
    let revision = e.data.rooms["lobby"].revision;
    let sequence = e.data.next_sequence;
    let cursors = e.read_positions.clone();
    // Warm history before updating it to exercise revision-based invalidation.
    e.history("alice", "lobby").unwrap();
    e.execute(
        Some("bob"),
        Some("lobby"),
        &format!("/edit {} **你好**\n@alice @missing", original.id),
    )
    .unwrap();
    let edited = room_messages(&e, "lobby");
    assert!(edited[0].edited);
    assert_eq!(edited[0].text, "**你好**\n@alice @missing");
    assert_eq!(edited[0].id, original.id);
    assert_eq!(edited[0].sequence, original.sequence);
    assert_eq!(edited[0].time, original.time);
    assert!(edited[0].markdown);
    assert_eq!(edited[0].mentions, BTreeSet::from(["alice".into()]));
    assert_eq!(edited[0].reactions, before[0].reactions);
    assert!(edited[1] == before[1]);
    assert_eq!(e.data.next_sequence, sequence);
    assert_eq!(e.data.rooms["lobby"].revision, revision + 1);
    assert_eq!(e.read_positions, cursors);
    assert_eq!(
        e.changed_conversation.as_ref(),
        Some(&e.data.rooms["lobby"].id)
    );
    assert_eq!(
        e.history("alice", "lobby").unwrap().messages[0].text,
        edited[0].text
    );
}

#[test]
fn edits_enforce_ownership_scope_permissions_and_validation() {
    let mut e = engine();
    e.execute(Some("bob"), Some("lobby"), "original").unwrap();
    let id = room_messages(&e, "lobby")[0].id.clone();
    for actor in [Some("alice"), Some("eve")] {
        assert!(
            e.execute(actor, Some("lobby"), &format!("/edit {id} forbidden"))
                .is_err()
        );
    }
    for text in [
        format!("/edit {id}"),
        format!("/edit {id}   "),
        format!("/edit {id} {}", "界".repeat(4001)),
        "/edit missing value".into(),
    ] {
        assert!(e.execute(Some("bob"), Some("lobby"), &text).is_err());
    }
    e.execute(Some("alice"), None, "/new room other").unwrap();
    e.execute(Some("alice"), None, "/add bob #other").unwrap();
    assert!(
        e.execute(Some("bob"), Some("other"), &format!("/edit {id} forbidden"))
            .is_err()
    );
    e.execute(None, None, "/revoke bob user #lobby").unwrap();
    e.execute(None, None, "/grant bob #lobby r:message.read")
        .unwrap();
    e.execute(None, None, "/grant bob #lobby /edit").unwrap();
    assert!(
        e.execute(Some("bob"), Some("lobby"), &format!("/edit {id} forbidden"))
            .is_err()
    );
    e.execute(None, None, "/grant bob #lobby w:message.edit.own")
        .unwrap();
    e.execute(None, None, "/revoke bob #lobby /edit").unwrap();
    assert!(
        e.execute(Some("bob"), Some("lobby"), &format!("/edit {id} forbidden"))
            .is_err()
    );
    assert_eq!(room_messages(&e, "lobby")[0].text, "original");
    e.execute(None, None, "/grant bob #lobby /edit").unwrap();
    e.execute(
        Some("bob"),
        Some("lobby"),
        &format!("/edit {id} {}", "界".repeat(4000)),
    )
    .unwrap();
    e.execute(Some("bob"), None, "/rename robert").unwrap();
    e.execute(
        Some("robert"),
        Some("lobby"),
        &format!("/edit {id} renamed author"),
    )
    .unwrap();
    e.execute(Some("robert"), Some("lobby"), &format!("/retract {id}"))
        .unwrap_err();
    e.execute(None, Some("lobby"), &format!("/retract {id}"))
        .unwrap();
    assert!(
        e.execute(
            Some("robert"),
            Some("lobby"),
            &format!("/edit {id} expired")
        )
        .is_err()
    );
}

#[test]
fn private_edits_are_scoped_and_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    let id;
    {
        let mut e = Engine::open(&path).unwrap();
        for name in ["alice", "bob", "eve"] {
            e.provision(name, "hash".into(), false, false).unwrap();
        }
        e.execute(Some("bob"), None, "/tell alice original")
            .unwrap();
        e.execute(Some("bob"), None, "/tell eve other").unwrap();
        id = private_messages(&e, "alice:bob")[0].id.clone();
        assert!(
            e.execute(
                Some("bob"),
                Some("@direct:eve"),
                &format!("/edit {id} leaked")
            )
            .is_err()
        );
        assert!(
            e.execute(Some("eve"), None, &format!("/edit {id} leaked"))
                .is_err()
        );
        assert!(
            e.execute(
                Some("alice"),
                Some("@direct:bob"),
                &format!("/edit {id} forbidden")
            )
            .is_err()
        );
        e.execute(
            Some("bob"),
            Some("@direct:alice"),
            &format!("/edit {id} updated @alice @eve"),
        )
        .unwrap();
        assert_eq!(
            private_messages(&e, "alice:bob")[0].mentions,
            BTreeSet::from(["alice".into()])
        );
    }
    let e = Engine::open(&path).unwrap();
    let message = private_messages(&e, "alice:bob")[0].clone();
    assert_eq!(message.id, id);
    assert_eq!(message.text, "updated @alice @eve");
    assert!(message.edited);
}

#[test]
fn failed_edit_rolls_back_payload_and_revision() {
    let mut e = engine();
    e.execute(Some("bob"), Some("lobby"), "original").unwrap();
    let before = room_messages(&e, "lobby")[0].clone();
    let revision = e.data.rooms["lobby"].revision;
    for table in ["messages", "entities"] {
        e.db.lock().unwrap().execute_batch(&format!(
            "CREATE TRIGGER reject_edit BEFORE UPDATE ON {table} BEGIN SELECT RAISE(ABORT, 'injected edit failure'); END;"
        )).unwrap();
        assert!(
            e.execute(
                Some("bob"),
                Some("lobby"),
                &format!("/edit {} changed", before.id)
            )
            .is_err()
        );
        assert!(room_messages(&e, "lobby")[0] == before);
        assert_eq!(e.data.rooms["lobby"].revision, revision);
        assert!(e.changed_conversation.is_none());
        e.db.lock()
            .unwrap()
            .execute_batch("DROP TRIGGER reject_edit;")
            .unwrap();
    }
}

#[test]
fn unchanged_edits_do_not_mark_messages_or_write_storage() {
    let mut e = engine();
    e.execute(Some("bob"), Some("lobby"), "original").unwrap();
    e.execute(Some("bob"), None, "/tell alice private original")
        .unwrap();
    // No-op edits must succeed even when every storage write is rejected.
    e.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_noop_message BEFORE UPDATE ON messages BEGIN SELECT RAISE(ABORT, 'unexpected message write'); END; CREATE TRIGGER reject_noop_revision BEFORE UPDATE ON entities BEGIN SELECT RAISE(ABORT, 'unexpected revision write'); END;").unwrap();
    for (view, conversation, text) in [
        ("lobby", "lobby", "original"),
        ("@direct:alice", "alice:bob", "private original"),
    ] {
        let messages = if view == "lobby" {
            room_messages(&e, conversation)
        } else {
            private_messages(&e, conversation)
        };
        let original = messages[0].clone();
        let before = e.data.clone();
        let revision = e.revision;
        e.execute(
            Some("bob"),
            Some(view),
            &format!("/edit {}   {text}  ", original.id),
        )
        .unwrap();
        assert!(e.data == before);
        assert_eq!(e.revision, revision);
        assert!(e.changed_conversation.is_none());
        let messages = if view == "lobby" {
            room_messages(&e, conversation)
        } else {
            private_messages(&e, conversation)
        };
        assert!(messages[0] == original);
        assert!(!messages[0].edited);
    }
    e.db.lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_noop_message; DROP TRIGGER reject_noop_revision;")
        .unwrap();
    let id = room_messages(&e, "lobby")[0].id.clone();
    e.execute(Some("bob"), Some("lobby"), &format!("/edit {id} changed"))
        .unwrap();
    let revision = e.revision;
    e.execute(Some("bob"), Some("lobby"), &format!("/edit {id} changed"))
        .unwrap();
    assert!(room_messages(&e, "lobby")[0].edited);
    assert_eq!(e.revision, revision);
}

#[test]
fn edit_any_grants_survive_restart_and_revoke() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("chat.sqlite");
    let id;
    {
        let mut e = Engine::open(&path).unwrap();
        e.provision("alice", "hash".into(), true, false).unwrap();
        e.provision("eve", "hash".into(), false, false).unwrap();
        e.execute(Some("alice"), None, "/new room team").unwrap();
        e.execute(Some("alice"), Some("team"), "original").unwrap();
        id = room_messages(&e, "team")[0].id.clone();
        for permission in ["w:message.edit.any", "r:message.read", "/edit"] {
            e.execute(None, None, &format!("/grant eve #team {permission}"))
                .unwrap();
        }
    }
    let mut e = Engine::open(&path).unwrap();
    e.execute(
        Some("eve"),
        Some("team"),
        &format!("/edit {id} after restart"),
    )
    .unwrap();
    assert_eq!(room_messages(&e, "team")[0].from, "alice");
    e.execute(None, None, "/revoke eve #team w:message.edit.any")
        .unwrap();
    assert!(
        e.execute(Some("eve"), Some("team"), &format!("/edit {id} denied"))
            .is_err()
    );
    assert_eq!(room_messages(&e, "team")[0].text, "after restart");
}
#[test]
fn room_deletion_cleans_cursors_atomically_and_survives_name_reuse() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    let mut e = Engine::open(&path).unwrap();
    e.provision("alice", "hash".into(), true, false).unwrap();
    e.execute(Some("alice"), None, "/new room removed").unwrap();
    e.execute(Some("alice"), Some("removed"), "old history")
        .unwrap();
    let sequence = room_messages(&e, "removed")[0].sequence;
    e.mark_read("alice", "removed", sequence).unwrap();
    e.execute(Some("alice"), None, "/new room retained")
        .unwrap();
    e.execute(Some("alice"), Some("retained"), "retained history")
        .unwrap();
    let retained = room_messages(&e, "retained")[0].sequence;
    e.mark_read("alice", "retained", retained).unwrap();
    e.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_cursor_deletion BEFORE DELETE ON read_positions BEGIN SELECT RAISE(ABORT,'forced failure'); END;").unwrap();
    let before = e.data.clone();
    assert!(e.execute(Some("alice"), None, "/rm room removed").is_err());
    assert!(e.data == before);
    assert_eq!(e.read_position("alice", "room:removed"), sequence);
    e.db.lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_cursor_deletion;")
        .unwrap();
    e.execute(Some("alice"), None, "/rm room removed").unwrap();
    assert_eq!(e.read_position("alice", "room:removed"), 0);
    assert_eq!(e.read_position("alice", "room:retained"), retained);
    let rows: usize =
        e.db.lock()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM read_positions WHERE conversation='room:removed'",
                [],
                |row| row.get(0),
            )
            .unwrap();
    assert_eq!(rows, 0);
    drop(e);
    let mut e = Engine::open(&path).unwrap();
    e.execute(Some("alice"), None, "/new room removed").unwrap();
    assert_eq!(e.read_position("alice", "room:removed"), 0);
    assert_eq!(e.read_position("alice", "room:retained"), retained);
}

#[test]
fn session_only_rollback_preserves_expired_and_replaced_sessions_on_failure() {
    let mut e = engine();
    e.login("existing".into(), "bob", "test-hash", None)
        .unwrap();
    e.data.sessions.insert(
        "expired".into(),
        Session {
            username: "alice".into(),
            expires: 0,
        },
    );
    let before = e.data.clone();
    e.db.lock()
        .unwrap()
        .execute_batch("PRAGMA query_only=ON;")
        .unwrap();
    assert!(
        e.login("new".into(), "bob", "test-hash", Some("existing"))
            .is_err()
    );
    assert!(e.data == before);
    assert!(e.logout("existing").is_err());
    assert!(e.data == before);
}

#[test]
fn session_eviction_is_per_account_and_does_not_block_other_logins() {
    let mut e = engine();
    for i in 0..256 {
        e.login(format!("alice-{i}"), "alice", "test-hash", None)
            .unwrap();
    }
    assert_eq!(e.data.sessions.len(), 8);
    e.login("bob-session".into(), "bob", "test-hash", None)
        .unwrap();
    assert_eq!(e.session("bob-session").as_deref(), Some("bob"));
    assert!(e.session("alice-0").is_none());
    assert_eq!(
        e.data
            .sessions
            .values()
            .filter(|s| s.username == "alice")
            .count(),
        8
    );
}
#[test]
fn disabled_administrators_do_not_allow_removing_the_last_active_admin() {
    for command in [
        "/disable alice",
        "/revoke alice admin",
        "/rm group alice admin",
    ] {
        let mut e = engine();
        e.execute(None, None, "/grant bob admin").unwrap();
        e.execute(None, None, "/disable bob").unwrap();
        assert!(
            e.execute(Some("alice"), None, command)
                .unwrap_err()
                .contains("last active"),
            "{command}"
        );
        assert!(e.active("alice") && e.is_admin("alice"));
    }
}
#[test]
fn rename_only_rewrites_indexed_references_and_revises_affected_conversations() {
    let mut e = engine();
    e.execute(Some("alice"), None, "/new room unrelated")
        .unwrap();
    e.execute(Some("alice"), Some("unrelated"), "untouched")
        .unwrap();
    e.execute(Some("bob"), Some("lobby"), "old author").unwrap();
    let original = room_messages(&e, "lobby")[0].id.clone();
    e.execute(
        Some("alice"),
        Some("lobby"),
        &format!("/reply {original} @bob hello"),
    )
    .unwrap();
    let unrelated = e.data.rooms["unrelated"].clone();
    let lobby_revision = e.data.rooms["lobby"].revision;
    e.db.lock().unwrap().execute_batch(&format!("CREATE TRIGGER reject_unrelated BEFORE UPDATE ON messages WHEN OLD.conversation='{}' BEGIN SELECT RAISE(ABORT,'unrelated history'); END;", unrelated.id)).unwrap();
    e.execute(Some("bob"), None, "/rename robert").unwrap();
    assert_eq!(e.data.rooms["unrelated"].revision, unrelated.revision);
    assert_eq!(e.data.rooms["lobby"].revision, lobby_revision + 1);
    let messages = room_messages(&e, "lobby");
    assert_eq!(messages[0].from, "robert");
    assert_eq!(messages[1].reply.as_ref().unwrap().from, "robert");
    assert!(messages[1].mentions.contains("robert"));
    let db = e.db.lock().unwrap();
    let old: usize = db
        .query_row(
            "SELECT count(*) FROM message_accounts WHERE name='bob'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(old, 0);
    let plan: String = db
        .query_row(
            "EXPLAIN QUERY PLAN SELECT conversation FROM message_accounts WHERE name='robert'",
            [],
            |r| r.get(3),
        )
        .unwrap();
    assert!(plan.contains("SEARCH"), "{plan}");
}

#[test]
fn maintenance_writes_only_target_revision_messages_and_append_only_audit() {
    let mut e = engine();
    e.execute(Some("alice"), None, "/new room untouched")
        .unwrap();
    e.execute(Some("bob"), Some("lobby"), "remove me").unwrap();
    let message = room_messages(&e, "lobby")[0].clone();
    e.execute(
        Some("alice"),
        Some("lobby"),
        &format!("/reply {} retained reply", message.id),
    )
    .unwrap();
    let state: String =
        e.db.lock()
            .unwrap()
            .query_row("SELECT json FROM state", [], |r| r.get(0))
            .unwrap();
    e.db.lock().unwrap().execute_batch("CREATE TRIGGER no_state BEFORE UPDATE ON state BEGIN SELECT RAISE(ABORT,'state rewrite'); END; CREATE TRIGGER no_entities_insert BEFORE INSERT ON entities BEGIN SELECT RAISE(ABORT,'metadata upsert'); END; CREATE TRIGGER no_audit_update BEFORE UPDATE ON audit_entries BEGIN SELECT RAISE(ABORT,'audit rewrite'); END;").unwrap();
    e.execute(
        Some("bob"),
        Some("lobby"),
        &format!("/retract {}", message.id),
    )
    .unwrap();
    assert_eq!(room_messages(&e, "lobby").len(), 1);
    assert!(room_messages(&e, "lobby")[0].reply.is_none());
    age_first(&e, &e.data.rooms["lobby"].id);
    e.execute(None, Some("lobby"), "/clean 7d #lobby").unwrap();
    assert!(room_messages(&e, "lobby").is_empty());
    assert_eq!(
        e.db.lock()
            .unwrap()
            .query_row::<String, _, _>("SELECT json FROM state", [], |r| r.get(0))
            .unwrap(),
        state
    );
    assert!(stored_data(&e) == e.data);
}
#[test]
fn maintenance_audit_failure_restores_messages_revisions_and_memory() {
    let mut e = engine();
    e.execute(Some("bob"), Some("lobby"), "retained").unwrap();
    let message = room_messages(&e, "lobby")[0].clone();
    let before = e.data.clone();
    e.db.lock().unwrap().execute_batch("CREATE TRIGGER no_audit BEFORE INSERT ON audit_entries BEGIN SELECT RAISE(ABORT,'audit failure'); END;").unwrap();
    assert!(
        e.execute(
            Some("bob"),
            Some("lobby"),
            &format!("/retract {}", message.id)
        )
        .is_err()
    );
    assert!(e.data == before);
    assert_eq!(room_messages(&e, "lobby")[0].text, "retained");
}
#[test]
fn audit_retention_appends_without_rewriting_policy_or_retained_entries() {
    let mut e = engine();
    e.db.lock().unwrap().execute_batch("CREATE TRIGGER no_state BEFORE UPDATE ON state BEGIN SELECT RAISE(ABORT,'state rewrite'); END; CREATE TRIGGER no_audit_update BEFORE UPDATE ON audit_entries BEGIN SELECT RAISE(ABORT,'audit rewrite'); END;").unwrap();
    for i in 0..1005 {
        e.audit(None, "test", &i.to_string());
        e.save().unwrap();
    }
    assert_eq!(e.data.policy.audit.len(), 1000);
    assert_eq!(e.data.policy.audit.front().unwrap().target, "5");
    assert!(stored_data(&e) == e.data);
}
#[test]
fn expired_sessions_are_not_loaded_and_login_removes_expired_disk_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    let mut e = Engine::open(&path).unwrap();
    e.provision("alice", "hash".into(), true, false).unwrap();
    e.login("expired".into(), "alice", "hash", None).unwrap();
    e.db.lock()
        .unwrap()
        .execute(
            "UPDATE entities SET json=json_set(json,'$.expires',0) WHERE kind='session'",
            [],
        )
        .unwrap();
    drop(e);
    let mut e = Engine::open(&path).unwrap();
    assert!(e.data.sessions.is_empty());
    e.login("fresh".into(), "alice", "hash", None).unwrap();
    assert_eq!(
        e.db.lock()
            .unwrap()
            .query_row::<usize, _, _>(
                "SELECT count(*) FROM entities WHERE kind='session'",
                [],
                |r| r.get(0)
            )
            .unwrap(),
        1
    );
    assert!(e.set_limits(257, 64, 1000).is_err());
    assert!(e.set_limits(64, 1025, 1000).is_err());
    assert!(e.set_limits(64, 64, 1_000_001).is_err());
}

#[test]
fn failed_rollback_reload_requires_restart_instead_of_reusing_partial_memory() {
    let mut e = engine();
    e.db.lock()
        .unwrap()
        .execute_batch("DROP TABLE entities;")
        .unwrap();
    assert!(e.restore_command_state(e.revision, None).is_err());
    assert!(e.ensure_recovered().unwrap_err().contains("restart"));
}
