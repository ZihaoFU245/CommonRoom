use super::*;
use crate::web;
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
async fn bearer_required_and_snapshots_expose_only_granted_commands_and_rooms() {
    let (app, secret) = fixture();
    for path in ["/api/bot/me", "/api/bot/history?view=support"] {
        assert_eq!(
            request(&app, None, "GET", path, json!({})).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(&app, Some("invalid"), "GET", path, json!({}))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
    }
    let (status, snapshot) = request(&app, Some(&secret), "GET", "/api/bot/me", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(snapshot["username"], "helper");
    assert_eq!(snapshot["rooms"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["rooms"][0]["name"], "support");
    let commands = snapshot["rooms"][0]["commands"].as_array().unwrap();
    assert!(commands.iter().any(|c| c["name"] == "/man"));
    assert!(commands.iter().any(|c| c["name"] == "/reply"));
    for forbidden in ["/passwd", "/rename", "/retract", "/clean", "/bot"] {
        assert!(!commands.iter().any(|c| c["name"] == forbidden));
    }
    let account = { app.engine().unwrap().data.users["helper"].clone() };
    app.engine()
        .unwrap()
        .login("cookie".into(), "helper", &account.hash, None)
        .unwrap();
    let response = web::router(app.clone())
        .oneshot(
            Request::builder()
                .uri("/api/bot/me")
                .header(header::COOKIE, "chat_session=cookie")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = web::router(app.clone())
        .oneshot(
            Request::builder()
                .uri("/api/bot/me")
                .header(header::AUTHORIZATION, format!("Bearer {secret}"))
                .header(header::AUTHORIZATION, "Bearer invalid")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn actions_history_mentions_reads_replies_and_reactions_share_engine_rules() {
    let (app, secret) = fixture();
    let mut updates = app.changes.subscribe();
    let (status, result) = request(
        &app,
        Some(&secret),
        "POST",
        "/api/bot/messages",
        json!({"id":1,"view":"support","text":"/rm user admin"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["message"]["from"], "helper");
    assert_eq!(result["message"]["text"], "/rm user admin");
    assert!(app.engine().unwrap().data.users.contains_key("admin"));
    assert!(matches!(
        updates.try_recv().unwrap(),
        Change::Conversation(_)
    ));
    app.engine()
        .unwrap()
        .execute(Some("admin"), Some("support"), "@helper hello")
        .unwrap();
    let (_, history) = request(
        &app,
        Some(&secret),
        "GET",
        "/api/bot/history?view=support",
        json!({}),
    )
    .await;
    let original = &history["messages"][1];
    assert_eq!(original["mentions"], json!(["helper"]));
    let id = original["id"].as_str().unwrap();
    assert_eq!(
        request(
            &app,
            Some(&secret),
            "POST",
            "/api/bot/messages",
            json!({"id":2,"view":"support","text":"answer","reply_to":id})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            Some(&secret),
            "POST",
            "/api/bot/commands",
            json!({"id":3,"view":"support","command":format!("/react {id} 👍")})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            Some(&secret),
            "POST",
            "/api/bot/read",
            json!({"view":"support","through":original["sequence"]})
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, history) = request(
        &app,
        Some(&secret),
        "GET",
        "/api/bot/history?view=support&after=1",
        json!({}),
    )
    .await;
    assert_eq!(history["messages"][0]["reactions"]["👍"], json!(["helper"]));
    assert_eq!(history["messages"][1]["reply"]["id"], id);
    assert_eq!(
        request(
            &app,
            Some(&secret),
            "POST",
            "/api/bot/commands",
            json!({"id":4,"view":"support","command":"/man reply"})
        )
        .await
        .0,
        StatusCode::OK
    );
    for command in [
        "/passwd old changed",
        "/rename changed",
        "/retract missing",
        "/bot new nope",
    ] {
        assert_eq!(
            request(
                &app,
                Some(&secret),
                "POST",
                "/api/bot/commands",
                json!({"id":5,"view":"support","command":command})
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        request(
            &app,
            Some(&secret),
            "GET",
            "/api/bot/history?view=secret",
            json!({})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            Some(&secret),
            "POST",
            "/api/bot/messages",
            json!({"id":6,"view":"secret","text":"denied"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/revoke helper support r:message.read")
        .unwrap();
    assert_eq!(
        request(
            &app,
            Some(&secret),
            "GET",
            "/api/bot/history?view=support",
            json!({})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn malformed_requests_revocation_shutdown_and_rate_limits_are_explicit() {
    let (app, secret) = fixture();
    assert_eq!(
        request(
            &app,
            Some(&secret),
            "POST",
            "/api/bot/messages",
            json!({"id":1,"view":"support","text":"","from":"admin"})
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        request(
            &app,
            Some(&secret),
            "POST",
            "/api/bot/messages",
            json!({"id":1,"view":"support","text":" "})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &app,
            Some(&secret),
            "POST",
            "/api/bot/messages",
            json!({"id":1,"view":"support","text":"x","reply_to":"id /rm user admin"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &app,
            Some(&secret),
            "POST",
            "/api/bot/commands",
            json!({"id":1,"view":"support","command":"plain text"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &app,
            Some(&secret),
            "GET",
            "/api/bot/history?view=support&limit=101",
            json!({})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    for id in 0..30 {
        assert_eq!(
            request(
                &app,
                Some(&secret),
                "POST",
                "/api/bot/commands",
                json!({"id":id,"view":"support","command":"/whoami"})
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    assert_eq!(
        request(
            &app,
            Some(&secret),
            "POST",
            "/api/bot/messages",
            json!({"id":31,"view":"support","text":"over budget"})
        )
        .await
        .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    app.engine()
        .unwrap()
        .execute(Some("admin"), None, "/disable helper")
        .unwrap();
    assert_eq!(
        request(&app, Some(&secret), "GET", "/api/bot/me", json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    app.engine()
        .unwrap()
        .execute(Some("admin"), None, "/enable helper")
        .unwrap();
    let token_id = app
        .engine()
        .unwrap()
        .data
        .bots
        .values()
        .next()
        .unwrap()
        .tokens
        .keys()
        .next()
        .unwrap()
        .clone();
    app.engine()
        .unwrap()
        .execute(
            Some("admin"),
            None,
            &format!("/bot token rm helper {token_id}"),
        )
        .unwrap();
    assert_eq!(
        request(&app, Some(&secret), "GET", "/api/bot/me", json!({}))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    app.stopping
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        request(&app, Some(&secret), "GET", "/api/bot/me", json!({}))
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn base_paths_and_unicode_message_limits_work() {
    let (mut app, secret) = fixture();
    app.config = Arc::new(Config {
        base_url: "/commonroom/".into(),
        ..Config::default()
    });
    assert_eq!(
        request(
            &app,
            Some(&secret),
            "GET",
            "/commonroom/api/bot/me",
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    let (status, value) = request(
        &app,
        Some(&secret),
        "POST",
        "/commonroom/api/bot/messages",
        json!({"id":1,"view":"support","text":"🙂".repeat(4000)}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(
        request(
            &app,
            Some(&secret),
            "POST",
            "/commonroom/api/bot/messages",
            json!({"id":2,"view":"support","text":"🙂".repeat(4001)})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn explicit_private_grants_allow_literal_direct_sends_without_other_private_access() {
    let (app, secret) = fixture();
    {
        let mut e = app.engine().unwrap();
        e.execute(Some("admin"), None, "/tell helper private invitation")
            .unwrap();
        e.execute(
            None,
            None,
            "/grant helper @private:admin:helper r:message.read",
        )
        .unwrap();
        e.execute(
            None,
            None,
            "/grant helper @private:admin:helper w:message.create",
        )
        .unwrap();
    }
    let (status, response) = request(
        &app,
        Some(&secret),
        "POST",
        "/api/bot/messages",
        json!({"id":1,"view":"@direct:admin","text":"/literal private text"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["message"]["to"], "admin");
    let (status, history) = request(
        &app,
        Some(&secret),
        "GET",
        "/api/bot/history?view=%40direct%3Aadmin",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(history["messages"].as_array().unwrap().len(), 2);
    assert_eq!(history["messages"][1]["text"], "/literal private text");
}

#[test]
fn bot_password_jobs_recheck_credentials_and_permissions_after_hashing() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .max_blocking_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        for revoke_token in [false, true] {
            let (app, secret) = fixture();
            app.engine()
                .unwrap()
                .execute(None, None, "/grant helper admin")
                .unwrap();
            let (release, wait) = std::sync::mpsc::channel();
            let (started, ready) = tokio::sync::oneshot::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                let _ = started.send(());
                let _ = wait.recv_timeout(std::time::Duration::from_secs(10));
            });
            ready.await.unwrap();
            let job_app = app.clone();
            let job = tokio::spawn(async move {
                let mut headers = HeaderMap::new();
                headers.insert(
                    header::AUTHORIZATION,
                    format!("Bearer {secret}").parse().unwrap(),
                );
                let input = commands::Input {
                    markdown: false,
                    id: 1,
                    room: None,
                    text: "/new user created long-fixture-password user".into(),
                };
                commands::execute_input(&job_app, &headers, "helper", &input).await
            });
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while app.password_jobs.available_permits() == 2 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            let expected = {
                let mut e = app.engine().unwrap();
                if revoke_token {
                    let token_id = e.data.bots[&e.data.users["helper"].id]
                        .tokens
                        .keys()
                        .next()
                        .unwrap()
                        .clone();
                    e.execute(None, None, &format!("/bot token rm helper {token_id}"))
                        .unwrap();
                } else {
                    e.execute(None, None, "/revoke helper admin").unwrap();
                }
                e.data.clone()
            };
            release.send(()).unwrap();
            blocker.await.unwrap();
            let error = job.await.unwrap().unwrap_err();
            assert!(
                error.contains(if revoke_token {
                    "Please log in"
                } else {
                    "Command not permitted"
                }),
                "{error}"
            );
            assert!(app.engine().unwrap().data == expected);
        }
    });
}

#[tokio::test]
async fn production_allows_direct_bot_http_only_from_trusted_socket_peer() {
    use axum::extract::ConnectInfo;
    use std::net::SocketAddr;
    let (mut app, secret) = fixture();
    app.config = Arc::new(Config {
        production: true,
        trust: "127.0.0.1".parse().unwrap(),
        base_url: "/commonroom/".into(),
        origins: vec!["https://chat.example.com".into()],
        ..Config::default()
    });
    for (peer, path, credential, forwarded, expected) in [
        (
            "127.0.0.1:4000",
            "/commonroom/api/bot/me",
            Some(secret.as_str()),
            None,
            StatusCode::OK,
        ),
        (
            "[::ffff:127.0.0.1]:4000",
            "/commonroom/api/bot/me",
            Some(secret.as_str()),
            None,
            StatusCode::OK,
        ),
        (
            "127.0.0.2:4000",
            "/commonroom/api/bot/me",
            Some(secret.as_str()),
            None,
            StatusCode::FORBIDDEN,
        ),
        (
            "127.0.0.1:4000",
            "/commonroom/api/bot/me",
            Some("invalid"),
            None,
            StatusCode::UNAUTHORIZED,
        ),
        (
            "127.0.0.1:4000",
            "/commonroom/api/bot/me",
            None,
            None,
            StatusCode::BAD_REQUEST,
        ),
        (
            "127.0.0.1:4000",
            "/commonroom/api/me",
            Some(secret.as_str()),
            None,
            StatusCode::BAD_REQUEST,
        ),
        (
            "127.0.0.1:4000",
            "/commonroom/api/bot/me",
            Some(secret.as_str()),
            Some("http"),
            StatusCode::BAD_REQUEST,
        ),
        (
            "127.0.0.1:4000",
            "/commonroom/api/bot/me",
            Some(secret.as_str()),
            Some("https"),
            StatusCode::OK,
        ),
        (
            "127.0.0.2:4000",
            "/commonroom/api/bot/me",
            Some(secret.as_str()),
            Some("https"),
            StatusCode::FORBIDDEN,
        ),
    ] {
        let mut builder = Request::builder()
            .uri(path)
            .extension(ConnectInfo(peer.parse::<SocketAddr>().unwrap()))
            .header("x-forwarded-for", "127.0.0.1");
        if let Some(credential) = credential {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {credential}"));
        }
        if let Some(forwarded) = forwarded {
            builder = builder.header("x-forwarded-proto", forwarded);
        }
        let response = web::router(app.clone())
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            expected,
            "peer={peer} path={path} forwarded={forwarded:?}"
        );
    }
    let response = web::router(app.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/commonroom/api/bot/messages")
                .extension(ConnectInfo("127.0.0.1:4000".parse::<SocketAddr>().unwrap()))
                .header(header::AUTHORIZATION, format!("Bearer {secret}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"id":1,"view":"support","text":"direct local HTTP"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = web::router(app.clone())
        .oneshot(
            Request::builder()
                .uri("/commonroom/api/bot/me")
                .header(header::AUTHORIZATION, format!("Bearer {secret}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "missing socket identity must fail closed"
    );
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
            "/api/bot/commands",
            json!({"id":3,"view":"support","command":"/bot config set helper prompt bypass"})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert!(ConfigCommand::parse("/bot config fetch helper prompt extra").is_err());
    assert!(ConfigCommand::parse("/bot config set helper bad/key x").is_err());
    assert!(ConfigCommand::parse("/bot config set helper prompt \"\"").is_ok());
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
