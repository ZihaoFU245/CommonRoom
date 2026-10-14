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
async fn runtime_config_roundtrip_is_private_delegated_and_rechecked() {
    use crate::bots::runtime::ConfigCommand;
    let (app, secret) = fixture();
    let target = app.engine_read().unwrap().data.users["helper"].id.clone();
    let (sender, mut requests) = tokio::sync::mpsc::channel(8);
    let lease = app
        .bot_runtimes
        .register(target.clone(), sender.clone())
        .unwrap();
    assert!(app.bot_runtimes.register(target, sender).is_err());
    let admin_hash = app.engine_read().unwrap().data.users["admin"].hash.clone();
    app.engine()
        .unwrap()
        .login("operator".into(), "admin", &admin_hash, None)
        .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(header::COOKIE, "chat_session=operator".parse().unwrap());
    let input = commands::Input {
        markdown: false,
        id: 1,
        room: None,
        text: "/bot config set helper prompt \"Help with Rust\"".into(),
    };
    let worker = tokio::spawn(async move {
        let request = requests.recv().await.unwrap();
        assert_eq!(request.command.value.as_deref(), Some("Help with Rust"));
        request
            .reply
            .send(Ok(std::collections::BTreeMap::from([(
                "prompt".into(),
                "Help with Rust".into(),
            )])))
            .unwrap();
        requests
    });
    let (output, change) = commands::execute_input(&app, &headers, "admin", &input)
        .await
        .unwrap();
    assert!(change.is_none());
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&output).unwrap()["values"]["prompt"],
        "Help with Rust"
    );
    let mut requests = worker.await.unwrap();
    let worker_app = app.clone();
    let worker = tokio::spawn(async move {
        let request = requests.recv().await.unwrap();
        worker_app
            .engine()
            .unwrap()
            .execute(None, None, "/disable admin")
            .unwrap();
        request
            .reply
            .send(Ok(std::collections::BTreeMap::from([(
                "prompt".into(),
                "hidden after revocation".into(),
            )])))
            .unwrap();
    });
    let error = commands::execute_input(
        &app,
        &headers,
        "admin",
        &commands::Input {
            markdown: false,
            id: 2,
            room: None,
            text: "/bot config fetch helper".into(),
        },
    )
    .await
    .unwrap_err();
    assert!(error.contains("log in"));
    worker.await.unwrap();
    drop(lease);
    // A bot token does not grant self-configuration or administration.
    assert_eq!(
        request(
            &app,
            Some(&secret),
            "POST",
            "/api/bot/v1/commands",
            json!({"id":3,"view":"support","command":"/bot config set helper prompt bypass"})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert!(ConfigCommand::parse("/bot config fetch helper prompt extra").is_err());
    assert!(ConfigCommand::parse("/bot config set helper bad/key x").is_err());
    assert!(ConfigCommand::parse("/bot config set helper prompt \"\"").is_ok());
}

#[tokio::test]
async fn sudo_runtime_config_keeps_the_caller_and_rechecks_elevation() {
    let (app, _) = fixture();
    {
        let mut engine = app.engine().unwrap();
        engine
            .provision("caller", "hash".into(), false, false)
            .unwrap();
        engine.execute(None, None, "/revoke caller user").unwrap();
        for permission in ["/sudo", "x:command.sudo"] {
            engine
                .execute(None, None, &format!("/grant caller @global {permission}"))
                .unwrap();
        }
        engine
            .login("sudo-config".into(), "caller", "hash", None)
            .unwrap();
    }
    let mut headers = HeaderMap::new();
    headers.insert(header::COOKIE, "chat_session=sudo-config".parse().unwrap());
    let target = app.engine_read().unwrap().data.users["helper"].id.clone();
    let (sender, mut requests) = tokio::sync::mpsc::channel(8);
    let _lease = app.bot_runtimes.register(target, sender).unwrap();
    for text in [
        "/bot config set helper prompt test",
        "/bot config fetch helper prompt",
    ] {
        let input = commands::Input {
            markdown: false,
            id: 1,
            room: Some("secret".into()),
            text: text.into(),
        };
        assert!(
            commands::execute_input(&app, &headers, "caller", &input)
                .await
                .is_err()
        );
        let elevated = commands::Input {
            text: format!("/sudo {text}"),
            ..input
        };
        let command = commands::execute_input(&app, &headers, "caller", &elevated);
        let worker = async {
            let request = requests.recv().await.unwrap();
            assert_eq!(request.authority.actor.as_deref(), Some("caller"));
            assert!(request.authority.sudo);
            assert!(!app.engine_read().unwrap().is_su(Some("caller")));
            request
                .reply
                .send(Ok(std::collections::BTreeMap::from([(
                    "prompt".into(),
                    "test".into(),
                )])))
                .unwrap();
        };
        let (result, ()) = tokio::join!(command, worker);
        let (output, change) = result.unwrap();
        assert!(change.is_none());
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&output).unwrap()["values"]["prompt"],
            "test"
        );
        assert!(!app.engine_read().unwrap().is_su(Some("caller")));
    }
    for permission in ["/sudo", "x:command.sudo"] {
        let input = commands::Input {
            markdown: false,
            id: 2,
            room: None,
            text: "/sudo /bot config fetch helper prompt".into(),
        };
        let command = commands::execute_input(&app, &headers, "caller", &input);
        let worker = async {
            let request = requests.recv().await.unwrap();
            app.engine()
                .unwrap()
                .execute(None, None, &format!("/revoke caller @global {permission}"))
                .unwrap();
            request
                .reply
                .send(Ok(std::collections::BTreeMap::from([(
                    "prompt".into(),
                    "must not be disclosed".into(),
                )])))
                .unwrap();
        };
        let (result, ()) = tokio::join!(command, worker);
        assert!(result.is_err());
        assert!(!app.engine_read().unwrap().is_su(Some("caller")));
        app.engine()
            .unwrap()
            .execute(None, None, &format!("/grant caller @global {permission}"))
            .unwrap();
    }
}

