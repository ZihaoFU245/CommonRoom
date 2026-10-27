use super::*;
use crate::web::media::tests::{app, request};

fn fixture(engine: Engine) -> App {
    let app = app(engine);
    app.engine()
        .unwrap()
        .provision(
            "alice",
            crate::engine::hash_password("native-password").unwrap(),
            false,
            true,
        )
        .unwrap();
    app
}
async fn attempt(app: &App, password: &str) -> Result<Response, ApiError> {
    let mut headers = HeaderMap::new();
    headers.insert(header::ORIGIN, "http://localhost:5173".parse().unwrap());
    login(
        State(app.clone()),
        ConnectInfo("127.0.0.1:1234".parse().unwrap()),
        headers,
        Json(Login {
            username: "alice".into(),
            password: password.into(),
        }),
    )
    .await
}
async fn sign_out(app: &App, value: &str) -> Result<Response, ApiError> {
    let mut headers = HeaderMap::new();
    headers.insert(header::ORIGIN, "http://localhost:5173".parse().unwrap());
    headers.insert(header::COOKIE, value.parse().unwrap());
    logout(State(app.clone()), None, headers).await
}
fn reject_events(db: &crate::engine::database::Connection, postgres: bool, reject: bool) {
    db.execute_batch(match (postgres, reject) {
        (true, true) => "CREATE FUNCTION reject_session_event() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected'; END $$; CREATE TRIGGER reject_session_event BEFORE INSERT ON auth_events FOR EACH ROW EXECUTE FUNCTION reject_session_event();",
        (true, false) => "DROP TRIGGER reject_session_event ON auth_events; DROP FUNCTION reject_session_event();",
        (false, true) => "CREATE TRIGGER reject_session_event BEFORE INSERT ON auth_events BEGIN SELECT RAISE(ABORT,'injected'); END;",
        (false, false) => "DROP TRIGGER reject_session_event;",
    }).unwrap();
}
async fn contract(app: App, db: crate::engine::database::Connection, postgres: bool) {
    let before = app.engine_read().unwrap().data.sessions.clone();
    let revision = app.engine_read().unwrap().revision;
    reject_events(&db, postgres, true);
    assert!(matches!(
        attempt(&app, "native-password").await,
        Err(ApiError(StatusCode::SERVICE_UNAVAILABLE, _))
    ));
    assert!(app.engine_read().unwrap().data.sessions == before);
    assert_eq!(app.engine_read().unwrap().revision, revision);
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM entities WHERE kind='session'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        i64::try_from(before.len()).unwrap()
    );
    reject_events(&db, postgres, false);
    let response = attempt(&app, "native-password").await.unwrap();
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let token = cookie.split_once('=').unwrap().1;
    assert_eq!(
        app.engine_read().unwrap().session(token).as_deref(),
        Some("alice")
    );
    let mut notices = app.changes.subscribe();
    let revision = app.engine_read().unwrap().revision;
    reject_events(&db, postgres, true);
    assert!(matches!(
        sign_out(&app, &cookie).await,
        Err(ApiError(StatusCode::SERVICE_UNAVAILABLE, _))
    ));
    assert_eq!(
        app.engine_read().unwrap().session(token).as_deref(),
        Some("alice")
    );
    assert_eq!(app.engine_read().unwrap().revision, revision);
    assert!(notices.try_recv().is_err());
    reject_events(&db, postgres, false);
    assert_eq!(
        sign_out(&app, &cookie).await.unwrap().status(),
        StatusCode::OK
    );
    assert!(app.engine_read().unwrap().session(token).is_none());
    assert!(matches!(notices.try_recv().unwrap(), Change::Account(user) if user=="alice"));
    assert!(matches!(
        attempt(&app, "wrong-password").await,
        Err(ApiError(StatusCode::UNAUTHORIZED, _))
    ));
    assert_eq!(
        sign_out(&app, &cookie).await.unwrap().status(),
        StatusCode::OK
    );
    assert!(notices.try_recv().is_err());
    assert_eq!(
        db.query_row("SELECT count(*) FROM auth_events", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        3
    );
    assert_eq!(app.password_jobs.available_permits(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_auth_sessions_and_events_commit_atomically() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("chat.sqlite");
    contract(
        fixture(Engine::open(&path).unwrap()),
        crate::engine::database::Connection::open(&path).unwrap(),
        false,
    )
    .await;
    let engine = Engine::open(&path).unwrap();
    assert!(
        !engine
            .data
            .sessions
            .values()
            .any(|session| session.username == "alice")
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_native_auth_sessions_and_events_commit_atomically() {
    let fixture_db = crate::engine::database::tests::PostgresFixture::new();
    contract(
        fixture(Engine::open_postgres(&fixture_db.url).unwrap()),
        crate::engine::database::Connection::postgres(&fixture_db.url, false).unwrap(),
        true,
    )
    .await;
    let engine = Engine::open_postgres(&fixture_db.url).unwrap();
    assert!(
        !engine
            .data
            .sessions
            .values()
            .any(|session| session.username == "alice")
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_cancelled_login_commits_before_queued_disable_without_engine_guard() {
    let fixture_db = crate::engine::database::tests::PostgresFixture::new();
    let app = fixture(Engine::open_postgres(&fixture_db.url).unwrap());
    let before = app.engine_read().unwrap().data.sessions.clone();
    let blocker = crate::engine::database::Connection::postgres(&fixture_db.url, false).unwrap();
    let hold = blocker.unchecked_transaction().unwrap();
    hold.execute_batch("LOCK TABLE auth_events IN ACCESS EXCLUSIVE MODE")
        .unwrap();
    let signing_in = tokio::spawn({
        let app = app.clone();
        async move { attempt(&app, "native-password").await }
    });
    let inspect = crate::engine::database::Connection::postgres(&fixture_db.url, false).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
    loop {
        let waiting: bool = inspect.query_row("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'INSERT INTO auth_events%')", [], |row| row.get(0)).unwrap();
        if waiting {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "login did not reach event lock"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(app.engine.try_read().is_ok());
    assert!(app.engine_read().unwrap().data.sessions == before);
    assert_eq!(
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            request(&app, "bob", "GET", "/api/session", json!(null))
        )
        .await
        .unwrap()
        .0,
        StatusCode::OK
    );
    signing_in.abort();
    assert!(signing_in.await.unwrap_err().is_cancelled());
    let disabling = tokio::task::spawn_blocking({
        let app = app.clone();
        move || {
            app.engine()
                .unwrap()
                .execute(None, None, "/user disable alice")
                .unwrap()
        }
    });
    tokio::task::yield_now().await;
    assert!(!disabling.is_finished());
    hold.rollback().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), disabling)
        .await
        .unwrap()
        .unwrap();
    assert!(app.engine_read().unwrap().data.users["alice"].disabled);
    assert!(
        !app.engine_read()
            .unwrap()
            .data
            .sessions
            .values()
            .any(|session| session.username == "alice")
    );
    assert_eq!(
        inspect
            .query_row(
                "SELECT count(*) FROM auth_events WHERE outcome='success'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while app.password_jobs.available_permits() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
