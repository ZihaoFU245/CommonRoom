//! Native dispatcher guarantees, independent of transport or the engine lock.
use super::*;
use crate::engine::{
    Engine,
    database::{CursorAdvance, MessageMutation, params},
};
use std::time::{Duration, Instant};

#[test]
fn sqlite_has_one_writer_and_cancelled_acquisition_releases_capacity() {
    let engine = Engine::open(Path::new(":memory:")).unwrap();
    let database = &engine.store.db;
    let Pool::Sqlite(pool) = &database.pool else {
        panic!("expected SQLite");
    };
    assert_eq!(pool.options().get_max_connections(), 1);
    runtime::run(async {
        let first = database.acquire().await?;
        assert!(
            tokio::time::timeout(Duration::from_millis(30), database.acquire())
                .await
                .is_err()
        );
        drop(first);
        let second = database.acquire().await?;
        drop(second);
        Ok(())
    })
    .unwrap();
}

#[test]
fn sqlite_media_reads_use_readers_and_command_reads_keep_their_transaction() {
    let directory = tempfile::tempdir().unwrap();
    let mut engine = Engine::open(&directory.path().join("chat.sqlite")).unwrap();
    engine
        .provision("alice", "hash".into(), true, false)
        .unwrap();
    engine
        .execute(Some("alice"), None, "/room new files")
        .unwrap();
    let mut media = engine
        .allocate_media("alice", &engine.data.rooms["files"].id, "file", 8)
        .unwrap();
    let command = engine.store.db.command_scope_blocking().unwrap();
    command.lock().unwrap().begin_command().unwrap();
    media.ready = true;
    media.object_key = media.final_key();
    runtime::run(async {
        command
            .media_write(&super::super::MediaWrite::Ready {
                media: &media,
                at: crate::engine::now(),
            })
            .await?;
        let ordinary = tokio::time::timeout(
            Duration::from_millis(100),
            engine.store.db.stored_media(&media.id),
        )
        .await
        .map_err(|_| Error::Conversion("media read waited for the SQLite writer"))??;
        assert!(
            !ordinary.0.ready,
            "ordinary reader saw an uncommitted media update"
        );
        assert!(command.stored_media(&media.id).await?.0.ready);
        Ok(())
    })
    .unwrap();
    drop(command);
    assert!(
        !engine
            .store
            .db
            .stored_media_blocking(&media.id)
            .unwrap()
            .0
            .ready
    );
}

#[test]
fn native_message_rejects_stale_revision_without_overwriting_the_commit() {
    let mut engine = crate::engine::tests::engine();
    engine
        .execute(Some("alice"), Some("lobby"), "original")
        .unwrap();
    let mut message = engine.history("alice", "lobby").unwrap().messages[0].clone();
    message.text = "committed edit".into();
    message.edited = true;
    let room = &engine.data.rooms["lobby"];
    let request = MessageWrite {
        conversation: &room.id,
        kind: "room",
        key: "lobby",
        revision: room.revision + 1,
        expected_revision: Some(room.revision),
        message: &message,
        mutation: MessageMutation::Edit {
            actor: "test-editor",
            reconcile_media: false,
        },
        new_conversation: None,
        receipt: None,
        at: crate::engine::now(),
    };
    engine.store.db.save_message_blocking(&request).unwrap();
    let mut stale = message.clone();
    stale.text = "stale overwrite".into();
    let request = MessageWrite {
        message: &stale,
        ..request
    };
    assert!(matches!(
        engine.store.db.save_message_blocking(&request),
        Err(Error::RevisionConflict)
    ));
    let json: String = engine
        .store
        .db
        .lock()
        .unwrap()
        .query_row("SELECT json FROM messages", [], |row| row.get(0))
        .unwrap();
    assert!(json.contains("committed edit"));
    assert!(!json.contains("stale overwrite"));
}

