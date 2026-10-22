use super::security::{check_origin, check_read_origin_or_bot, client_ip};
use super::*;
use crate::engine::{valid_name, verify_password};
use axum::{
    extract::{ConnectInfo, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;

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
    check_origin(&app, &headers)?;
    if !valid_name(&input.username) || input.password.len() > 128 {
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
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let result = login_inner(&app, &headers, &input);
        // Pending attempts count until completion, so concurrent requests cannot
        // overshoot a limit. Cancelled HTTP futures still finish this bookkeeping.
        let failed = matches!(&result, Err((StatusCode::UNAUTHORIZED, _)));
        app.attempts
            .lock()
            .map_err(|_| login_state_unavailable())?
            .finish(reservation, failed);
        if failed {
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
fn login_inner(app: &App, headers: &HeaderMap, input: &Login) -> Result<Response, ApiError> {
    let account = app
        .engine_read_api()?
        .data
        .users
        .get(&input.username)
        .cloned()
        .filter(|u| !u.disabled);
    let valid = verify_login_password(&input.password, account.as_ref().map(|a| a.hash.as_str()));
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
            token(headers),
        )
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
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
pub(crate) async fn me(State(app): State<App>, headers: HeaderMap) -> Result<Response, ApiError> {
    check_read_origin_or_bot(&app, &headers)?;
    let permit = app
        .io_permit()
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        authenticate(&app, &headers)?;
        let value = super::snapshots::value(&app, &headers, false)
            .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
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
    check_read_origin_or_bot(&app, &headers)?;
    let permit = app
        .io_permit()
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        authenticate(&app, &headers)?;
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
