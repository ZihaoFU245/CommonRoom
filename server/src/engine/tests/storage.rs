//! Storage guarantees shared by mutations, receipt transactions and recovery.
use super::*;

fn assert_invariants(e: &Engine) {
    assert!(e.working.is_none());
    let db = e.store.db.lock().unwrap();
    assert_eq!(
        db.query_row::<String, _, _>("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap(),
        "ok"
    );
    assert!(
        !db.prepare("PRAGMA foreign_key_check")
            .unwrap()
            .exists([])
            .unwrap()
    );
    let bad_retention: usize = db.query_row("SELECT count(*) FROM (SELECT conversation,count(*) AS n,min(sequence) AS first FROM messages GROUP BY conversation) AS m LEFT JOIN retention AS r ON r.conversation=m.conversation WHERE r.count IS NULL OR r.count!=m.n OR r.oldest!=m.first", [], |r| r.get(0)).unwrap();
    assert_eq!(bad_retention, 0);
    assert_eq!(db.query_row::<usize, _, _>("SELECT count(*) FROM retention r WHERE NOT EXISTS(SELECT 1 FROM messages m WHERE m.conversation=r.conversation)", [], |r| r.get(0)).unwrap(), 0);
    // Derive expected account references independently from decoded message rows.
    let rows = db
        .prepare("SELECT conversation,sequence,json FROM messages")
        .unwrap()
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, u64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let mut expected = BTreeSet::new();
    let mut expected_media = BTreeSet::new();
    for (conversation, sequence, raw) in rows {
        let m: Message = serde_json::from_str(&raw).unwrap();
        for id in media::media_references(&m.text) {
            expected_media.insert((conversation.clone(), sequence, id));
        }
        let names = std::iter::once(&m.from)
            .chain(m.to.iter())
            .chain(m.reply.iter().map(|r| &r.from))
            .chain(m.mentions.iter());
        for name in names {
            expected.insert((name.clone(), conversation.clone(), sequence));
        }
    }
    let actual = db
        .prepare("SELECT name,conversation,sequence FROM message_accounts")
        .unwrap()
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, u64>(2)?,
            ))
        })
        .unwrap()
        .collect::<Result<BTreeSet<_>, _>>()
        .unwrap();
    assert_eq!(actual, expected);
    let actual_media = db
        .prepare("SELECT conversation,sequence,media_id FROM message_media")
        .unwrap()
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, u64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .collect::<Result<BTreeSet<_>, _>>()
        .unwrap();
    assert_eq!(actual_media, expected_media);
}

#[test]
fn mixed_mutations_preserve_database_indexes_references_and_retention() {
    let mut e = engine();
    e.set_limits(64, 64, 7).unwrap();
    let room = e.data.rooms["lobby"].id.clone();
    let mut attachment = e.allocate_media("alice", &room, "file.png", 8).unwrap();
    attachment.ready = true;
    attachment.object_key = attachment.final_key();
    attachment.content_type = "image/png".into();
    e.finish_media("alice", &attachment).unwrap();
    e.execute(
        Some("alice"),
        Some("lobby"),
        &format!(
            "![file](https://example.com/api/media/{}/file)",
            attachment.id
        ),
    )
    .unwrap();
    assert_invariants(&e);
    let mut seed = 17_u64;
    for n in 0..80 {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let messages = room_messages(&e, "lobby");
        let first = messages.front().map(|m| m.id.clone());
        match (seed % 6, first) {
            (0, Some(id)) => {
                e.execute(None, Some("lobby"), &format!("/retract {id}"))
                    .unwrap();
            }
            (1, Some(id)) => {
                e.execute(None, Some("lobby"), &format!("/edit {id} edited @bob {n}"))
                    .unwrap();
            }
            (2, Some(id)) => {
                e.execute(Some("bob"), Some("lobby"), &format!("/react {id} ok"))
                    .unwrap();
            }
            (3, _) => {
                e.execute(Some("alice"), None, &format!("/tell bob private {n}"))
                    .unwrap();
            }
            _ => {
                e.execute(Some("alice"), Some("lobby"), &format!("message @bob {n}"))
                    .unwrap();
            }
        }
        assert_invariants(&e);
    }
    e.execute(Some("bob"), None, "/user rename robert").unwrap();
    assert_invariants(&e);
    e.execute(None, None, "/user delete robert").unwrap();
    assert_invariants(&e);
    e.execute(None, None, "/clean -1 #lobby").unwrap();
    assert_invariants(&e);
}

