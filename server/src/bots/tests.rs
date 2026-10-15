use super::*;
use crate::web::{self, commands};
use crate::{config::Config, engine::Engine};
use axum::{
    body::{Body, to_bytes},
    http::{Request, header},
};
use std::{
    path::Path,
    sync::{Arc, RwLock},
};
use tower::ServiceExt;
#[path = "command_tests.rs"]
mod command_tests;

fn fixture() -> (App, String) {
    let mut engine = Engine::open(Path::new(":memory:")).unwrap();
    engine
        .provision("admin", "hash".into(), true, false)
        .unwrap();
    engine
        .execute(Some("admin"), None, "/new room support")
        .unwrap();
    engine.execute(None, None, "/new room secret").unwrap();
    engine
        .execute(Some("admin"), Some("support"), "/bot new helper")
        .unwrap();
    let secret = engine
        .execute(Some("admin"), None, "/bot token new helper test")
        .unwrap()
        .lines()
        .last()
        .unwrap()
        .to_owned();
    let (changes, _) = tokio::sync::broadcast::channel(16);
    (
        App {
            engine: Arc::new(RwLock::new(engine)),
            attempts: Default::default(),
            changes,
            config: Arc::new(Config::default()),
            connections: Arc::new(tokio::sync::Semaphore::new(128)),
            io_jobs: Arc::new(tokio::sync::Semaphore::new(64)),
            password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
            snapshots: Default::default(),
            bot_runtimes: Default::default(),
            stopping: Default::default(),
        },
        secret,
    )
}
async fn request(
    app: &App,
    secret: Option<&str>,
    method: &str,
    path: &str,
    value: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if method == "POST" && path == "/api/bot/v1/commands" {
        builder = builder.header("idempotency-key", uuid::Uuid::new_v4().to_string());
    }
    if let Some(secret) = secret {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {secret}"));
    }
    let response = web::router(app.clone())
        .oneshot(builder.body(Body::from(value.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    let value = serde_json::from_slice(&body)
        .unwrap_or_else(|_| json!({"raw":String::from_utf8_lossy(&body)}));
    (status, value)
}

#[tokio::test]
async fn removed_remote_config_is_rejected_by_browser_and_engine() {
    let (app, _) = fixture();
    app.engine()
        .unwrap()
        .login("operator".into(), "admin", "hash", None)
        .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(header::COOKIE, "chat_session=operator".parse().unwrap());
    for command in [
        "/bot config set helper prompt test",
        "/bot config fetch helper",
    ] {
        for prefix in ["", "/sudo "] {
            let input = commands::Input {
                markdown: false,
                id: 1,
                room: None,
                text: format!("{prefix}{command}"),
            };
            assert!(
                commands::execute_input(&app, &headers, "admin", &input)
                    .await
                    .is_err()
            );
            assert!(
                app.engine()
                    .unwrap()
                    .execute(None, None, &input.text)
                    .is_err()
            );
        }
    }
    assert!(!crate::commands::help(true, true).contains("config fetch"));
}

fn conversation(app: &App) -> String {
    app.engine_read().unwrap().data.rooms["support"].id.clone()
}
async fn send(
    app: &App,
    token: &str,
    key: &str,
    input: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = web::router(app.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/bot/v1/messages")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header("idempotency-key", key)
                .body(Body::from(input.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["bot-api-version"], "1");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
#[tokio::test]
async fn profile_is_bearer_only_scoped_and_has_no_legacy_command_transport() {
    let (app, token) = fixture();
    assert_eq!(
        request(&app, None, "GET", "/api/bot/v1/me", json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, snapshot) = request(&app, Some(&token), "GET", "/api/bot/v1/me", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(snapshot["account"]["username"], "helper");
    assert_eq!(snapshot["conversations"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["conversations"][0]["id"], conversation(&app));
    assert_eq!(snapshot["conversations"][0]["permissions"]["reply"], true);
    assert!(snapshot.get("rooms").is_none());
    for path in ["/api/bot/me", "/api/bot/ws"] {
        assert_eq!(
            request(&app, Some(&token), "GET", path, json!({})).await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            request(
                &app,
                Some(&token),
                "POST",
                path,
                json!({"command":"/whoami"})
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
    let (_, schema) = request(&app, Some(&token), "GET", "/api/bot/v1/schema", json!({})).await;
    assert_eq!(schema["$id"], "urn:commonroom:bot-api:v1");
    assert!(schema["definitions"]["ServerFrame"].is_object());
}
#[tokio::test]
async fn bot_markdown_defaults_on_and_plain_text_remains_explicit() {
    let (app, token) = fixture();
    let id = conversation(&app);
    let input = json!({"conversation_id":id,"text":"**default**"});
    let (status, sent) = send(&app, &token, "markdown-default", input.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(sent["message"]["markdown"], true);
    let mut explicit = input;
    explicit["markdown"] = json!(true);
    let (status, replay) = send(&app, &token, "markdown-default", explicit.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["replayed"], true);
    explicit["markdown"] = json!(false);
    assert_eq!(
        send(&app, &token, "markdown-default", explicit.clone())
            .await
            .0,
        StatusCode::CONFLICT
    );
    explicit["reply_to"] = sent["message"]["id"].clone();
    let (status, plain) = send(&app, &token, "plain", explicit.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(plain["message"]["markdown"], false);
    explicit.as_object_mut().unwrap().remove("markdown");
    let (status, reply) = send(&app, &token, "markdown-reply", explicit).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reply["message"]["markdown"], true);
}

#[tokio::test]
async fn keyed_plain_sends_and_replies_are_atomic_literal_and_uniform() {
    let (app, token) = fixture();
    let id = conversation(&app);
    let input = json!({"conversation_id":id,"text":"/rm user admin","markdown":true});
    let (a, b) = tokio::join!(
        send(&app, &token, "send-1", input.clone()),
        send(&app, &token, "send-1", input)
    );
    assert_eq!(a.0, StatusCode::OK);
    assert_eq!(b.0, StatusCode::OK);
    assert_eq!(a.1["message"]["id"], b.1["message"]["id"]);
    assert_ne!(a.1["replayed"], b.1["replayed"]);
    assert_eq!(a.1["message"]["text"], "/rm user admin");
    assert_eq!(a.1["message"]["markdown"], true);
    assert!(app.engine_read().unwrap().data.users.contains_key("admin"));
    let reply = json!({"conversation_id":id,"text":"**answer**","reply_to":a.1["message"]["id"],"markdown":true});
    let (status, result) = send(&app, &token, "reply-1", reply.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["message"]["reply"]["id"], a.1["message"]["id"]);
    assert_eq!(
        send(&app, &token, "reply-1", reply).await.1["replayed"],
        true
    );
    let conflict = send(
        &app,
        &token,
        "send-1",
        json!({"conversation_id":id,"text":"different"}),
    )
    .await;
    assert_eq!(conflict.0, StatusCode::CONFLICT);
    assert_eq!(conflict.1["error"]["code"], "idempotency_conflict");
    let (_, page) = request(
        &app,
        Some(&token),
        "GET",
        &format!("/api/bot/v1/history?conversation_id={id}"),
        json!({}),
    )
    .await;
    assert_eq!(page["messages"].as_array().unwrap().len(), 2);
    assert!(page.get("view").is_none());
    let seq = result["message"]["sequence"].as_u64().unwrap();
    assert_eq!(
        request(
            &app,
            Some(&token),
            "POST",
            "/api/bot/v1/read",
            json!({"conversation_id":id,"through":seq,"acknowledge_shared_unread":true})
        )
        .await
        .0,
        StatusCode::OK
    );
    let reaction = json!({"conversation_id":id,"message_id":result["message"]["id"],"reaction":"👍","active":true});
    for _ in 0..2 {
        assert_eq!(
            request(
                &app,
                Some(&token),
                "PUT",
                "/api/bot/v1/reactions",
                reaction.clone()
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    let (_, page) = request(
        &app,
        Some(&token),
        "GET",
        &format!("/api/bot/v1/history?conversation_id={id}"),
        json!({}),
    )
    .await;
    assert_eq!(page["messages"][1]["reactions"]["👍"], json!(["helper"]));
}
#[tokio::test]
async fn read_cursor_inputs_share_the_budget_across_credentials_and_mutations() {
    let (app, token) = fixture();
    let id = conversation(&app);
    let sequence = {
        let mut engine = app.engine().unwrap();
        engine
            .execute(Some("admin"), Some("support"), "read target")
            .unwrap();
        engine
            .try_snapshot("admin")
            .unwrap()
            .unwrap()
            .rooms
            .iter()
            .find(|room| room.name == "support")
            .unwrap()
            .messages[0]
            .sequence
    };
    for _ in 0..30 {
        assert_eq!(
            request(
                &app,
                Some(&token),
                "POST",
                "/api/bot/v1/read",
                json!({"conversation_id":id,"through":sequence,"acknowledge_shared_unread":true})
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    let second = app
        .engine()
        .unwrap()
        .execute(Some("admin"), None, "/bot token new helper second")
        .unwrap()
        .lines()
        .last()
        .unwrap()
        .to_owned();
    for credential in [&token, &second] {
        let blocked = request(
            &app,
            Some(credential),
            "POST",
            "/api/bot/v1/read",
            json!({"conversation_id":id,"through":sequence,"acknowledge_shared_unread":true}),
        )
        .await;
        assert_eq!(blocked.0, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(blocked.1["error"]["code"], "rate_limited");
    }
    let blocked = send(
        &app,
        &second,
        "budget-blocked",
        json!({"conversation_id":id,"text":"must not send"}),
    )
    .await;
    assert_eq!(blocked.0, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn browser_and_bot_history_distinguish_revision_conflicts_from_storage_failures() {
    let (app, token) = fixture();
    let id = conversation(&app);
    let (store, revision) = {
        let mut engine = app.engine().unwrap();
        engine
            .execute(Some("admin"), Some("support"), "history target")
            .unwrap();
        (
            engine.history_store.clone(),
            engine.data.rooms["support"].revision,
        )
    };
    store
        .read(|db| {
            db.execute_batch("PRAGMA query_only=OFF;")
                .map_err(|e| e.to_string())?;
            db.execute(
                "UPDATE entities SET revision=?1 WHERE kind='room' AND key='support'",
                [revision + 1],
            )
            .map_err(|e| e.to_string())?;
            db.execute_batch("PRAGMA query_only=ON;")
                .map_err(|e| e.to_string())
        })
        .unwrap();
    for path in [
        format!("/api/bot/v1/history?conversation_id={id}"),
        "/api/history?view=support".into(),
    ] {
        let response = request(&app, Some(&token), "GET", &path, json!({})).await;
        assert_eq!(response.0, StatusCode::CONFLICT);
        if path.contains("/bot/") {
            assert_eq!(response.1["error"]["code"], "conflict");
        }
    }
    store
        .read(|db| {
            db.execute_batch("PRAGMA query_only=OFF;")
                .map_err(|e| e.to_string())?;
            db.execute(
                "UPDATE entities SET revision=?1 WHERE kind='room' AND key='support'",
                [revision],
            )
            .map_err(|e| e.to_string())?;
            db.execute(
                "UPDATE messages SET json=json_set(json,'$.time','invalid') WHERE conversation=?1",
                [&id],
            )
            .map_err(|e| e.to_string())?;
            db.execute_batch("PRAGMA query_only=ON;")
                .map_err(|e| e.to_string())
        })
        .unwrap();
    for path in [
        format!("/api/bot/v1/history?conversation_id={id}"),
        "/api/history?view=support".into(),
    ] {
        let response = request(&app, Some(&token), "GET", &path, json!({})).await;
        assert_eq!(response.0, StatusCode::SERVICE_UNAVAILABLE);
        if path.contains("/bot/") {
            assert_eq!(response.1["error"]["code"], "unavailable");
        }
    }
}

#[tokio::test]
async fn typed_errors_distinguish_permissions_missing_records_and_failed_storage() {
    let (app, token) = fixture();
    let id = conversation(&app);
    let foreign = app.engine_read().unwrap().data.rooms["secret"].id.clone();
    for (view, expected) in [(foreign.as_str(), "forbidden"), ("missing", "not_found")] {
        let (_, body) = request(
            &app,
            Some(&token),
            "POST",
            "/api/bot/v1/read",
            json!({"conversation_id":view,"through":1,"acknowledge_shared_unread":true}),
        )
        .await;
        assert_eq!(body["error"]["code"], expected);
    }
    let sent = send(
        &app,
        &token,
        "a",
        json!({"conversation_id":id,"text":"question"}),
    )
    .await
    .1;
    app.engine()
        .unwrap()
        .execute(None, None, "/revoke helper support /reply")
        .unwrap();
    let denied = send(
        &app,
        &token,
        "b",
        json!({"conversation_id":id,"text":"answer","reply_to":sent["message"]["id"]}),
    )
    .await;
    assert_eq!(denied.0, StatusCode::FORBIDDEN);
    app.engine()
        .unwrap()
        .execute(
            None,
            Some("support"),
            &format!("/retract {}", sent["message"]["id"].as_str().unwrap()),
        )
        .unwrap();
    let gone = send(
        &app,
        &token,
        "a",
        json!({"conversation_id":id,"text":"question"}),
    )
    .await;
    assert_eq!(gone.0, StatusCode::GONE);
    assert_eq!(gone.1["error"]["code"], "message_gone");
    app.stopping
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let unavailable = request(&app, Some(&token), "GET", "/api/bot/v1/me", json!({})).await;
    assert_eq!(unavailable.0, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(unavailable.1["error"]["code"], "unavailable");
}
#[tokio::test]
async fn extractor_limits_throttling_and_production_ingress_obey_the_contract() {
    use axum::extract::ConnectInfo;
    use std::net::SocketAddr;
    let (mut app, token) = fixture();
    let id = conversation(&app);
    for (method, path, body, code) in [
        (
            "GET",
            "/api/bot/v1/history?conversation_id=x&after=-1",
            json!({}),
            "invalid_request",
        ),
        (
            "POST",
            "/api/bot/v1/messages",
            json!({"conversation_id":id,"text":"x","view":"support"}),
            "invalid_request",
        ),
        (
            "POST",
            "/api/bot/v1/messages",
            json!({"conversation_id":id,"text":"x".repeat(32768)}),
            "payload_too_large",
        ),
        ("POST", "/api/bot/v1/me", json!({}), "method_not_allowed"),
    ] {
        let (_, result) = request(&app, Some(&token), method, path, body).await;
        assert_eq!(result["error"]["code"], code);
    }
    for key in ["", "with spaces", &"x".repeat(129)] {
        assert_eq!(
            send(&app, &token, key, json!({"conversation_id":id,"text":"x"}))
                .await
                .1["error"]["code"],
            "invalid_request"
        );
    }
    assert_eq!(
        send(
            &app,
            &token,
            "unicode",
            json!({"conversation_id":id,"text":"🙂".repeat(4000)})
        )
        .await
        .0,
        StatusCode::OK
    );
    for n in 1..30 {
        assert_eq!(
            send(
                &app,
                &token,
                &format!("key-{n}"),
                json!({"conversation_id":id,"text":"x"})
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    let response = web::router(app.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/bot/v1/messages")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header("idempotency-key", "full")
                .body(Body::from(
                    json!({"conversation_id":id,"text":"x"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers()["retry-after"], "10");
    app.config = Arc::new(Config {
        production: true,
        base_url: "/commonroom/".into(),
        origins: vec!["https://chat.example.com".into()],
        ..Config::default()
    });
    for (peer, path, forwarded, status) in [
        (
            "127.0.0.1:4000",
            "/commonroom/api/bot/v1/me",
            false,
            StatusCode::OK,
        ),
        (
            "127.0.0.2:4000",
            "/commonroom/api/bot/v1/me",
            true,
            StatusCode::FORBIDDEN,
        ),
        (
            "127.0.0.1:4000",
            "/commonroom/api/bot/me",
            true,
            StatusCode::NOT_FOUND,
        ),
        (
            "127.0.0.1:4000",
            "/commonroom/api/bot/me",
            false,
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let mut builder = Request::builder()
            .uri(path)
            .extension(ConnectInfo(peer.parse::<SocketAddr>().unwrap()))
            .header(header::AUTHORIZATION, format!("Bearer {token}"));
        if forwarded {
            builder = builder.header("x-forwarded-proto", "https");
        }
        assert_eq!(
            web::router(app.clone())
                .oneshot(builder.body(Body::empty()).unwrap())
                .await
                .unwrap()
                .status(),
            status
        );
    }
}

#[tokio::test]
async fn authentication_tracks_stable_accounts_through_rename_and_rejects_cookie_fallback() {
    let (app, token) = fixture();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    let account = authenticate(&app, &headers).unwrap();
    app.engine()
        .unwrap()
        .execute(None, None, "/rename helper renamed")
        .unwrap();
    assert_eq!(
        account.user(&app.engine_read().unwrap()).unwrap(),
        "renamed"
    );
    let snapshot = request(&app, Some(&token), "GET", "/api/bot/v1/me", json!({}))
        .await
        .1;
    assert_eq!(snapshot["account"]["id"], account.account);
    assert_eq!(snapshot["account"]["username"], "renamed");
    let hash = app.engine_read().unwrap().data.users["renamed"]
        .hash
        .clone();
    app.engine()
        .unwrap()
        .login("cookie".into(), "renamed", &hash, None)
        .unwrap();
    let response = web::router(app.clone())
        .oneshot(
            Request::builder()
                .uri("/api/bot/v1/me")
                .header(header::COOKIE, "chat_session=cookie")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    headers.append(header::AUTHORIZATION, "Bearer invalid".parse().unwrap());
    assert!(authenticate(&app, &headers).is_err());
}

#[tokio::test]
async fn revisions_detect_edits_and_retractions_without_advancing_sequence() {
    let (app, token) = fixture();
    let id = conversation(&app);
    let sent = send(
        &app,
        &token,
        "revision",
        json!({"conversation_id":id,"text":"original"}),
    )
    .await
    .1;
    let path = format!("/api/bot/v1/history?conversation_id={id}");
    let first = request(&app, Some(&token), "GET", &path, json!({})).await.1;
    let revision = first["content_revision"].as_u64().unwrap();
    let message = sent["message"]["id"].as_str().unwrap();
    app.engine()
        .unwrap()
        .execute(None, Some("support"), &format!("/edit {message} edited"))
        .unwrap();
    let edited = request(&app, Some(&token), "GET", &path, json!({})).await.1;
    assert_eq!(
        edited["messages"][0]["sequence"],
        sent["message"]["sequence"]
    );
    assert!(edited["content_revision"].as_u64().unwrap() > revision);
    assert_eq!(
        request(
            &app,
            Some(&token),
            "GET",
            &format!("{path}&content_revision={revision}"),
            json!({})
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    app.engine()
        .unwrap()
        .execute(None, Some("support"), &format!("/retract {message}"))
        .unwrap();
    let removed = request(&app, Some(&token), "GET", &path, json!({})).await.1;
    assert!(removed["messages"].as_array().unwrap().is_empty());
    assert!(
        removed["content_revision"].as_u64().unwrap()
            > edited["content_revision"].as_u64().unwrap()
    );
}

#[tokio::test]
async fn restricted_credentials_are_scoped_expiring_and_cannot_escape_v1() {
    let (app, token) = fixture();
    let id = conversation(&app);
    let expires = crate::bots::now() + 300;
    let issued = request(
        &app,
        Some(&token),
        "POST",
        "/api/bot/v1/credentials",
        json!({"label":"reader","expires":expires,"conversations":[id],"operations":["read"]}),
    )
    .await;
    assert_eq!(issued.0, StatusCode::OK);
    let restricted = issued.1["token"].as_str().unwrap();
    let snapshot = request(&app, Some(restricted), "GET", "/api/bot/v1/me", json!({}))
        .await
        .1;
    assert_eq!(snapshot["credential"]["expires"], expires);
    assert_eq!(snapshot["conversations"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["conversations"][0]["permissions"]["send"], false);
    assert_eq!(
        send(
            &app,
            restricted,
            "denied",
            json!({"conversation_id":id,"text":"no"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(&app, Some(restricted), "GET", "/api/me", json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            Some(restricted),
            "POST",
            "/api/bot/v1/credentials",
            json!({"label":"escape","expires":expires,"conversations":[id],"operations":["send"]})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    {
        let mut engine = app.engine().unwrap();
        engine
            .execute(None, None, "/rename helper renamed")
            .unwrap();
        engine
            .execute(None, None, "/rename room support renamed_room")
            .unwrap();
    }
    let snapshot = request(&app, Some(restricted), "GET", "/api/bot/v1/me", json!({}))
        .await
        .1;
    assert_eq!(snapshot["account"]["username"], "renamed");
    assert_eq!(snapshot["conversations"][0]["id"], id);
    let account = app.engine_read().unwrap().data.users["renamed"].id.clone();
    app.engine()
        .unwrap()
        .data
        .bots
        .get_mut(&account)
        .unwrap()
        .tokens
        .get_mut(issued.1["id"].as_str().unwrap())
        .unwrap()
        .expires = Some(crate::bots::now());
    assert_eq!(
        request(&app, Some(restricted), "GET", "/api/bot/v1/me", json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn conversation_creation_uses_existing_grants_and_shared_read_requires_consent() {
    let (app, token) = fixture();
    let room = json!({"kind":"room","name":"created"});
    app.engine()
        .unwrap()
        .execute(None, None, "/grant helper @global w:room.create")
        .unwrap();
    assert_eq!(
        request(
            &app,
            Some(&token),
            "POST",
            "/api/bot/v1/conversations",
            room.clone()
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/grant helper @global /new")
        .unwrap();
    assert_eq!(
        request(
            &app,
            Some(&token),
            "POST",
            "/api/bot/v1/conversations",
            room
        )
        .await
        .0,
        StatusCode::OK
    );
    let direct = json!({"kind":"direct","username":"admin"});
    app.engine()
        .unwrap()
        .execute(None, None, "/grant helper @global w:private.create")
        .unwrap();
    assert_eq!(
        request(
            &app,
            Some(&token),
            "POST",
            "/api/bot/v1/conversations",
            direct.clone()
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/grant helper @global /tell")
        .unwrap();
    app.engine()
        .unwrap()
        .execute(None, None, "/grant helper @global w:message.create")
        .unwrap();
    let created = request(
        &app,
        Some(&token),
        "POST",
        "/api/bot/v1/conversations",
        direct.clone(),
    )
    .await;
    assert_eq!(created.0, StatusCode::OK);
    let repeated = request(
        &app,
        Some(&token),
        "POST",
        "/api/bot/v1/conversations",
        direct,
    )
    .await;
    assert_eq!(created.1, repeated.1);
    let id = conversation(&app);
    for consent in [
        json!({"conversation_id":id,"through":0}),
        json!({"conversation_id":id,"through":0,"acknowledge_shared_unread":false}),
    ] {
        assert!(
            request(&app, Some(&token), "POST", "/api/bot/v1/read", consent)
                .await
                .0
                .is_client_error()
        );
    }
}

#[tokio::test]
async fn unhealthy_unread_metadata_does_not_hide_healthy_bot_conversations() {
    let (app, token) = fixture();
    app.engine()
        .unwrap()
        .execute(None, None, "/grant helper secret r:message.read")
        .unwrap();
    let id = conversation(&app);
    send(
        &app,
        &token,
        "corrupt",
        json!({"conversation_id":id,"text":"fixture"}),
    )
    .await;
    let store = app.engine_read().unwrap().history_store.clone();
    store
        .read(|db| {
            db.execute_batch(
                "PRAGMA query_only=OFF; UPDATE messages SET sequence=-1; PRAGMA query_only=ON;",
            )
            .map_err(|e| e.to_string())
        })
        .unwrap();
    let (status, snapshot) = request(&app, Some(&token), "GET", "/api/bot/v1/me", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(snapshot["conversations"].as_array().unwrap().len(), 2);
    let broken = snapshot["conversations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["view"] == "support")
        .unwrap();
    assert!(broken["unread"].is_null());
}

#[path = "discovery_tests.rs"]
mod discovery_tests;
