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
#[path = "commands.rs"]
mod command_tests;

fn fixture() -> (App, String) {
    fixture_with_engine(Engine::open(Path::new(":memory:")).unwrap())
}
fn fixture_with_engine(mut engine: Engine) -> (App, String) {
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
            media: None,
            engine: Arc::new(RwLock::new(engine)),
            attempts: Default::default(),
            changes,
            config: Arc::new(Config::default()),
            admission: Default::default(),
            io_jobs: Arc::new(tokio::sync::Semaphore::new(64)),
            password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
            snapshots: Default::default(),
            mutations: Default::default(),
            bot_runtimes: Default::default(),
            stopping: Default::default(),
            shutdown: Default::default(),
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
            engine.history_store().clone(),
            engine.data.rooms["support"].revision,
        )
    };
    store
        .read(|db| {
            db.execute_batch("PRAGMA query_only=OFF;")
                .map_err(crate::engine::EngineError::from)?;
            db.execute(
                "UPDATE entities SET revision=?1 WHERE kind='room' AND key='support'",
                [revision + 1],
            )
            .map_err(crate::engine::EngineError::from)?;
            db.execute_batch("PRAGMA query_only=ON;")
                .map_err(crate::engine::EngineError::from)
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
                .map_err(crate::engine::EngineError::from)?;
            db.execute(
                "UPDATE entities SET revision=?1 WHERE kind='room' AND key='support'",
                [revision],
            )
            .map_err(crate::engine::EngineError::from)?;
            db.execute(
                "UPDATE messages SET json=json_set(json,'$.time','invalid') WHERE conversation=?1",
                [&id],
            )
            .map_err(crate::engine::EngineError::from)?;
            db.execute_batch("PRAGMA query_only=ON;")
                .map_err(crate::engine::EngineError::from)
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
        .execute(None, None, "/revoke helper #support /reply")
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
        direct_bot_http: true,
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
    let mut changes = app.changes.subscribe();
    let issued = request(
        &app,
        Some(&token),
        "POST",
        "/api/bot/v1/credentials",
        json!({"label":"reader","expires":expires,"conversations":[id],"operations":["read"]}),
    )
    .await;
    assert_eq!(issued.0, StatusCode::OK);
    let change = changes.try_recv().unwrap();
    let notice = match change {
        crate::web::Change::Metadata(notice) => Some(notice),
        _ => None,
    }
    .unwrap();
    assert!(notice.targets(app.engine_read().unwrap().account_id("helper").unwrap()));
    assert!(notice.catalogues().any(|conversation| conversation == &id));
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
        .execute(None, None, "/grant helper #secret r:message.read")
        .unwrap();
    let id = conversation(&app);
    send(
        &app,
        &token,
        "corrupt",
        json!({"conversation_id":id,"text":"fixture"}),
    )
    .await;
    let store = app.engine_read().unwrap().history_store().clone();
    store
        .read(|db| {
            db.execute_batch(
                "PRAGMA query_only=OFF; UPDATE messages SET sequence=-1; PRAGMA query_only=ON;",
            )
            .map_err(crate::engine::EngineError::from)
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

#[path = "discovery.rs"]
mod discovery_tests;

#[test]
fn bot_baselines_share_buffers_but_isolate_credentials_and_recheck_revocation() {
    let (app, secret) = fixture();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {secret}").parse().unwrap(),
    );
    let context = Context::authenticate(&app, &headers).unwrap();
    let first = { crate::web::snapshots::bot_value(&app, &context).unwrap() };
    let again = { crate::web::snapshots::bot_value(&app, &context).unwrap() };
    assert_eq!(first.as_str().as_ptr(), again.as_str().as_ptr());
    app.update_presence(None, Some("admin")).unwrap();
    let after_presence = { crate::web::snapshots::bot_value(&app, &context).unwrap() };
    assert_eq!(first.as_str().as_ptr(), after_presence.as_str().as_ptr());
    let second = app
        .engine()
        .unwrap()
        .execute(None, None, "/bot token new helper scoped")
        .unwrap()
        .lines()
        .last()
        .unwrap()
        .to_owned();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {second}").parse().unwrap(),
    );
    let scoped = Context::authenticate(&app, &headers).unwrap();
    let scoped_value = { crate::web::snapshots::bot_value(&app, &scoped).unwrap() };
    let parsed: Value = serde_json::from_str(&scoped_value).unwrap();
    assert_eq!(parsed["credential"]["id"], scoped.credential);
    assert_ne!(parsed["credential"]["id"], context.credential);
    app.engine()
        .unwrap()
        .execute(
            None,
            None,
            &format!("/bot token rm helper {}", scoped.credential),
        )
        .unwrap();
    assert!(crate::web::snapshots::bot_value(&app, &scoped).is_err());
    assert!(crate::web::snapshots::bot_value(&app, &context).is_ok());
}

#[tokio::test]
async fn reconnect_session_check_authenticates_without_building_history() {
    let (app, _) = fixture();
    app.engine()
        .unwrap()
        .login("web".into(), "admin", "hash", None)
        .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(header::COOKIE, "chat_session=web".parse().unwrap());
    headers.insert(header::ORIGIN, "http://localhost:5173".parse().unwrap());
    let before = app.engine_read().unwrap().history_store().stats().unwrap();
    assert!(
        web::auth::session_status(State(app.clone()), headers.clone())
            .await
            .is_ok()
    );
    assert_eq!(
        app.engine_read().unwrap().history_store().stats().unwrap(),
        before
    );
    app.engine().unwrap().logout("web").unwrap();
    assert_eq!(
        web::auth::session_status(State(app), headers)
            .await
            .unwrap_err()
            .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn account_admission_uses_the_typed_bot_rate_limit_contract() {
    let (app, secret) = fixture();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {secret}").parse().unwrap(),
    );
    let context = crate::bots::auth::Context::authenticate(&app, &headers).unwrap();
    let leases: Vec<_> = (0..8)
        .map(|_| app.admission.http(context.account.clone()).unwrap())
        .collect();
    let (status, value) = request(&app, Some(&secret), "GET", "/api/bot/v1/me", json!({})).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(value["error"]["code"], "rate_limited");
    assert_eq!(value["error"]["retryable"], true);
    drop(leases);
    assert_eq!(
        request(&app, Some(&secret), "GET", "/api/bot/v1/me", json!({}))
            .await
            .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn production_direct_bot_http_requires_explicit_opt_in_and_never_accepts_forwarded_http() {
    use axum::extract::ConnectInfo;
    let (mut app, token) = fixture();
    for (enabled, proto, status) in [
        (false, None, StatusCode::BAD_REQUEST),
        (true, None, StatusCode::OK),
        (true, Some("http"), StatusCode::BAD_REQUEST),
        (false, Some("https"), StatusCode::OK),
    ] {
        app.config = Arc::new(Config {
            production: true,
            direct_bot_http: enabled,
            origins: vec!["https://chat.example.com".into()],
            ..Config::default()
        });
        let mut builder = Request::builder()
            .uri("/api/bot/v1/me")
            .extension(ConnectInfo(
                "127.0.0.1:1234".parse::<std::net::SocketAddr>().unwrap(),
            ))
            .header(header::AUTHORIZATION, format!("Bearer {token}"));
        if let Some(proto) = proto {
            builder = builder.header("x-forwarded-proto", proto);
        }
        let response = web::router(app.clone())
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}

#[test]
fn socket_batches_preserve_live_credential_scopes_and_skip_unrelated_events() {
    let (app, secret) = fixture();
    let visible = conversation(&app);
    let hidden = app.engine_read().unwrap().data.rooms["secret"].id.clone();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {secret}").parse().unwrap(),
    );
    let context = Context::authenticate(&app, &headers).unwrap();
    let changes = [
        crate::web::Change::Conversation(visible.clone()),
        crate::web::Change::Conversation(hidden),
        crate::web::Change::Read("other".into()),
        crate::web::Change::Account("other".into()),
        crate::web::Change::Presence,
        crate::web::Change::BotCommands(visible.clone()),
        crate::web::Change::Avatar("other".into()),
    ];
    let (_, frames) = crate::bots::socket::render_changes(&app, &context, &changes).unwrap();
    assert_eq!(frames.len(), 2);
    assert_eq!(
        serde_json::from_str::<Value>(&frames[0]).unwrap()["conversation_id"],
        visible
    );
    assert_eq!(
        serde_json::from_str::<Value>(&frames[1]).unwrap()["kind"],
        "presence"
    );
    let scoped = app
        .engine()
        .unwrap()
        .issue_bot_credential(
            &context,
            crate::bots::models::CredentialInput {
                label: "batch-send-only".into(),
                expires: crate::bots::now() + 100,
                conversations: [visible.clone()].into(),
                operations: ["send".into()].into(),
            },
        )
        .unwrap();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {}", scoped["token"].as_str().unwrap())
            .parse()
            .unwrap(),
    );
    let context = Context::authenticate(&app, &headers).unwrap();
    assert!(
        crate::bots::socket::render_changes(&app, &context, &changes)
            .unwrap()
            .1
            .is_empty()
    );
    app.engine()
        .unwrap()
        .execute(
            None,
            None,
            &format!("/bot token rm helper {}", context.credential),
        )
        .unwrap();
    assert_eq!(
        crate::bots::socket::render_changes(
            &app,
            &context,
            &[crate::web::Change::Conversation(visible)]
        )
        .unwrap_err()
        .code,
        Code::Unauthorized
    );
}

#[tokio::test]
async fn browser_native_text_preserves_send_only_permissions_and_committed_notifications() {
    let (app, _) = fixture();
    {
        let mut engine = app.engine().unwrap();
        engine
            .provision("writer", "hash".into(), false, false)
            .unwrap();
        engine.execute(None, None, "/revoke writer user").unwrap();
        engine
            .execute(None, None, "/grant writer #support w:message.create")
            .unwrap();
        engine
            .login("writer".into(), "writer", "hash", None)
            .unwrap();
    }
    let id = conversation(&app);
    let before = app.engine_read().unwrap().data.next_sequence;
    let mut changes = app.changes.subscribe();
    let mut headers = HeaderMap::new();
    headers.insert(header::COOKIE, "chat_session=writer".parse().unwrap());
    let input = commands::Input {
        id: 1,
        room: Some("support".into()),
        text: "  **native** @admin @outsider  ".into(),
        markdown: true,
    };
    let (reply, change) = commands::execute_input(&app, &headers, "writer", &input)
        .await
        .unwrap();
    assert!(reply.is_empty());
    assert!(
        change.is_none(),
        "the accepted task already broadcast the commit"
    );
    assert!(
        matches!(changes.try_recv().unwrap(), web::Change::Conversation(changed) if changed == id)
    );
    assert!(changes.try_recv().is_err());
    {
        let engine = app.engine_read().unwrap();
        assert!(engine.history("writer", "support").is_err());
        let page = engine.history("admin", "support").unwrap();
        let sent = page.messages.last().unwrap();
        assert_eq!(sent.text, "**native** @admin @outsider");
        assert!(sent.markdown);
        assert_eq!(sent.sequence, before + 1);
        assert!(sent.mentions.contains("admin"));
        assert!(!sent.mentions.contains("outsider"));
    }
    let attachment = commands::Input {
        text: format!("![denied](/api/media/{}/file)", uuid::Uuid::new_v4()),
        ..input
    };
    assert!(
        commands::execute_input(&app, &headers, "writer", &attachment)
            .await
            .unwrap_err()
            .contains("r:message.read")
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/revoke writer #support w:message.create")
        .unwrap();
    assert!(
        commands::execute_input(
            &app,
            &headers,
            "writer",
            &commands::Input {
                text: "denied".into(),
                ..attachment
            }
        )
        .await
        .is_err()
    );
    assert_eq!(app.engine_read().unwrap().data.next_sequence, before + 1);
}

#[tokio::test]
async fn browser_native_tell_creates_once_rolls_back_failures_and_survives_restart() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("chat.sqlite");
    let (app, _) = fixture_with_engine(Engine::open(&path).unwrap());
    {
        let mut engine = app.engine().unwrap();
        engine
            .provision("bob", "hash".into(), false, false)
            .unwrap();
        engine.login("admin".into(), "admin", "hash", None).unwrap();
        engine.login("bob".into(), "bob", "hash", None).unwrap();
    }
    let before = app.engine_read().unwrap().data.next_sequence;
    let db = crate::engine::database::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER reject_first_dm BEFORE INSERT ON messages WHEN json_extract(NEW.json,'$.text')='fail first' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    let mut changes = app.changes.subscribe();
    let mut admin_headers = HeaderMap::new();
    admin_headers.insert(header::COOKIE, "chat_session=admin".parse().unwrap());
    let mut bob_headers = HeaderMap::new();
    bob_headers.insert(header::COOKIE, "chat_session=bob".parse().unwrap());
    let failed = commands::Input {
        id: 1,
        room: None,
        text: "/tell bob fail first".into(),
        markdown: false,
    };
    assert!(
        commands::execute_input(&app, &admin_headers, "admin", &failed)
            .await
            .is_err()
    );
    {
        let engine = app.engine_read().unwrap();
        assert!(engine.data.private.is_empty());
        assert_eq!(engine.data.next_sequence, before);
    }
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM entities WHERE kind='private'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        db.query_row("SELECT sequence FROM counters WHERE id=1", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        i64::try_from(before).unwrap()
    );
    assert!(changes.try_recv().is_err());
    db.execute_batch("DROP TRIGGER reject_first_dm;").unwrap();
    let first = commands::Input {
        text: "/tell bob **first** @bob @outsider".into(),
        markdown: true,
        ..failed
    };
    let second = commands::Input {
        id: 2,
        room: None,
        text: "/tell admin second".into(),
        markdown: false,
    };
    let (one, two) = tokio::join!(
        commands::execute_input(&app, &admin_headers, "admin", &first),
        commands::execute_input(&app, &bob_headers, "bob", &second),
    );
    assert_eq!(one.unwrap().0, "Private message sent to bob.");
    assert_eq!(two.unwrap().0, "Private message sent to admin.");
    let metadata = changes.try_recv().unwrap();
    assert!(matches!(metadata, web::Change::Metadata(_)));
    assert!(matches!(
        changes.try_recv().unwrap(),
        web::Change::Conversation(_)
    ));
    assert!(changes.try_recv().is_err());
    let id = {
        let engine = app.engine_read().unwrap();
        assert_eq!(engine.data.private.len(), 1);
        let chat = &engine.data.private["admin:bob"];
        assert_eq!(chat.revision, 2);
        let page = engine.history("bob", "@direct:admin").unwrap();
        assert_eq!(page.messages.len(), 2);
        assert_eq!(page.messages[0].sequence, before + 1);
        assert_eq!(page.messages[1].sequence, before + 2);
        let original = page
            .messages
            .iter()
            .find(|message| message.from == "admin")
            .unwrap();
        assert!(original.markdown);
        assert_eq!(original.to.as_deref(), Some("bob"));
        assert_eq!(original.private_id, chat.id);
        assert!(original.mentions.contains("bob"));
        assert!(!original.mentions.contains("outsider"));
        let snapshot = engine.snapshot("bob").unwrap();
        assert!(
            serde_json::to_string(&snapshot)
                .unwrap()
                .contains("**first**")
        );
        chat.id.clone()
    };
    // Explicit messaging rights keep existing chats usable without Direct.
    {
        let mut engine = app.engine().unwrap();
        engine.execute(None, None, "/revoke bob user").unwrap();
        for permission in ["/tell", "w:message.create", "r:message.read"] {
            engine
                .execute(None, None, &format!("/grant bob @global {permission}"))
                .unwrap();
        }
    }
    commands::execute_input(&app, &bob_headers, "bob", &second)
        .await
        .unwrap();
    let denied = commands::Input {
        text: "/tell helper denied".into(),
        ..second
    };
    assert!(
        commands::execute_input(&app, &bob_headers, "bob", &denied)
            .await
            .unwrap_err()
            .contains("w:private.create")
    );
    assert_eq!(app.engine_read().unwrap().data.private.len(), 1);
    drop(db);
    drop(app);
    let reopened = Engine::open(&path).unwrap();
    assert_eq!(reopened.data.private["admin:bob"].id, id);
    assert_eq!(reopened.data.private["admin:bob"].revision, 3);
    assert_eq!(
        reopened
            .history("bob", "@direct:admin")
            .unwrap()
            .messages
            .len(),
        3
    );
    assert_eq!(reopened.data.next_sequence, before + 3);
}

async fn browser_execute(
    app: &App,
    user: &str,
    view: Option<&str>,
    text: String,
    markdown: bool,
) -> Result<String, String> {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        format!("chat_session={user}").parse().unwrap(),
    );
    commands::execute_input(
        app,
        &headers,
        user,
        &commands::Input {
            id: 1,
            room: view.map(str::to_owned),
            text,
            markdown,
        },
    )
    .await
    .map(|(reply, change)| {
        assert!(change.is_none(), "native task already publishes the change");
        reply
    })
}

async fn browser_action_contract(app: App) {
    {
        let mut engine = app.engine().unwrap();
        engine
            .provision("bob", "hash".into(), false, false)
            .unwrap();
        engine
            .execute(None, None, "/room add bob #support")
            .unwrap();
        engine
            .execute(None, None, "/grant admin #support w:message.edit.any")
            .unwrap();
        for user in ["admin", "bob"] {
            engine.login(user.into(), user, "hash", None).unwrap();
        }
    }
    browser_execute(
        &app,
        "bob",
        Some("support"),
        "**original** @admin".into(),
        true,
    )
    .await
    .unwrap();
    let original = app
        .engine_read()
        .unwrap()
        .history("admin", "support")
        .unwrap()
        .messages
        .last()
        .unwrap()
        .clone();
    let before = app.engine_read().unwrap().data.next_sequence;
    let upload = {
        let engine = app.engine_read().unwrap();
        let room = &engine.data.rooms["support"].id;
        let mut media = engine
            .allocate_media("admin", room, "editor.png", 8)
            .unwrap();
        media.ready = true;
        media.object_key = media.final_key();
        media.content_type = "image/png".into();
        engine.finish_media("admin", &media).unwrap();
        media
    };
    let edited_text = format!(
        "edited ![editor](/api/media/{}/file) @admin @outsider",
        upload.id
    );
    assert_eq!(
        browser_execute(
            &app,
            "admin",
            Some("support"),
            format!("/edit {} {edited_text}", original.id),
            false
        )
        .await
        .unwrap(),
        "Message edited."
    );
    {
        let engine = app.engine_read().unwrap();
        let edited = engine
            .history("admin", "support")
            .unwrap()
            .messages
            .last()
            .unwrap()
            .clone();
        assert_eq!(edited.author_id, original.author_id);
        assert_eq!(edited.from, "bob");
        assert_eq!(edited.sequence, original.sequence);
        assert!(edited.markdown && edited.edited);
        assert!(!edited.mentions.contains("outsider"));
        assert!(engine.media_attached(&upload.id).unwrap());
    }
    // The original author can retain an existing attachment without owning it.
    let retained_text = format!("{edited_text} retained");
    browser_execute(
        &app,
        "bob",
        Some("support"),
        format!("/edit {} {retained_text}", original.id),
        false,
    )
    .await
    .unwrap();
    let mut changes = app.changes.subscribe();
    let revision = app.engine_read().unwrap().data.rooms["support"].revision;
    assert_eq!(
        browser_execute(
            &app,
            "bob",
            Some("support"),
            format!("/edit {} {retained_text}", original.id),
            false
        )
        .await
        .unwrap(),
        "Message unchanged."
    );
    assert_eq!(
        app.engine_read().unwrap().data.rooms["support"].revision,
        revision
    );
    assert!(changes.try_recv().is_err());
    for active in [true, false] {
        browser_execute(
            &app,
            "admin",
            Some("support"),
            format!("/react {} 👍", original.id),
            false,
        )
        .await
        .unwrap();
        let page = app
            .engine_read()
            .unwrap()
            .history("admin", "support")
            .unwrap();
        assert_eq!(
            page.messages.last().unwrap().reactions.contains_key("👍"),
            active
        );
    }
    assert_eq!(app.engine_read().unwrap().data.next_sequence, before);
    browser_execute(
        &app,
        "admin",
        Some("support"),
        format!("/reply {} quoted @bob", original.id),
        true,
    )
    .await
    .unwrap();
    let reply = app
        .engine_read()
        .unwrap()
        .history("admin", "support")
        .unwrap()
        .messages
        .last()
        .unwrap()
        .clone();
    assert_eq!(reply.reply.as_ref().unwrap().id, original.id);
    assert_eq!(reply.reply.as_ref().unwrap().from, "bob");
    assert_eq!(reply.sequence, before + 1);
    assert!(
        browser_execute(
            &app,
            "bob",
            Some("support"),
            format!("/edit {} denied", reply.id),
            false
        )
        .await
        .unwrap_err()
        .contains("own messages")
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/grant admin su")
        .unwrap();
    browser_execute(
        &app,
        "admin",
        None,
        format!("/edit {} global edit", reply.id),
        false,
    )
    .await
    .unwrap();
    for command in ["/reply", "/react"] {
        assert!(
            browser_execute(
                &app,
                "admin",
                None,
                format!("{command} {} denied", reply.id),
                false
            )
            .await
            .is_err()
        );
    }
    browser_execute(&app, "admin", None, "/tell bob private seed".into(), false)
        .await
        .unwrap();
    let seed = app
        .engine_read()
        .unwrap()
        .history("bob", "@direct:admin")
        .unwrap()
        .messages
        .last()
        .unwrap()
        .clone();
    browser_execute(
        &app,
        "bob",
        Some("@direct:admin"),
        format!("/reply {} private reply @admin @outsider", seed.id),
        true,
    )
    .await
    .unwrap();
    let private = app
        .engine_read()
        .unwrap()
        .history("admin", "@direct:bob")
        .unwrap()
        .messages
        .last()
        .unwrap()
        .clone();
    assert_eq!(private.from, "bob");
    assert_eq!(private.to.as_deref(), Some("admin"));
    assert_eq!(private.private_id, seed.private_id);
    assert_eq!(private.reply.as_ref().unwrap().id, seed.id);
    assert!(!private.mentions.contains("outsider"));
    browser_execute(
        &app,
        "bob",
        Some("@direct:admin"),
        format!("/edit {} private edited", private.id),
        false,
    )
    .await
    .unwrap();
    browser_execute(
        &app,
        "admin",
        Some("@direct:bob"),
        format!("/react {} 👍", private.id),
        false,
    )
    .await
    .unwrap();
    assert_eq!(app.engine_read().unwrap().data.next_sequence, before + 3);
}

#[tokio::test]
async fn browser_native_actions_preserve_authority_attachments_and_message_identity() {
    let (app, _) = fixture();
    browser_action_contract(app).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_browser_native_actions_preserve_editor_uploads_and_message_identity() {
    let fixture = crate::engine::database::tests::PostgresFixture::new();
    let (app, _) = fixture_with_engine(Engine::open_postgres(&fixture.url).unwrap());
    browser_action_contract(app).await;
}

async fn browser_history_contract(
    app: App,
    db: crate::engine::database::Connection,
    postgres: bool,
) {
    {
        let mut engine = app.engine().unwrap();
        engine
            .provision("bob", "hash".into(), false, false)
            .unwrap();
        engine
            .execute(None, None, "/room add bob #support")
            .unwrap();
        engine.execute(None, None, "/grant admin su").unwrap();
        engine.execute(None, None, "/room new other").unwrap();
        for user in ["admin", "bob"] {
            engine.login(user.into(), user, "hash", None).unwrap();
        }
    }
    let upload = {
        let engine = app.engine_read().unwrap();
        let mut media = engine
            .allocate_media("admin", &engine.data.rooms["support"].id, "photo.png", 8)
            .unwrap();
        media.ready = true;
        media.object_key = media.final_key();
        media.content_type = "image/png".into();
        engine.finish_media("admin", &media).unwrap();
        media
    };
    browser_execute(
        &app,
        "admin",
        Some("support"),
        format!("![photo](/api/media/{}/file)", upload.id),
        true,
    )
    .await
    .unwrap();
    let original = app
        .engine_read()
        .unwrap()
        .history("admin", "support")
        .unwrap()
        .messages
        .last()
        .unwrap()
        .clone();
    browser_execute(
        &app,
        "bob",
        Some("support"),
        format!("/reply {} quoted", original.id),
        false,
    )
    .await
    .unwrap();
    let before = app.engine_read().unwrap().data.next_sequence;
    assert!(
        browser_execute(
            &app,
            "bob",
            Some("support"),
            format!("/retract {}", original.id),
            false
        )
        .await
        .unwrap_err()
        .contains("own messages")
    );
    let mut changes = app.changes.subscribe();
    assert_eq!(
        browser_execute(
            &app,
            "admin",
            Some("support"),
            format!("/retract {}", original.id),
            false
        )
        .await
        .unwrap(),
        "Message deleted."
    );
    assert!(matches!(
        changes.try_recv().unwrap(),
        web::Change::Conversation(_)
    ));
    assert!(changes.try_recv().is_err());
    {
        let engine = app.engine_read().unwrap();
        let retained = engine.history("admin", "support").unwrap();
        assert_eq!(retained.messages.len(), 1);
        assert!(retained.messages[0].reply.is_none());
        assert!(!engine.media_attached(&upload.id).unwrap());
        assert_eq!(engine.data.next_sequence, before);
        let audit = engine.data.policy.audit.back().unwrap();
        assert_eq!(audit.operation, "/retract");
        assert_eq!(audit.target, original.id);
        assert_eq!(audit.actor_id, engine.data.users["admin"].id);
    }
    for (view, text) in [
        (Some("support"), "injected rollback"),
        (Some("other"), "other room"),
        (None, "/tell bob private history"),
    ] {
        browser_execute(&app, "admin", view, text.into(), false)
            .await
            .unwrap();
    }
    // Force the deletion to fail after revision and audit preparation. Both a
    // single retraction and a multi-conversation cleanup must roll back fully.
    if postgres {
        db.execute_batch("CREATE FUNCTION reject_history_delete() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF (OLD.json::json ->> 'text')='injected rollback' THEN RAISE EXCEPTION 'injected'; END IF; RETURN OLD; END $$; CREATE TRIGGER reject_history_delete BEFORE DELETE ON messages FOR EACH ROW EXECUTE FUNCTION reject_history_delete();").unwrap();
    } else {
        db.execute_batch("CREATE TRIGGER reject_history_delete BEFORE DELETE ON messages WHEN json_extract(OLD.json,'$.text')='injected rollback' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    }
    let fault = app
        .engine_read()
        .unwrap()
        .history("admin", "support")
        .unwrap()
        .messages
        .last()
        .unwrap()
        .id
        .clone();
    let count = db
        .query_row("SELECT count(*) FROM messages", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
    let audits = db
        .query_row("SELECT count(*) FROM audit_entries", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
    let state = app.engine_read().unwrap().data.clone();
    let mut changes = app.changes.subscribe();
    for command in [format!("/retract {fault}"), "/clean -1 @global".into()] {
        assert!(
            browser_execute(&app, "admin", Some("support"), command, false)
                .await
                .is_err()
        );
        assert!(app.engine_read().unwrap().data == state);
        assert_eq!(
            db.query_row("SELECT count(*) FROM messages", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            count
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM audit_entries", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            audits
        );
        assert!(changes.try_recv().is_err());
    }
    db.execute_batch(if postgres {
        "DROP TRIGGER reject_history_delete ON messages; DROP FUNCTION reject_history_delete();"
    } else {
        "DROP TRIGGER reject_history_delete;"
    })
    .unwrap();
    {
        let mut engine = app.engine().unwrap();
        for permission in ["/clean", "x:history.clean"] {
            engine
                .execute(None, None, &format!("/grant bob @global {permission}"))
                .unwrap();
        }
        engine
            .execute(None, None, "/user set bob history.clean_min_age -1 @global")
            .unwrap();
        engine
            .execute(None, None, "/user set bob history.clean_min_age 0 #support")
            .unwrap();
    }
    assert!(
        browser_execute(&app, "bob", None, "/clean -1 @global".into(), false)
            .await
            .is_err()
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM messages", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        count
    );
    let state = app.engine_read().unwrap().data.clone();
    let preview = browser_execute(
        &app,
        "admin",
        None,
        "/clean -1 @global --dry-run".into(),
        false,
    )
    .await
    .unwrap();
    assert!(preview.contains(&format!("Messages matching: {count}")));
    assert!(app.engine_read().unwrap().data == state);
    assert!(changes.try_recv().is_err());
    let result = browser_execute(&app, "admin", None, "/clean -1 @global".into(), false)
        .await
        .unwrap();
    assert!(result.contains(&format!("Messages deleted: {count}")));
    assert_eq!(
        db.query_row("SELECT count(*) FROM messages", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(matches!(
        changes.try_recv().unwrap(),
        web::Change::Metadata(_)
    ));
    let state = app.engine_read().unwrap().data.clone();
    let revision = app.engine_read().unwrap().revision;
    browser_execute(&app, "admin", None, "/clean -1 @global".into(), false)
        .await
        .unwrap();
    assert!(app.engine_read().unwrap().data == state);
    assert_eq!(app.engine_read().unwrap().revision, revision);
    assert_eq!(state.policy.audit.back().unwrap().operation, "/clean");
    assert!(changes.try_recv().is_err());
    // Exercise the audit cap through the typed interface, then the native delta.
    let (database, actor) = {
        let engine = app.engine_read().unwrap();
        (
            engine
                .prepare_browser_message("admin", Some("support"), "audit fixture", false)
                .unwrap()
                .database,
            engine.data.users["admin"].id.clone(),
        )
    };
    let seeded = (0..1000)
        .map(|_| crate::engine::authorization::Audit {
            id: uuid::Uuid::new_v4().to_string(),
            time: 1,
            actor_id: actor.clone(),
            operation: "/seed".into(),
            target: "fixture".into(),
        })
        .collect::<std::collections::VecDeque<_>>();
    database
        .save_metadata(&crate::engine::database::MetadataWrite {
            operations: vec![crate::engine::database::MetadataOp::Audit(&seeded)],
        })
        .await
        .unwrap();
    app.engine().unwrap().data.policy.audit = seeded.clone();
    browser_execute(&app, "admin", Some("support"), "audit cap".into(), false)
        .await
        .unwrap();
    let id = app
        .engine_read()
        .unwrap()
        .history("admin", "support")
        .unwrap()
        .messages[0]
        .id
        .clone();
    browser_execute(
        &app,
        "admin",
        Some("support"),
        format!("/retract {id}"),
        false,
    )
    .await
    .unwrap();
    let engine = app.engine_read().unwrap();
    assert_eq!(engine.data.policy.audit.len(), 1000);
    assert_eq!(engine.data.policy.audit.front().unwrap().id, seeded[1].id);
    assert_eq!(
        engine.data.policy.audit.back().unwrap().operation,
        "/retract"
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM audit_entries", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1000
    );
    assert_eq!(
        db.query_row(
            "SELECT entry_id FROM audit_entries ORDER BY sequence LIMIT 1",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        seeded[1].id
    );
}

#[tokio::test]
async fn browser_native_history_deletions_are_atomic_audited_and_noop_safe() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("chat.sqlite");
    let (app, _) = fixture_with_engine(Engine::open(&path).unwrap());
    let db = crate::engine::database::Connection::open(&path).unwrap();
    browser_history_contract(app, db, false).await;
    let engine = Engine::open(&path).unwrap();
    assert!(
        engine
            .history("admin", "support")
            .unwrap()
            .messages
            .is_empty()
    );
    assert!(
        engine
            .history("bob", "@direct:admin")
            .unwrap()
            .messages
            .is_empty()
    );
    assert_eq!(
        engine.data.policy.audit.back().unwrap().operation,
        "/retract"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_browser_native_history_deletions_are_atomic_audited_and_noop_safe() {
    let fixture = crate::engine::database::tests::PostgresFixture::new();
    let (app, _) = fixture_with_engine(Engine::open_postgres(&fixture.url).unwrap());
    let db = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
    browser_history_contract(app, db, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_cancelled_cleanup_releases_engine_and_does_not_block_unrelated_sends() {
    let fixture = crate::engine::database::tests::PostgresFixture::new();
    let (app, token) = fixture_with_engine(Engine::open_postgres(&fixture.url).unwrap());
    let first = conversation(&app);
    let second = {
        let mut engine = app.engine().unwrap();
        engine.login("admin".into(), "admin", "hash", None).unwrap();
        engine
            .execute(Some("admin"), None, "/room new other")
            .unwrap();
        engine
            .execute(None, None, "/room add helper #other")
            .unwrap();
        engine.data.rooms["other"].id.clone()
    };
    assert_eq!(
        send(
            &app,
            &token,
            "cleanup-seed",
            json!({"conversation_id":first,"text":"delete after cancellation"})
        )
        .await
        .0,
        StatusCode::OK
    );
    let before = app.engine_read().unwrap().data.next_sequence;
    let blocker = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
    let hold = blocker.unchecked_transaction().unwrap();
    hold.query_row(
        "SELECT revision FROM entities WHERE kind='room' AND key='support' FOR UPDATE",
        [],
        |row| row.get::<_, i64>(0),
    )
    .unwrap();
    let mut changes = app.changes.subscribe();
    let cleaning = tokio::spawn({
        let app = app.clone();
        async move { browser_execute(&app, "admin", None, "/clean -1 #support".into(), false).await }
    });
    let inspect = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        let waiting: bool = inspect.query_row("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'UPDATE entities SET revision=$5 WHERE kind=$1%')", [], |row| row.get(0)).unwrap();
        if waiting {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "cleanup did not reach its revision lock"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(app.engine.try_read().is_ok());
    let unrelated = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        send(
            &app,
            &token,
            "unrelated",
            json!({"conversation_id":second,"text":"concurrent append"}),
        ),
    )
    .await
    .unwrap();
    assert_eq!(unrelated.0, StatusCode::OK);
    cleaning.abort();
    assert!(cleaning.await.unwrap_err().is_cancelled());
    let (started, waiting) = tokio::sync::oneshot::channel();
    let metadata = tokio::task::spawn_blocking({
        let app = app.clone();
        move || {
            started.send(()).unwrap();
            app.engine()
                .unwrap()
                .execute(None, None, "/disable helper")
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
        assert!(
            engine
                .history("admin", "support")
                .unwrap()
                .messages
                .is_empty()
        );
        assert_eq!(engine.history("admin", "other").unwrap().messages.len(), 1);
        assert_eq!(engine.data.next_sequence, before + 1);
        assert!(
            engine
                .data
                .policy
                .audit
                .iter()
                .any(|entry| entry.operation == "/clean")
        );
        assert_eq!(
            engine.data.policy.audit.back().unwrap().operation,
            "/disable"
        );
    }
    assert!(
        std::iter::from_fn(|| changes.try_recv().ok())
            .any(|change| matches!(change, web::Change::Conversation(id) if id == first))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_async_sends_release_engine_and_order_metadata_after_cancelled_requests() {
    for ingress in 0..3 {
        let fixture = crate::engine::database::tests::PostgresFixture::new();
        let (app, token) = fixture_with_engine(Engine::open_postgres(&fixture.url).unwrap());
        app.engine()
            .unwrap()
            .login("browser".into(), "admin", "hash", None)
            .unwrap();
        let mut changes = app.changes.subscribe();
        let first = if ingress == 2 {
            app.engine_read()
                .unwrap()
                .browser_message_conversation(
                    "admin",
                    None,
                    "/tell helper accepted work survives cancellation",
                )
                .unwrap()
        } else {
            conversation(&app)
        };
        let second = {
            let mut engine = app.engine().unwrap();
            engine
                .execute(Some("admin"), None, "/new room other")
                .unwrap();
            engine
                .execute(None, None, "/room add helper #other")
                .unwrap();
            engine
                .execute(None, None, "/grant admin #other w:message.edit.any")
                .unwrap();
            engine
                .execute(None, None, "/grant admin #other w:message.retract.any")
                .unwrap();
            engine.data.rooms["other"].id.clone()
        };
        let seed = send(
            &app,
            &token,
            "seed",
            json!({"conversation_id":second,"text":"seed"}),
        )
        .await;
        assert_eq!(seed.0, StatusCode::OK);
        let message_id = seed.1["message"]["id"].as_str().unwrap().to_owned();
        let deletion_seed = send(
            &app,
            &token,
            "deletion-seed",
            json!({"conversation_id":second,"text":"delete independently"}),
        )
        .await;
        assert_eq!(deletion_seed.0, StatusCode::OK);
        let deletion_id = deletion_seed.1["message"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let before = app.engine_read().unwrap().data.next_sequence;
        let blocker = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
        let hold = blocker.unchecked_transaction().unwrap();
        hold.query_row(
            "SELECT sequence FROM counters WHERE id=1 FOR UPDATE",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
        let sending = tokio::spawn({
            let app = app.clone();
            let token = token.clone();
            let first = first.clone();
            async move {
                if ingress != 0 {
                    let mut headers = HeaderMap::new();
                    headers.insert(header::COOKIE, "chat_session=browser".parse().unwrap());
                    commands::execute_input(
                        &app,
                        &headers,
                        "admin",
                        &commands::Input {
                            id: 1,
                            room: Some("support".into()),
                            text: if ingress == 2 {
                                "/tell helper accepted work survives cancellation".into()
                            } else {
                                "accepted work survives cancellation".into()
                            },
                            markdown: false,
                        },
                    )
                    .await
                    .unwrap();
                } else {
                    assert_eq!(send(
                    &app,
                    &token,
                    "cancelled-http",
                    json!({"conversation_id":first,"text":"accepted work survives cancellation"}),
                ).await.0, StatusCode::OK);
                }
            }
        });
        let inspect = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        loop {
            let waiting: bool = inspect.query_row("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'UPDATE counters SET sequence=sequence+1%RETURNING sequence')", [], |row| row.get(0)).unwrap();
            if waiting {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "HTTP append did not reach its counter lock"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        if ingress == 2 {
            assert!(
                app.engine_read().unwrap().data.private.is_empty(),
                "uncommitted first DM leaked its directory"
            );
        }
        // An independent native write and an engine reader must both progress while
        // the first HTTP request is blocked inside its PostgreSQL transaction.
        let reacted = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        request(
            &app,
            Some(&token),
            "PUT",
            "/api/bot/v1/reactions",
            json!({"conversation_id":second,"message_id":message_id,"reaction":"👍","active":true}),
        ),
    )
    .await
    .unwrap();
        assert_eq!(reacted.0, StatusCode::OK);
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, "chat_session=browser".parse().unwrap());
        for text in [
            format!("/react {message_id} ✅"),
            format!("/edit {message_id} browser edit while append waits"),
        ] {
            tokio::time::timeout(
                std::time::Duration::from_millis(500),
                commands::execute_input(
                    &app,
                    &headers,
                    "admin",
                    &commands::Input {
                        id: 1,
                        room: Some("other".into()),
                        text,
                        markdown: false,
                    },
                ),
            )
            .await
            .unwrap()
            .unwrap();
        }
        let acknowledged = tokio::time::timeout(std::time::Duration::from_millis(500), request(
        &app, Some(&token), "POST", "/api/bot/v1/read",
        json!({"conversation_id":second,"through":seed.1["message"]["sequence"],"acknowledge_shared_unread":true}),
    )).await.unwrap();
        assert_eq!(acknowledged.0, StatusCode::OK);
        for text in [
            format!("/retract {deletion_id}"),
            "/clean -1 #other --dry-run".into(),
            "/clean -1 #other".into(),
        ] {
            tokio::time::timeout(
                std::time::Duration::from_millis(500),
                commands::execute_input(
                    &app,
                    &headers,
                    "admin",
                    &commands::Input {
                        id: 1,
                        room: Some("other".into()),
                        text,
                        markdown: false,
                    },
                ),
            )
            .await
            .unwrap()
            .unwrap();
        }

        assert!(app.engine.try_read().is_ok());
        sending.abort();
        assert!(sending.await.unwrap_err().is_cancelled());
        let (started, waiting) = tokio::sync::oneshot::channel();
        let metadata = tokio::task::spawn_blocking({
            let app = app.clone();
            move || {
                started.send(()).unwrap();
                app.engine()
                    .unwrap()
                    .execute(None, None, "/disable helper")
                    .unwrap();
            }
        });
        waiting.await.unwrap();
        tokio::task::yield_now().await;
        assert!(!metadata.is_finished());
        assert!(
            app.engine.try_read().is_ok(),
            "a waiting metadata writer held the engine guard"
        );
        hold.rollback().unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), metadata)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(app.engine_read().unwrap().data.next_sequence, before + 1);
        let mut notified = false;
        while let Ok(change) = changes.try_recv() {
            if if ingress == 2 {
                matches!(change, web::Change::Metadata(_))
            } else {
                matches!(change, web::Change::Conversation(ref id) if id == &first)
            } {
                notified = true;
            }
        }
        assert!(notified, "cancelled transport lost committed notification");
        assert_eq!(
            app.engine_read()
                .unwrap()
                .history(
                    "admin",
                    if ingress == 2 {
                        "@direct:helper"
                    } else {
                        "support"
                    }
                )
                .unwrap()
                .messages
                .last()
                .unwrap()
                .text,
            "accepted work survives cancellation"
        );
        if ingress == 0 {
            app.engine()
                .unwrap()
                .execute(None, None, "/enable helper")
                .unwrap();
            app.engine()
                .unwrap()
                .execute(None, None, "/room add helper #support")
                .unwrap();
            let replay = send(
                &app,
                &token,
                "cancelled-http",
                json!({"conversation_id":first,"text":"accepted work survives cancellation"}),
            )
            .await;
            assert_eq!(replay.0, StatusCode::OK);
            assert_eq!(replay.1["replayed"], true);
            assert_eq!(app.engine_read().unwrap().data.next_sequence, before + 1);
        }
    }
}

mod native_control;
