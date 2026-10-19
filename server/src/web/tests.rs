use super::*;
use super::{
    auth::cookie,
    security::{check_origin, trusted_proxy},
};
use axum::http::{HeaderMap, HeaderValue, header};

#[test]
#[allow(clippy::panic)] // Deliberately simulate a panic during a state mutation.
fn poisoned_engine_refuses_access_instead_of_reusing_partial_state() {
    let (changes, _) = broadcast::channel(1);
    let app = App {
        media: None,
        engine: Arc::new(RwLock::new(
            Engine::open(std::path::Path::new(":memory:")).unwrap(),
        )),
        attempts: Default::default(),
        changes,
        config: Arc::new(Config::default()),
        connections: Arc::new(tokio::sync::Semaphore::new(128)),
        io_jobs: Arc::new(tokio::sync::Semaphore::new(64)),
        password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
        snapshots: Default::default(),
        bot_runtimes: Default::default(),
        stopping: Default::default(),
    };
    let engine = app.engine.clone();
    assert!(
        std::thread::spawn(move || {
            let _guard = engine.write().unwrap();
            panic!("simulated partial mutation");
        })
        .join()
        .is_err()
    );
    assert!(app.engine().is_err());
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        HeaderValue::from_static("chat_session=fixture"),
    );
    let (status, _) = auth::authenticate(&app, &headers).unwrap_err();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(app.engine.is_poisoned());
}
#[test]
fn proxy_trust_uses_the_socket_peer() {
    let config = Config::default();
    assert!(trusted_proxy(&config, "127.0.0.1".parse().unwrap()));
    assert!(trusted_proxy(&config, "::ffff:127.0.0.1".parse().unwrap()));
    assert!(!trusted_proxy(&config, "127.0.0.2".parse().unwrap()));
    assert!(!trusted_proxy(&config, "10.0.0.2".parse().unwrap()));
}

#[tokio::test]
#[allow(clippy::panic)] // Simulate a reader panic while its mutex is held.
async fn poisoned_history_returns_restart_diagnostics_to_http_and_socket_snapshots() {
    let (changes, _) = broadcast::channel(1);
    let mut engine = Engine::open(std::path::Path::new(":memory:")).unwrap();
    engine
        .provision("alice", "hash".into(), false, false)
        .unwrap();
    engine
        .login("fixture".into(), "alice", "hash", None)
        .unwrap();
    let store = engine.history_store.clone();
    assert!(
        std::thread::spawn(move || store.read::<()>(|_| panic!("poison reader")))
            .join()
            .is_err()
    );
    let app = App {
        media: None,
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
    };
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        HeaderValue::from_static("chat_session=fixture"),
    );
    headers.insert(
        header::ORIGIN,
        HeaderValue::from_static("http://localhost:5173"),
    );
    let error = auth::me(axum::extract::State(app.clone()), headers.clone())
        .await
        .unwrap_err();
    assert_eq!(error.0, StatusCode::SERVICE_UNAVAILABLE);
    assert!(error.1.0.to_string().contains("restart the service"));
    assert!(
        snapshots::value(&app, &headers, false)
            .unwrap_err()
            .contains("restart the service")
    );
}
#[test]
fn cookies_are_http_only_and_secure_in_production() {
    let config = Config {
        bind: String::new(),
        origins: vec![],
        production: true,
        ..Config::default()
    };
    let (changes, _) = broadcast::channel(1);
    let app = App {
        media: None,
        engine: Arc::new(RwLock::new(
            Engine::open(std::path::Path::new(":memory:")).unwrap(),
        )),
        attempts: Default::default(),
        changes,
        config: Arc::new(config),
        connections: Arc::new(tokio::sync::Semaphore::new(128)),
        io_jobs: Arc::new(tokio::sync::Semaphore::new(64)),
        password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
        snapshots: Default::default(),
        bot_runtimes: Default::default(),
        stopping: Default::default(),
    };
    let value = cookie(&app, "token", 43200);
    assert!(value.contains("HttpOnly"));
    assert!(value.contains("; Secure"));
    let mut headers = HeaderMap::new();
    headers.insert(
        header::ORIGIN,
        HeaderValue::from_static("https://evil.example"),
    );
    assert!(check_origin(&app, &headers).is_err());
}

