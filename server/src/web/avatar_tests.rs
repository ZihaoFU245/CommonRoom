use super::super::media::tests::{app, request, storage_server};
use super::*;

async fn contract(mut app: App, db: crate::engine::database::Connection, postgres: bool) {
    app.io_jobs = Arc::new(tokio::sync::Semaphore::new(64));
    let (storage, server) = storage_server().await;
    app.media = Some(storage);
    {
        let mut engine = app.engine().unwrap();
        let mut config = app.config.media.clone();
        config.pending_limit = 1;
        engine.configure_media(config);
        engine.set_avatar_hosts(["example.com".into()].into());
    }
    let mut notices = app.changes.subscribe();
    let id = uuid::Uuid::new_v4().to_string();
    let input = json!({"size":8,"upload_id":id});
    let (one, two) = tokio::join!(
        request(&app, "alice", "POST", "/api/avatars/uploads", input.clone()),
        request(&app, "alice", "POST", "/api/avatars/uploads", input.clone()),
    );
    for response in [one, two] {
        assert_eq!(response.0, StatusCode::OK, "{}", response.1);
        assert_eq!(response.1["media_id"], id);
    }
    assert_eq!(
        request(&app, "bob", "POST", "/api/avatars/uploads", input)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &app,
            "alice",
            "POST",
            "/api/avatars/uploads",
            json!({"size":9,"upload_id":id})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM media", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(notices.try_recv().is_err());
    let store = app.engine_read().unwrap().media_store();
    let before = app.engine_read().unwrap().data.clone();
    let revision = app.engine_read().unwrap().revision;
    let color = app.engine_read().unwrap().color_version();
    // Profile failure must also roll back the ready/publication flags.
    db.execute_batch(if postgres {
        "CREATE FUNCTION reject_profile() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF OLD.kind='account' AND OLD.key='alice' THEN RAISE EXCEPTION 'injected'; END IF; RETURN NEW; END $$; CREATE TRIGGER reject_profile BEFORE UPDATE ON entities FOR EACH ROW EXECUTE FUNCTION reject_profile();"
    } else { "CREATE TRIGGER reject_profile BEFORE UPDATE ON entities WHEN OLD.kind='account' AND OLD.key='alice' BEGIN SELECT RAISE(ABORT,'injected'); END;" }).unwrap();
    let complete = format!("/api/avatars/uploads/{id}/complete");
    assert_eq!(
        request(&app, "alice", "POST", &complete, json!(null))
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(!store.load(&id).await.unwrap().0.ready);
    assert!(app.engine_read().unwrap().data == before);
    assert_eq!(app.engine_read().unwrap().revision, revision);
    assert!(notices.try_recv().is_err());
    db.execute_batch(if postgres {
        "DROP TRIGGER reject_profile ON entities; DROP FUNCTION reject_profile();"
    } else {
        "DROP TRIGGER reject_profile;"
    })
    .unwrap();
    let completed = request(&app, "alice", "POST", &complete, json!(null)).await;
    assert_eq!(completed.0, StatusCode::OK, "{}", completed.1);
    let media = store.load(&id).await.unwrap().0;
    assert!(media.ready && media.expires.is_none());
    assert_eq!(media.content_type, "image/png");
    assert_eq!(
        app.engine_read().unwrap().data.users["alice"].metadata["@global"]["avatar"],
        crate::engine::avatars::avatar_path(&id)
    );
    assert!(matches!(notices.try_recv().unwrap(), Change::Avatar(_)));
    let published = app.engine_read().unwrap().revision;
    assert_eq!(
        request(&app, "alice", "POST", &complete, json!(null))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(app.engine_read().unwrap().revision, published);
    assert_eq!(app.engine_read().unwrap().color_version(), color);
    assert!(notices.try_recv().is_err());
    let info = format!("/api/avatars/{id}/info");
    assert_eq!(
        request(&app, "bob", "GET", &info, json!(null)).await.0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            "bob",
            "GET",
            &format!("/api/avatars/{id}/file"),
            json!(null)
        )
        .await
        .0,
        StatusCode::TEMPORARY_REDIRECT
    );
    {
        let mut engine = app.engine().unwrap();
        engine
            .execute(Some("alice"), None, "/room new shared")
            .unwrap();
        engine.execute(None, None, "/revoke bob user").unwrap();
        engine
            .execute(None, None, "/grant bob #shared r:message.read")
            .unwrap();
        engine
            .execute(Some("alice"), Some("shared"), "historical author")
            .unwrap();
    }
    assert_eq!(
        request(&app, "bob", "GET", &info, json!(null)).await.0,
        StatusCode::OK
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/clean -1 #shared")
        .unwrap();
    assert_eq!(
        request(&app, "bob", "GET", &info, json!(null)).await.0,
        StatusCode::NOT_FOUND
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/grant bob user")
        .unwrap();
    app.engine()
        .unwrap()
        .execute(None, None, "/disable alice")
        .unwrap();
    assert_eq!(
        request(&app, "bob", "GET", &info, json!(null)).await.0,
        StatusCode::NOT_FOUND
    );
    assert!(
        store
            .expired(crate::engine::media::timestamp() + 1)
            .await
            .unwrap()
            .is_empty(),
        "disabled accounts must retain selected avatar bytes"
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/enable alice")
        .unwrap();
    app.engine()
        .unwrap()
        .login("alice".into(), "alice", "hash", None)
        .unwrap();
    assert_eq!(
        request(&app, "alice", "POST", "/api/color", json!({"color":"blue"}))
            .await
            .0,
        StatusCode::OK
    );
    assert_ne!(app.engine_read().unwrap().color_version(), color);
    assert!(matches!(notices.try_recv().unwrap(), Change::Metadata(_)));
    let (avatar, color) = tokio::join!(
        request(
            &app,
            "carol",
            "POST",
            "/api/avatars",
            json!({"url":"https://example.com/carol.png"})
        ),
        request(&app, "carol", "POST", "/api/color", json!({"color":"teal"})),
    );
    assert_eq!(avatar.0, StatusCode::OK);
    assert_eq!(color.0, StatusCode::OK);
    {
        let engine = app.engine_read().unwrap();
        assert_eq!(
            engine.data.users["carol"].metadata["@global"]["avatar"],
            "https://example.com/carol.png"
        );
        assert_eq!(
            engine.data.users["carol"].metadata["@global"]["color"],
            "teal"
        );
        assert_eq!(
            engine.data.users["alice"].metadata["@global"]["color"],
            "blue"
        );
    }
    assert_eq!(
        request(&app, "alice", "POST", "/api/avatars", json!({"url":null}))
            .await
            .0,
        StatusCode::OK
    );
    assert!(super::super::media::cleanup(&app).await.unwrap());
    assert!(store.load(&id).await.unwrap().0.deleted);
    assert_eq!(
        request(&app, "bob", "GET", &info, json!(null)).await.0,
        StatusCode::NOT_FOUND
    );
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn native_avatar_profile_commits_are_atomic_replay_safe_and_visible() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("chat.sqlite");
    let app = app(Engine::open(&path).unwrap());
    let db = crate::engine::database::Connection::open(&path).unwrap();
    contract(app, db, false).await;
    let engine = Engine::open(&path).unwrap();
    assert_eq!(
        engine.data.users["carol"].metadata["@global"]["avatar"],
        "https://example.com/carol.png"
    );
    assert_eq!(
        engine.data.users["carol"].metadata["@global"]["color"],
        "teal"
    );
    assert!(!engine.data.users["alice"].metadata["@global"].contains_key("avatar"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_native_avatar_profile_commits_are_atomic_replay_safe_and_visible() {
    let fixture = crate::engine::database::tests::PostgresFixture::new();
    let app = app(Engine::open_postgres(&fixture.url).unwrap());
    let db = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
    contract(app, db, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_avatar_retry_id_cannot_be_claimed_by_two_accounts() {
    let fixture = crate::engine::database::tests::PostgresFixture::new();
    let mut app = app(Engine::open_postgres(&fixture.url).unwrap());
    app.io_jobs = Arc::new(tokio::sync::Semaphore::new(64));
    app.media = Some(crate::media::tests::test_storage("http://127.0.0.1:1"));
    let id = uuid::Uuid::new_v4().to_string();
    let body = json!({"upload_id":id,"size":8});
    let (alice, bob) = tokio::join!(
        request(&app, "alice", "POST", "/api/avatars/uploads", body.clone()),
        request(&app, "bob", "POST", "/api/avatars/uploads", body.clone()),
    );
    assert!(matches!(
        (alice.0, bob.0),
        (StatusCode::OK, StatusCode::BAD_REQUEST) | (StatusCode::BAD_REQUEST, StatusCode::OK)
    ));
    let winner = if alice.0 == StatusCode::OK {
        "alice"
    } else {
        "bob"
    };
    let store = app.engine_read().unwrap().media_store();
    let media = store.load(&id).await.unwrap().0;
    assert_eq!(
        media.uploader,
        app.engine_read().unwrap().data.users[winner].id
    );
    assert_eq!(
        request(&app, winner, "POST", "/api/avatars/uploads", body)
            .await
            .0,
        StatusCode::OK
    );
    let db = crate::engine::database::Connection::postgres(&fixture.url, true).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM media", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_profile_writes_progress_independently_and_cancelled_updates_publish() {
    let fixture = crate::engine::database::tests::PostgresFixture::new();
    let mut app = app(Engine::open_postgres(&fixture.url).unwrap());
    app.io_jobs = Arc::new(tokio::sync::Semaphore::new(64));
    app.engine()
        .unwrap()
        .set_avatar_hosts(["example.com".into()].into());
    let blocker = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
    let hold = blocker.unchecked_transaction().unwrap();
    hold.query_row(
        "SELECT key FROM entities WHERE kind='account' AND key='alice' FOR UPDATE",
        [],
        |row| row.get::<_, String>(0),
    )
    .unwrap();
    let updating = tokio::spawn({
        let app = app.clone();
        async move { request(&app, "alice", "POST", "/api/color", json!({"color":"red"})).await }
    });
    let inspect = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        let waiting: bool = inspect.query_row("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT projection #>> % FROM entities WHERE kind=%FOR UPDATE')", [], |row| row.get(0)).unwrap();
        if waiting {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "profile did not reach its account lock"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(app.engine.try_read().is_ok());
    let bob = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        request(&app, "bob", "POST", "/api/color", json!({"color":"blue"})),
    )
    .await
    .unwrap();
    assert_eq!(bob.0, StatusCode::OK);
    assert!(
        !app.engine_read().unwrap().data.users["alice"]
            .metadata
            .contains_key("@global")
    );
    updating.abort();
    assert!(updating.await.unwrap_err().is_cancelled());
    let queued = tokio::spawn({
        let app = app.clone();
        async move {
            request(
                &app,
                "alice",
                "POST",
                "/api/avatars",
                json!({"url":"https://example.com/queued.png"}),
            )
            .await
        }
    });
    tokio::task::yield_now().await;
    let (started, waiting) = tokio::sync::oneshot::channel();
    let metadata = tokio::task::spawn_blocking({
        let app = app.clone();
        move || {
            started.send(()).unwrap();
            app.engine()
                .unwrap()
                .execute(None, None, "/disable alice")
                .unwrap();
        }
    });
    waiting.await.unwrap();
    tokio::task::yield_now().await;
    assert!(!metadata.is_finished());
    assert!(app.engine.try_read().is_ok());
    hold.rollback().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), metadata)
        .await
        .unwrap()
        .unwrap();
    let queued = tokio::time::timeout(std::time::Duration::from_secs(2), queued)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(queued.0, StatusCode::UNAUTHORIZED);
    let engine = app.engine_read().unwrap();
    assert_eq!(
        engine.data.users["alice"].metadata["@global"]["color"],
        "red"
    );
    assert_eq!(
        engine.data.users["bob"].metadata["@global"]["color"],
        "blue"
    );
    assert!(engine.data.users["alice"].disabled);
    assert!(!engine.data.users["alice"].metadata["@global"].contains_key("avatar"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_avatar_history_visibility_releases_engine_and_discards_revoked_reads() {
    let fixture = crate::engine::database::tests::PostgresFixture::new();
    let app = app(Engine::open_postgres(&fixture.url).unwrap());
    let id = {
        let mut engine = app.engine().unwrap();
        let mut media = engine.allocate_avatar("alice", 8).unwrap();
        media.ready = true;
        media.content_type = "image/png".into();
        media.object_key = media.final_key();
        engine.finish_avatar("alice", &media).unwrap();
        engine
            .execute(Some("alice"), None, "/room new shared")
            .unwrap();
        engine
            .execute(Some("alice"), Some("shared"), "historical author")
            .unwrap();
        engine.execute(None, None, "/revoke bob user").unwrap();
        engine
            .execute(None, None, "/grant bob #shared r:message.read")
            .unwrap();
        media.id
    };
    let blocker = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
    let hold = blocker.unchecked_transaction().unwrap();
    hold.execute_batch("LOCK TABLE messages IN ACCESS EXCLUSIVE MODE")
        .unwrap();
    let reading = tokio::spawn({
        let app = app.clone();
        async move {
            request(
                &app,
                "bob",
                "GET",
                &format!("/api/avatars/{id}/info"),
                json!(null),
            )
            .await
        }
    });
    let inspect = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        let waiting: bool = inspect.query_row("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT EXISTS(SELECT 1 FROM messages WHERE conversation=ANY%')", [], |row| row.get(0)).unwrap();
        if waiting {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "avatar did not reach its author lookup"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(app.engine.try_read().is_ok());
    tokio::time::timeout(
        std::time::Duration::from_millis(500),
        tokio::task::spawn_blocking({
            let app = app.clone();
            move || {
                app.engine()
                    .unwrap()
                    .execute(None, None, "/revoke bob #shared r:message.read")
                    .unwrap()
            }
        }),
    )
    .await
    .unwrap()
    .unwrap();
    hold.rollback().unwrap();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(2), reading)
            .await
            .unwrap()
            .unwrap()
            .0,
        StatusCode::NOT_FOUND
    );
}
