use super::*;
use crate::engine::Engine;
use crate::web::media::tests::{app, request, storage_server};

fn setup(engine: Engine) -> App {
    let mut app = app(engine);
    app.io_jobs = Arc::new(tokio::sync::Semaphore::new(8));
    {
        let mut engine = app.engine_api().unwrap();
        engine
            .execute(Some("alice"), None, "/room new source")
            .unwrap();
        engine.execute(None, None, "/room add bob #source").unwrap();
        engine.execute(None, None, "/room new destination").unwrap();
        engine
            .execute(None, None, "/room add bob #destination")
            .unwrap();
    }
    app
}
async fn upload(app: &App, owner: &str) -> String {
    let (status, allocated) = request(
        app,
        owner,
        "POST",
        "/api/media/uploads",
        json!({"conversation_id":"@library","filename":"sticker.png","size":8}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{allocated}");
    let id = allocated["media_id"].as_str().unwrap();
    let (status, complete) = request(
        app,
        owner,
        "POST",
        &format!("/api/media/uploads/{id}/complete"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{complete}");
    id.into()
}
async fn action(app: &App, user: &str, id: &str, action: &str, confirm: bool) -> StatusCode {
    request(
        app,
        user,
        "POST",
        &format!("/api/library/{id}"),
        json!({"action":action,"confirm_last":confirm}),
    )
    .await
    .0
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn library_http_batches_sections_and_syncs_private_order_without_storage_work() {
    let mut app = setup(Engine::open(std::path::Path::new(":memory:")).unwrap());
    let (storage, server) = storage_server().await;
    app.media = Some(storage);
    let a = upload(&app, "alice").await;
    let b = upload(&app, "alice").await;
    let c = upload(&app, "alice").await;
    // Reordering and list reads work even with object storage disconnected.
    app.media = None;
    let mut changes = app.changes.subscribe();
    let (status, result) = request(
        &app,
        "alice",
        "POST",
        &format!("/api/library/{c}"),
        json!({"action":"move","before_id":a}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let account = app
        .engine_api()
        .unwrap()
        .active_account_id("alice")
        .unwrap()
        .to_owned();
    assert!(
        matches!(changes.recv().await.unwrap(), super::super::Change::Media(Some(target), None) if target == account)
    );
    let (status, batch) = request(
        &app,
        "alice",
        "GET",
        "/api/library?sections=true",
        json!(null),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{batch}");
    let ordered: Vec<_> = batch["library"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["media_id"].as_str().unwrap())
        .collect();
    assert_eq!(ordered, vec![c.as_str(), a.as_str(), b.as_str()]);
    assert!(
        batch["recent"]["items"].as_array().unwrap().is_empty(),
        "manual order does not affect recents"
    );
    for payload in [
        json!({"action":"move","before_id":a,"after_id":b}),
        json!({"action":"move","before_id":"invalid"}),
    ] {
        assert_eq!(
            request(&app, "alice", "POST", &format!("/api/library/{c}"), payload)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        request(
            &app,
            "bob",
            "POST",
            &format!("/api/library/{c}"),
            json!({"action":"move","before_id":a})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let (_, other) = request(
        &app,
        "bob",
        "GET",
        "/api/library?sections=true",
        json!(null),
    )
    .await;
    assert!(other["library"]["items"].as_array().unwrap().is_empty());
    assert_eq!(
        request(
            &app,
            "alice",
            "GET",
            "/api/library?sections=true&after=1",
            json!(null)
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn library_http_enforces_access_reuses_bytes_and_preserves_unavailable_messages() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("library.sqlite");
    let app = setup(Engine::open(&path).unwrap());
    let db = crate::engine::database::Connection::open(path).unwrap();
    library_read_contract(app, db).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run with --test-threads=1"]
async fn postgres_library_read_leases_preserve_visibility_and_retirement() {
    let fixture = crate::engine::database::tests::PostgresFixture::new();
    let engine = Engine::open_postgres(&fixture.url).unwrap();
    let db = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
    library_read_contract(setup(engine), db).await;
}

async fn resolve(app: &App, user: &str, id: &str) -> serde_json::Value {
    let (status, body) = request(
        app,
        user,
        "POST",
        "/api/media/resolve",
        json!({"items":[{"kind":"media","id":id,"link":true}]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["items"][0].clone()
}

async fn library_read_contract(mut app: App, db: crate::engine::database::Connection) {
    let (storage, server) = storage_server().await;
    app.media = Some(storage);
    let id = upload(&app, "alice").await;
    assert_eq!(
        action(&app, "bob", &id, "save", false).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        action(&app, "carol", &id, "acquire", false).await,
        StatusCode::NOT_FOUND
    );
    let text = format!("![sticker](/api/media/{id}/file)");
    app.engine_api()
        .unwrap()
        .execute(Some("alice"), Some("source"), &text)
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
    let (_, before_save) = request(
        &app,
        "bob",
        "GET",
        &format!("/api/media/{id}/info"),
        json!(null),
    )
    .await;
    assert_eq!(before_save["saved"], false);
    let read = resolve(&app, "bob", &id).await;
    assert_eq!(read["status"], 200);
    assert_eq!(read["info"]["library"], true);
    assert_eq!(read["info"]["saved"], false);
    assert_eq!(resolve(&app, "carol", &id).await["status"], 404);

    let (_, owner_info) = request(
        &app,
        "alice",
        "GET",
        &format!("/api/media/{id}/info"),
        json!(null),
    )
    .await;
    assert_eq!(owner_info["saved"], true);
    assert_eq!(
        request(
            &app,
            "carol",
            "GET",
            &format!("/api/media/{id}/info"),
            json!(null)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let source = app.engine_read().unwrap().data.rooms["source"].id.clone();
    assert_ne!(
        request(
            &app,
            "carol",
            "POST",
            &format!("/api/library/{id}"),
            json!({"action":"save","source_conversation":source})
        )
        .await
        .0,
        StatusCode::OK
    );
    for _ in 0..2 {
        assert_eq!(
            action(&app, "bob", &id, "save", false).await,
            StatusCode::OK
        );
        assert_eq!(
            action(&app, "bob", &id, "acquire", false).await,
            StatusCode::OK
        );
    }
    let (_, after_save) = request(
        &app,
        "bob",
        "GET",
        &format!("/api/media/{id}/info"),
        json!(null),
    )
    .await;
    assert_eq!(after_save["saved"], true);
    app.engine_api()
        .unwrap()
        .execute(Some("alice"), Some("source"), "/kick bob #source")
        .unwrap();
    // Independent saved access survives loss of source membership and reuses the
    // same media ID in another room without an upload reservation.
    app.engine_api()
        .unwrap()
        .execute(Some("bob"), Some("destination"), &text)
        .unwrap();
    let read = resolve(&app, "bob", &id).await;
    assert_eq!(read["status"], 200);
    assert_eq!(
        read["info"]["saved"], true,
        "independent saved visibility survives source removal"
    );
    let (_, owned) = request(&app, "bob", "GET", "/api/library?owned=true", json!(null)).await;
    assert_eq!(owned["items"].as_array().unwrap().len(), 1);
    assert_eq!(owned["usage"], 8);
    assert_eq!(owned["items"][0]["owner_count"], 2);
    assert_eq!(
        action(&app, "alice", &id, "remove", false).await,
        StatusCode::OK
    );
    let (_, after_remove) = request(
        &app,
        "alice",
        "GET",
        &format!("/api/media/{id}/info"),
        json!(null),
    )
    .await;
    assert_eq!(after_remove["saved"], false);
    assert_eq!(
        action(&app, "alice", &id, "release", false).await,
        StatusCode::OK
    );
    assert_eq!(
        action(&app, "bob", &id, "delete", false).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        action(&app, "bob", &id, "release", false).await,
        StatusCode::CONFLICT
    );
    assert_eq!(
        action(&app, "bob", &id, "release", true).await,
        StatusCode::OK
    );
    let read = resolve(&app, "bob", &id).await;
    assert_eq!(read["status"], 200);
    assert_eq!(read["info"]["unavailable"], false);
    assert!(
        read.get("link").is_some(),
        "final release preserves referenced bytes during grace"
    );
    assert!(read["info"]["expires"].as_u64().unwrap() > crate::engine::media::timestamp());
    let mut changes = app.changes.subscribe();
    assert_eq!(
        action(&app, "bob", &id, "acquire", false).await,
        StatusCode::OK
    );
    assert!(
        matches!(
            changes.recv().await.unwrap(),
            super::super::Change::Media(None, None)
        ),
        "restoring permanent ownership refreshes all viewers' expiry leases"
    );
    assert!(resolve(&app, "bob", &id).await["info"]["expires"].is_null());
    assert_eq!(
        action(&app, "bob", &id, "release", true).await,
        StatusCode::OK
    );
    let deadline = crate::engine::media::timestamp() + 9;
    db.execute(
        "UPDATE media SET expires=$2 WHERE id=$1",
        crate::engine::database::params![&id, deadline],
    )
    .unwrap();
    let capped = resolve(&app, "bob", &id).await;
    assert!((1..=9).contains(&capped["lease_seconds"].as_u64().unwrap()));
    assert!((1..=9).contains(&capped["link"]["expires_in"].as_u64().unwrap()));
    db.execute(
        "UPDATE media SET expires=$2 WHERE id=$1",
        crate::engine::database::params![&id, crate::engine::media::timestamp() - 1],
    )
    .unwrap();
    let expired = resolve(&app, "bob", &id).await;
    assert_eq!(expired["info"]["expired"], true);
    assert!(expired.get("link").is_none());
    assert_eq!(
        action(&app, "bob", &id, "acquire", false).await,
        StatusCode::NOT_FOUND
    );
    for endpoint in ["info", "link", "file"] {
        assert_eq!(
            request(
                &app,
                "bob",
                "GET",
                &format!("/api/media/{id}/{endpoint}"),
                json!(null)
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
    let (_, expired_list) = request(&app, "bob", "GET", "/api/library", json!(null)).await;
    assert!(expired_list["items"].as_array().unwrap().is_empty());
    assert_eq!(
        action(&app, "alice", &id, "delete", false).await,
        StatusCode::OK
    );
    let read = resolve(&app, "bob", &id).await;
    assert_eq!(read["info"]["unavailable"], true);
    assert!(
        read.get("link").is_none(),
        "retired media never gets a signed link"
    );

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
    assert_eq!(
        request(
            &app,
            "bob",
            "GET",
            &format!("/api/media/{id}/link"),
            json!(null)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let (status, info) = request(
        &app,
        "bob",
        "GET",
        &format!("/api/media/{id}/info?include_expired=true"),
        json!(null),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(info["unavailable"], true);
    assert_eq!(info["filename"], "sticker.png");
    let store = app.engine_read().unwrap().media_store();
    let (retired, _) = store.load(&id).await.unwrap();
    store
        .cleanup_done(std::slice::from_ref(&retired), std::slice::from_ref(&id))
        .await
        .unwrap();
    let (status, cleaned) = request(
        &app,
        "bob",
        "GET",
        &format!("/api/media/{id}/info?include_expired=true"),
        json!(null),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cleaned["unavailable"], true);
    assert_eq!(cleaned["filename"], "sticker.png");
    assert_eq!(
        action(&app, "bob", &id, "acquire", false).await,
        StatusCode::NOT_FOUND
    );
    let (_, library) = request(&app, "bob", "GET", "/api/library", json!(null)).await;
    assert_eq!(library["usage"], 0);
    assert!(library["items"].as_array().unwrap().is_empty());
    assert_eq!(
        app.engine_read()
            .unwrap()
            .history("bob", "destination")
            .unwrap()
            .messages
            .len(),
        1
    );
    assert_eq!(
        action(&app, "bob", &id, "remove", false).await,
        StatusCode::OK
    );
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn library_http_orders_final_release_and_acquisition_and_refunds_admin_removal() {
    let mut app = setup(Engine::open(std::path::Path::new(":memory:")).unwrap());
    let (storage, server) = storage_server().await;
    app.media = Some(storage);
    let id = upload(&app, "alice").await;
    let text = format!("![sticker](/api/media/{id}/file)");
    app.engine_api()
        .unwrap()
        .execute(Some("alice"), Some("source"), &text)
        .unwrap();
    assert_eq!(
        action(&app, "bob", &id, "save", false).await,
        StatusCode::OK
    );
    let (released, acquired) = tokio::join!(
        action(&app, "alice", &id, "release", true),
        action(&app, "bob", &id, "acquire", false)
    );
    assert_eq!(released, StatusCode::OK);
    let (_, owned) = request(&app, "bob", "GET", "/api/library?owned=true", json!(null)).await;
    assert_eq!(
        acquired,
        StatusCode::OK,
        "either ordering preserves a reacquirable grace period"
    );
    {
        assert_eq!(owned["usage"], 8);
        assert_eq!(owned["items"][0]["owner_count"], 1);
        assert_eq!(
            action(&app, "alice", &id, "delete", false).await,
            StatusCode::OK
        );
        let (_, owned) = request(&app, "bob", "GET", "/api/library?owned=true", json!(null)).await;
        assert_eq!(owned["usage"], 0);
        assert!(owned["items"].as_array().unwrap().is_empty());
    }
    let shared = upload(&app, "alice").await;
    app.engine_api()
        .unwrap()
        .execute(
            Some("alice"),
            Some("source"),
            &format!("![shared](/api/media/{shared}/file)"),
        )
        .unwrap();
    assert_eq!(
        action(&app, "bob", &shared, "save", false).await,
        StatusCode::OK
    );
    assert_eq!(
        action(&app, "bob", &shared, "acquire", false).await,
        StatusCode::OK
    );
    assert_eq!(
        action(&app, "alice", &shared, "delete", false).await,
        StatusCode::OK
    );
    assert_eq!(
        action(&app, "alice", &shared, "delete", false).await,
        StatusCode::OK
    );
    for user in ["alice", "bob"] {
        let (_, owned) = request(&app, user, "GET", "/api/library?owned=true", json!(null)).await;
        assert_eq!(owned["usage"], 0);
        assert!(owned["items"].as_array().unwrap().is_empty());
    }
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn library_invalidations_scope_asset_ids_to_private_audiences() {
    use super::super::Change;
    let app = setup(Engine::open(std::path::Path::new(":memory:")).unwrap());
    let bob = app
        .engine_read()
        .unwrap()
        .active_account_id("bob")
        .unwrap()
        .to_owned();
    let asset = uuid::Uuid::new_v4().to_string();
    for (user, expected) in [("alice", 0), ("bob", 1)] {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            format!("chat_session={user}").parse().unwrap(),
        );
        let mut interests = super::super::transport::Interests::default();
        let (_, frames) = super::super::updates::browser_with_interests_async(
            &app,
            &headers,
            false,
            &[super::super::Change::Media(Some(bob.clone()), None)],
            &mut interests,
        )
        .await
        .unwrap();
        assert_eq!(frames.len(), expected);
        let targeted = Change::Media(Some(bob.clone()), Some(asset.clone()));
        let (_, frames) = super::super::updates::browser_with_interests_async(
            &app,
            &headers,
            false,
            &[targeted],
            &mut interests,
        )
        .await
        .unwrap();
        assert_eq!(frames.len(), expected);
        if expected == 1 {
            let text = frames[0].as_str();
            let frame: serde_json::Value = serde_json::from_str(text).unwrap();
            assert_eq!(frame, json!({"kind":"media","ids":[asset]}));
        }
        let (_, frames) = super::super::updates::browser_with_interests_async(
            &app,
            &headers,
            false,
            &[super::super::Change::Media(None, Some(asset.clone()))],
            &mut interests,
        )
        .await
        .unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(frames[0].as_str()).unwrap(),
            json!({"kind":"media"})
        );
    }
}