#[test]
fn real_ip_requires_trusted_peer_and_one_valid_address() {
    let mut config = Config::default();
    let proxy = "127.0.0.1".parse().unwrap();
    let client: std::net::IpAddr = "198.51.100.7".parse().unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-for", HeaderValue::from_static("198.51.100.7"));
    assert_eq!(security::client_ip(&config, proxy, &headers), client);
    assert_eq!(
        security::client_ip(&config, "::ffff:127.0.0.1".parse().unwrap(), &headers),
        client
    );
    let untrusted: std::net::IpAddr = "198.51.100.8".parse().unwrap();
    assert_eq!(security::client_ip(&config, untrusted, &headers), untrusted);
    for value in [
        "",
        "unknown",
        "198.51.100.7:1234",
        "198.51.100.7, 198.51.100.8",
    ] {
        headers.insert("x-forwarded-for", HeaderValue::from_str(value).unwrap());
        assert_eq!(security::client_ip(&config, proxy, &headers), proxy);
    }
    headers.insert(
        "x-forwarded-for",
        HeaderValue::from_static("::ffff:198.51.100.7"),
    );
    assert_eq!(security::client_ip(&config, proxy, &headers), client);
    headers.insert("x-forwarded-for", HeaderValue::from_static("2001:db8::7"));
    assert_eq!(
        security::client_ip(&config, proxy, &headers),
        "2001:db8::7".parse::<std::net::IpAddr>().unwrap()
    );
    headers.append("x-forwarded-for", HeaderValue::from_static("198.51.100.8"));
    assert_eq!(security::client_ip(&config, proxy, &headers), proxy);
    headers.clear();
    assert_eq!(security::client_ip(&config, proxy, &headers), proxy);
    headers.insert("x-real-ip", HeaderValue::from_static("198.51.100.7"));
    config.set_real_ip_from = Some("X-Real-IP".into());
    assert_eq!(security::client_ip(&config, proxy, &headers), client);
    config.set_real_ip_from = None;
    assert_eq!(security::client_ip(&config, proxy, &headers), proxy);
}

