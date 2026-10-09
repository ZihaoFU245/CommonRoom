use super::security::check_origin;
use super::*;
use crate::engine::verify_password;
use axum::{
    extract::{ConnectInfo, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::time::Duration;

pub(super) fn token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|part| part.trim().strip_prefix("chat_session="))
}
pub(super) fn authenticate(app: &App, headers: &HeaderMap) -> Result<String, ApiError> {
    if app.stopping.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Server is shutting down.",
        ));
    }
    token(headers)
        .and_then(|t| app.engine.lock().unwrap().session(t))
        .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Please log in."))
}
#[derive(Deserialize)]
pub(super) struct Login {
    username: String,
    password: String,
}
pub(super) async fn login(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(input): Json<Login>,
) -> Result<Response, ApiError> {
    check_origin(&app, &headers)?;
    if input.username.len() > 32 || input.password.len() > 128 {
        return Err(error(
            StatusCode::UNAUTHORIZED,
            "Invalid username or password.",
        ));
    }
    {
        let mut attempts = app.attempts.lock().unwrap();
        attempts.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(60));
        // Key on IP, never the client's ephemeral TCP port.
        let key = SocketAddr::new(peer.ip(), 0);
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
        .engine
        .lock()
        .unwrap()
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
        expected.is_some_and(|h| verify_password(&password, &h))
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
    app.engine
        .lock()
        .unwrap()
        .login(
            value.clone(),
            &input.username,
            &account.unwrap().hash,
            token(&headers),
        )
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    tracing::info!(user = %input.username, "Login succeeded");
    let mut response = Json(json!({ "ok": true })).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie(&app, &value, 43200)).unwrap(),
    );
    Ok(response)
}
pub(super) fn cookie(app: &App, value: &str, age: u32) -> String {
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
pub(super) async fn logout(
    State(app): State<App>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    check_origin(&app, &headers)?;
    if let Some(token) = token(&headers) {
        app.engine
            .lock()
            .unwrap()
            .logout(token)
            .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    }
    let _ = app.changes.send(Change::All);
    let mut response = Json(json!({ "ok": true })).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie(&app, "", 0)).unwrap(),
    );
    Ok(response)
}
pub(super) async fn me(
    State(app): State<App>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let username = authenticate(&app, &headers)?;
    let snapshot = app.engine.lock().unwrap().snapshot(&username);
    Ok(Json(json!(snapshot)))
}