#[test]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
fn postgres_message_writes_progress_on_independent_connections_and_rows() {
    let fixture = super::super::tests::PostgresFixture::new();
    let mut engine = Engine::open_postgres(&fixture.url).unwrap();
    engine
        .provision("alice", "hash".into(), true, false)
        .unwrap();
    for room in ["one", "two"] {
        engine
            .execute(Some("alice"), None, &format!("/new room {room}"))
            .unwrap();
        engine
            .execute(Some("alice"), Some(room), "original")
            .unwrap();
    }
    let Pool::Postgres(pool) = &engine.store.db.pool else {
        panic!("expected PostgreSQL");
    };
    assert_eq!(pool.options().get_max_connections(), 8);
    let make_edit = |room: &str| {
        let mut message = engine.history("alice", room).unwrap().messages[0].clone();
        message.text = "independent edit".into();
        message.edited = true;
        let room = &engine.data.rooms[room];
        (room.id.clone(), room.revision, message)
    };
    let first = make_edit("one");
    let second = make_edit("two");
    let blocker = Connection::postgres(&fixture.url, false).unwrap();
    let hold = blocker.unchecked_transaction().unwrap();
    hold.query_row(
        "SELECT revision FROM entities WHERE kind='room' AND key='one' FOR UPDATE",
        [],
        |row| row.get::<_, i64>(0),
    )
    .unwrap();
    let database = engine.store.db.clone();
    let first_job = std::thread::spawn(move || {
        database.save_message_blocking(&MessageWrite {
            conversation: &first.0,
            kind: "room",
            key: "one",
            revision: first.1 + 1,
            expected_revision: Some(first.1),
            message: &first.2,
            mutation: MessageMutation::Edit {
                actor: "test-editor",
                reconcile_media: false,
            },
            new_conversation: None,
            receipt: None,
            at: crate::engine::now(),
        })
    });
    // Confirm the first request reached PostgreSQL and is waiting on the held
    // row before checking whether another write can finish.
    let inspect = Connection::postgres(&fixture.url, false).unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let waiting: bool = inspect.query_row("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT revision,%FOR UPDATE')", [], |row| row.get(0)).unwrap();
        if waiting {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "first write never reached its row lock"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    engine
        .store
        .db
        .save_message_blocking(&MessageWrite {
            conversation: &second.0,
            kind: "room",
            key: "two",
            revision: second.1 + 1,
            expected_revision: Some(second.1),
            message: &second.2,
            mutation: MessageMutation::Edit {
                actor: "test-editor",
                reconcile_media: false,
            },
            new_conversation: None,
            receipt: None,
            at: crate::engine::now(),
        })
        .unwrap();
    assert!(!first_job.is_finished(), "first row must still be locked");
    hold.commit().unwrap();
    first_job.join().unwrap().unwrap();
    let revisions: u64 = inspect
        .query_row(
            "SELECT count(*) FROM entities WHERE kind='room' AND revision=$1",
            [second.1 + 1],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(revisions, 2);

    let jobs: Vec<_> = (1..=16)
        .map(|through| {
            let database = engine.store.db.clone();
            std::thread::spawn(move || {
                database.advance_cursor_blocking(&CursorAdvance {
                    user: "alice",
                    conversation: "room:one",
                    through,
                })
            })
        })
        .collect();
    for job in jobs {
        job.join().unwrap().unwrap();
    }
    let cursor: u64 = inspect.query_row("SELECT sequence FROM read_positions WHERE username='alice' AND conversation='room:one'", [], |row| row.get(0)).unwrap();
    assert_eq!(cursor, 16);
}

#[test]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
fn postgres_idle_pool_connection_can_be_replaced_without_losing_ownership() {
    let fixture = super::super::tests::PostgresFixture::new();
    let engine = Engine::open_postgres(&fixture.url).unwrap();
    let database = &engine.store.db;
    let owner = database.ownership_pid().unwrap();
    let pid: i32 = database
        .lock()
        .unwrap()
        .query_row("SELECT pg_backend_pid()", [], |row| row.get(0))
        .unwrap();
    assert_ne!(pid, owner);
    let inspect = Connection::postgres(&fixture.url, false).unwrap();
    assert!(
        inspect
            .query_row("SELECT pg_terminate_backend($1)", [pid], |row| row
                .get::<_, bool>(0))
            .unwrap()
    );
    database
        .advance_cursor_blocking(&CursorAdvance {
            user: "alice",
            conversation: "room:one",
            through: 1,
        })
        .unwrap();
    assert_eq!(database.ownership_pid().unwrap(), owner);
    let count: u64 = inspect
        .query_row("SELECT count(*) FROM read_positions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
fn postgres_upload_quotas_lock_each_uploader_and_cleanup_skips_busy_rows() {
    use crate::engine::database::{MediaAllocation, MediaWrite};
    use crate::engine::media::Media;
    let fixture = super::super::tests::PostgresFixture::new();
    let mut engine = Engine::open_postgres(&fixture.url).unwrap();
    engine
        .provision("alice", "hash".into(), true, false)
        .unwrap();
    engine
        .provision("bob", "hash".into(), false, false)
        .unwrap();
    engine
        .execute(Some("alice"), None, "/new room uploads")
        .unwrap();
    let at = crate::engine::now();
    let make_media = |user: &str| Media {
        id: uuid::Uuid::new_v4().to_string(),
        uploader: engine.data.users[user].id.clone(),
        conversation: engine.data.rooms["uploads"].id.clone(),
        filename: "file.bin".into(),
        hints: Default::default(),
        size: 8,
        content_type: "application/octet-stream".into(),
        object_key: "uploads/test".into(),
        ready: false,
        created: at,
        max_age: 86400,
        expires: None,
        deleting: false,
        deleted: false,
    };
    let blocker = Connection::postgres(&fixture.url, false).unwrap();
    let hold = blocker.unchecked_transaction().unwrap();
    hold.query_row(
        "SELECT key FROM entities WHERE kind='account' AND key='alice' FOR UPDATE",
        [],
        |row| row.get::<_, String>(0),
    )
    .unwrap();
    let jobs: Vec<_> = (0..2)
        .map(|_| {
            let database = engine.store.db.clone();
            let media = make_media("alice");
            std::thread::spawn(move || {
                database.media_write_blocking(&MediaWrite::Allocate(MediaAllocation {
                    media: &media,
                    quota: 8,
                    pending_limit: 16,
                    rate_limit: 16,
                    at,
                    private: None,
                }))
            })
        })
        .collect();
    let inspect = Connection::postgres(&fixture.url, false).unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let waiting: usize = inspect.query_row("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT key FROM entities%FOR UPDATE'", [], |row|row.get(0)).unwrap();
        if waiting == 2 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "upload requests did not lock the uploader before quota accounting"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let bob = make_media("bob");
    engine
        .store
        .db
        .media_write_blocking(&MediaWrite::Allocate(MediaAllocation {
            media: &bob,
            quota: 8,
            pending_limit: 16,
            rate_limit: 16,
            at,
            private: None,
        }))
        .unwrap();
    assert!(jobs.iter().all(|job| !job.is_finished()));
    hold.commit().unwrap();
    let outcomes: Vec<_> = jobs.into_iter().map(|job| job.join().unwrap()).collect();
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    assert!(outcomes.iter().any(|result|matches!(result,Err(Error::InvalidRequest(message)) if message.contains("byte quota"))));
    let bytes: u64 = inspect
        .query_row(
            "SELECT sum(size)::bigint FROM media WHERE uploader=$1",
            [&engine.data.users["alice"].id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(bytes, 8);
    let held = blocker.unchecked_transaction().unwrap();
    let alice_media: String = held
        .query_row(
            "SELECT id FROM media WHERE uploader=$1 FOR UPDATE",
            [&engine.data.users["alice"].id],
            |row| row.get(0),
        )
        .unwrap();
    let first = engine
        .store
        .db
        .media_write_blocking(&MediaWrite::ClaimCleanup {
            at,
            pending_before: at.saturating_add(1),
            attempted_before: at,
            limit: 10,
        })
        .unwrap();
    assert_eq!(first.media.len(), 1);
    assert_eq!(first.media[0].id, bob.id);
    held.commit().unwrap();
    let second = engine
        .store
        .db
        .media_write_blocking(&MediaWrite::ClaimCleanup {
            at,
            pending_before: at.saturating_add(1),
            attempted_before: at,
            limit: 10,
        })
        .unwrap();
    assert_eq!(second.media.len(), 1);
    assert_eq!(second.media[0].id, alice_media);
}

fn append_allocation_contract(engine: &mut Engine) {
    engine
        .provision("alice", "hash".into(), true, false)
        .unwrap();
    engine
        .execute(Some("alice"), None, "/new room allocation")
        .unwrap();
    engine
        .execute(Some("alice"), Some("allocation"), "seed")
        .unwrap();
    let baseline = engine.data.next_sequence;
    let room = &engine.data.rooms["allocation"];
    let mut message = engine.history("alice", "allocation").unwrap().messages[0].clone();
    message.id = uuid::Uuid::new_v4().to_string();
    message.text = "database allocates this".into();
    message.sequence = 99_999; // Caller state cannot allocate the persisted cursor.
    let receipt = super::super::MessageReceipt {
        account_id: &engine.data.users["alice"].id,
        key: "allocated",
        digest: "digest",
        created: crate::engine::now(),
    };
    let request = MessageWrite {
        conversation: &room.id,
        kind: "room",
        key: "allocation",
        revision: room.revision + 1,
        expected_revision: Some(room.revision),
        message: &message,
        mutation: MessageMutation::Append { retention: 1000 },
        new_conversation: None,
        receipt: Some(receipt),
        at: crate::engine::now(),
    };
    let committed = engine.store.db.save_message_blocking(&request).unwrap();
    assert_eq!(committed.sequence, baseline + 1);
    let stored = engine
        .store
        .db
        .message_receipt_blocking(&super::super::ReceiptRead {
            account: receipt.account_id,
            key: receipt.key,
            after: 0,
        })
        .unwrap()
        .unwrap();
    assert_eq!(stored.sequence, committed.sequence);
    let page = runtime::run(engine.store.db.history_page(&super::super::PageRead {
        conversation: &room.id,
        revision: committed.revision,
        range: crate::engine::HistoryRange::default(),
        limit: 100,
        max_bytes: 1024 * 1024,
    }))
    .unwrap();
    assert_eq!(page.messages.last().unwrap().sequence, committed.sequence);
    // An optimistic conflict occurs after counter allocation. It must undo the
    // allocation as part of the same transaction, including the receipt.
    assert!(matches!(
        engine.store.db.save_message_blocking(&MessageWrite {
            receipt: None,
            ..request
        }),
        Err(Error::RevisionConflict)
    ));
    let counter: u64 = engine
        .store
        .db
        .lock()
        .unwrap()
        .query_row("SELECT sequence FROM counters WHERE id=1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(counter, committed.sequence);
    let request = MessageWrite {
        expected_revision: Some(committed.revision),
        revision: committed.revision + 1,
        receipt: None,
        ..request
    };
    engine
        .store
        .db
        .lock()
        .unwrap()
        .execute_batch("UPDATE counters SET sequence=9007199254740991 WHERE id=1")
        .unwrap();
    assert!(matches!(
        engine.store.db.save_message_blocking(&request),
        Err(Error::InvalidRequest("Message sequence exhausted."))
    ));
    let count: u64 = engine
        .store
        .db
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM messages", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn sqlite_allocates_append_cursor_and_receipt_inside_transaction() {
    append_allocation_contract(&mut Engine::open(Path::new(":memory:")).unwrap());
}

#[test]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
fn postgres_allocates_append_cursor_and_receipt_inside_transaction() {
    let fixture = super::super::tests::PostgresFixture::new();
    append_allocation_contract(&mut Engine::open_postgres(&fixture.url).unwrap());
}

#[test]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
fn postgres_append_counter_prevents_later_commits_skipping_an_uncommitted_cursor() {
    let fixture = super::super::tests::PostgresFixture::new();
    let mut engine = Engine::open_postgres(&fixture.url).unwrap();
    engine
        .provision("alice", "hash".into(), true, false)
        .unwrap();
    engine
        .execute(Some("alice"), None, "/new room ordered")
        .unwrap();
    engine
        .execute(Some("alice"), Some("ordered"), "seed")
        .unwrap();
    let baseline = engine.data.next_sequence;
    let room = &engine.data.rooms["ordered"];
    let id = room.id.clone();
    let revision = room.revision;
    let mut message = engine.history("alice", "ordered").unwrap().messages[0].clone();
    message.id = uuid::Uuid::new_v4().to_string();
    let blocker = Connection::postgres(&fixture.url, false).unwrap();
    let hold = blocker.unchecked_transaction().unwrap();
    hold.execute_batch("UPDATE counters SET sequence=sequence+1 WHERE id=1")
        .unwrap();
    let database = engine.store.db.clone();
    let job = std::thread::spawn(move || {
        database.save_message_blocking(&MessageWrite {
            conversation: &id,
            kind: "room",
            key: "ordered",
            revision: revision + 1,
            expected_revision: Some(revision),
            message: &message,
            mutation: MessageMutation::Append { retention: 1000 },
            new_conversation: None,
            receipt: None,
            at: crate::engine::now(),
        })
    });
    let inspect = Connection::postgres(&fixture.url, false).unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let waiting: bool = inspect.query_row("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'UPDATE counters SET sequence=sequence+1%RETURNING sequence')", [], |r| r.get(0)).unwrap();
        if waiting {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "append did not wait for counter owner"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!job.is_finished());
    let visible: u64 = inspect
        .query_row("SELECT sequence FROM counters WHERE id=1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(visible, baseline);
    drop(hold); // Rolls back the allocated but unpublished cursor.
    assert_eq!(job.join().unwrap().unwrap().sequence, baseline + 1);
}

#[test]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
fn postgres_cancelled_append_rolls_back_without_blocking_lease_return() {
    let fixture = super::super::tests::PostgresFixture::new();
    let mut engine = Engine::open_postgres(&fixture.url).unwrap();
    engine
        .provision("alice", "hash".into(), true, false)
        .unwrap();
    engine
        .execute(Some("alice"), None, "/new room cancel")
        .unwrap();
    engine
        .execute(Some("alice"), Some("cancel"), "seed")
        .unwrap();
    let baseline = engine.data.next_sequence;
    let room = &engine.data.rooms["cancel"];
    let id = room.id.clone();
    let revision = room.revision;
    let mut message = engine.history("alice", "cancel").unwrap().messages[0].clone();
    message.id = uuid::Uuid::new_v4().to_string();
    let blocker = Connection::postgres(&fixture.url, false).unwrap();
    let hold = blocker.unchecked_transaction().unwrap();
    hold.execute_batch("UPDATE counters SET sequence=sequence+1 WHERE id=1")
        .unwrap();
    let database = engine.store.db.clone();
    let Pool::Postgres(pool) = &database.pool else {
        panic!("expected PostgreSQL");
    };
    let pool = pool.clone();
    let cancelled_id = id.clone();
    let cancelled_message = message.clone();
    runtime::run(async move {
        let job = tokio::spawn(async move {
            database.save_message(&MessageWrite {
                conversation: &cancelled_id,
                kind: "room",
                key: "cancel",
                revision: revision + 1,
                expected_revision: Some(revision),
                message: &cancelled_message,
                mutation: MessageMutation::Append { retention: 1000 },
                new_conversation: None,
                receipt: None,
                at: crate::engine::now(),
            }).await
        });
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'UPDATE counters SET sequence=sequence+1%RETURNING sequence')")
                .fetch_one(&pool).await?;
            if waiting { break; }
            assert!(Instant::now() < deadline, "append did not reach counter lock");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        job.abort();
        // Returning a cancelled lease must not wait for the blocked SQL query.
        let cancelled = tokio::time::timeout(Duration::from_millis(100), job).await.unwrap();
        assert!(matches!(cancelled, Err(error) if error.is_cancelled()));
        Ok(())
    }).unwrap();
    drop(hold);
    let committed = engine
        .store
        .db
        .save_message_blocking(&MessageWrite {
            conversation: &id,
            kind: "room",
            key: "cancel",
            revision: revision + 1,
            expected_revision: Some(revision),
            message: &message,
            mutation: MessageMutation::Append { retention: 1000 },
            new_conversation: None,
            receipt: None,
            at: crate::engine::now(),
        })
        .unwrap();
    assert_eq!(committed.sequence, baseline + 1);
    let count: u64 = engine
        .store
        .db
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM messages", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn dropping_sqlite_command_scope_rolls_back_and_releases_its_single_writer() {
    let engine = Engine::open(Path::new(":memory:")).unwrap();
    let command = engine.store.db.command_scope_blocking().unwrap();
    command.lock().unwrap().begin_command().unwrap();
    command
        .save_metadata_blocking(&super::super::MetadataWrite {
            operations: vec![super::super::MetadataOp::Entity {
                kind: "session",
                key: "uncommitted".into(),
                json: "{}".into(),
            }],
        })
        .unwrap();
    drop(command);
    let count: u64 = engine
        .store
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM entities WHERE key='uncommitted'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
fn postgres_command_scope_does_not_capture_independent_dispatcher_requests() {
    use super::super::{MetadataOp, MetadataWrite};
    let fixture = super::super::tests::PostgresFixture::new();
    let engine = Engine::open_postgres(&fixture.url).unwrap();
    let ordinary = engine.store.db.clone();
    let command = ordinary.command_scope_blocking().unwrap();
    command.lock().unwrap().begin_command().unwrap();
    command
        .save_metadata_blocking(&MetadataWrite {
            operations: vec![MetadataOp::Entity {
                kind: "session",
                key: "inside-command".into(),
                json: "{}".into(),
            }],
        })
        .unwrap();
    ordinary
        .save_metadata_blocking(&MetadataWrite {
            operations: vec![MetadataOp::Entity {
                kind: "session",
                key: "independent".into(),
                json: "{}".into(),
            }],
        })
        .unwrap();
    let inspect = Connection::postgres(&fixture.url, true).unwrap();
    let visible: String = inspect
        .query_row("SELECT key FROM entities WHERE kind='session'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(visible, "independent");
    drop(command); // Only this handle's effects are rolled back.
    let visible: String = ordinary
        .lock()
        .unwrap()
        .query_row("SELECT key FROM entities WHERE kind='session'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(visible, "independent");
}

#[test]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
fn postgres_receipt_capacity_is_atomic_across_independent_conversations() {
    let fixture = super::super::tests::PostgresFixture::new();
    let mut engine = Engine::open_postgres(&fixture.url).unwrap();
    engine
        .provision("alice", "hash".into(), true, false)
        .unwrap();
    let mut requests = Vec::new();
    for key in ["one", "two"] {
        engine
            .execute(Some("alice"), None, &format!("/new room {key}"))
            .unwrap();
        engine.execute(Some("alice"), Some(key), "seed").unwrap();
        let room = &engine.data.rooms[key];
        let mut message = engine.history("alice", key).unwrap().messages[0].clone();
        message.id = uuid::Uuid::new_v4().to_string();
        requests.push((key.to_owned(), room.id.clone(), room.revision, message));
    }
    let account = engine.data.users["alice"].id.clone();
    let at = crate::engine::now();
    engine.store.db.lock().unwrap().execute("INSERT INTO bot_receipts SELECT $1,'existing-'||n,'digest',$2,1,$3 FROM generate_series(1,4095) n", params![account, requests[0].1, at]).unwrap();
    let before = engine.data.next_sequence;
    let jobs: Vec<_> = requests
        .into_iter()
        .map(|(key, id, revision, message)| {
            let database = engine.store.db.clone();
            let account = account.clone();
            std::thread::spawn(move || {
                database.save_message_blocking(&MessageWrite {
                    conversation: &id,
                    kind: "room",
                    key: &key,
                    revision: revision + 1,
                    expected_revision: Some(revision),
                    message: &message,
                    mutation: MessageMutation::Append { retention: 1000 },
                    new_conversation: None,
                    receipt: Some(super::super::MessageReceipt {
                        account_id: &account,
                        key: &key,
                        digest: "digest",
                        created: at,
                    }),
                    at,
                })
            })
        })
        .collect();
    let mut commits = 0;
    let mut full = 0;
    for job in jobs {
        match job.join().unwrap() {
            Ok(_) => commits += 1,
            Err(Error::ReceiptCapacity) => full += 1,
            result => panic!(
                "unexpected quota result: {}",
                result
                    .err()
                    .map(|error| error.to_string())
                    .unwrap_or_default()
            ),
        }
    }
    assert_eq!((commits, full), (1, 1));
    let count: u64 = engine
        .store
        .db
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM bot_receipts", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 4096);
    let sequence: u64 = engine
        .store
        .db
        .lock()
        .unwrap()
        .query_row("SELECT sequence FROM counters WHERE id=1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(sequence, before + 1);
}
