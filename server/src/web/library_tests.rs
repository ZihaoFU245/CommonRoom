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
async fn library_http_enforces_access_reuses_bytes_and_preserves_unavailable_messages() {
    let mut app = setup(Engine::open(std::path::Path::new(":memory:")).unwrap());
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
    assert_eq!(library["items"][0]["available"], false);
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
    if acquired == StatusCode::OK {
        assert_eq!(owned["usage"], 8);
        assert_eq!(owned["items"][0]["owner_count"], 1);
        assert_eq!(
            action(&app, "alice", &id, "delete", false).await,
            StatusCode::OK
        );
        let (_, owned) = request(&app, "bob", "GET", "/api/library?owned=true", json!(null)).await;
        assert_eq!(owned["usage"], 0);
        assert!(owned["items"].as_array().unwrap().is_empty());
    } else {
        assert_eq!(acquired, StatusCode::NOT_FOUND);
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
async fn library_invalidations_preserve_private_audiences_and_omit_asset_identity() {
    let app = setup(Engine::open(std::path::Path::new(":memory:")).unwrap());
    let bob = app
        .engine_read()
        .unwrap()
        .active_account_id("bob")
        .unwrap()
        .to_owned();
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
            &[super::super::Change::Media(Some(bob.clone()))],
            &mut interests,
        )
        .await
        .unwrap();
        assert_eq!(frames.len(), expected);
        let (_, frames) = super::super::updates::browser_with_interests_async(
            &app,
            &headers,
            false,
            &[super::super::Change::Media(None)],
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
