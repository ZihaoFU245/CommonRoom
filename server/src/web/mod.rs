pub(crate) mod auth;
pub(crate) mod commands;
pub(crate) mod history;
pub(crate) mod security;
pub(crate) mod socket;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // Test fixtures intentionally assert success.
mod tests;

use crate::{config::Config, engine::Engine};
use axum::{
    Json, Router,
    http::StatusCode,
    middleware,
    routing::{get, post},
};
use serde_json::json;
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex, RwLock},
    time::Instant,
};
use tokio::sync::broadcast;

#[derive(Clone, Debug)]
pub enum Change {
    All,
    Read(String),
    Conversation(String),
}

#[derive(Clone)]
pub struct App {
    pub engine: Arc<RwLock<Engine>>,
    pub attempts: Arc<Mutex<HashMap<SocketAddr, (Instant, u32)>>>,
    pub changes: broadcast::Sender<Change>,
    pub config: Arc<Config>,
    pub connections: Arc<tokio::sync::Semaphore>,
    pub io_jobs: Arc<tokio::sync::Semaphore>,
    pub password_jobs: Arc<tokio::sync::Semaphore>,
    pub bot_runtimes: crate::bots::runtime::Hub,
    pub stopping: Arc<std::sync::atomic::AtomicBool>,
}
impl App {
    pub fn io_permit(&self) -> Result<tokio::sync::OwnedSemaphorePermit, String> {
        self.io_jobs
            .clone()
            .try_acquire_owned()
            .map_err(|_| "Server busy; retry shortly.".into())
    }

    // Fail closed after a panic; a poisoned engine may contain a partial mutation.
    pub fn engine(&self) -> Result<std::sync::RwLockWriteGuard<'_, Engine>, String> {
        self.engine
            .write()
            .map_err(|_| "Server state unavailable; restart the service.".into())
    }
    pub fn engine_read(&self) -> Result<std::sync::RwLockReadGuard<'_, Engine>, String> {
        self.engine
            .read()
            .map_err(|_| "Server state unavailable; restart the service.".into())
    }
    fn engine_read_api(&self) -> Result<std::sync::RwLockReadGuard<'_, Engine>, ApiError> {
        self.engine_read()
            .map_err(|message| error(StatusCode::SERVICE_UNAVAILABLE, &message))
    }
    fn engine_api(&self) -> Result<std::sync::RwLockWriteGuard<'_, Engine>, ApiError> {
        self.engine()
            .map_err(|message| error(StatusCode::SERVICE_UNAVAILABLE, &message))
    }
}
pub(crate) type ApiError = (StatusCode, Json<serde_json::Value>);
pub(crate) fn error(code: StatusCode, message: &str) -> ApiError {
    (code, Json(json!({ "error": message })))
}

pub fn router(app: App) -> Router {
    let routes = Router::new()
        .merge(crate::bots::routes())
        .route("/api/login", post(auth::login))
        .route("/api/logout", post(auth::logout))
        .route("/api/me", get(auth::me))
        .route("/api/history", get(history::history))
        .route("/api/read", post(history::read))
        .route("/api/health", get(|| async { Json(json!({ "ok": true })) }))
        .route("/ws", get(socket::upgrade));
    let routes = if app.config.base_url == "/" {
        routes
    } else {
        Router::new().nest(app.config.base_url.trim_end_matches('/'), routes)
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
