use super::security::{check_origin, client_ip};
use super::*;
use crate::engine::{valid_name, verify_password};
use axum::{
    extract::{ConnectInfo, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::time::Duration;

// Valid PHC encoding with the default Argon2 work parameters. Its verification
// result is discarded: missing and disabled accounts must never authenticate.
const DUMMY_PASSWORD_HASH: &str =
    "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHQ$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

fn verify_login_password(password: &str, expected: Option<&str>) -> bool {
    let valid = verify_password(password, expected.unwrap_or(DUMMY_PASSWORD_HASH));
    expected.is_some() && valid
}

pub(crate) fn token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|part| part.trim().strip_prefix("chat_session="))
}
pub(crate) fn authenticate(app: &App, headers: &HeaderMap) -> Result<String, ApiError> {
    if app.stopping.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Server is shutting down.",
        ));
    }
    let engine = app.engine_read_api()?;
    identity(&engine, headers)
        .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Invalid or expired credential."))
}
pub(crate) fn bearer(headers: &HeaderMap) -> Option<&str> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    value.strip_prefix("Bearer ").filter(|s| !s.is_empty())
}
pub(crate) fn identity(engine: &Engine, headers: &HeaderMap) -> Option<String> {
    if headers.contains_key(header::AUTHORIZATION) {
        engine.bot_identity(bearer(headers)?)
    } else {
        engine.session(token(headers)?)
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
    let permit = app
        .io_permit()
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        runtime.block_on(login_inner(
            State(app),
            ConnectInfo(peer),
            headers,
            Json(input),
        ))
    })
    .await
    .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "Login worker unavailable."))?
}
async fn login_inner(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(input): Json<Login>,
) -> Result<Response, ApiError> {
    check_origin(&app, &headers)?;
    if !valid_name(&input.username) || input.password.len() > 128 {
        return Err(error(
            StatusCode::UNAUTHORIZED,
            "Invalid username or password.",
        ));
    }
    {
        let mut attempts = app.attempts.lock().map_err(|_| {
            error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Login state unavailable; restart the service.",
            )
        })?;
        attempts.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(60));
        // Key on IP, never the client's ephemeral TCP port.
        let key = SocketAddr::new(client_ip(&app.config, peer.ip(), &headers), 0);
        let entry = attempts.entry(key).or_insert((Instant::now(), 0));
        if entry.1 >= 10 {
            return Err(error(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many login attempts. Wait a minute.",
            ));
        }
        entry.1 += 1;
    }
    let account = app
        .engine_read_api()?
        .data
        .users
        .get(&input.username)
        .cloned()
        .filter(|u| !u.disabled);
    let expected = account.as_ref().map(|a| a.hash.clone());
    let password = input.password;
    let permit = app.password_jobs.clone().try_acquire_owned().map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Login busy. Please try again shortly.",
        )
    })?;
    let valid = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        verify_login_password(&password, expected.as_deref())
    })
    .await
    .unwrap_or(false);
    if !valid {
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
    app.engine_api()?
        .login(
            value.clone(),
            &input.username,
            &account
                .as_ref()
                .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Invalid username or password."))?
                .hash,
            token(&headers),
        )
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    tracing::info!(user = %input.username, "Login succeeded");
    let mut response = Json(json!({ "ok": true })).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie(&app, &value, 43200)).map_err(|_| {
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
        "chat_session={value}; HttpOnly; SameSite=Strict; Path={}; Max-Age={age}{}",
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
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let permit = app
        .io_permit()
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        runtime.block_on(logout_inner(State(app), headers))
    })
    .await
    .map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Logout worker unavailable.",
        )
    })?
}
async fn logout_inner(State(app): State<App>, headers: HeaderMap) -> Result<Response, ApiError> {
    check_origin(&app, &headers)?;
    if let Some(token) = token(&headers) {
        app.engine_api()?
            .logout(token)
            .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    }
    let _ = app.changes.send(Change::All);
    let mut response = Json(json!({ "ok": true })).into_response();
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
}
pub(crate) async fn me(
    State(app): State<App>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let permit = app
        .io_permit()
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let username = authenticate(&app, &headers)?;
        let snapshot = app
            .engine_read_api()?
            .try_snapshot(&username)
            .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?
            .ok_or_else(|| error(StatusCode::SERVICE_UNAVAILABLE, "Snapshot unavailable."))?;
        Ok(Json(json!(snapshot)))
    })
    .await
    .map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Snapshot worker unavailable.",
        )
    })?
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Test assertions intentionally fail on invalid fixtures.
mod tests {
    use super::*;
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
