use super::security::{check_origin, check_read_origin, client_ip};
use super::*;
use crate::engine::{EngineError, valid_name, verify_password};
use axum::{
    extract::{ConnectInfo, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;

#[derive(Clone)]
pub(crate) struct Identity {
    pub(crate) headers: HeaderMap,
    pub(crate) user: String,
    pub(crate) account: String,
    pub(crate) config: Arc<crate::config::Config>,
}
impl Identity {
    pub(crate) fn verify(&self, engine: &Engine) -> Result<(), EngineError> {
        if identity(engine, &self.config, &self.headers).as_deref() != Some(&self.user)
            || engine.active_account_id(&self.user) != Some(self.account.as_str())
        {
            return Err("Please log in.".into());
        }
        Ok(())
    }
}

// Valid PHC encoding with the default Argon2 work parameters. Its verification
// result is discarded: missing and disabled accounts must never authenticate.
const DUMMY_PASSWORD_HASH: &str =
    "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHQ$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

fn verify_login_password(password: &str, expected: Option<&str>) -> bool {
    let valid = verify_password(password, expected.unwrap_or(DUMMY_PASSWORD_HASH));
    expected.is_some() && valid
}

fn cookie_name(config: &Config) -> &'static str {
    if config.production && config.base_url == "/" {
        "__Host-chat_session"
    } else {
        "chat_session"
    }
}
pub(crate) fn token<'a>(config: &Config, headers: &'a HeaderMap) -> Option<&'a str> {
    let expected = cookie_name(config);
    let mut token = None;
    for header in headers.get_all(header::COOKIE) {
        for part in header.as_bytes().split(|byte| *byte == b';') {
            let part = part.trim_ascii();
            let Some(index) = part.iter().position(|byte| *byte == b'=') else {
                continue;
            };
            if &part[..index] == expected.as_bytes() {
                let value = &part[index + 1..];
                if token.is_some() || value.is_empty() || !value.iter().all(u8::is_ascii_graphic) {
                    return None;
                }
                token = Some(std::str::from_utf8(value).ok()?);
            }
        }
    }
    token
}
#[cfg(test)]
pub(crate) fn authenticate(app: &App, headers: &HeaderMap) -> Result<String, ApiError> {
    if app.stopping.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Server is shutting down.",
        ));
    }
    let engine = app.engine_read_api()?;
    identity(&engine, &app.config, headers)
        .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Invalid or expired credential."))
}
pub(crate) async fn authenticated_identity(
    app: &App,
    headers: &HeaderMap,
) -> Result<Identity, ApiError> {
    if app.stopping.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Server is shutting down.",
        ));
    }
    let (identity, store) = app
        .inspect_engine({
            let headers = headers.clone();
            let config = app.config.clone();
            move |engine, _| -> Result<_, ApiError> {
                let user = identity(engine, &config, &headers).ok_or_else(|| {
                    error(StatusCode::UNAUTHORIZED, "Invalid or expired credential.")
                })?;
                let account = engine
                    .active_account_id(&user)
                    .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Account unavailable."))?
                    .to_owned();
                Ok((
                    Identity {
                        headers,
                        user,
                        account,
                        config,
                    },
                    engine.auth_store(),
                ))
            }
        })
        .await?;
    store.health().await?;
    app.inspect_engine({
        let identity = identity.clone();
        move |engine, _| {
            identity
                .verify(engine)
                .map_err(|_| error(StatusCode::UNAUTHORIZED, "Invalid or expired credential."))
        }
    })
    .await?;
    Ok(identity)
}
pub(crate) fn bearer(headers: &HeaderMap) -> Option<&str> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    value.strip_prefix("Bearer ").filter(|s| !s.is_empty())
}
pub(crate) fn identity(engine: &Engine, config: &Config, headers: &HeaderMap) -> Option<String> {
    if headers.contains_key(header::AUTHORIZATION) {
        None // Machine credentials are accepted only by explicit Bot API handlers.
    } else {
        engine.session(token(config, headers)?)
    }
}
#[derive(Deserialize)]
pub(crate) struct Login {
    username: String,
    password: String,
}
pub(crate) async fn login(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(input): Json<Login>,
) -> Result<Response, ApiError> {
    check_origin(&app, &headers)?;
    if !valid_name(&input.username) || input.password.len() > crate::limits::MAX_PASSWORD_BYTES {
        return Err(error(
            StatusCode::UNAUTHORIZED,
            "Invalid username or password.",
        ));
    }
    let ip = client_ip(&app.config, peer.ip(), &headers)
        .map_err(|message| error(StatusCode::BAD_REQUEST, message))?;
    // Admit password work before reserving attempts. Busy workers must not spend
    // the caller's allowance, and login never occupies the general HTTP pool.
    let permit = app.password_jobs.clone().try_acquire_owned().map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Login busy. Please try again shortly.",
        )
    })?;
    let reservation = app
        .attempts
        .lock()
        .map_err(|_| login_state_unavailable())?
        .admit(ip, &input.username, std::time::Instant::now())
        .map_err(|message| error(StatusCode::TOO_MANY_REQUESTS, message))?;
    tokio::spawn(async move {
        let _permit = permit;
        let result = login_inner(&app, &headers, &input, ip).await;
        // Pending attempts count until completion, so concurrent requests cannot
        // overshoot a limit. Cancelled HTTP futures still finish this bookkeeping.
        let outcome = match &result {
            Ok(_) => Some(true),
            Err(ApiError(StatusCode::UNAUTHORIZED, _)) => Some(false),
            // Infrastructure/conflict errors release this reservation without
            // clearing failures accumulated by other login attempts.
            Err(_) => None,
        };
        app.attempts
            .lock()
            .map_err(|_| login_state_unavailable())?
            .finish(reservation, outcome, std::time::Instant::now());
        if outcome == Some(false) {
            tracing::warn!(client = %ip, "Login rejected");
        }
        result
    })
    .await
    .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "Login worker unavailable."))?
}
fn login_state_unavailable() -> ApiError {
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        "Login state unavailable; restart the service.",
    )
}
async fn login_inner(
    app: &App,
    headers: &HeaderMap,
    input: &Login,
    client: std::net::IpAddr,
) -> Result<Response, ApiError> {
    let (expected, store) = app
        .inspect_engine({
            let user = input.username.clone();
            move |engine, _| -> Result<_, ApiError> {
                Ok((
                    engine.password_hash(&user).map(str::to_owned),
                    engine.auth_store(),
                ))
            }
        })
        .await?;
    let valid = tokio::task::spawn_blocking({
        let password = input.password.clone();
        let expected = expected.clone();
        move || verify_login_password(&password, expected.as_deref())
    })
    .await
    .map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Password worker unavailable.",
        )
    })?;
    if !valid {
        let event = crate::engine::AuthEvent::new(
            Some(&input.username),
            Some(client),
            crate::engine::AuthOutcome::Failure,
        );
        store.record(event).await?;
        return Err(error(
            StatusCode::UNAUTHORIZED,
            "Invalid username or password.",
        ));
    }
    let value = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let ordering = app.mutations.metadata().await;
    let pending = app
        .with_metadata(&ordering, {
            let value = value.clone();
            let user = input.username.clone();
            let old = token(&app.config, headers).map(str::to_owned);
            move |engine| -> Result<_, ApiError> {
                engine
                    .prepare_login(
                        value,
                        &user,
                        expected.as_deref().ok_or_else(|| {
                            error(StatusCode::UNAUTHORIZED, "Invalid username or password.")
                        })?,
                        old.as_deref(),
                        crate::engine::AuthEvent::new(
                            Some(&user),
                            Some(client),
                            crate::engine::AuthOutcome::Success,
                        ),
                    )
                    .map_err(ApiError::from)
            }
        })
        .await?;
    let pending = pending.save().await?;
    app.with_metadata(&ordering, move |engine| {
        engine.publish_sessions(pending).map_err(ApiError::from)
    })
    .await?;
    tracing::info!(user = %input.username, "Login succeeded");
    let mut response = Json(json!({ "ok": true })).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie(app, &value, 43200)).map_err(|_| {
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Invalid session cookie configuration.",
            )
        })?,
    );
    Ok(response)
}

