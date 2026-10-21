pub(crate) mod auth;
mod avatars;
mod bot_commands;
pub(crate) mod commands;
pub(crate) mod history;
pub(crate) mod media;
pub(crate) mod security;
pub(crate) mod snapshots;
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
    Presence,
    Read(String),
    Conversation(String),
    BotCommands(String),
}

#[derive(Clone)]
pub struct App {
    pub media: Option<Arc<crate::media::MediaStorage>>,
    pub engine: Arc<RwLock<Engine>>,
    pub attempts: Arc<Mutex<HashMap<SocketAddr, (Instant, u32)>>>,
    pub changes: broadcast::Sender<Change>,
    pub config: Arc<Config>,
    pub connections: Arc<tokio::sync::Semaphore>,
    pub io_jobs: Arc<tokio::sync::Semaphore>,
    pub password_jobs: Arc<tokio::sync::Semaphore>,
    pub snapshots: Arc<Mutex<snapshots::SnapshotCache>>,
    pub bot_runtimes: crate::bots::runtime::Hub,
    pub stopping: Arc<std::sync::atomic::AtomicBool>,
    pub shutdown: Arc<tokio::sync::Notify>,
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
        let engine = self
            .engine
            .write()
            .map_err(|_| "Server state unavailable; restart the service.")?;
        engine.ensure_recovered()?;
        Ok(engine)
    }
    // Extra tabs only change a counter; preserve shared baselines unless the
    // visible online directory changes.
    pub(crate) fn update_presence(
        &self,
        old: Option<&str>,
        current: Option<&str>,
    ) -> Result<(), String> {
        let mut engine = self
            .engine
            .write()
            .map_err(|_| "Server state unavailable; restart the service.")?;
        engine.ensure_recovered()?;
        let disconnected = old.is_some_and(|name| engine.disconnect(name));
        let connected = current.is_some_and(|name| engine.connect(name));
        if disconnected || connected {
            let _ = self.changes.send(Change::Presence);
        }
        Ok(())
    }
    pub fn engine_read(&self) -> Result<std::sync::RwLockReadGuard<'_, Engine>, String> {
        let engine = self
            .engine
            .read()
            .map_err(|_| "Server state unavailable; restart the service.")?;
        engine.ensure_recovered()?;
        Ok(engine)
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
        .route("/api/avatars", post(avatars::set))
        .route("/api/avatars/uploads", post(avatars::allocate))
        .route(
            "/api/avatars/uploads/{id}/complete",
            post(avatars::complete),
        )
        .route("/api/avatars/{id}/info", get(avatars::info))
        .route("/api/avatars/{id}/file", get(avatars::download))
        .route("/api/media/config", get(media::config))
        .route("/api/media/uploads", post(media::allocate))
        .route("/api/media/uploads/{id}/complete", post(media::complete))
        .route("/api/media/uploads/{id}/abandon", post(media::abandon))
        .route("/api/media/{id}/file", get(media::download))
        .route("/api/media/{id}/info", get(media::info))
        .route("/api/login", post(auth::login))
        .route("/api/logout", post(auth::logout))
        .route("/api/me", get(auth::me))
        .route("/api/session", get(auth::session_status))
        .route("/api/history", get(history::history))
        .route("/api/read", post(history::read))
        .route("/api/bot-commands", get(bot_commands::get))
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
        .layer(middleware::from_fn_with_state(
            app.clone(),
            crate::bots::contract,
        ))
        .with_state(app)
}