#[tokio::test]
async fn session_gets_require_trusted_origin_or_same_origin_fetch_metadata() {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    let (changes, _) = broadcast::channel(1);
    let mut engine = Engine::open(std::path::Path::new(":memory:")).unwrap();
    engine
        .provision("alice", "hash".into(), true, false)
        .unwrap();
    engine
        .execute(Some("alice"), None, "/new room support")
        .unwrap();
    engine
        .login("fixture".into(), "alice", "hash", None)
        .unwrap();
    engine
        .execute(None, Some("support"), "/bot new helper")
        .unwrap();
    let credential = engine
        .execute(None, None, "/bot token new helper read-tests")
        .unwrap()
        .lines()
        .last()
        .unwrap()
        .to_owned();
    let app = App {
        media: None,
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
    };
    for path in ["/api/me", "/api/history?view=support"] {
        for (origin, site, status) in [
            (Some("http://localhost:5173"), None, StatusCode::OK),
            (None, Some("same-origin"), StatusCode::OK),
            (
                Some("https://evil.example"),
                Some("same-origin"),
                StatusCode::FORBIDDEN,
            ),
            (None, Some("same-site"), StatusCode::FORBIDDEN),
            (None, Some("cross-site"), StatusCode::FORBIDDEN),
            (None, None, StatusCode::FORBIDDEN),
        ] {
            let mut builder = Request::builder()
                .uri(path)
                .header(header::COOKIE, "chat_session=fixture");
            if let Some(origin) = origin {
                builder = builder.header(header::ORIGIN, origin);
            }
            if let Some(site) = site {
                builder = builder.header("sec-fetch-site", site);
            }
            let response = router(app.clone())
                .oneshot(builder.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), status, "{path}: {origin:?}, {site:?}");
        }
        let response = router(app.clone())
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header(header::AUTHORIZATION, format!("Bearer {credential}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}

#[tokio::test]
async fn login_accepts_unicode_names_and_rejects_disabled_and_missing_accounts() {
    let (changes, _) = broadcast::channel(1);
    let mut engine = Engine::open(std::path::Path::new(":memory:")).unwrap();
    let password = "unicode-password";
    let hash = crate::engine::hash_password(password).unwrap();
    let names = ["界".repeat(32), "😀".repeat(32)];
    for name in &names {
        engine.provision(name, hash.clone(), false, false).unwrap();
    }
    engine.provision("disabled", hash, false, false).unwrap();
    engine.execute(None, None, "/disable disabled").unwrap();
    let app = App {
        media: None,
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
    };
    for (name, succeeds) in names
        .iter()
        .map(|n| (n.as_str(), true))
        .chain([("missing", false), ("disabled", false)])
    {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://localhost:5173"),
        );
        let input = serde_json::from_value(json!({"username":name,"password":password})).unwrap();
        let result = auth::login(
            axum::extract::State(app.clone()),
            axum::extract::ConnectInfo("127.0.0.1:1234".parse().unwrap()),
            headers,
            Json(input),
        )
        .await;
        if succeeds {
            assert_eq!(result.unwrap().status(), StatusCode::OK);
        } else {
            assert_eq!(result.unwrap_err().0, StatusCode::UNAUTHORIZED);
        }
    }
}

#[tokio::test]
async fn login_throttling_separates_verified_clients_and_ignores_spoofing() {
    let (changes, _) = broadcast::channel(1);
    let app = App {
        media: None,
        engine: Arc::new(RwLock::new(
            Engine::open(std::path::Path::new(":memory:")).unwrap(),
        )),
        attempts: Default::default(),
        changes,
        config: Arc::new(Config::default()),
        connections: Arc::new(tokio::sync::Semaphore::new(128)),
        io_jobs: Arc::new(tokio::sync::Semaphore::new(64)),
        password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
        snapshots: Default::default(),
        bot_runtimes: Default::default(),
        stopping: Default::default(),
    };
    async fn attempt(app: &App, peer: &str, forwarded: &str) -> StatusCode {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://localhost:5173"),
        );
        headers.insert("x-forwarded-for", HeaderValue::from_str(forwarded).unwrap());
        let input =
            serde_json::from_value(json!({"username":"missing","password":"invalid"})).unwrap();
        auth::login(
            axum::extract::State(app.clone()),
            axum::extract::ConnectInfo(peer.parse().unwrap()),
            headers,
            Json(input),
        )
        .await
        .unwrap_err()
        .0
    }
    for _ in 0..10 {
        assert_eq!(
            attempt(&app, "127.0.0.1:1234", "198.51.100.7").await,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        attempt(&app, "127.0.0.1:5678", "::ffff:198.51.100.7").await,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        attempt(&app, "127.0.0.1:1234", "198.51.100.8").await,
        StatusCode::UNAUTHORIZED
    );
    for n in 0..10 {
        assert_eq!(
            attempt(&app, "198.51.100.9:1234", &format!("203.0.113.{n}")).await,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        attempt(&app, "198.51.100.9:5678", "203.0.113.99").await,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn sudo_authorizes_web_special_commands_without_changing_the_caller() {
    let mut engine = Engine::open(std::path::Path::new(":memory:")).unwrap();
    let actor_hash = crate::engine::hash_password("actor-old-password").unwrap();
    engine
        .provision("actor", actor_hash.clone(), false, false)
        .unwrap();
    engine
        .provision("target", "hash".into(), false, false)
        .unwrap();
    engine.execute(None, None, "/grant target su").unwrap();
    engine.execute(None, None, "/disable target").unwrap();
    engine.execute(None, None, "/new room team").unwrap();
    engine.execute(None, None, "/revoke actor user").unwrap();
    for permission in ["/sudo", "x:command.sudo"] {
        engine
            .execute(None, None, &format!("/grant actor @global {permission}"))
            .unwrap();
    }
    engine
        .login("fixture".into(), "actor", &actor_hash, None)
        .unwrap();
    let (changes, _) = broadcast::channel(1);
    let app = App {
        media: None,
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
    };
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        HeaderValue::from_static("chat_session=fixture"),
    );
    for (text, changed, expected) in [
        ("/sudo /configs", false, None),
        (
            "/sudo /passwd actor-old-password actor-new-password",
            true,
            Some("Password changed. Other sessions signed out."),
        ),
        (
            "/sudo /reset target long-sudo-password",
            true,
            Some("Account target password reset."),
        ),
        ("/sudo /enable target", true, Some("Enabled target.")),
        (
            "/sudo /new user created long-created-password admin",
            true,
            Some("Account created created."),
        ),
        ("/sudo /clear", false, Some("Done.")),
        ("/sudo /logout", false, Some("Done.")),
    ] {
        let input = super::commands::Input {
            markdown: false,
            id: 1,
            room: None,
            text: text.into(),
        };
        let (reply, mutation) = super::commands::execute_input(&app, &headers, "actor", &input)
            .await
            .unwrap();
        if let Some(expected) = expected {
            assert_eq!(reply, expected, "{text}");
        } else {
            let config: serde_json::Value = serde_json::from_str(&reply).unwrap();
            assert_eq!(config["bind"], "127.0.0.1:3000");
        }
        assert_eq!(mutation.is_some(), changed, "{text}");
        assert!(!app.engine().unwrap().is_su(Some("actor")));
    }
    let join = super::commands::Input {
        markdown: false,
        id: 2,
        room: None,
        text: "/sudo /join team".into(),
    };
    let (reply, mutation) = super::commands::execute_input(&app, &headers, "actor", &join)
        .await
        .unwrap();
    assert_eq!(reply, "Joined #team.");
    assert!(matches!(mutation, Some(Change::All)));
    let snapshot = app.engine().unwrap().snapshot("actor").unwrap();
    let room = snapshot.rooms.iter().find(|r| r.name == "team").unwrap();
    assert!(room.members.contains("actor"));
    assert!(room.permissions.contains(&"w:message.create".into()));
    assert!(snapshot.groups.is_empty());
    let repeated = super::commands::Input {
        text: "/sudo /join team".into(),
        ..join
    };
    let (repeated_reply, mutation) =
        super::commands::execute_input(&app, &headers, "actor", &repeated)
            .await
            .unwrap();
    assert_eq!(reply, repeated_reply);
    assert!(mutation.is_none());
    for (command, expected) in [
        ("/man sudo", crate::manual::manual(Some("sudo"))),
        ("/join missing", Err("Room not found.".into())),
    ] {
        let elevated = super::commands::Input {
            markdown: false,
            id: 3,
            room: None,
            text: format!("/sudo {command}"),
        };
        let elevated_result =
            super::commands::execute_input(&app, &headers, "actor", &elevated).await;
        assert_eq!(
            expected,
            elevated_result.map(|(reply, _)| reply),
            "{command}"
        );
    }
    let e = app.engine().unwrap();
    assert!(crate::engine::verify_password(
        "actor-new-password",
        &e.data.users["actor"].hash
    ));
    assert_eq!(e.session("fixture").as_deref(), Some("actor"));
    assert!(crate::engine::verify_password(
        "long-sudo-password",
        &e.data.users["target"].hash
    ));
    assert!(e.is_su(Some("target")));
    assert!(e.is_admin("created"));
    assert_eq!(
        e.data.policy.audit.back().unwrap().actor_id,
        e.data.users["actor"].id
    );
    assert!(e.groups(Some("actor")).is_empty());
}

#[test]
fn password_jobs_recheck_authority_session_and_target_after_hashing() {
    // One worker runs the command dispatcher; the other queues hashing behind
    // this test barrier, rather than relying on timing sleeps.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .max_blocking_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        for case in [
            "action",
            "command",
            "target-su",
            "target-disabled-su",
            "target-replaced",
            "session",
            "disabled",
            "creation",
            "sudo-action",
            "sudo-command",
            "sudo-disabled",
            "sudo-session",
            "sudo-target-replaced",
        ] {
            let mut engine = Engine::open(std::path::Path::new(":memory:")).unwrap();
            engine
                .provision("actor", "hash".into(), true, false)
                .unwrap();
            engine
                .provision("target", "hash".into(), false, false)
                .unwrap();
            engine
                .execute(
                    None,
                    None,
                    "/grant actor @account:target x:account.password.reset",
                )
                .unwrap();
            engine
                .execute(None, None, "/grant actor @account:target /reset")
                .unwrap();
            engine
                .login("fixture".into(), "actor", "hash", None)
                .unwrap();
            if case.starts_with("sudo-") {
                for permission in ["/sudo", "x:command.sudo"] {
                    engine
                        .execute(None, None, &format!("/grant actor @global {permission}"))
                        .unwrap();
                }
            }
            let (changes, _) = broadcast::channel(1);
            let app = App {
                media: None,
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
            };
            let (release, wait) = std::sync::mpsc::channel();
            let (started, ready) = tokio::sync::oneshot::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                let _ = started.send(());
                let _ = wait.recv();
            });
            ready.await.unwrap();
            let job_app = app.clone();
            let text = if case == "creation" {
                "/new user created long-fixture-password user"
            } else if case.starts_with("sudo-") {
                "/sudo /reset target long-fixture-password"
            } else {
                "/reset target long-fixture-password"
            };
            let job = tokio::spawn(async move {
                let mut headers = HeaderMap::new();
                headers.insert(
                    header::COOKIE,
                    HeaderValue::from_static("chat_session=fixture"),
                );
                let input = super::commands::Input {
                    markdown: false,
                    id: 1,
                    room: None,
                    text: text.into(),
                };
                super::commands::execute_input(&job_app, &headers, "actor", &input).await
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
                match case {
                    "sudo-action" => {
                        e.execute(None, None, "/revoke actor @global x:command.sudo")
                            .unwrap();
                    }
                    "sudo-command" => {
                        e.execute(None, None, "/revoke actor @global /sudo")
                            .unwrap();
                    }
                    "action" => {
                        e.execute(
                            None,
                            None,
                            "/revoke actor @account:target x:account.password.reset",
                        )
                        .unwrap();
                    }
                    "command" => {
                        e.execute(None, None, "/revoke actor @account:target /reset")
                            .unwrap();
                    }
                    "target-su" => {
                        e.execute(None, None, "/grant target su").unwrap();
                    }
                    "target-disabled-su" => {
                        e.execute(None, None, "/grant target su").unwrap();
                        e.execute(None, None, "/disable target").unwrap();
                    }
                    "target-replaced" | "sudo-target-replaced" => {
                        e.execute(None, None, "/rm user target").unwrap();
                        e.provision("target", "replacement-hash".into(), false, false)
                            .unwrap();
                        // Regrant on the replacement to ensure rejection checks identity as well as rights.
                        e.execute(
                            None,
                            None,
                            "/grant actor @account:target x:account.password.reset",
                        )
                        .unwrap();
                        e.execute(None, None, "/grant actor @account:target /reset")
                            .unwrap();
                    }
                    "session" | "sudo-session" => {
                        e.provision("actor", "rotated-hash".into(), false, true)
                            .unwrap();
                    }
                    "disabled" | "sudo-disabled" => {
                        e.execute(None, None, "/disable actor").unwrap();
                    }
                    "creation" => {
                        e.execute(None, None, "/revoke actor admin").unwrap();
                    }
                    _ => (),
                }
                e.data.clone()
            };
            release.send(()).unwrap();
            blocker.await.unwrap();
            let error = job.await.unwrap().unwrap_err();
            let expected_error = match case {
                "action" | "sudo-action" => "Permission required",
                "command" | "creation" | "sudo-command" => "Command not permitted",
                "target-su" | "target-disabled-su" => "Only su",
                "target-replaced" | "sudo-target-replaced" => "Account changed",
                "session" | "disabled" | "sudo-session" | "sudo-disabled" => "Please log in",
                _ => "",
            };
            assert!(error.contains(expected_error), "{case}: {error}");
            assert!(
                app.engine().unwrap().data == expected,
                "stale password job committed: {case}"
            );
        }
    });
}

