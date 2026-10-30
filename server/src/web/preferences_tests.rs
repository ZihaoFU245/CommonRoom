use super::*;
use crate::web::media::tests::{app, request};

async fn write(
    app: &App,
    user: &str,
    values: serde_json::Value,
    initialize: bool,
) -> (StatusCode, serde_json::Value) {
    request(
        app,
        user,
        "POST",
        "/api/preferences",
        json!({"values":values,"initialize":initialize}),
    )
    .await
}

#[tokio::test]
async fn preferences_seed_missing_keys_patch_reset_and_private_snapshots() {
    let app = app(Engine::open(std::path::Path::new(":memory:")).unwrap());
    let mut notices = app.changes.subscribe();
    let first = write(
        &app,
        "bob",
        json!({"theme":"dark","name_colors":"true"}),
        true,
    )
    .await;
    assert_eq!(first.0, StatusCode::OK, "{}", first.1);
    let Change::Metadata(notice) = notices.try_recv().unwrap() else {
        panic!("missing metadata notice");
    };
    {
        let engine = app.engine_read().unwrap();
        assert!(notice.targets(engine.account_id("bob").unwrap()));
        assert!(!notice.targets(engine.account_id("alice").unwrap()));
        assert!(engine.snapshot("alice").unwrap().preferences.is_empty());
        assert_eq!(engine.snapshot("bob").unwrap().preferences["theme"], "dark");
    }

    let second = write(&app, "bob", json!({"theme":"light","latex":"true"}), true).await;
    assert_eq!(second.1["values"]["theme"], "dark");
    assert_eq!(second.1["values"]["latex"], "true");
    let patch = write(&app, "bob", json!({"theme":"custom"}), false).await;
    assert_eq!(patch.1["values"]["name_colors"], "true");
    let reset = write(&app, "bob", json!({"theme":null}), false).await;
    assert!(reset.1["values"].get("theme").is_none());
    let seeded = write(&app, "bob", json!({"theme":"light","latex":"false"}), true).await;
    assert_eq!(seeded.1["values"]["theme"], "light");
    assert_eq!(seeded.1["values"]["latex"], "true");

    // Existing generic metadata storage remains the source of truth.
    assert_eq!(
        app.engine_read().unwrap().data.users["bob"].metadata["@global"]["preferences.theme"],
        "light"
    );
    let repeated = write(&app, "bob", json!({"theme":"dark"}), true).await;
    assert_eq!(repeated.1["values"]["theme"], "light");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preferences_concurrent_login_first_commit_wins_and_invalid_writes_are_atomic() {
    let mut app = app(Engine::open(std::path::Path::new(":memory:")).unwrap());
    app.io_jobs = Arc::new(tokio::sync::Semaphore::new(8));
    let (a, b) = tokio::join!(
        write(&app, "bob", json!({"theme":"dark","font.chat":"20"}), true),
        write(&app, "bob", json!({"theme":"light","font.chat":"14"}), true),
    );
    assert_eq!(a.0, StatusCode::OK, "{}", a.1);
    assert_eq!(b.0, StatusCode::OK, "{}", b.1);
    assert_eq!(a.1["values"], b.1["values"]);
    let before = app.engine_read().unwrap().data.clone();
    let mut notices = app.changes.subscribe();
    for values in [
        json!({"theme":"blue"}),
        json!({"font.chat":"99"}),
        json!({"unknown":null}),
        json!({"theme":"light","allow_html":"yes"}),
        json!({"media.user_quota":"1"}),
    ] {
        assert_eq!(
            write(&app, "bob", values, false).await.0,
            StatusCode::BAD_REQUEST
        );
        assert!(app.engine_read().unwrap().data == before);
        assert!(notices.try_recv().is_err());
    }
    assert_eq!(
        write(&app, "expired", json!({"theme":"dark"}), false)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            "bob",
            "POST",
            "/api/preferences",
            json!({"values":{"theme":"dark"},"user":"alice"})
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn preferences_failed_commit_rolls_back_and_does_not_consume_metadata_allowance() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.db");
    let app = app(Engine::open(&path).unwrap());
    for n in 0..32 {
        app.engine()
            .unwrap()
            .execute(None, None, &format!("/user set bob key{n} value @global"))
            .unwrap();
    }
    let saved = write(&app, "bob", json!({"theme":"dark"}), true).await;
    assert_eq!(saved.0, StatusCode::OK, "{}", saved.1);
    let before = app.engine_read().unwrap().data.clone();
    let mut notices = app.changes.subscribe();
    let db = crate::engine::database::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER reject_preferences BEFORE UPDATE ON entities WHEN OLD.kind='account' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert_eq!(
        write(&app, "bob", json!({"theme":"light"}), false).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(app.engine_read().unwrap().data == before);
    assert!(notices.try_recv().is_err());
    db.execute_batch("DROP TRIGGER reject_preferences;")
        .unwrap();
    drop(db);
    drop(app);
    let reopened = Engine::open(&path).unwrap();
    assert_eq!(reopened.preferences("bob")["theme"], "dark");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preferences_commits_reach_every_connected_device_without_relogin() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
    type Socket = tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >;
    async fn baseline(socket: &mut Socket, theme: Option<&str>) -> serde_json::Value {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                match socket.next().await.unwrap().unwrap() {
                    Message::Text(text) => {
                        let frame: serde_json::Value = serde_json::from_str(&text).unwrap();
                        if frame["kind"] == "snapshot"
                            && theme.is_none_or(|theme| frame["preferences"]["theme"] == theme)
                        {
                            return frame;
                        }
                    }
                    Message::Ping(bytes) => socket.send(Message::Pong(bytes)).await.unwrap(),
                    _ => {}
                }
            }
        })
        .await
        .unwrap()
    }
    let app = app(Engine::open(std::path::Path::new(":memory:")).unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(super::super::http_server::serve(
        listener,
        router(app.clone()),
        {
            let app = app.clone();
            async move {
                let _ = stopped.await;
                app.stopping
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                app.shutdown.notify_waiters();
            }
        },
    ));
    let mut request = format!("ws://{address}/ws").into_client_request().unwrap();
    request
        .headers_mut()
        .insert("origin", "http://localhost:5173".parse().unwrap());
    request
        .headers_mut()
        .insert("cookie", "chat_session=bob".parse().unwrap());
    let (mut first, _) = tokio_tungstenite::connect_async(request.clone())
        .await
        .unwrap();
    let (mut second, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    assert_eq!(baseline(&mut first, None).await["preferences"], json!({}));
    assert_eq!(baseline(&mut second, None).await["preferences"], json!({}));
    let updated = write(
        &app,
        "bob",
        json!({"theme":"dark","allow_html":"true"}),
        true,
    )
    .await;
    assert_eq!(updated.0, StatusCode::OK, "{}", updated.1);
    for socket in [&mut first, &mut second] {
        let frame = baseline(socket, Some("dark")).await;
        assert_eq!(frame["preferences"]["allow_html"], "true");
    }
    assert_eq!(
        write(&app, "bob", json!({"theme":"light"}), false).await.0,
        StatusCode::OK
    );
    for socket in [&mut first, &mut second] {
        let frame = baseline(socket, Some("light")).await;
        assert_eq!(frame["preferences"]["allow_html"], "true");
    }
    first.close(None).await.unwrap();
    second.close(None).await.unwrap();
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn preferences_are_root_only_and_metadata_authority_cannot_change_or_reset_them() {
    let app = app(Engine::open(std::path::Path::new(":memory:")).unwrap());
    assert_eq!(
        write(&app, "bob", json!({"theme":"dark"}), true).await.0,
        StatusCode::OK
    );
    {
        let mut engine = app.engine().unwrap();
        engine.execute(None, None, "/grant alice su").unwrap();
        engine.execute(None, None, "/room new scoped").unwrap();
        engine
            .execute(None, None, "/user set bob label ordinary @global")
            .unwrap();
        let before = engine.data.clone();
        for actor in [Some("alice"), None] {
            for command in [
                "/user set bob preferences.theme light @global",
                "/user set bob preferences.theme light #scoped",
                "/user set @all preferences.theme light @global",
                "/user reset bob preferences.theme @global",
                "/user reset @all preferences.theme @global",
                "/user set bob preferences.theme @unset @global",
            ] {
                assert!(engine.execute(actor, None, command).is_err(), "{command}");
                assert!(engine.data == before);
            }
        }
        engine
            .execute(Some("alice"), None, "/user reset @all @all @global")
            .unwrap();
        assert_eq!(engine.preferences("bob")["theme"], "dark");
        assert!(!engine.data.users["bob"].metadata["@global"].contains_key("label"));
        engine
            .execute(None, None, "/user reset bob @all @global")
            .unwrap();
        assert_eq!(engine.preferences("bob")["theme"], "dark");
        assert_eq!(engine.data.users["bob"].metadata.len(), 1);
    }
    // A caller cannot select a scope or another account on the self-service route.
    assert_eq!(
        request(
            &app,
            "alice",
            "POST",
            "/api/preferences",
            json!({"values":{"theme":"light"},"scope":"@global","user":"bob"})
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        write(&app, "alice", json!({"theme":"light"}), false)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        app.engine_read().unwrap().preferences("bob")["theme"],
        "dark"
    );
    assert_eq!(
        write(&app, "bob", json!({"theme":null}), false).await.0,
        StatusCode::OK
    );
    assert!(app.engine_read().unwrap().preferences("bob").is_empty());
}
