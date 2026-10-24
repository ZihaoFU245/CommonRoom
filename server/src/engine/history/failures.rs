#[test]
fn one_corrupt_conversation_does_not_abort_healthy_snapshot_data() {
    let mut e = super::super::tests::engine();
    e.execute(Some("alice"), None, "/new room broken").unwrap();
    e.execute(Some("alice"), Some("broken"), "bad record")
        .unwrap();
    e.execute(Some("alice"), Some("lobby"), "healthy record")
        .unwrap();
    let id = e.data.rooms["broken"].id.clone();
    e.store
        .db
        .lock()
        .unwrap()
        .execute(
            "UPDATE messages SET json=json_set(json,'$.time','invalid') WHERE conversation=?1",
            [&id],
        )
        .unwrap();
    let (snapshot, complete) = e.snapshot_baseline("alice").unwrap().unwrap();
    assert!(!complete);
    assert!(
        snapshot
            .rooms
            .iter()
            .find(|r| r.name == "broken")
            .unwrap()
            .messages
            .is_empty()
    );
    assert_eq!(
        snapshot
            .rooms
            .iter()
            .find(|r| r.name == "lobby")
            .unwrap()
            .messages[0]
            .text,
        "healthy record"
    );
    assert!(snapshot.unread.contains_key("lobby"));
    assert!(snapshot.unread.contains_key("broken"));
    // An unavailable unread store still preserves the authorized directory.
    let e = super::super::tests::engine();
    e.store
        .db
        .lock()
        .unwrap()
        .execute_batch("DROP TABLE messages;")
        .unwrap();
    let snapshot = e.try_snapshot("alice").unwrap().unwrap();
    assert_eq!(snapshot.rooms.len(), 1);
    assert!(snapshot.unread.is_empty());
    assert!(e.unreads("alice").is_err());
}
#[test]
fn poisoned_history_locks_fail_explicitly_and_are_never_reused() {
    for lock in 0..3 {
        let e = super::super::tests::engine();
        let store = e.history_store().clone();
        assert!(
            std::thread::spawn(move || {
                match lock {
                    0 => {
                        let _guard = store.cache[0].lock().unwrap();
                        panic!("poison page cache");
                    }
                    1 => {
                        let _guard = store.unread_cache[0].lock().unwrap();
                        panic!("poison unread cache");
                    }
                    _ => {
                        let _guard = store.readers[0].lock().unwrap();
                        panic!("poison reader");
                    }
                }
            })
            .join()
            .is_err()
        );
        let error = e.try_snapshot("alice").err().unwrap();
        assert!(error.contains("restart the service"));
        assert!(
            e.history_store()
                .stats()
                .err()
                .unwrap()
                .contains("restart the service")
        );
        assert!(
            e.history("alice", "lobby")
                .err()
                .unwrap()
                .contains("restart the service")
        );
        assert!(
            e.unreads("alice")
                .err()
                .unwrap()
                .contains("restart the service")
        );
        assert!(e.history_store().ensure_available().is_err());
    }
}
