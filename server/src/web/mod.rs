mod assets;
mod auth;
mod commands;
mod history;
mod security;
mod socket;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // Test fixtures intentionally assert success.
mod tests;

use crate::{config::Config, engine::Engine};
use axum::{
    Json, Router,
    http::StatusCode,
    middleware,
    response::Redirect,
    routing::{get, post},
};
use serde_json::json;
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::sync::broadcast;

#[derive(Clone)]
pub enum Change {
    All,
    Read(String),
}

#[derive(Clone)]
pub struct App {
    pub engine: Arc<Mutex<Engine>>,
    pub attempts: Arc<Mutex<HashMap<SocketAddr, (Instant, u32)>>>,
    pub changes: broadcast::Sender<Change>,
    pub config: Arc<Config>,
    pub connections: Arc<tokio::sync::Semaphore>,
    pub password_jobs: Arc<tokio::sync::Semaphore>,
    pub stopping: Arc<std::sync::atomic::AtomicBool>,
}
impl App {
    // Fail closed after a panic; a poisoned engine may contain a partial mutation.
    pub fn engine(&self) -> Result<std::sync::MutexGuard<'_, Engine>, String> {
        self.engine
            .lock()
            .map_err(|_| "Server state unavailable; restart the service.".into())
    }
    fn engine_api(&self) -> Result<std::sync::MutexGuard<'_, Engine>, ApiError> {
        self.engine()
            .map_err(|message| error(StatusCode::SERVICE_UNAVAILABLE, &message))
    }
}
type ApiError = (StatusCode, Json<serde_json::Value>);
fn error(code: StatusCode, message: &str) -> ApiError {
    (code, Json(json!({ "error": message })))
}

pub fn router(app: App) -> Router {
    let routes = Router::new()
        .route("/", get(assets::assets))
        .route("/api/login", post(auth::login))
        .route("/api/logout", post(auth::logout))
        .route("/api/me", get(auth::me))
        .route("/api/history", get(history::history))
        .route("/api/read", post(history::read))
        .route("/api/health", get(|| async { Json(json!({ "ok": true })) }))
        .route("/ws", get(socket::upgrade))
        .fallback(assets::assets);
    let routes = if app.config.base_url == "/" {
        routes
    } else {
        let base_url = app.config.base_url.clone();
        let prefix = base_url.trim_end_matches('/').to_owned();
        Router::new()
            .route(
                &prefix,
                get(move || async move { Redirect::permanent(&base_url) }),
            )
            .nest(&app.config.base_url, routes)
    };
    routes
        .layer(axum::extract::DefaultBodyLimit::max(4096))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(middleware::from_fn_with_state(
            app.clone(),
            security::security,
        ))
        .with_state(app)
}
