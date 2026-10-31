use super::super::media::tests::app;
use super::*;
use axum::http::{HeaderMap, header};

async fn command(app: &App, user: &str, view: Option<&str>, text: &str) -> Result<String, String> {
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
    .map(|(text, change)| {
        assert!(change.is_none());
        text
    })
}
async fn contract(mut app: App) {
    app.io_jobs = Arc::new(tokio::sync::Semaphore::new(64));
    {
        let mut engine = app.engine().unwrap();
        engine.execute(None, None, "/room new history").unwrap();
        engine
            .execute(None, None, "/room add bob #history")
            .unwrap();
        engine
            .execute(Some("bob"), Some("history"), "first query message")
            .unwrap();
        engine
            .execute(Some("bob"), None, "/tell alice private query message")
            .unwrap();
        for permission in ["/sudo", "x:command.sudo"] {
            engine
                .execute(None, None, &format!("/grant bob @global {permission}"))
                .unwrap();
        }
    }
    let before = app.engine_read().unwrap().data.clone();
    let revision = app.engine_read().unwrap().revision;
    for (user, view, text) in [
        ("bob", None, "/help"),
        ("bob", None, "/whoami"),
        ("bob", None, "/ls room"),
        ("bob", Some("history"), "/members"),
        ("alice", None, "/user ls"),
        ("alice", None, "/user get bob"),
        ("bob", Some("history"), "/permissions"),
        ("bob", Some("history"), "/history"),
        ("bob", None, "/history #history"),
        ("bob", Some("history"), "/history 1 100"),
        ("alice", Some("@direct:bob"), "/history"),
        ("bob", None, "/sudo /history #history"),
        ("bob", None, "/sudo /whoami"),
        ("bob", None, "/sudo /clean -1 #history --dry-run"),
    ] {
        assert!(supports(text).unwrap());
        let expected = app
            .engine()
            .unwrap()
            .execute(Some(user), view, text)
            .unwrap();
        assert_eq!(
            command(&app, user, view, text).await.unwrap(),
            expected,
            "{text}"
        );
    }
    let configs = command(&app, "bob", None, "/sudo /system configs")
        .await
        .unwrap();
    assert!(configs.starts_with("# System configuration"));
    let raw = configs
        .split("```json\n")
        .nth(1)
        .unwrap()
        .split("\n```")
        .next()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(raw).unwrap(),
        serde_json::from_str::<serde_json::Value>(&app.config.display().unwrap()).unwrap()
    );
    let metrics = command(&app, "bob", None, "/sudo /system metrics")
        .await
        .unwrap();
    assert!(metrics.starts_with("# System metrics"));
    assert!(metrics.contains("| `readers.open` |"));
    for key in [
        "io_jobs_limit",
        "admission.http.rejections.io",
        "admission.http.connections.peak",
        "ordering.metadata.completed",
    ] {
        assert!(metrics.contains(&format!("| `{key}` |")), "{key}");
    }
    assert!(command(&app, "bob", None, "/system").await.is_err());
    assert!(
        command(&app, "alice", None, "/system configs")
            .await
            .is_err()
    ); // ordinary admin
    for text in [
        "/history",
        "/history #missing",
        "/history 4 1",
        "/configs",
        "/system configs unexpected",
        "/sudo /system unknown",
        "/sudo /sudo /help",
    ] {
        assert!(command(&app, "bob", None, text).await.is_err(), "{text}");
    }
    let engine = app.engine_read().unwrap();
    assert!(engine.data == before);
    assert_eq!(engine.revision, revision);
    assert!(!engine.is_su(Some("bob")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_read_commands_preserve_output_permissions_and_state() {
    let directory = tempfile::tempdir().unwrap();
    contract(app(
        Engine::open(&directory.path().join("chat.sqlite")).unwrap()
    ))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn system_accepts_global_command_grants_and_tracks_revocation() {
    let directory = tempfile::tempdir().unwrap();
    let app = app(Engine::open(&directory.path().join("chat.sqlite")).unwrap());
    {
        let mut engine = app.engine().unwrap();
        for permission in ["/system", "r:server.config"] {
            engine
                .execute(None, None, &format!("/grant bob @global {permission}"))
                .unwrap();
        }
        assert!(
            engine
                .commands_for(Some("bob"), None)
                .iter()
                .any(|command| command.name == "/system")
        );
    }
    for text in [
        "/system",
        "/system configs",
        "/system metrics",
        "/system metrics --txt",
    ] {
        assert!(command(&app, "bob", None, text).await.is_ok(), "{text}");
        assert!(command(&app, "alice", None, text).await.is_err(), "{text}");
    }
    let text = command(&app, "bob", None, "/system metrics --txt")
        .await
        .unwrap();
    assert!(text.starts_with("```text\nSystem metrics"));
    assert!(text.ends_with("\n```"));
    assert!(!text.contains("| Metric |"));
    app.engine()
        .unwrap()
        .execute(None, None, "/revoke bob @global /system")
        .unwrap();
    assert!(command(&app, "bob", None, "/system metrics").await.is_err());
    app.engine()
        .unwrap()
        .execute(None, None, "/room new lobby")
        .unwrap();
    for permission in ["r:server.config", "/system"] {
        let scope = if permission == "/system" {
            "#lobby"
        } else {
            "@global"
        };
        app.engine()
            .unwrap()
            .execute(None, None, &format!("/grant alice {scope} {permission}"))
            .unwrap();
    }
    assert!(
        command(&app, "alice", None, "/system metrics")
            .await
            .is_err()
    );
    assert!(
        !app.engine_read()
            .unwrap()
            .commands_for(Some("alice"), None)
            .iter()
            .any(|command| command.name == "/system")
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/grant alice su")
        .unwrap();
    assert!(
        app.engine_read()
            .unwrap()
            .commands_for(Some("alice"), None)
            .iter()
            .any(|command| command.name == "/system")
    );
    assert!(
        command(&app, "alice", None, "/system")
            .await
            .unwrap()
            .starts_with("# System")
    );
    assert!(
        command(&app, "alice", None, "/system metrics")
            .await
            .unwrap()
            .contains("Snapshot cache")
    );
    assert!(
        command(&app, "alice", None, "/system configs")
            .await
            .unwrap()
            .contains("```json")
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/revoke alice su")
        .unwrap();
    assert!(
        command(&app, "alice", None, "/system metrics")
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_native_read_commands_preserve_output_permissions_and_state() {
    let fixture = crate::engine::database::tests::PostgresFixture::new();
    contract(app(Engine::open_postgres(&fixture.url).unwrap())).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_history_commands_release_engine_and_recheck_revocation() {
    for sudo in [false, true] {
        let fixture = crate::engine::database::tests::PostgresFixture::new();
        let mut app = app(Engine::open_postgres(&fixture.url).unwrap());
        app.io_jobs = Arc::new(tokio::sync::Semaphore::new(64));
        {
            let mut engine = app.engine().unwrap();
            engine.execute(None, None, "/room new secret").unwrap();
            engine
                .execute(None, Some("secret"), "must not leak")
                .unwrap();
            engine.execute(None, None, "/revoke bob user").unwrap();
            for permission in if sudo {
                vec!["/sudo", "x:command.sudo"]
            } else {
                vec!["/history", "r:message.read"]
            } {
                engine
                    .execute(
                        None,
                        None,
                        &format!(
                            "/grant bob {} {permission}",
                            if sudo { "@global" } else { "#secret" }
                        ),
                    )
                    .unwrap();
            }
        }
        let blocker = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
        let hold = blocker.unchecked_transaction().unwrap();
        hold.execute_batch("LOCK TABLE messages IN ACCESS EXCLUSIVE MODE")
            .unwrap();
        let reading = tokio::spawn({
            let app = app.clone();
            async move {
                command(
                    &app,
                    "bob",
                    None,
                    if sudo {
                        "/sudo /history #secret"
                    } else {
                        "/history #secret"
                    },
                )
                .await
            }
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
                "history command did not reach its reader"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(app.engine.try_read().is_ok());
        assert!(!app.engine_read().unwrap().is_su(Some("bob")));
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            tokio::task::spawn_blocking({
                let app = app.clone();
                move || {
                    app.engine()
                        .unwrap()
                        .execute(
                            None,
                            None,
                            if sudo {
                                "/revoke bob @global x:command.sudo"
                            } else {
                                "/revoke bob #secret r:message.read"
                            },
                        )
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
        assert!(result.is_err(), "revoked read leaked history: {result:?}");
        assert!(!app.engine_read().unwrap().is_su(Some("bob")));
    }
}
