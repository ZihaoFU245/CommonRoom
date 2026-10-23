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
        admission: Default::default(),
        io_jobs: Arc::new(tokio::sync::Semaphore::new(64)),
        password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
        snapshots: Default::default(),
        bot_runtimes: Default::default(),
        stopping: Default::default(),
        shutdown: Default::default(),
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
    let ApiError(status, _) = auth::authenticate(&app, &headers).unwrap_err();
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
    let store = engine.history_store().clone();
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
        admission: Default::default(),
        io_jobs: Arc::new(tokio::sync::Semaphore::new(64)),
        password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
        snapshots: Default::default(),
        bot_runtimes: Default::default(),
        stopping: Default::default(),
        shutdown: Default::default(),
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
        admission: Default::default(),
        io_jobs: Arc::new(tokio::sync::Semaphore::new(64)),
        password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
        snapshots: Default::default(),
        bot_runtimes: Default::default(),
        stopping: Default::default(),
        shutdown: Default::default(),
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
    assert_eq!(
        security::client_ip(&config, proxy, &headers).unwrap(),
        client
    );
    assert_eq!(
        security::client_ip(&config, "::ffff:127.0.0.1".parse().unwrap(), &headers).unwrap(),
        client
    );
    let untrusted: std::net::IpAddr = "198.51.100.8".parse().unwrap();
    assert_eq!(
        security::client_ip(&config, untrusted, &headers).unwrap(),
        untrusted
    );
    for value in [
        "",
        "unknown",
        "198.51.100.7:1234",
        "198.51.100.7, 198.51.100.8",
    ] {
        headers.insert("x-forwarded-for", HeaderValue::from_str(value).unwrap());
        assert!(security::client_ip(&config, proxy, &headers).is_err());
    }
    headers.insert(
        "x-forwarded-for",
        HeaderValue::from_static("::ffff:198.51.100.7"),
    );
    assert_eq!(
        security::client_ip(&config, proxy, &headers).unwrap(),
        client
    );
    headers.insert("x-forwarded-for", HeaderValue::from_static("2001:db8::7"));
    assert_eq!(
        security::client_ip(&config, proxy, &headers).unwrap(),
        "2001:db8::7".parse::<std::net::IpAddr>().unwrap()
    );
    headers.append("x-forwarded-for", HeaderValue::from_static("198.51.100.8"));
    assert!(security::client_ip(&config, proxy, &headers).is_err());
    headers.clear();
    assert_eq!(
        security::client_ip(&config, proxy, &headers).unwrap(),
        proxy
    );
    headers.insert("x-real-ip", HeaderValue::from_static("198.51.100.7"));
    config.set_real_ip_from = Some("X-Real-IP".into());
    assert_eq!(
        security::client_ip(&config, proxy, &headers).unwrap(),
        client
    );
    config.set_real_ip_from = None;
    assert_eq!(
        security::client_ip(&config, proxy, &headers).unwrap(),
        proxy
    );
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
        .execute(Some("alice"), None, "/room new support")
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
        admission: Default::default(),
        io_jobs: Arc::new(tokio::sync::Semaphore::new(64)),
        password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
        snapshots: Default::default(),
        bot_runtimes: Default::default(),
        stopping: Default::default(),
        shutdown: Default::default(),
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
    engine
        .execute(None, None, "/user disable disabled")
        .unwrap();
    let app = App {
        media: None,
        engine: Arc::new(RwLock::new(engine)),
        attempts: Default::default(),
        changes,
        config: Arc::new(Config::default()),
        admission: Default::default(),
        io_jobs: Arc::new(tokio::sync::Semaphore::new(64)),
        password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
        snapshots: Default::default(),
        bot_runtimes: Default::default(),
        stopping: Default::default(),
        shutdown: Default::default(),
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
        admission: Default::default(),
        io_jobs: Arc::new(tokio::sync::Semaphore::new(64)),
        password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
        snapshots: Default::default(),
        bot_runtimes: Default::default(),
        stopping: Default::default(),
        shutdown: Default::default(),
    };
    async fn attempt(app: &App, peer: &str, forwarded: &str) -> StatusCode {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://localhost:5173"),
        );
        headers.insert("x-forwarded-for", HeaderValue::from_str(forwarded).unwrap());
        let input = serde_json::from_value(
            json!({"username":uuid::Uuid::new_v4().simple().to_string(),"password":"invalid"}),
        )
        .unwrap();
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
    engine.execute(None, None, "/user disable target").unwrap();
    engine.execute(None, None, "/room new team").unwrap();
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
        admission: Default::default(),
        io_jobs: Arc::new(tokio::sync::Semaphore::new(64)),
        password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
        snapshots: Default::default(),
        bot_runtimes: Default::default(),
        stopping: Default::default(),
        shutdown: Default::default(),
    };
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        HeaderValue::from_static("chat_session=fixture"),
    );
    for (text, changed, expected) in [
        ("/sudo /configs", false, None),
        (
            "/sudo /user passwd actor-old-password actor-new-password",
            true,
            Some("Password changed. Other sessions signed out."),
        ),
        (
            "/sudo /user reset target password long-sudo-password",
            true,
            Some("Account target password reset."),
        ),
        ("/sudo /user enable target", true, Some("Enabled target.")),
        (
            "/sudo /user new created long-created-password admin",
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
        text: "/sudo /room join #team".into(),
    };
    let (reply, mutation) = super::commands::execute_input(&app, &headers, "actor", &join)
        .await
        .unwrap();
    assert_eq!(reply, "Joined #team.");
    let actor_id = app
        .engine_read()
        .unwrap()
        .account_id("actor")
        .unwrap()
        .to_owned();
    assert!(matches!(mutation, Some(Change::Metadata(notice)) if notice.targets(&actor_id)));
    let snapshot = app.engine().unwrap().snapshot("actor").unwrap();
    let room = snapshot.rooms.iter().find(|r| r.name == "team").unwrap();
    assert!(room.members.contains("actor"));
    assert!(room.permissions.contains(&"w:message.create".into()));
    assert!(snapshot.groups.is_empty());
    let repeated = super::commands::Input {
        text: "/sudo /room join #team".into(),
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
        ("/room join #missing", Err("Room not found.".into())),
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
                .execute(None, None, "/grant actor @account:target /user")
                .unwrap();
            if case == "command" {
                engine.execute(None, None, "/revoke actor admin").unwrap();
            }
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
                admission: Default::default(),
                io_jobs: Arc::new(tokio::sync::Semaphore::new(64)),
                password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
                snapshots: Default::default(),
                bot_runtimes: Default::default(),
                stopping: Default::default(),
                shutdown: Default::default(),
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
                "/user new created long-fixture-password user"
            } else if case.starts_with("sudo-") {
                "/sudo /user reset target password long-fixture-password"
            } else {
                "/user reset target password long-fixture-password"
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
                        e.execute(None, None, "/revoke actor @account:target /user")
                            .unwrap();
                    }
                    "target-su" => {
                        e.execute(None, None, "/grant target su").unwrap();
                    }
                    "target-disabled-su" => {
                        e.execute(None, None, "/grant target su").unwrap();
                        e.execute(None, None, "/user disable target").unwrap();
                    }
                    "target-replaced" | "sudo-target-replaced" => {
                        e.execute(None, None, "/user delete target").unwrap();
                        e.provision("target", "replacement-hash".into(), false, false)
                            .unwrap();
                        // Regrant on the replacement to ensure rejection checks identity as well as rights.
                        e.execute(
                            None,
                            None,
                            "/grant actor @account:target x:account.password.reset",
                        )
                        .unwrap();
                        e.execute(None, None, "/grant actor @account:target /user")
                            .unwrap();
                    }
                    "session" | "sudo-session" => {
                        e.provision("actor", "rotated-hash".into(), false, true)
                            .unwrap();
                    }
                    "disabled" | "sudo-disabled" => {
                        e.execute(None, None, "/user disable actor").unwrap();
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
        admission: Default::default(),
        io_jobs: Arc::new(tokio::sync::Semaphore::new(64)),
        password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
        snapshots: Default::default(),
        bot_runtimes: Default::default(),
        stopping: Default::default(),
        shutdown: Default::default(),
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
fn baselines_survive_unrelated_history_and_sessions_but_refresh_visible_changes() {
    let mut engine = Engine::open(std::path::Path::new(":memory:")).unwrap();
    engine
        .provision("alice", "hash".into(), true, false)
        .unwrap();
    engine
        .provision("bob", "hash".into(), false, false)
        .unwrap();
    engine
        .execute(Some("alice"), None, "/room new visible")
        .unwrap();
    engine
        .execute(Some("alice"), Some("visible"), "/room add bob")
        .unwrap();
    engine.execute(None, None, "/room new hidden").unwrap();
    engine.login("bob".into(), "bob", "hash", None).unwrap();
    let mut app = login_test_app();
    app.engine = Arc::new(RwLock::new(engine));
    let mut headers = HeaderMap::new();
    headers.insert(header::COOKIE, HeaderValue::from_static("chat_session=bob"));
    let baseline = snapshots::value(&app, &headers, false).unwrap();
    {
        let mut engine = app.engine().unwrap();
        engine
            .execute(None, Some("hidden"), "hidden content")
            .unwrap();
        engine
            .login("temporary".into(), "alice", "hash", None)
            .unwrap();
        engine.logout("temporary").unwrap();
    }
    let unchanged = snapshots::value(&app, &headers, false).unwrap();
    assert_eq!(baseline.as_str().as_ptr(), unchanged.as_str().as_ptr());
    assert!(!unchanged.contains("hidden content"));
    app.engine()
        .unwrap()
        .execute(Some("alice"), Some("visible"), "visible content")
        .unwrap();
    let changed = snapshots::value(&app, &headers, false).unwrap();
    assert_ne!(baseline.as_str().as_ptr(), changed.as_str().as_ptr());
    assert!(changed.contains("visible content"));
    app.engine()
        .unwrap()
        .execute(None, None, "/revoke bob user #visible")
        .unwrap();
    let revoked = snapshots::value(&app, &headers, false).unwrap();
    assert!(!revoked.contains("visible content"));
    let parsed: serde_json::Value = serde_json::from_str(&revoked).unwrap();
    assert!(parsed["rooms"].as_array().unwrap().is_empty());
}

#[test]
fn socket_baselines_share_concurrent_builds_and_recheck_credentials_and_mutations() {
    let mut engine = Engine::open(std::path::Path::new(":memory:")).unwrap();
    engine
        .provision("alice", "hash".into(), true, false)
        .unwrap();
    engine
        .execute(Some("alice"), None, "/room new support")
        .unwrap();
    engine
        .execute(Some("alice"), Some("support"), "initial")
        .unwrap();
    engine
        .provision("bob", "hash".into(), false, false)
        .unwrap();
    engine
        .execute(Some("alice"), Some("support"), "/room add bob")
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
        admission: Default::default(),
        io_jobs: Arc::new(tokio::sync::Semaphore::new(64)),
        password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
        snapshots: Default::default(),
        bot_runtimes: Default::default(),
        stopping: Default::default(),
        shutdown: Default::default(),
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
        app.engine_read().unwrap().history_store().stats().unwrap()["misses"],
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
        .get_mut(&crate::engine::session_digest("one"))
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
        .execute(None, None, "/revoke bob user #support")
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
        .execute(Some("alice"), None, "/room new files")
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
        admission: Default::default(),
        io_jobs: Arc::new(tokio::sync::Semaphore::new(1)),
        password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
        snapshots: Default::default(),
        bot_runtimes: Default::default(),
        stopping: Default::default(),
        shutdown: Default::default(),
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

#[tokio::test]
#[allow(clippy::panic)] // Include the failing command in table-driven assertions.
async fn every_catalogued_command_form_runs_through_web_dispatch() {
    let mut engine = Engine::open(std::path::Path::new(":memory:")).unwrap();
    let hash = crate::engine::hash_password("actor-old-password").unwrap();
    engine
        .provision("actor", hash.clone(), true, false)
        .unwrap();
    engine.execute(None, None, "/grant actor su").unwrap();
    for name in ["bob", "ls", "get"] {
        engine.provision(name, "hash".into(), false, false).unwrap();
    }
    engine
        .login("fixture".into(), "actor", &hash, None)
        .unwrap();
    let (changes, _) = broadcast::channel(1);
    let app = App {
        media: None,
        engine: Arc::new(RwLock::new(engine)),
        attempts: Default::default(),
        changes,
        config: Arc::new(Config::default()),
        admission: Default::default(),
        io_jobs: Arc::new(tokio::sync::Semaphore::new(64)),
        password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
        snapshots: Default::default(),
        bot_runtimes: Default::default(),
        stopping: Default::default(),
        shutdown: Default::default(),
    };
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        HeaderValue::from_static("chat_session=fixture"),
    );
    let probes = [
        (None, "/room new #team"),
        (None, "/room ls"),
        (None, "/room add bob #team"),
        (Some("team"), "/room add ls"),
        (Some("team"), "/room add get"),
        (Some("team"), "/room join #team"),
        (Some("team"), "/user ls #team"),
        (Some("team"), "/user set @all label member"),
        (Some("team"), "/user get @all"),
        (Some("team"), "/grant bob #team w:message.react"),
        (Some("team"), "/revoke bob #team w:message.react"),
        (Some("team"), "/grant bob user #team"),
        (Some("team"), "/revoke bob user #team"),
        (Some("team"), "/room add bob"),
        (Some("team"), "/room kick ls"),
        (Some("team"), "/room owner bob"),
        (Some("team"), "/room leave"),
        (None, "/room join #team"),
        (Some("team"), "/help"),
        (Some("team"), "/man room"),
        (Some("team"), "/permissions"),
        (Some("team"), "/debug on"),
        (Some("team"), "/whoami"),
        (Some("team"), "/console"),
        (Some("team"), "/tell bob private-message"),
        (Some("team"), "original-message"),
        (Some("team"), "/reply $message reply-message"),
        (Some("team"), "/react $message helpful"),
        (Some("team"), "/edit $message edited-message"),
        (Some("team"), "/history"),
        (Some("team"), "/history #team"),
        (Some("team"), "/retract $message"),
        (Some("team"), "/clean 7d #team --dry-run"),
        (Some("team"), "/clean 7d #team"),
        (None, "/configs"),
        (None, "/sudo /whoami"),
        // Clear/logout are browser-local; their elevated forms authorize here.
        (None, "/sudo /clear"),
        (None, "/sudo /logout"),
        (Some("team"), "/bot new helper"),
        (Some("team"), "/bot ls"),
        (Some("team"), "/bot info helper"),
        (None, "/bot owner helper bob"),
        (None, "/bot token new helper worker"),
        (None, "/bot token ls helper"),
        (None, "/bot token rm helper $token"),
        (None, "/bot rm helper @global"),
        (None, "/user new created long-created-password user"),
        (None, "/user reset created password long-reset-password"),
        (None, "/user disable created"),
        (None, "/user enable created"),
        (None, "/user rename created renamed"),
        (None, "/user delete renamed"),
        (None, "/user passwd actor-old-password actor-new-password"),
        (None, "/room rename #team renamed"),
        (None, "/room delete #renamed"),
    ];
    let command_path = |text: &str| {
        let parts = text.split_whitespace().collect::<Vec<_>>();
        let count = match parts.as_slice() {
            ["/bot", "token", ..] => 3,
            ["/user" | "/room" | "/bot", ..] => 2,
            _ => 1,
        };
        parts.into_iter().take(count).collect::<Vec<_>>().join(" ")
    };
    let mut covered = std::collections::BTreeSet::new();
    let mut message = String::new();
    let mut token_id = String::new();
    for (room, source) in probes {
        let text = source
            .replace("$message", &message)
            .replace("$token", &token_id);
        let input = super::commands::Input {
            id: 1,
            room: room.map(str::to_owned),
            text: text.clone(),
            markdown: false,
        };
        let (reply, change) = super::commands::execute_input(&app, &headers, "actor", &input)
            .await
            .unwrap_or_else(|error| panic!("{text}: {error}"));
        if source.ends_with("--dry-run") {
            assert!(change.is_none());
            assert!(reply.starts_with("## Cleanup preview"));
        }
        covered.insert(command_path(source));
        if source == "/sudo /clear" {
            covered.insert("/clear".into());
        }
        if source == "/sudo /logout" {
            covered.insert("/logout".into());
        }
        if source == "original-message" {
            message = app
                .engine_read()
                .unwrap()
                .history("actor", "team")
                .unwrap()
                .messages
                .last()
                .unwrap()
                .id
                .clone();
        }
        if source.starts_with("/bot token new") {
            token_id = reply.split_whitespace().nth(2).unwrap().to_owned();
        }
        if source.starts_with("/room add") {
            let name = source.split_whitespace().nth(2).unwrap();
            assert!(
                app.engine_read().unwrap().data.rooms["team"]
                    .members
                    .contains(name),
                "{source}"
            );
        }
    }
    for command in crate::commands::available(true, false) {
        for form in crate::commands::usage_forms(command) {
            assert!(
                covered.contains(&command_path(&form)),
                "Missing success probe for {form}"
            );
        }
    }
    let engine = app.engine_read().unwrap();
    assert!(!engine.data.rooms.contains_key("renamed"));
    assert!(!engine.data.users.contains_key("renamed"));
    assert!(crate::engine::verify_password(
        "actor-new-password",
        &engine.data.users["actor"].hash
    ));
}

#[tokio::test]
async fn unauthenticated_uploads_do_not_reveal_storage_configuration_or_hint_validation() {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    let (changes, _) = broadcast::channel(1);
    let mut app = App {
        media: None,
        engine: Arc::new(RwLock::new(
            Engine::open(std::path::Path::new(":memory:")).unwrap(),
        )),
        attempts: Default::default(),
        changes,
        config: Arc::new(Config::default()),
        admission: Default::default(),
        io_jobs: Arc::new(tokio::sync::Semaphore::new(64)),
        password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
        snapshots: Default::default(),
        bot_runtimes: Default::default(),
        stopping: Default::default(),
        shutdown: Default::default(),
    };
    for enabled in [false, true] {
        if enabled {
            app.media = Some(crate::media::tests::test_storage("http://127.0.0.1:1"));
        }
        let request = Request::builder().method("POST").uri("/api/media/uploads")
            .header(header::ORIGIN, "http://localhost:5173")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"conversation_id":"missing","filename":"file","size":8,"hints":{"unknown":"bad"}}"#)).unwrap();
        let response = router(app.clone()).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let request = Request::builder()
            .method("POST")
            .uri("/api/avatars/uploads")
            .header(header::ORIGIN, "http://localhost:5173")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"size":8}"#))
            .unwrap();
        let response = router(app.clone()).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn avatar_api_is_self_service_authenticated_origin_checked_and_broadcast() {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    let mut engine = Engine::open(std::path::Path::new(":memory:")).unwrap();
    for name in ["alice", "bob"] {
        engine.provision(name, "hash".into(), false, false).unwrap();
        engine.login(name.into(), name, "hash", None).unwrap();
    }
    let (changes, mut notifications) = broadcast::channel(8);
    engine.set_avatar_hosts(["example.com".into()].into());
    let app = App {
        media: None,
        engine: Arc::new(RwLock::new(engine)),
        attempts: Default::default(),
        changes,
        config: Arc::new(Config::default()),
        admission: Default::default(),
        io_jobs: Arc::new(tokio::sync::Semaphore::new(64)),
        password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
        snapshots: Default::default(),
        bot_runtimes: Default::default(),
        stopping: Default::default(),
        shutdown: Default::default(),
    };
    let routes = router(app.clone());
    for (cookie, origin, body, status) in [
        (
            "",
            "http://localhost:5173",
            r#"{"url":"https://example.com/a.png"}"#,
            StatusCode::UNAUTHORIZED,
        ),
        (
            "alice",
            "https://evil.example",
            r#"{"url":"https://example.com/a.png"}"#,
            StatusCode::FORBIDDEN,
        ),
        (
            "alice",
            "http://localhost:5173",
            r#"{"url":"javascript:alert(1)"}"#,
            StatusCode::BAD_REQUEST,
        ),
        (
            "alice",
            "http://localhost:5173",
            r#"{"url":"https://example.com/a.png","user":"bob"}"#,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "alice",
            "http://localhost:5173",
            r#"{"url":"https://example.com/a.png"}"#,
            StatusCode::OK,
        ),
    ] {
        let response = routes
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/avatars")
                    .header(header::ORIGIN, origin)
                    .header(header::COOKIE, format!("chat_session={cookie}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
    assert!(matches!(notifications.try_recv(), Ok(Change::Avatar(_))));
    let revision = app.engine_read().unwrap().revision;
    let repeated = routes
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/avatars")
                .header(header::ORIGIN, "http://localhost:5173")
                .header(header::COOKIE, "chat_session=alice")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"url":"https://example.com/a.png"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(repeated.status(), StatusCode::OK);
    assert_eq!(app.engine_read().unwrap().revision, revision);
    assert!(matches!(
        notifications.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
    assert_eq!(
        app.engine_read().unwrap().avatar_directory()["alice"],
        "https://example.com/a.png"
    );
    assert!(
        !app.engine_read()
            .unwrap()
            .avatar_directory()
            .contains_key("bob")
    );
    let response = routes
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/avatars")
                .header(header::ORIGIN, "http://localhost:5173")
                .header(header::COOKIE, "chat_session=alice")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"url":null}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(app.engine_read().unwrap().avatar_directory().is_empty());
    let media = {
        let mut engine = app.engine_api().unwrap();
        let mut media = engine.allocate_avatar("alice", 64).unwrap();
        media.ready = true;
        media.content_type = "image/png".into();
        media.object_key = media.final_key();
        engine.finish_avatar("alice", &media).unwrap();
        media
    };
    let routes = router(app.clone());
    let info_path = format!("/api/avatars/{}/info", media.id);
    for (cookie, origin, status) in [
        ("", "http://localhost:5173", StatusCode::UNAUTHORIZED),
        ("bob", "https://evil.example", StatusCode::FORBIDDEN),
        ("bob", "http://localhost:5173", StatusCode::OK),
    ] {
        let response = routes
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&info_path)
                    .header(header::ORIGIN, origin)
                    .header(header::COOKIE, format!("chat_session={cookie}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
    app.engine_api()
        .unwrap()
        .execute(None, None, "/revoke bob user @global")
        .unwrap();
    let response = routes
        .oneshot(
            Request::builder()
                .uri(&info_path)
                .header(header::ORIGIN, "http://localhost:5173")
                .header(header::COOKIE, "chat_session=bob")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

pub(super) fn login_test_app() -> App {
    let (changes, _) = broadcast::channel(16);
    let mut engine = Engine::open(std::path::Path::new(":memory:")).unwrap();
    engine
        .provision(
            "alice",
            crate::engine::hash_password("correct-password").unwrap(),
            false,
            false,
        )
        .unwrap();
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
        bot_runtimes: Default::default(),
        stopping: Default::default(),
        shutdown: Default::default(),
    }
}
async fn routed_login(app: &App, password: &str, forwarded: &str) -> axum::response::Response {
    use axum::{body::Body, extract::ConnectInfo, http::Request};
    use tower::ServiceExt;
    router(app.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/login")
                .extension(ConnectInfo("127.0.0.1:1234".parse::<SocketAddr>().unwrap()))
                .header(header::ORIGIN, "http://localhost:5173")
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-forwarded-for", forwarded)
                .body(Body::from(
                    json!({"username":"alice","password":password}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
}
#[tokio::test]
async fn repeated_successful_logins_do_not_spend_failure_allowance_or_need_general_workers() {
    let app = login_test_app();
    let _general = app.io_jobs.clone().acquire_many_owned(64).await.unwrap();
    for _ in 0..12 {
        let response = routed_login(&app, "correct-password", "198.51.100.1").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().contains_key(header::SET_COOKIE));
    }
    assert_eq!(
        routed_login(&app, "wrong", "198.51.100.1").await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        routed_login(&app, "correct-password", "198.51.100.1")
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
}
#[tokio::test]
async fn login_account_budget_survives_rotating_client_addresses() {
    let app = login_test_app();
    assert_eq!(
        routed_login(&app, "wrong", "198.51.100.1").await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        routed_login(&app, "wrong", "198.51.100.99").await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
}
#[tokio::test]
async fn busy_password_workers_and_malformed_forwarding_do_not_spend_login_allowance() {
    let app = login_test_app();
    let jobs = app
        .password_jobs
        .clone()
        .acquire_many_owned(2)
        .await
        .unwrap();
    for _ in 0..12 {
        assert_eq!(
            routed_login(&app, "wrong", "198.51.100.1").await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    drop(jobs);
    for _ in 0..12 {
        assert_eq!(
            routed_login(&app, "wrong", "198.51.100.1, 10.0.0.1")
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        routed_login(&app, "correct-password", "198.51.100.1")
            .await
            .status(),
        StatusCode::OK
    );
}
#[tokio::test]
async fn production_ingress_rejections_always_have_security_headers() {
    use axum::{body::Body, extract::ConnectInfo, http::Request};
    use tower::ServiceExt;
    let mut app = login_test_app();
    app.config = Arc::new(Config {
        production: true,
        origins: vec!["https://chat.example.com".into()],
        ..Config::default()
    });
    for (peer, proto, status) in [
        ("127.0.0.2:1234", "https", StatusCode::FORBIDDEN),
        ("127.0.0.1:1234", "http", StatusCode::BAD_REQUEST),
        ("127.0.0.1:1234", "https", StatusCode::OK),
    ] {
        let response = router(app.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/health")
                    .extension(ConnectInfo(peer.parse::<SocketAddr>().unwrap()))
                    .header("x-forwarded-proto", proto)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(
            response.headers()[header::X_CONTENT_TYPE_OPTIONS],
            "nosniff"
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
        assert_eq!(
            response.headers()[header::STRICT_TRANSPORT_SECURITY],
            "max-age=31536000"
        );
        assert!(
            response
                .headers()
                .contains_key(header::CONTENT_SECURITY_POLICY)
        );
    }
}

#[tokio::test]
async fn http_account_admission_preserves_other_accounts_and_releases_capacity() {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    let app = login_test_app();
    let alice = app.engine_read().unwrap().data.users["alice"].clone();
    {
        let mut engine = app.engine().unwrap();
        engine
            .login("alice-session".into(), "alice", &alice.hash, None)
            .unwrap();
        engine
            .provision("bob", "hash".into(), false, false)
            .unwrap();
        engine
            .login("bob-session".into(), "bob", "hash", None)
            .unwrap();
    }
    let mut leases: Vec<_> = (0..8)
        .map(|_| app.admission.http(alice.id.clone()).unwrap())
        .collect();
    for (token, status) in [
        ("alice-session", StatusCode::TOO_MANY_REQUESTS),
        ("bob-session", StatusCode::OK),
    ] {
        let response = router(app.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/session")
                    .header(header::COOKIE, format!("chat_session={token}"))
                    .header(header::ORIGIN, "http://localhost:5173")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
    leases.pop();
    let response = router(app.clone())
        .oneshot(
            Request::builder()
                .uri("/api/session")
                .header(header::COOKIE, "chat_session=alice-session")
                .header(header::ORIGIN, "http://localhost:5173")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn media_and_avatar_storage_failures_are_retryable_instead_of_invalid_requests() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    let mut engine = Engine::open(&path).unwrap();
    engine
        .provision("alice", "hash".into(), false, false)
        .unwrap();
    engine
        .login("fixture".into(), "alice", "hash", None)
        .unwrap();
    let mut app = login_test_app();
    app.engine = Arc::new(RwLock::new(engine));
    app.media = Some(crate::media::tests::test_storage("http://127.0.0.1:1"));
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER reject_upload BEFORE INSERT ON media BEGIN SELECT RAISE(FAIL,'injected database failure'); END;").unwrap();
    for (method, path, body, status, code) in [
        (
            "POST",
            "/api/avatars/uploads",
            r#"{"size":8}"#,
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
        ),
        (
            "POST",
            "/api/avatars/uploads",
            r#"{"size":0}"#,
            StatusCode::BAD_REQUEST,
            "invalid_request",
        ),
    ] {
        let response = router(app.clone())
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::COOKIE, "chat_session=fixture")
                    .header(header::ORIGIN, "http://localhost:5173")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        let value: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(value["code"], code);
        assert!(!value["error"].as_str().unwrap().contains("injected"));
    }
    db.execute_batch("DROP TABLE media;").unwrap();
    for path in ["/api/media/missing/info", "/api/avatars/missing/info"] {
        let response = router(app.clone())
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header(header::COOKIE, "chat_session=fixture")
                    .header(header::ORIGIN, "http://localhost:5173")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let value: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(value["code"], "unavailable");
        assert_eq!(value["retryable"], true);
    }
}

#[tokio::test]
async fn real_login_logout_and_failure_write_only_bounded_auth_event_fields() {
    use axum::{body::Body, extract::ConnectInfo, http::Request};
    use tower::ServiceExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    let mut engine = Engine::open(&path).unwrap();
    engine
        .provision(
            "alice",
            crate::engine::hash_password("correct-password").unwrap(),
            false,
            false,
        )
        .unwrap();
    let mut app = login_test_app();
    app.engine = Arc::new(RwLock::new(engine));
    let response = routed_login(&app, "correct-password", "198.51.100.1").await;
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let mut changes = app.changes.subscribe();
    let response = router(app.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/logout")
                .extension(ConnectInfo("127.0.0.1:1234".parse::<SocketAddr>().unwrap()))
                .header(header::COOKIE, &cookie)
                .header("x-forwarded-for", "198.51.100.1")
                .header(header::ORIGIN, "http://localhost:5173")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(matches!(changes.try_recv(),Ok(Change::Account(name)) if name=="alice"));
    assert_eq!(
        routed_login(&app, "wrong-password", "198.51.100.1")
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let db = rusqlite::Connection::open(&path).unwrap();
    let rows = db
        .prepare("SELECT username,client,outcome FROM auth_events ORDER BY sequence")
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    assert_eq!(
        rows.iter().map(|row| row.2.as_str()).collect::<Vec<_>>(),
        ["success", "logout", "failure"]
    );
    assert!(
        rows.iter()
            .all(|row| row.0 == "alice" && row.1 == "198.51.100.1")
    );
    let mut changes = app.changes.subscribe();
    let response = router(app.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/logout")
                .header(header::ORIGIN, "http://localhost:5173")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(matches!(
        changes.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
    assert_eq!(
        db.query_row::<usize, _, _>("SELECT count(*) FROM auth_events", [], |row| row.get(0))
            .unwrap(),
        3
    );
}

#[tokio::test]
async fn avatar_http_budget_preserves_noops_other_accounts_and_committed_profile() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    let app = login_test_app();
    {
        let mut engine = app.engine_api().unwrap();
        engine.set_avatar_hosts(["example.com".into()].into());
        for name in ["alice", "bob"] {
            engine
                .provision(name, "hash".into(), false, name == "alice")
                .unwrap();
            engine.login(name.into(), name, "hash", None).unwrap();
        }
    }
    let routes = router(app.clone());
    for (user, index, expected) in [
        ("alice", 0, 200),
        ("alice", 1, 200),
        ("alice", 2, 200),
        ("alice", 3, 200),
        ("alice", 4, 429),
        ("alice", 3, 200),
        ("bob", 0, 200),
    ] {
        let response = routes
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/avatars")
                    .header(header::ORIGIN, "http://localhost:5173")
                    .header(header::COOKIE, format!("chat_session={user}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({"url":format!("https://example.com/{index}.png")}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), expected);
        if expected == 429 {
            let value: serde_json::Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 8192).await.unwrap())
                    .unwrap();
            assert_eq!(value["code"], "rate_limited");
            assert_eq!(value["retryable"], true);
        }
    }
    assert_eq!(
        app.engine_read().unwrap().avatar_directory()["alice"],
        "https://example.com/3.png"
    );
}
