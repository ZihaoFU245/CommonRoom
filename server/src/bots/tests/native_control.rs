use super::*;
use crate::bots::{
    auth::Context,
    discovery::{Control, Runtime},
};

async fn contract(engine: Engine, db: crate::engine::database::Connection, postgres: bool) {
    let (app, token) = fixture_with_engine(engine);
    app.engine()
        .unwrap()
        .execute(None, None, "/grant helper admin")
        .unwrap();
    let id = conversation(&app);
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    let context = Context::authenticate_async(&app, &headers).await.unwrap();
    let mut runtime = Runtime::default();
    runtime
        .apply(&app, &context, Some(Control::Ready))
        .await
        .unwrap();
    let query = runtime.next().unwrap();
    runtime
        .apply(
            &app,
            &context,
            Some(Control::Response {
                id: query["id"].as_str().unwrap().into(),
                commands: vec![],
            }),
        )
        .await
        .unwrap();
    let before = app.engine_read().unwrap().data.clone();
    let revision = app.engine_read().unwrap().revision;
    let mut notices = app.changes.subscribe();
    db.execute_batch(if postgres {
        "CREATE FUNCTION reject_native_control() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected'; END $$; CREATE TRIGGER reject_native_control BEFORE UPDATE ON state FOR EACH ROW EXECUTE FUNCTION reject_native_control(); CREATE TRIGGER reject_native_control_insert BEFORE INSERT ON entities FOR EACH ROW EXECUTE FUNCTION reject_native_control();"
    } else {
        "CREATE TRIGGER reject_native_control BEFORE UPDATE ON state BEGIN SELECT RAISE(ABORT,'injected'); END; CREATE TRIGGER reject_native_control_insert BEFORE INSERT ON entities BEGIN SELECT RAISE(ABORT,'injected'); END;"
    }).unwrap();
    for (path, input) in [
        (
            "/api/bot/v1/credentials",
            json!({"label":"native","expires":crate::bots::now()+300,"conversations":[id],"operations":["read"]}),
        ),
        (
            "/api/bot/v1/conversations",
            json!({"kind":"direct","username":"admin"}),
        ),
    ] {
        let result = request(&app, Some(&token), "POST", path, input).await;
        assert_eq!(result.0, StatusCode::SERVICE_UNAVAILABLE, "{result:?}");
        let engine = app.engine_read().unwrap();
        assert!(engine.data == before);
        assert_eq!(engine.revision, revision);
        assert!(notices.try_recv().is_err());
    }
    db.execute_batch(if postgres {
        "DROP TRIGGER reject_native_control ON state; DROP TRIGGER reject_native_control_insert ON entities; DROP FUNCTION reject_native_control();"
    } else {
        "DROP TRIGGER reject_native_control; DROP TRIGGER reject_native_control_insert;"
    })
    .unwrap();
    let issued = request(&app, Some(&token), "POST", "/api/bot/v1/credentials", json!({"label":"native","expires":crate::bots::now()+300,"conversations":[id],"operations":["read"]})).await;
    assert_eq!(issued.0, StatusCode::OK, "{issued:?}");
    let restricted = issued.1["token"].as_str().unwrap();
    assert_eq!(
        request(&app, Some(restricted), "GET", "/api/bot/v1/me", json!({}))
            .await
            .0,
        StatusCode::OK
    );
    let direct = request(
        &app,
        Some(&token),
        "POST",
        "/api/bot/v1/conversations",
        json!({"kind":"direct","username":"admin"}),
    )
    .await;
    assert_eq!(direct.0, StatusCode::OK, "{direct:?}");
    let repeated = request(
        &app,
        Some(&token),
        "POST",
        "/api/bot/v1/conversations",
        json!({"kind":"direct","username":"admin"}),
    )
    .await;
    assert_eq!(repeated, direct);
    app.engine()
        .unwrap()
        .execute(
            None,
            None,
            &format!("/bot token rm helper {}", context.credential),
        )
        .unwrap();
    assert!(
        runtime
            .apply(&app, &context, Some(Control::Changed))
            .await
            .is_err()
    );
    assert!(context.verify_async(&app).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_bot_control_rolls_back_and_rechecks_live_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    let engine = Engine::open(&path).unwrap();
    let db = crate::engine::database::Connection::open(&path).unwrap();
    contract(engine, db, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_native_bot_control_rolls_back_and_rechecks_live_credentials() {
    let fixture = crate::engine::database::tests::PostgresFixture::new();
    let engine = Engine::open_postgres(&fixture.url).unwrap();
    let db = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
    contract(engine, db, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_bot_source_read_releases_engine_and_rechecks_revocation() {
    let fixture = crate::engine::database::tests::PostgresFixture::new();
    let (app, token) = fixture_with_engine(Engine::open_postgres(&fixture.url).unwrap());
    let id = conversation(&app);
    let sent = send(
        &app,
        &token,
        "native-source",
        json!({"conversation_id":id,"text":"private source"}),
    )
    .await;
    assert_eq!(sent.0, StatusCode::OK);
    let source = sent.1["message"]["id"].as_str().unwrap();
    let path = format!("/api/bot/v1/message-context?conversation_id={id}&message_id={source}");
    let blocker = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
    let hold = blocker.unchecked_transaction().unwrap();
    hold.execute_batch("LOCK TABLE messages IN ACCESS EXCLUSIVE MODE")
        .unwrap();
    let reading = tokio::spawn({
        let app = app.clone();
        let token = token.clone();
        async move { request(&app, Some(&token), "GET", &path, json!({})).await }
    });
    let inspect = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let waiting: bool = inspect.query_row("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%FROM messages%')", [], |row| row.get(0)).unwrap();
        if waiting {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "source lookup did not reach reader"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(app.engine.try_read().is_ok());
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        tokio::task::spawn_blocking({
            let app = app.clone();
            move || {
                app.engine()
                    .unwrap()
                    .execute(None, None, "/disable helper")
                    .unwrap();
            }
        }),
    )
    .await
    .unwrap()
    .unwrap();
    hold.rollback().unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(3), reading)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.0, StatusCode::UNAUTHORIZED, "{result:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_cancelled_bot_credential_commits_before_queued_disable() {
    let fixture = crate::engine::database::tests::PostgresFixture::new();
    let (app, token) = fixture_with_engine(Engine::open_postgres(&fixture.url).unwrap());
    let id = conversation(&app);
    let blocker = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
    let hold = blocker.unchecked_transaction().unwrap();
    hold.query_row("SELECT json FROM state WHERE id=1 FOR UPDATE", [], |row| {
        row.get::<_, String>(0)
    })
    .unwrap();
    let changing = tokio::spawn({
        let app = app.clone();
        async move {
            request(&app, Some(&token), "POST", "/api/bot/v1/credentials", json!({"label":"cancelled","expires":crate::bots::now()+300,"conversations":[id],"operations":["read"]})).await
        }
    });
    let inspect = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let waiting: bool = inspect.query_row("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%state%')", [], |row| row.get(0)).unwrap();
        if waiting {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "credential write did not reach database"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(app.engine.try_read().is_ok());
    changing.abort();
    assert!(changing.await.unwrap_err().is_cancelled());
    let disabling = tokio::task::spawn_blocking({
        let app = app.clone();
        move || app.engine().unwrap().execute(None, None, "/disable helper")
    });
    hold.rollback().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), disabling)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let engine = app.engine_read().unwrap();
    let bot = &engine.data.bots[engine.account_id("helper").unwrap()];
    assert!(
        bot.tokens
            .values()
            .any(|credential| credential.label == "cancelled")
    );
    assert!(!engine.active("helper"));
    let count: i64 = inspect
        .query_row(
            "SELECT count(*) FROM state WHERE json LIKE '%cancelled%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}
