use super::super::media::tests::app;
use super::*;
use axum::http::{HeaderMap, header};

pub(super) async fn command(
    app: &App,
    user: &str,
    view: Option<&str>,
    text: &str,
) -> Result<String, String> {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        format!("chat_session={user}").parse().unwrap(),
    );
    super::super::commands::execute_input(
        app,
        &headers,
        user,
        &Input {
            id: 1,
            room: view.map(str::to_owned),
            text: text.into(),
            markdown: false,
        },
    )
    .await
    .map(|(reply, change)| {
        assert!(change.is_none());
        reply
    })
}
async fn contract(mut app: App, db: crate::engine::database::Connection, postgres: bool) {
    app.io_jobs = Arc::new(tokio::sync::Semaphore::new(64));
    app.engine()
        .unwrap()
        .execute(None, None, "/grant alice su")
        .unwrap();
    let mut notices = app.changes.subscribe();
    let before = app.engine_read().unwrap().data.clone();
    let revision = app.engine_read().unwrap().revision;
    let colors = app.engine_read().unwrap().color_version();
    db.execute_batch(if postgres {
        "CREATE FUNCTION reject_native_metadata() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected'; END $$; CREATE TRIGGER reject_native_metadata BEFORE UPDATE ON state FOR EACH ROW EXECUTE FUNCTION reject_native_metadata();"
    } else {
        "CREATE TRIGGER reject_native_metadata BEFORE UPDATE ON state BEGIN SELECT RAISE(ABORT,'injected'); END;"
    }).unwrap();
    assert!(
        command(&app, "alice", None, "/room new atomic")
            .await
            .is_err()
    );
    assert!(
        command(&app, "alice", None, "/bot new rejected @global")
            .await
            .is_err()
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM entities WHERE kind='account' AND key='rejected'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert!(app.engine_read().unwrap().data == before);
    assert_eq!(app.engine_read().unwrap().revision, revision);
    assert_eq!(app.engine_read().unwrap().color_version(), colors);
    assert!(notices.try_recv().is_err());
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM entities WHERE kind='room' AND key='atomic'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    db.execute_batch(if postgres {
        "DROP TRIGGER reject_native_metadata ON state; DROP FUNCTION reject_native_metadata();"
    } else {
        "DROP TRIGGER reject_native_metadata;"
    })
    .unwrap();
    assert_eq!(
        command(&app, "alice", None, "/room new atomic")
            .await
            .unwrap(),
        "Created #atomic. Admission: invitation-only."
    );
    assert!(matches!(notices.try_recv().unwrap(), Change::Metadata(_)));
    for text in [
        "/room add bob #atomic",
        "/grant carol user #atomic",
        "/grant bob #atomic r:message.metadata",
        "/revoke bob #atomic r:message.metadata",
        "/room owner bob #atomic",
    ] {
        command(&app, "alice", None, text).await.unwrap();
    }
    let before = app.engine_read().unwrap().data.clone();
    let revision = app.engine_read().unwrap().revision;
    let mut failed_notices = app.changes.subscribe();
    db.execute_batch(if postgres {
        "CREATE FUNCTION reject_account_metadata() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF OLD.kind='account' AND OLD.key='bob' THEN RAISE EXCEPTION 'injected'; END IF; RETURN NEW; END $$; CREATE TRIGGER reject_account_metadata BEFORE UPDATE ON entities FOR EACH ROW EXECUTE FUNCTION reject_account_metadata();"
    } else {
        "CREATE TRIGGER reject_account_metadata BEFORE UPDATE ON entities WHEN OLD.kind='account' AND OLD.key='bob' BEGIN SELECT RAISE(ABORT,'injected'); END;"
    }).unwrap();
    assert!(
        command(&app, "alice", Some("atomic"), "/user set @all color red")
            .await
            .is_err()
    );
    assert!(app.engine_read().unwrap().data == before);
    assert_eq!(app.engine_read().unwrap().revision, revision);
    assert!(failed_notices.try_recv().is_err());
    // Alice precedes Bob in the native batch and must roll back too.
    let stored: String = db
        .query_row(
            "SELECT json FROM entities WHERE kind='account' AND key='alice'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&stored).unwrap(),
        serde_json::to_value(&before.users["alice"]).unwrap()
    );
    db.execute_batch(if postgres {
        "DROP TRIGGER reject_account_metadata ON entities; DROP FUNCTION reject_account_metadata();"
    } else {
        "DROP TRIGGER reject_account_metadata;"
    })
    .unwrap();
    command(&app, "alice", Some("atomic"), "/user set @all color red")
        .await
        .unwrap();
    command(&app, "bob", Some("atomic"), "/user set bob color blue")
        .await
        .unwrap();
    assert!(
        command(&app, "bob", Some("atomic"), "/user set alice color blue")
            .await
            .is_err()
    );
    command(
        &app,
        "alice",
        Some("atomic"),
        "/user set @all media.user_quota 1000",
    )
    .await
    .unwrap();
    assert_eq!(
        app.engine_read().unwrap().data.users["carol"].metadata["@global"]["media.user_quota"],
        "1000"
    );
    command(&app, "alice", Some("atomic"), "/user reset @all color")
        .await
        .unwrap();
    command(
        &app,
        "alice",
        Some("atomic"),
        "/user set @all history.clean_min_age 7d",
    )
    .await
    .unwrap();
    command(&app, "alice", Some("atomic"), "/user reset bob @all")
        .await
        .unwrap();
    assert!(
        command(&app, "bob", Some("atomic"), "/room leave")
            .await
            .is_err()
    );
    command(&app, "alice", None, "/room owner alice #atomic")
        .await
        .unwrap();
    command(&app, "bob", Some("atomic"), "/room leave")
        .await
        .unwrap();
    assert!(
        !app.engine_read().unwrap().data.rooms["atomic"]
            .members
            .contains("bob")
    );
    for permission in ["/sudo", "x:command.sudo"] {
        command(
            &app,
            "alice",
            None,
            &format!("/grant bob @global {permission}"),
        )
        .await
        .unwrap();
    }
    command(&app, "bob", None, "/sudo /room join #atomic")
        .await
        .unwrap();
    assert!(
        app.engine_read().unwrap().data.rooms["atomic"]
            .members
            .contains("bob")
    );
    assert!(!app.engine_read().unwrap().is_su(Some("bob")));
    command(&app, "alice", None, "/room kick bob #atomic")
        .await
        .unwrap();
    command(&app, "alice", None, "/room add bob #atomic")
        .await
        .unwrap();
    command(&app, "alice", None, "/bot new native #atomic")
        .await
        .unwrap();
    command(&app, "alice", None, "/bot owner native carol")
        .await
        .unwrap();
    let issued = command(&app, "alice", None, "/bot token new native worker")
        .await
        .unwrap();
    let secret = issued.lines().last().unwrap();
    let bot = app.engine_read().unwrap().data.users["native"].id.clone();
    let credential = app.engine_read().unwrap().data.bots[&bot]
        .tokens
        .keys()
        .next()
        .unwrap()
        .clone();
    assert_eq!(
        app.engine_read().unwrap().bot_identity(secret).as_deref(),
        Some("native")
    );
    let listed = command(&app, "alice", None, "/bot token ls native")
        .await
        .unwrap();
    assert!(listed.contains(&credential) && !listed.contains(secret));
    assert!(
        command(&app, "alice", None, "/bot info native #atomic")
            .await
            .unwrap()
            .contains("Owner: carol")
    );
    assert!(
        command(&app, "carol", None, "/bot token ls native")
            .await
            .is_err()
    );
    command(
        &app,
        "alice",
        None,
        &format!("/bot token rm native {credential}"),
    )
    .await
    .unwrap();
    assert!(app.engine_read().unwrap().bot_identity(secret).is_none());
    command(&app, "alice", None, "/bot rm native @global")
        .await
        .unwrap();
    assert!(app.engine_read().unwrap().account_id("native").is_none());
    assert!(!app.engine_read().unwrap().data.bots.contains_key(&bot));
    // Populate a retained history/cursor to prove room removal is one transaction.
    {
        let mut engine = app.engine().unwrap();
        engine
            .execute(Some("bob"), Some("atomic"), "retained")
            .unwrap();
        let sequence = engine.history("bob", "atomic").unwrap().messages[0].sequence;
        engine.mark_read("bob", "atomic", sequence).unwrap();
    }
    let before = app.engine_read().unwrap().data.clone();
    let revision = app.engine_read().unwrap().revision;
    let mut rename_notices = app.changes.subscribe();
    db.execute_batch(if postgres {
        "CREATE FUNCTION reject_cursor_rename() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected'; END $$; CREATE TRIGGER reject_cursor_rename BEFORE UPDATE ON read_positions FOR EACH ROW EXECUTE FUNCTION reject_cursor_rename();"
    } else {
        "CREATE TRIGGER reject_cursor_rename BEFORE UPDATE ON read_positions BEGIN SELECT RAISE(ABORT,'injected'); END;"
    }).unwrap();
    assert!(
        command(&app, "alice", None, "/room rename #atomic renamed")
            .await
            .is_err()
    );
    assert!(app.engine_read().unwrap().data == before);
    assert_eq!(app.engine_read().unwrap().revision, revision);
    assert!(rename_notices.try_recv().is_err());
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM entities WHERE kind='room' AND key='renamed'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    db.execute_batch(if postgres {
        "DROP TRIGGER reject_cursor_rename ON read_positions; DROP FUNCTION reject_cursor_rename();"
    } else {
        "DROP TRIGGER reject_cursor_rename;"
    })
    .unwrap();
    command(&app, "alice", None, "/room rename #atomic renamed")
        .await
        .unwrap();
    {
        let engine = app.engine_read().unwrap();
        assert_eq!(engine.data.rooms["renamed"].id, before.rooms["atomic"].id);
        assert_eq!(
            engine.data.rooms["renamed"].revision,
            before.rooms["atomic"].revision + 1
        );
        assert_eq!(
            engine.history("bob", "renamed").unwrap().messages[0].text,
            "retained"
        );
        assert_eq!(engine.unreads("bob").unwrap()["renamed"].count, 0);
    }
    command(&app, "alice", None, "/room rename #renamed atomic")
        .await
        .unwrap();
    let sequence = app.engine_read().unwrap().data.next_sequence;
    command(&app, "alice", None, "/user disable bob")
        .await
        .unwrap();
    assert!(!app.engine_read().unwrap().active("bob"));
    assert!(app.engine_read().unwrap().session("bob").is_none());
    assert!(
        !app.engine_read().unwrap().data.rooms["atomic"]
            .members
            .contains("bob")
    );
    command(&app, "alice", None, "/user enable bob")
        .await
        .unwrap();
    assert!(app.engine_read().unwrap().active("bob"));
    command(&app, "alice", None, "/room delete #atomic")
        .await
        .unwrap();
    assert!(!app.engine_read().unwrap().data.rooms.contains_key("atomic"));
    assert_eq!(app.engine_read().unwrap().data.next_sequence, sequence);
    assert_eq!(
        db.query_row("SELECT count(*) FROM messages", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM read_positions WHERE conversation='room:atomic'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM audit_entries", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        i64::try_from(app.engine_read().unwrap().data.policy.audit.len()).unwrap()
    );
    command(
        &app,
        "alice",
        None,
        "/user new dave native-old-password user",
    )
    .await
    .unwrap();
    {
        let mut engine = app.engine().unwrap();
        let expected = engine.password_hash("dave").unwrap().to_owned();
        engine
            .login("dave".into(), "dave", &expected, None)
            .unwrap();
        engine
            .login("dave-other".into(), "dave", &expected, None)
            .unwrap();
    }
    let before = app.engine_read().unwrap().data.clone();
    let mut credentials = app.changes.subscribe();
    db.execute_batch(if postgres {
        "CREATE FUNCTION reject_password_change() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF OLD.kind='account' AND OLD.key='dave' THEN RAISE EXCEPTION 'injected'; END IF; RETURN NEW; END $$; CREATE TRIGGER reject_password_change BEFORE UPDATE ON entities FOR EACH ROW EXECUTE FUNCTION reject_password_change();"
    } else {
        "CREATE TRIGGER reject_password_change BEFORE UPDATE ON entities WHEN OLD.kind='account' AND OLD.key='dave' BEGIN SELECT RAISE(ABORT,'injected'); END;"
    }).unwrap();
    assert!(
        command(
            &app,
            "dave",
            None,
            "/user passwd native-old-password native-new-password"
        )
        .await
        .is_err()
    );
    assert!(app.engine_read().unwrap().data == before);
    assert!(credentials.try_recv().is_err());
    db.execute_batch(if postgres {
        "DROP TRIGGER reject_password_change ON entities; DROP FUNCTION reject_password_change();"
    } else {
        "DROP TRIGGER reject_password_change;"
    })
    .unwrap();
    assert_eq!(
        command(
            &app,
            "dave",
            None,
            "/user passwd native-old-password native-new-password"
        )
        .await
        .unwrap(),
        "Password changed. Other sessions signed out."
    );
    assert!(matches!(credentials.try_recv().unwrap(), Change::Account(user) if user=="dave"));
    {
        let engine = app.engine_read().unwrap();
        assert_eq!(engine.session("dave").as_deref(), Some("dave"));
        assert!(engine.session("dave-other").is_none());
        assert!(crate::engine::verify_password(
            "native-new-password",
            engine.password_hash("dave").unwrap()
        ));
    }
    assert!(
        command(
            &app,
            "dave",
            None,
            "/user passwd native-old-password should-not-change"
        )
        .await
        .is_err()
    );
    command(
        &app,
        "alice",
        None,
        "/user reset dave password native-reset-password",
    )
    .await
    .unwrap();
    assert!(matches!(
        credentials.try_recv().unwrap(),
        Change::Metadata(_)
    ));
    let engine = app.engine_read().unwrap();
    assert!(engine.session("dave").is_none());
    assert!(crate::engine::verify_password(
        "native-reset-password",
        engine.password_hash("dave").unwrap()
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_metadata_commands_commit_atomically_and_preserve_lifecycle() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("chat.sqlite");
    contract(
        app(Engine::open(&path).unwrap()),
        crate::engine::database::Connection::open(&path).unwrap(),
        false,
    )
    .await;
    let engine = Engine::open(&path).unwrap();
    assert!(!engine.data.rooms.contains_key("atomic"));
    assert!(engine.active("bob"));
    assert!(engine.session("bob").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_native_metadata_commands_commit_atomically_and_preserve_lifecycle() {
    let fixture = crate::engine::database::tests::PostgresFixture::new();
    contract(
        app(Engine::open_postgres(&fixture.url).unwrap()),
        crate::engine::database::Connection::postgres(&fixture.url, false).unwrap(),
        true,
    )
    .await;
    let engine = Engine::open_postgres(&fixture.url).unwrap();
    assert!(!engine.data.rooms.contains_key("atomic"));
    assert!(engine.active("bob"));
    assert!(engine.session("bob").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_cancelled_metadata_commit_releases_engine_and_publishes_before_queued_writes() {
    cancelled_metadata_contract(false).await;
    cancelled_metadata_contract(true).await;
}
async fn cancelled_metadata_contract(expire_delivery: bool) {
    let fixture = crate::engine::database::tests::PostgresFixture::new();
    let mut app = app(Engine::open_postgres(&fixture.url).unwrap());
    app.io_jobs = Arc::new(tokio::sync::Semaphore::new(64));
    app.engine()
        .unwrap()
        .execute(None, None, "/grant alice su")
        .unwrap();
    let expires = if expire_delivery {
        let mut engine = app.engine().unwrap();
        engine.execute(None, None, "/grant bob su").unwrap();
        let expires = crate::engine::media::timestamp() + 2;
        for session in engine
            .data
            .sessions
            .values_mut()
            .filter(|session| session.username == "alice")
        {
            session.expires = expires;
        }
        Some(expires)
    } else {
        None
    };
    let blocker = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
    let hold = blocker.unchecked_transaction().unwrap();
    hold.query_row("SELECT id FROM state WHERE id=1 FOR UPDATE", [], |row| {
        row.get::<_, i64>(0)
    })
    .unwrap();
    let mut notices = app.changes.subscribe();
    let updating = tokio::spawn({
        let app = app.clone();
        async move { command(&app, "alice", None, "/room new queued").await }
    });
    let inspect = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let waiting: bool = inspect.query_row("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'INSERT INTO state%')", [], |row| row.get(0)).unwrap();
        if waiting {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "metadata did not reach state lock"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(app.engine.try_read().is_ok());
    assert!(!app.engine_read().unwrap().data.rooms.contains_key("queued"));
    assert!(notices.try_recv().is_err());
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            command(&app, "bob", None, "/whoami")
        )
        .await
        .unwrap()
        .is_ok()
    );
    updating.abort();
    assert!(updating.await.unwrap_err().is_cancelled());
    if let Some(expires) = expires {
        while crate::engine::media::timestamp() < expires {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(app.engine_read().unwrap().session("alice").is_none());
    }
    let queued = tokio::spawn({
        let app = app.clone();
        async move {
            command(
                &app,
                if expire_delivery { "bob" } else { "alice" },
                None,
                "/room add bob #queued",
            )
            .await
        }
    });
    tokio::task::yield_now().await;
    assert!(!queued.is_finished());
    hold.rollback().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), queued)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        app.engine_read().unwrap().data.rooms["queued"]
            .members
            .contains("bob")
    );
    assert!(matches!(notices.try_recv().unwrap(), Change::Metadata(_)));
    assert!(matches!(notices.try_recv().unwrap(), Change::Metadata(_)));
    assert!(notices.try_recv().is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_cancelled_password_reset_publishes_before_queued_revocation() {
    let fixture = crate::engine::database::tests::PostgresFixture::new();
    let mut app = app(Engine::open_postgres(&fixture.url).unwrap());
    app.io_jobs = Arc::new(tokio::sync::Semaphore::new(64));
    app.engine()
        .unwrap()
        .execute(
            None,
            None,
            "/grant alice @account:bob x:account.password.reset",
        )
        .unwrap();
    let blocker = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
    let hold = blocker.unchecked_transaction().unwrap();
    hold.query_row(
        "SELECT key FROM entities WHERE kind='account' AND key='bob' FOR UPDATE",
        [],
        |row| row.get::<_, String>(0),
    )
    .unwrap();
    let mut notices = app.changes.subscribe();
    let resetting = tokio::spawn({
        let app = app.clone();
        async move {
            command(
                &app,
                "alice",
                None,
                "/user reset bob password native-reset-password",
            )
            .await
        }
    });
    let inspect = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
    loop {
        let waiting: bool = inspect.query_row("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'INSERT INTO entities%')", [], |row| row.get(0)).unwrap();
        if waiting {
            break;
        }
        if std::time::Instant::now() >= deadline || resetting.is_finished() {
            hold.rollback().unwrap();
            let result = tokio::time::timeout(std::time::Duration::from_secs(2), resetting)
                .await
                .unwrap()
                .unwrap();
            assert!(
                waiting,
                "password reset did not reach account lock: {result:?}"
            );
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(app.engine.try_read().is_ok());
    assert_eq!(
        app.engine_read().unwrap().password_hash("bob"),
        Some("hash")
    );
    assert_eq!(
        app.engine_read().unwrap().session("bob").as_deref(),
        Some("bob")
    );
    assert!(notices.try_recv().is_err());
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            command(&app, "bob", None, "/whoami")
        )
        .await
        .unwrap()
        .is_ok()
    );
    resetting.abort();
    assert!(resetting.await.unwrap_err().is_cancelled());
    let revoking = tokio::spawn({
        let app = app.clone();
        async move { command(&app, "alice", None, "/user disable bob").await }
    });
    tokio::task::yield_now().await;
    assert!(!revoking.is_finished());
    hold.rollback().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), revoking)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let engine = app.engine_read().unwrap();
    assert!(engine.data.users["bob"].disabled);
    assert!(crate::engine::verify_password(
        "native-reset-password",
        &engine.data.users["bob"].hash
    ));
    assert!(engine.session("bob").is_none());
    assert!(matches!(notices.try_recv().unwrap(), Change::Metadata(_)));
    assert!(matches!(notices.try_recv().unwrap(), Change::Metadata(_)));
    assert!(notices.try_recv().is_err());
}
