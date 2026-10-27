use super::*;
use axum::{
    body::{Body, to_bytes},
    http::{Request, header},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

pub(in crate::web) fn app(mut engine: Engine) -> App {
    for (user, admin) in [("alice", true), ("bob", false), ("carol", false)] {
        engine.provision(user, "hash".into(), admin, false).unwrap();
        engine.login(user.into(), user, "hash", None).unwrap();
    }
    App {
        engine: Arc::new(RwLock::new(engine)),
        media: None,
        changes: broadcast::channel(16).0,
        config: Arc::new(Config::default()),
        attempts: Default::default(),
        admission: Default::default(),
        io_jobs: Arc::new(tokio::sync::Semaphore::new(1)),
        password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
        snapshots: Default::default(),
        mutations: Default::default(),
        bot_runtimes: Default::default(),
        stopping: Default::default(),
        shutdown: Default::default(),
    }
}
pub(in crate::web) async fn request(
    app: &App,
    user: &str,
    method: &str,
    path: &str,
    input: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = router(app.clone())
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::COOKIE, format!("chat_session={user}"))
                .header(header::ORIGIN, "http://localhost:5173")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(input.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}
pub(in crate::web) async fn storage_server()
-> (Arc<crate::media::MediaStorage>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let storage =
        crate::media::tests::test_storage(&format!("http://{}", listener.local_addr().unwrap()));
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut chunk = [0; 4096];
                let size = socket.read(&mut chunk).await.unwrap();
                if size == 0 {
                    break;
                }
                bytes.extend_from_slice(&chunk[..size]);
                if bytes.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
            let response = if request.starts_with("head ") {
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    if request.contains(".availability") {
                        0
                    } else {
                        8
                    }
                )
                .into_bytes()
            } else if request.starts_with("put ") {
                let body = "<CopyObjectResult><ETag>test</ETag></CopyObjectResult>";
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .into_bytes()
            } else if request.starts_with("get ") {
                b"HTTP/1.1 206 Partial Content\r\nContent-Length: 8\r\nContent-Range: bytes 0-7/8\r\nConnection: close\r\n\r\n\x89PNG\r\n\x1a\n".to_vec()
            } else if request.starts_with("post ") {
                // Exercise the provider's batch-delete fallback too.
                b"HTTP/1.1 501 Not Implemented\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_vec()
            } else {
                assert!(request.starts_with("delete "));
                b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_vec()
            };
            socket.write_all(&response).await.unwrap();
        }
    });
    (storage, server)
}
async fn contract(mut app: App, db: crate::engine::database::Connection, postgres: bool) {
    let (storage, server) = storage_server().await;
    app.media = Some(storage);
    let mut changes = app.changes.subscribe();
    let bad = request(&app, "alice", "POST", "/api/media/uploads", json!({"conversation_id":"@direct:bob","filename":"bad","size":8,"hints":{"unknown":"bad"}})).await;
    assert_eq!(bad.0, StatusCode::BAD_REQUEST);
    assert!(app.engine_read().unwrap().data.private.is_empty());
    let reserved = request(
        &app,
        "alice",
        "POST",
        "/api/media/uploads",
        json!({"conversation_id":"@direct:bob","filename":"photo.png","size":8}),
    )
    .await;
    assert_eq!(reserved.0, StatusCode::OK, "{}", reserved.1);
    let id = reserved.1["media_id"].as_str().unwrap();
    assert_eq!(app.engine_read().unwrap().data.private.len(), 1);
    assert!(changes.try_recv().is_err(), "reservation must stay silent");
    assert_eq!(
        request(
            &app,
            "alice",
            "GET",
            &format!("/api/media/{id}/info"),
            json!(null)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(
            &app,
            "bob",
            "POST",
            &format!("/api/media/uploads/{id}/complete"),
            json!(null)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let completed = request(
        &app,
        "alice",
        "POST",
        &format!("/api/media/uploads/{id}/complete"),
        json!(null),
    )
    .await;
    assert_eq!(completed.0, StatusCode::OK, "{}", completed.1);
    let store = app.engine_read().unwrap().media_store();
    let (media, attached) = store.load(id).await.unwrap();
    assert!(media.ready && !attached);
    assert!(media.expires.is_some());
    assert_eq!(media.content_type, "image/png");
    let mut headers = HeaderMap::new();
    headers.insert(header::COOKIE, "chat_session=alice".parse().unwrap());
    crate::web::commands::execute_input(
        &app,
        &headers,
        "alice",
        &crate::web::commands::Input {
            id: 1,
            room: None,
            text: format!("/tell bob ![photo](/api/media/{id}/file)"),
            markdown: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        request(
            &app,
            "bob",
            "GET",
            &format!("/api/media/{id}/info"),
            json!(null)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            "bob",
            "GET",
            &format!("/api/media/{id}/file"),
            json!(null)
        )
        .await
        .0,
        StatusCode::TEMPORARY_REDIRECT
    );
    assert_eq!(
        request(
            &app,
            "alice",
            "POST",
            &format!("/api/media/uploads/{id}/abandon"),
            json!(null)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert!(
        !store.load(id).await.unwrap().0.deleting,
        "published upload was abandoned"
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/revoke bob user")
        .unwrap();
    assert_eq!(
        request(
            &app,
            "bob",
            "GET",
            &format!("/api/media/{id}/info"),
            json!(null)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/grant bob user")
        .unwrap();
    app.engine()
        .unwrap()
        .execute(None, None, "/user set alice media.user_quota 8 @global")
        .unwrap();
    let denied = request(
        &app,
        "alice",
        "POST",
        "/api/media/uploads",
        json!({"conversation_id":"@direct:carol","filename":"pending.png","size":8}),
    )
    .await;
    assert_eq!(denied.0, StatusCode::BAD_REQUEST, "{}", denied.1);
    assert!(
        !app.engine_read()
            .unwrap()
            .data
            .private
            .contains_key("alice:carol")
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/user set alice media.user_quota 16 @global")
        .unwrap();
    let pending = request(
        &app,
        "alice",
        "POST",
        "/api/media/uploads",
        json!({"conversation_id":"@direct:carol","filename":"pending.png","size":8}),
    )
    .await;
    assert_eq!(pending.0, StatusCode::OK, "{}", pending.1);
    let pending_id = pending.1["media_id"].as_str().unwrap();
    assert_eq!(
        request(
            &app,
            "alice",
            "POST",
            &format!("/api/media/uploads/{pending_id}/abandon"),
            json!(null)
        )
        .await
        .0,
        StatusCode::OK
    );
    let created =
        crate::engine::media::timestamp().saturating_sub(app.config.media.pending_ttl + 1);
    db.execute(if postgres {
        "UPDATE media SET created=$2,json=(json::jsonb || jsonb_build_object('created',$2::bigint))::text WHERE id=$1"
    } else { "UPDATE media SET created=?2,json=json_set(json,'$.created',?2) WHERE id=?1" }, crate::engine::database::params![pending_id, created]).unwrap();
    assert!(cleanup(&app).await.unwrap());
    assert!(
        store.load(pending_id).await.is_err(),
        "old confirmed-deleted staging record was not reaped"
    );
    assert!(
        store.load(id).await.unwrap().0.ready,
        "cleanup deleted a retained attachment"
    );
    assert_eq!(app.io_jobs.available_permits(), 1);
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn native_media_http_and_cleanup_keep_quota_permissions_and_private_reservations_atomic() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("chat.sqlite");
    let app = app(Engine::open(&path).unwrap());
    let db = crate::engine::database::Connection::open(path).unwrap();
    contract(app, db, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_native_media_http_and_cleanup_keep_quota_permissions_and_private_reservations_atomic()
 {
    let fixture = crate::engine::database::tests::PostgresFixture::new();
    let app = app(Engine::open_postgres(&fixture.url).unwrap());
    let db = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
    contract(app, db, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_cancelled_media_reservation_releases_engine_and_orders_revocation() {
    let fixture = crate::engine::database::tests::PostgresFixture::new();
    let mut app = app(Engine::open_postgres(&fixture.url).unwrap());
    app.io_jobs = Arc::new(tokio::sync::Semaphore::new(4));
    let (storage, server) = storage_server().await;
    app.media = Some(storage);
    let blocker = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
    let hold = blocker.unchecked_transaction().unwrap();
    hold.query_row(
        "SELECT key FROM entities WHERE kind='account' AND key='alice' FOR UPDATE",
        [],
        |row| row.get::<_, String>(0),
    )
    .unwrap();
    let mut changes = app.changes.subscribe();
    let reserving = tokio::spawn({
        let app = app.clone();
        async move {
            request(
                &app,
                "alice",
                "POST",
                "/api/media/uploads",
                json!({"conversation_id":"@direct:bob","filename":"accepted.png","size":8}),
            )
            .await
        }
    });
    let inspect = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        let waiting: bool = inspect.query_row("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT key FROM entities WHERE kind=%FOR UPDATE')", [], |row| row.get(0)).unwrap();
        if waiting {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "reservation did not reach its quota lock"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(app.engine.try_read().is_ok());
    assert!(
        app.engine_read().unwrap().data.private.is_empty(),
        "pending reservation leaked its directory"
    );
    let config = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        request(&app, "bob", "GET", "/api/media/config", json!(null)),
    )
    .await
    .unwrap();
    assert_eq!(config.0, StatusCode::OK);
    let independent = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        request(
            &app,
            "carol",
            "POST",
            "/api/media/uploads",
            json!({"conversation_id":"@direct:bob","filename":"independent.png","size":8}),
        ),
    )
    .await
    .unwrap();
    assert_eq!(independent.0, StatusCode::OK, "{}", independent.1);
    reserving.abort();
    assert!(reserving.await.unwrap_err().is_cancelled());
    let (started, waiting) = tokio::sync::oneshot::channel();
    let metadata = tokio::task::spawn_blocking({
        let app = app.clone();
        move || {
            started.send(()).unwrap();
            app.engine()
                .unwrap()
                .execute(None, None, "/revoke alice admin")
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
    {
        let engine = app.engine_read().unwrap();
        assert!(engine.data.private.contains_key("alice:bob"));
        assert!(engine.data.private.contains_key("bob:carol"));
        assert_eq!(engine.data.next_sequence, 0);
    }
    let id: String = inspect.query_row("SELECT id FROM media WHERE uploader=(SELECT projection #>> '{id}' FROM entities WHERE kind='account' AND key='alice')", [], |row| row.get(0)).unwrap();
    let store = app.engine_read().unwrap().media_store();
    assert!(!store.load(&id).await.unwrap().0.ready);
    assert_eq!(
        request(
            &app,
            "alice",
            "POST",
            &format!("/api/media/uploads/{id}/complete"),
            json!(null)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert!(
        changes.try_recv().is_err(),
        "upload reservations must not broadcast directories"
    );
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}
