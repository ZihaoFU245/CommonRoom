use super::*;
use super::{
    auth::cookie,
    security::{check_origin, trusted_proxy},
};
use axum::http::{HeaderMap, HeaderValue, header};

/// App fixture with no provider traffic: agent calls need a live network.
fn fixture(config: Config) -> App {
    let (changes, _) = broadcast::channel(1);
    App {
        engine: Arc::new(Mutex::new(
            Engine::open(std::path::Path::new(":memory:")).unwrap(),
        )),
        attempts: Default::default(),
        changes,
        config: Arc::new(config),
        connections: Arc::new(tokio::sync::Semaphore::new(128)),
        password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
        agent_jobs: Arc::new(tokio::sync::Semaphore::new(1)),
        http: reqwest::Client::new(),
        stopping: Default::default(),
    }
}

#[test]
#[allow(clippy::panic)] // Deliberately simulate a panic during a state mutation.
fn poisoned_engine_refuses_access_instead_of_reusing_partial_state() {
    let app = fixture(Config::default());
    let engine = app.engine.clone();
    assert!(
        std::thread::spawn(move || {
            let _guard = engine.lock().unwrap();
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
#[test]
fn cookies_are_http_only_and_secure_in_production() {
    let config = Config {
        bind: String::new(),
        origins: vec![],
        production: true,
        ..Config::default()
    };
    let app = fixture(config);
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
async fn login_throttling_separates_verified_clients_and_ignores_spoofing() {
    let (changes, _) = broadcast::channel(1);
    let app = App {
        engine: Arc::new(Mutex::new(
            Engine::open(std::path::Path::new(":memory:")).unwrap(),
        )),
        attempts: Default::default(),
        changes,
        config: Arc::new(Config::default()),
        connections: Arc::new(tokio::sync::Semaphore::new(128)),
        password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
        agent_jobs: Arc::new(tokio::sync::Semaphore::new(1)),
        http: reqwest::Client::new(),
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
    engine
        .provision("actor", "hash".into(), false, false)
        .unwrap();
    engine
        .provision("target", "hash".into(), false, false)
        .unwrap();
    engine.execute(None, None, "/grant target su").unwrap();
    engine.execute(None, None, "/disable target").unwrap();
    for permission in ["/sudo", "x:command.sudo"] {
        engine
            .execute(None, None, &format!("/grant actor @global {permission}"))
            .unwrap();
    }
    engine
        .login("fixture".into(), "actor", "hash", None)
        .unwrap();
    let (changes, _) = broadcast::channel(1);
    let app = App {
        engine: Arc::new(Mutex::new(engine)),
        attempts: Default::default(),
        changes,
        config: Arc::new(Config::default()),
        connections: Arc::new(tokio::sync::Semaphore::new(128)),
        password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
        agent_jobs: Arc::new(tokio::sync::Semaphore::new(1)),
        http: reqwest::Client::new(),
        stopping: Default::default(),
    };
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        HeaderValue::from_static("chat_session=fixture"),
    );
    for (text, changed) in [
        ("/sudo /configs", false),
        ("/sudo /reset target long-sudo-password", true),
        ("/sudo /enable target", true),
        ("/sudo /user created long-created-password admin", true),
        ("/sudo /clear", false),
        ("/sudo /logout", false),
    ] {
        let input = super::commands::Input {
            id: 1,
            room: None,
            text: text.into(),
        };
        let outcome = super::commands::run_input(&app, &headers, "actor", &input)
            .await
            .unwrap();
        assert_eq!(outcome.changed_state(), changed, "{text}");
        assert!(!app.engine().unwrap().is_su(Some("actor")));
    }
    let e = app.engine().unwrap();
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
    assert_eq!(e.groups(Some("actor")), vec!["user"]);
}

#[test]
fn password_jobs_recheck_authority_session_and_target_after_hashing() {
    // Queue hashing behind a blocking worker, rather than relying on timing sleeps.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .max_blocking_threads(1)
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
                engine: Arc::new(Mutex::new(engine)),
                attempts: Default::default(),
                changes,
                config: Arc::new(Config::default()),
                connections: Arc::new(tokio::sync::Semaphore::new(128)),
                password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
                agent_jobs: Arc::new(tokio::sync::Semaphore::new(1)),
                http: reqwest::Client::new(),
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
                "/user created long-fixture-password user"
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
                    id: 1,
                    room: None,
                    text: text.into(),
                };
                super::commands::run_input(&job_app, &headers, "actor", &input)
                    .await
                    .map(|outcome| outcome.reply())
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
                        e.execute(None, None, "/deleteuser target").unwrap();
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