#[test]
fn attachment_free_inserts_edits_and_identity_rewrites_do_not_read_media_references() {
    let mut e = engine();
    // Deliberately remove the reference table: any SELECT or DML against it fails.
    e.store
        .db
        .lock()
        .unwrap()
        .execute_batch("DROP TABLE message_media;")
        .unwrap();
    e.execute(Some("alice"), Some("lobby"), "plain").unwrap();
    let id = room_messages(&e, "lobby")[0].id.clone();
    e.execute(
        Some("alice"),
        Some("lobby"),
        &format!("/edit {id} plain edited"),
    )
    .unwrap();
    e.execute(Some("alice"), None, "/user rename alicia")
        .unwrap();
    assert_eq!(room_messages(&e, "lobby")[0].from, "alicia");
    assert!(e.working.is_none());
}

#[test]
fn competing_writer_failure_rolls_back_and_next_mutation_recovers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    let mut e = Engine::open(&path).unwrap();
    e.execute(None, None, "/room new room").unwrap();
    e.execute(None, Some("room"), "retained").unwrap();
    e.store
        .db
        .lock()
        .unwrap()
        .busy_timeout(std::time::Duration::from_millis(20))
        .unwrap();
    let mut competitor = Connection::open(&path).unwrap();
    let transaction = competitor
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let before = e.data.clone();
    let revision = e.revision;
    assert!(matches!(
        e.execute(None, Some("room"), "blocked"),
        Err(EngineError::Unavailable(_))
    ));
    assert!(e.data == before);
    assert_eq!(e.revision, revision);
    assert!(e.working.is_none());
    transaction.rollback().unwrap();
    e.execute(None, Some("room"), "recovered").unwrap();
    assert_eq!(room_messages(&e, "room").len(), 2);
    assert_invariants(&e);
}

// This test is also the subprocess entry point; normal suite runs do no child work.
#[test]
fn crash_writer_child() {
    let Some(folder) = std::env::var_os("CHATROOM_CRASH_TEST_DIR") else {
        return;
    };
    let folder = std::path::PathBuf::from(folder);
    let mut e = Engine::open(&folder.join("chat.sqlite")).unwrap();
    let account = e.data.users["alice"].id.clone();
    e.store
        .db
        .lock()
        .unwrap()
        .execute_batch("SAVEPOINT crash_outer;")
        .unwrap();
    e.bot_command_once(&account, "crash-key", "digest", |e| {
        e.execute(Some("alice"), Some("room"), "durable effect")
            .map_err(crate::bots::error::Error::storage)?;
        e.advance_read_position("alice", "room:room", 2)
            .map_err(crate::bots::error::Error::storage)?;
        e.login("crash-session".into(), "alice", "hash", None)
            .map_err(crate::bots::error::Error::storage)?;
        Ok("complete".into())
    })
    .unwrap();
    if std::env::var_os("CHATROOM_CRASH_TEST_COMMIT").is_some() {
        e.store
            .db
            .lock()
            .unwrap()
            .execute_batch("RELEASE crash_outer;")
            .unwrap();
    }
    std::fs::write(folder.join("ready"), b"ready").unwrap();
    loop {
        std::thread::park();
    }
}

#[test]
fn abrupt_exit_recovers_effect_receipt_session_and_cursor_atomically() {
    for commit in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite");
        {
            let mut e = Engine::open(&path).unwrap();
            e.provision("alice", "hash".into(), true, false).unwrap();
            e.execute(Some("alice"), None, "/room new room").unwrap();
            e.execute(Some("alice"), Some("room"), "baseline").unwrap();
            e.mark_read("alice", "room", 1).unwrap();
        }
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "engine::tests::storage::crash_writer_child",
                "--nocapture",
            ])
            .env("CHATROOM_CRASH_TEST_DIR", dir.path())
            .env_remove("CHATROOM_CRASH_TEST_COMMIT")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit());
        if commit {
            command.env("CHATROOM_CRASH_TEST_COMMIT", "1");
        }
        let mut child = command.spawn().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !dir.path().join("ready").exists()
            && std::time::Instant::now() < deadline
            && child.try_wait().unwrap().is_none()
        {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let ready = dir.path().join("ready").exists();
        let killed = child.kill();
        let status = child.wait().unwrap();
        assert!(
            ready,
            "child never reached its transaction boundary: {status}"
        );
        killed.unwrap();
        let e = Engine::open(&path).unwrap();
        let expected = if commit { 2 } else { 1 };
        assert_eq!(room_messages(&e, "room").len(), expected);
        assert_eq!(
            e.read_position("alice", "room:room"),
            u64::try_from(expected).unwrap()
        );
        assert_eq!(e.data.next_sequence, u64::try_from(expected).unwrap());
        assert_eq!(
            e.data.rooms["room"].revision,
            u64::try_from(expected).unwrap()
        );
        assert_eq!(e.session("crash-session").is_some(), commit);
        assert_eq!(
            e.bot_command_receipt(&e.data.users["alice"].id, "crash-key")
                .unwrap()
                .is_some(),
            commit
        );
        assert_invariants(&e);
    }
}
