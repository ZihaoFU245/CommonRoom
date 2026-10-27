pub(crate) mod admission;
pub(crate) mod auth;
mod avatars;
mod colors;
mod error;
mod metadata_commands;
mod profiles;
mod query_commands;
pub(crate) use error::ApiError;
mod bot_commands;
mod cleanup_commands;
pub(crate) mod commands;
pub(crate) mod history;
pub(crate) mod login_limits;
pub(crate) mod media;
pub(crate) mod mutations;
pub(crate) mod security;
pub(crate) mod snapshots;
pub(crate) mod socket;
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // Test fixtures intentionally assert success.
mod tests;
pub(crate) mod transport;
pub(crate) mod updates;

use crate::{config::Config, engine::Engine};
use axum::{
    Json, Router,
    http::StatusCode,
    middleware,
    routing::{get, post},
};
use serde_json::json;
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex, RwLock},
};
use tokio::sync::broadcast;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    All,
    Metadata(Arc<crate::engine::MetadataNotice>),
    Presence,
    Shutdown,
    Read(String),
    Account(String),
    Avatar(String),
    Conversation(String),
    BotCommands(String),
}

impl Change {
    pub(crate) fn command(change: crate::engine::CommandChange) -> Option<Self> {
        match change {
            crate::engine::CommandChange::Unchanged => None,
            crate::engine::CommandChange::Metadata(notice) => Some(Self::Metadata(notice)),
            crate::engine::CommandChange::Conversation(id) => Some(Self::Conversation(id)),
        }
    }
}

#[derive(Clone)]
pub struct App {
    pub media: Option<Arc<crate::media::MediaStorage>>,
    pub engine: Arc<RwLock<Engine>>,
    pub attempts: Arc<Mutex<login_limits::LoginLimits>>,
    pub changes: broadcast::Sender<Change>,
    pub config: Arc<Config>,
    pub admission: Arc<admission::Admission>,
    pub io_jobs: Arc<tokio::sync::Semaphore>,
    pub password_jobs: Arc<tokio::sync::Semaphore>,
    pub snapshots: Arc<Mutex<snapshots::SnapshotCache>>,
    pub(crate) mutations: Arc<mutations::Coordinator>,
    pub bot_runtimes: crate::bots::runtime::Hub,
    pub stopping: Arc<std::sync::atomic::AtomicBool>,
    pub shutdown: Arc<tokio::sync::Notify>,
}
pub(crate) const IO_JOBS: u32 = 64;
pub(crate) const PASSWORD_JOBS: u32 = 2;
impl App {
    pub fn io_permit(&self) -> Result<admission::IoPermit, String> {
        admission::io_permit(&self.io_jobs)
    }

    /// Capture or validate owned metadata on a worker; database futures run
    /// outside this guard through the typed async storage interface.
    pub(crate) async fn inspect_engine<T, E>(
        &self,
        inspect: impl FnOnce(&Engine, &Config) -> Result<T, E> + Send + 'static,
    ) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<crate::engine::EngineError> + Send + 'static,
    {
        let app = self.clone();
        tokio::task::spawn_blocking(move || {
            let engine = app.engine_read_state().map_err(E::from)?;
            inspect(&engine, &app.config)
        })
        .await
        .map_err(|error| E::from(crate::engine::EngineError::unavailable(error)))?
    }