#[test]
fn engine_readers_overlap_and_io_admission_is_bounded() {
    let (changes, _) = broadcast::channel(1);
    let app = App {
        media: None,
        engine: Arc::new(RwLock::new(
            Engine::open(std::path::Path::new(":memory:")).unwrap(),
        )),
        attempts: Default::default(),
        changes,
        config: Arc::new(Config::default()),
        connections: Arc::new(tokio::sync::Semaphore::new(128)),
        io_jobs: Arc::new(tokio::sync::Semaphore::new(64)),
        password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
        snapshots: Default::default(),
        bot_runtimes: Default::default(),
        stopping: Default::default(),
    };
    let reader = app.engine_read().unwrap();
    let other = app.clone();
    std::thread::spawn(move || {
        let second_reader = other.engine_read().unwrap();
        assert_eq!(second_reader.revision, 0);
        assert!(other.engine.try_write().is_err());
    })
    .join()
    .unwrap();
    drop(reader);
    assert!(app.engine.try_write().is_ok());
    let permits: Vec<_> = (0..64).map(|_| app.io_permit().unwrap()).collect();
    assert!(app.io_permit().is_err());
    drop(permits);
    assert!(app.io_permit().is_ok());
}

#[test]
fn configs_with_cache_statistics_remain_one_json_document() {
    let config = Config::default();
    let stats = json!({"hits":42,"messages":100});
    let result: serde_json::Value =
        serde_json::from_str(&config.display_with_history(stats.clone()).unwrap()).unwrap();
    assert_eq!(result["history_cache"], stats);
    assert_eq!(result["max_messages"], 1000);
    assert_eq!(result["bind"], config.bind);
}