pub(crate) fn cookie(app: &App, value: &str, age: u32) -> String {
    format!(
        "{}={value}; HttpOnly; SameSite=Strict; Path={}; Max-Age={age}{}",
        cookie_name(&app.config),
        app.config.base_url,
        if app.config.production {
            "; Secure"
        } else {
            ""
        }
    )
}
pub(crate) async fn logout(
    State(app): State<App>,
    peer: Option<axum::Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    check_origin(&app, &headers)?;
    let client = peer.map(|axum::Extension(ConnectInfo(peer))| {
        client_ip(&app.config, peer.ip(), &headers).unwrap_or_else(|_| peer.ip().to_canonical())
    });
    let permit = app
        .io_permit()
        .map_err(|message| error(StatusCode::SERVICE_UNAVAILABLE, &message))?;
    tokio::spawn(async move {
        let _permit = permit;
        if let Some(token) = token(&app.config, &headers).map(str::to_owned) {
            let ordering = app.mutations.metadata().await;
            let (user, pending) = app
                .with_metadata(&ordering, move |engine| -> Result<_, ApiError> {
                    let user = engine.session(&token);
                    let event = crate::engine::AuthEvent::new(
                        user.as_deref(),
                        client,
                        crate::engine::AuthOutcome::Logout,
                    );
                    Ok(engine.prepare_logout(&token, event))
                })
                .await?;
            if let Some(pending) = pending {
                let pending = pending.save().await?;
                let changes = app.changes.clone();
                app.with_metadata(&ordering, move |engine| -> Result<_, ApiError> {
                    engine.publish_sessions(pending)?;
                    if let Some(user) = user {
                        let _ = changes.send(Change::Account(user));
                    }
                    Ok(())
                })
                .await?;
            }
        }
        let mut response = Json(json!({"ok":true})).into_response();
        response.headers_mut().insert(
            header::SET_COOKIE,
            HeaderValue::from_str(&cookie(&app, "", 0)).map_err(|_| {
                error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Invalid session cookie configuration.",
                )
            })?,
        );
        Ok(response)
    })
    .await
    .map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Logout worker unavailable.",
        )
    })?
}
pub(crate) async fn me(State(app): State<App>, headers: HeaderMap) -> Result<Response, ApiError> {
    check_read_origin(&app, &headers)?;
    let permit = app
        .io_permit()
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    tokio::spawn(async move {
        let _permit = permit;
        let value = super::snapshots::value_async(&app, &headers, false)
            .await
            .map_err(|cause| match cause {
                crate::engine::EngineError::Forbidden(_) => {
                    error(StatusCode::UNAUTHORIZED, "Invalid or expired credential.")
                }
                cause => ApiError::from(cause),
            })?;
        Ok((
            [(header::CONTENT_TYPE, "application/json")],
            value.to_string(),
        )
            .into_response())
    })
    .await
    .map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Snapshot worker unavailable.",
        )
    })?
}