    pub(crate) async fn checkpoint_after_jobs(&self) -> Result<(), crate::engine::EngineError> {
        use crate::engine::EngineError;
        if !self.stopping.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(EngineError::Internal(
                "Shutdown requires stopped admission.".into(),
            ));
        }
        // Accepted operations retain these permits through commit/publication,
        // including cancelled HTTP/console work and password-only login jobs.
        let _io = self
            .io_jobs
            .clone()
            .acquire_many_owned(IO_JOBS)
            .await
            .map_err(EngineError::unavailable)?;
        let _password = self
            .password_jobs
            .clone()
            .acquire_many_owned(PASSWORD_JOBS)
            .await
            .map_err(EngineError::unavailable)?;
        let checkpoint = self
            .inspect_engine(|engine, _| Ok::<_, EngineError>(engine.checkpoint_async()))
            .await?;
        checkpoint.await?;
        self.io_jobs.close();
        self.password_jobs.close();
        Ok(())
    }

    // Fail closed after a panic; a poisoned engine may contain a partial mutation.
    #[cfg(test)]
    pub fn engine(&self) -> Result<mutations::EngineGuard<'_>, crate::engine::EngineError> {
        let metadata = self.mutations.exclusive()?;
        let engine = self.engine.write().map_err(|_| {
            crate::engine::EngineError::Unavailable(
                "Server state unavailable; restart the service.".into(),
            )
        })?;
        engine.ensure_recovered()?;
        Ok(mutations::EngineGuard {
            engine,
            _metadata: metadata,
        })
    }
    // Extra tabs only change a counter; preserve shared baselines unless the
    // visible online directory changes.
    pub(crate) fn update_presence(
        &self,
        old: Option<&str>,
        current: Option<&str>,
    ) -> Result<(), String> {
        let mut engine = self.engine.write().map_err(|_| {
            crate::engine::EngineError::Unavailable(
                "Server state unavailable; restart the service.".into(),
            )
        })?;
        engine.ensure_memory()?;
        let disconnected = old.is_some_and(|name| engine.disconnect(name));
        let connected = current.is_some_and(|name| engine.connect(name));
        if disconnected || connected {
            let _ = self.changes.send(Change::Presence);
        }
        Ok(())
    }
    fn engine_read_state(
        &self,
    ) -> Result<std::sync::RwLockReadGuard<'_, Engine>, crate::engine::EngineError> {
        let engine = self.engine.read().map_err(|_| {
            crate::engine::EngineError::Unavailable(
                "Server state unavailable; restart the service.".into(),
            )
        })?;
        engine.ensure_memory()?;
        Ok(engine)
    }
    #[cfg(test)]
    pub fn engine_read(
        &self,
    ) -> Result<std::sync::RwLockReadGuard<'_, Engine>, crate::engine::EngineError> {
        let engine = self.engine_read_state()?;
        engine.ensure_recovered()?;
        Ok(engine)
    }
    #[cfg(test)]
    fn engine_read_api(&self) -> Result<std::sync::RwLockReadGuard<'_, Engine>, ApiError> {
        self.engine_read().map_err(ApiError::from)
    }
    #[cfg(test)]
    fn engine_api(&self) -> Result<mutations::EngineGuard<'_>, ApiError> {
        self.engine().map_err(ApiError::from)
    }
}
pub(crate) fn error(code: StatusCode, message: &str) -> ApiError {
    use crate::engine::EngineError;
    let typed = match code {
        StatusCode::BAD_REQUEST => Some(EngineError::Invalid(message.into())),
        StatusCode::NOT_FOUND => Some(EngineError::NotFound(message.into())),
        StatusCode::FORBIDDEN => Some(EngineError::Forbidden(message.into())),
        StatusCode::CONFLICT => Some(EngineError::Conflict(message.into())),
        StatusCode::SERVICE_UNAVAILABLE => Some(EngineError::Unavailable(message.into())),
        StatusCode::INTERNAL_SERVER_ERROR => Some(EngineError::Internal(message.into())),
        _ => None,
    };
    if let Some(error) = typed {
        return error.into();
    }
    let name = match code {
        StatusCode::UNAUTHORIZED => "unauthorized",
        StatusCode::TOO_MANY_REQUESTS => "rate_limited",
        _ => "request_failed",
    };
    ApiError(
        code,
        Json(
            json!({ "error": message, "code":name, "retryable":code == StatusCode::TOO_MANY_REQUESTS }),
        ),
    )
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
        .route("/api/color", post(colors::set))
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
        .layer(middleware::from_fn_with_state(app.clone(), admission::http))
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
