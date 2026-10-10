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
        engine: Arc::new(Mutex::new(
            Engine::open(std::path::Path::new(":memory:")).unwrap(),
        )),
        attempts: Default::default(),
        changes,
        config: Arc::new(Config::default()),
        connections: Arc::new(tokio::sync::Semaphore::new(128)),
        password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
        stopping: Default::default(),
    };
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
    let (changes, _) = broadcast::channel(1);
    let app = App {
        engine: Arc::new(Mutex::new(
            Engine::open(std::path::Path::new(":memory:")).unwrap(),
        )),
        attempts: Default::default(),
        changes,
        config: Arc::new(config),
        connections: Arc::new(tokio::sync::Semaphore::new(128)),
        password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
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