pub(crate) async fn session_status(
    State(app): State<App>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    check_read_origin(&app, &headers)?;
    let permit = app
        .io_permit()
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    tokio::spawn(async move {
        let _permit = permit;
        let _identity = authenticated_identity(&app, &headers).await?;
        Ok(Json(json!({"ok":true})))
    })
    .await
    .map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Session worker unavailable.",
        )
    })?
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "auth_transaction_tests.rs"]
mod transaction_tests;

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Test assertions intentionally fail on invalid fixtures.
mod tests {
    use super::*;
    #[test]
    fn cookie_selection_enforces_host_prefix_and_rejects_duplicate_values_across_headers() {
        let mut config = Config {
            production: true,
            ..Config::default()
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            "chat_session=shadow; __Host-chat_session=actual"
                .parse()
                .unwrap(),
        );
        assert_eq!(token(&config, &headers), Some("actual"));
        headers.insert(
            header::COOKIE,
            HeaderValue::from_bytes(b"foreign=\xff; __Host-chat_session=actual").unwrap(),
        );
        assert_eq!(token(&config, &headers), Some("actual"));
        headers.append(
            header::COOKIE,
            "__Host-chat_session=second".parse().unwrap(),
        );
        assert!(token(&config, &headers).is_none());
        headers.insert(header::COOKIE, "chat_session=legacy".parse().unwrap());
        assert!(token(&config, &headers).is_none());
        config.base_url = "/commonroom/".into();
        assert_eq!(token(&config, &headers), Some("legacy"));
        headers.insert(
            header::COOKIE,
            "chat_session=one; chat_session=two".parse().unwrap(),
        );
        assert!(token(&config, &headers).is_none());
        headers.insert(header::COOKIE, "chat_session=".parse().unwrap());
        assert!(token(&config, &headers).is_none());
        let mut app = crate::web::tests::login_test_app();
        config.base_url = "/".into();
        app.config = std::sync::Arc::new(config);
        let value = cookie(&app, "session", 43200);
        assert!(value.starts_with("__Host-chat_session="));
        assert!(value.contains("; Path=/;"));
        assert!(value.ends_with("; Secure"));
        assert!(!value.contains("Domain="));
    }
    #[test]
    fn dummy_login_hash_uses_the_same_work_parameters_as_real_passwords() {
        let real = crate::engine::hash_password("test-password").unwrap();
        let real = argon2::PasswordHash::new(&real).unwrap();
        let dummy = argon2::PasswordHash::new(DUMMY_PASSWORD_HASH).unwrap();
        assert_eq!(real.algorithm, dummy.algorithm);
        assert_eq!(real.version, dummy.version);
        assert_eq!(real.params, dummy.params);
        assert_eq!(real.hash.unwrap().len(), dummy.hash.unwrap().len());
        assert!(!verify_login_password("test-password", None));
        assert!(!verify_login_password(
            "wrong-password",
            Some(&real.to_string())
        ));
        assert!(verify_login_password(
            "test-password",
            Some(&real.to_string())
        ));
    }
}