#[tokio::test]
async fn runtime_config_offline_disconnect_and_response_limits_fail_cleanly() {
    use crate::bots::runtime::{Authority, ConfigCommand, validate_response};
    let (app, _) = fixture();
    let authority = Authority {
        actor: None,
        headers: Default::default(),
        view: None,
        sudo: false,
    };
    let command = ConfigCommand::parse("/bot config fetch helper")
        .unwrap()
        .unwrap();
    assert!(
        app.bot_runtimes
            .execute(&app, authority.clone(), command.clone())
            .await
            .unwrap_err()
            .contains("offline")
    );
    let target = app.engine_read().unwrap().data.users["helper"].id.clone();
    let (sender, mut requests) = tokio::sync::mpsc::channel(8);
    let lease = app.bot_runtimes.register(target, sender).unwrap();
    let worker = tokio::spawn(async move {
        drop(requests.recv().await.unwrap());
    });
    assert!(
        app.bot_runtimes
            .execute(&app, authority, command)
            .await
            .unwrap_err()
            .contains("disconnected")
    );
    worker.await.unwrap();
    drop(lease);
    assert!(validate_response(None, None).is_err());
    assert!(validate_response(Some(Default::default()), Some("error".into())).is_err());
    assert!(
        validate_response(
            Some(std::collections::BTreeMap::from([(
                "prompt".into(),
                "x".repeat(4097)
            )])),
            None
        )
        .is_err()
    );
    assert!(
        validate_response(None, Some("unsupported key".into()))
            .unwrap_err()
            .contains("unsupported key")
    );
}

#[tokio::test]
async fn runtime_config_timeout_cancels_the_pending_request() {
    use crate::bots::runtime::{Authority, ConfigCommand};
    let (app, _) = fixture();
    let target = app.engine_read().unwrap().data.users["helper"].id.clone();
    let (sender, mut requests) = tokio::sync::mpsc::channel(8);
    let _lease = app.bot_runtimes.register(target, sender).unwrap();
    let authority = Authority {
        actor: None,
        headers: Default::default(),
        view: None,
        sudo: false,
    };
    let command = ConfigCommand::parse("/bot config set helper prompt unanswered")
        .unwrap()
        .unwrap();
    let (result, pending) = tokio::join!(
        app.bot_runtimes.execute(&app, authority, command),
        requests.recv()
    );
    assert!(result.unwrap_err().contains("timed out"));
    assert!(pending.unwrap().reply.is_closed());
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
async fn frozen_profile_is_bearer_only_scoped_and_has_no_command_transport() {
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
    for path in ["/api/bot/me", "/api/bot/ws", "/api/bot/v1/commands"] {
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
            json!({"conversation_id":id,"through":seq})
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
            json!({"conversation_id":view,"through":1}),
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
        recheck(&app.engine_read().unwrap(), &headers, &account).unwrap(),
        "renamed"
    );
    let snapshot = request(&app, Some(&token), "GET", "/api/bot/v1/me", json!({}))
        .await
        .1;
    assert_eq!(snapshot["account"]["id"], account);
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
