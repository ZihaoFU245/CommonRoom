use super::*;
use super::{
    auth::cookie,
    security::{check_origin, trusted_proxy},
};
use axum::http::{HeaderMap, HeaderValue, header};
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