#[test]
fn socket_baselines_share_concurrent_builds_and_recheck_credentials_and_mutations() {
    let mut engine = Engine::open(std::path::Path::new(":memory:")).unwrap();
    engine
        .provision("alice", "hash".into(), true, false)
        .unwrap();
    engine
        .execute(Some("alice"), None, "/new room support")
        .unwrap();
    engine
        .execute(Some("alice"), Some("support"), "initial")
        .unwrap();
    engine
        .provision("bob", "hash".into(), false, false)
        .unwrap();
    engine
        .execute(Some("alice"), Some("support"), "/add bob")
        .unwrap();
    engine.login("bob".into(), "bob", "hash", None).unwrap();
    engine.login("one".into(), "alice", "hash", None).unwrap();
    engine.login("two".into(), "alice", "hash", None).unwrap();
    let (changes, _) = broadcast::channel(1);
    let app = App {
        media: None,
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
    };
    app.update_presence(None, Some("alice")).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let workers: Vec<_> = (0..8)
        .map(|index| {
            let app = app.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut headers = HeaderMap::new();
                headers.insert(
                    header::COOKIE,
                    HeaderValue::from_static(if index % 2 == 0 {
                        "chat_session=one"
                    } else {
                        "chat_session=two"
                    }),
                );
                barrier.wait();
                snapshots::value(&app, &headers, false).unwrap()
            })
        })
        .collect();
    let values: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert!(
        values
            .iter()
            .all(|value| value.as_str().as_ptr() == values[0].as_str().as_ptr())
    );
    assert_eq!(
        app.engine_read().unwrap().history_store.stats().unwrap()["misses"],
        1
    );
    let mut headers = HeaderMap::new();
    headers.insert(header::COOKIE, HeaderValue::from_static("chat_session=two"));
    let mut events = app.changes.subscribe();
    app.update_presence(None, Some("alice")).unwrap();
    app.update_presence(Some("alice"), None).unwrap();
    assert!(matches!(
        events.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
    assert_eq!(
        snapshots::value(&app, &headers, false)
            .unwrap()
            .as_str()
            .as_ptr(),
        values[0].as_str().as_ptr()
    );
    // Read-only commands acquire no mutation epoch and preserve cached buffers.
    app.engine()
        .unwrap()
        .execute(Some("alice"), None, "/help")
        .unwrap();
    assert_eq!(
        snapshots::value(&app, &headers, false)
            .unwrap()
            .as_str()
            .as_ptr(),
        values[0].as_str().as_ptr()
    );
    assert!(
        app.engine_read()
            .unwrap()
            .read_command(Some("alice"), None, "/whoami")
            .unwrap()
            .is_ok()
    );
    // Simulate a token expiring while the user's cached baseline remains available.
    app.engine
        .write()
        .unwrap()
        .data
        .sessions
        .get_mut("one")
        .unwrap()
        .expires = 0;
    let mut expired = HeaderMap::new();
    expired.insert(header::COOKIE, HeaderValue::from_static("chat_session=one"));
    assert!(snapshots::value(&app, &expired, false).is_err());
    assert_eq!(
        snapshots::value(&app, &headers, false)
            .unwrap()
            .as_str()
            .as_ptr(),
        values[0].as_str().as_ptr()
    );
    app.engine()
        .unwrap()
        .execute(Some("alice"), Some("support"), "changed")
        .unwrap();
    let changed = snapshots::value(&app, &headers, false).unwrap();
    assert!(changed.contains("changed"));
    assert_ne!(changed.as_str().as_ptr(), values[0].as_str().as_ptr());
    let mut bob_headers = HeaderMap::new();
    bob_headers.insert(header::COOKIE, HeaderValue::from_static("chat_session=bob"));
    assert!(
        snapshots::value(&app, &bob_headers, false)
            .unwrap()
            .contains("changed")
    );
    let bob_before_read = snapshots::value(&app, &bob_headers, false).unwrap();
    let sequence = app.engine_read().unwrap().data.next_sequence;
    app.engine()
        .unwrap()
        .mark_read("alice", "support", sequence)
        .unwrap();
    let bob_after_read = snapshots::value(&app, &bob_headers, false).unwrap();
    assert_eq!(
        bob_before_read.as_str().as_ptr(),
        bob_after_read.as_str().as_ptr()
    );
    let alice_after_read = snapshots::value(&app, &headers, false).unwrap();
    assert_ne!(
        changed.as_str().as_ptr(),
        alice_after_read.as_str().as_ptr()
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/revoke bob user support")
        .unwrap();
    let revoked = snapshots::value(&app, &bob_headers, false).unwrap();
    let snapshot: serde_json::Value = serde_json::from_str(&revoked).unwrap();
    assert!(snapshot["rooms"].as_array().unwrap().is_empty());
    app.engine().unwrap().logout("two").unwrap();
    assert!(snapshots::value(&app, &headers, false).is_err());
}

#[tokio::test]
async fn stalled_media_io_does_not_occupy_api_jobs_or_serialize_other_files() {
    use axum::{body::Body, http::Request};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tower::ServiceExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let storage =
        crate::media::tests::test_storage(&format!("http://{}", listener.local_addr().unwrap()));
    let mut engine = Engine::open(std::path::Path::new(":memory:")).unwrap();
    engine
        .provision("alice", "hash".into(), true, false)
        .unwrap();
    engine
        .login("fixture".into(), "alice", "hash", None)
        .unwrap();
    engine
        .execute(Some("alice"), None, "/new room files")
        .unwrap();
    let room = engine.data.rooms["files"].id.clone();
    let first = engine.allocate_media("alice", &room, "first", 1).unwrap();
    let second = engine.allocate_media("alice", &room, "second", 1).unwrap();
    let (changes, _) = broadcast::channel(1);
    let app = App {
        media: Some(storage),
        engine: Arc::new(RwLock::new(engine)),
        attempts: Default::default(),
        changes,
        config: Arc::new(Config::default()),
        connections: Arc::new(tokio::sync::Semaphore::new(128)),
        io_jobs: Arc::new(tokio::sync::Semaphore::new(1)),
        password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
        snapshots: Default::default(),
        bot_runtimes: Default::default(),
        stopping: Default::default(),
    };
    let post = |id: &str| {
        Request::builder()
            .method("POST")
            .uri(format!("/api/media/uploads/{id}/complete"))
            .header(header::ORIGIN, "http://localhost:5173")
            .header(header::COOKIE, "chat_session=fixture")
            .body(Body::empty())
            .unwrap()
    };
    let router = router(app.clone());
    let one = tokio::spawn(router.clone().oneshot(post(&first.id)));
    let (mut first_socket, _) =
        tokio::time::timeout(std::time::Duration::from_secs(2), listener.accept())
            .await
            .unwrap()
            .unwrap();
    let mut buffer = [0; 4096];
    assert!(first_socket.read(&mut buffer).await.unwrap() > 0);
    assert_eq!(app.io_jobs.available_permits(), 1);
    let ordinary = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/me")
                .header(header::ORIGIN, "http://localhost:5173")
                .header(header::COOKIE, "chat_session=fixture")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ordinary.status(), StatusCode::OK);
    let two = tokio::spawn(router.clone().oneshot(post(&second.id)));
    let (mut second_socket, _) =
        tokio::time::timeout(std::time::Duration::from_secs(2), listener.accept())
            .await
            .unwrap()
            .unwrap();
    assert!(second_socket.read(&mut buffer).await.unwrap() > 0);
    let duplicate = router.oneshot(post(&first.id)).await.unwrap();
    assert_eq!(duplicate.status(), StatusCode::SERVICE_UNAVAILABLE);
    let failed = b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    first_socket.write_all(failed).await.unwrap();
    second_socket
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    assert_eq!(
        one.await.unwrap().unwrap().status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        two.await.unwrap().unwrap().status(),
        StatusCode::BAD_REQUEST
    );
}
