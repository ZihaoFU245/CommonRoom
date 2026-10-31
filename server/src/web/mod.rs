pub(crate) mod admission;
pub(crate) mod auth;
pub(crate) mod avatars;
pub(crate) mod colors;
mod display_names;
mod error;
mod metadata_commands;
pub(crate) mod metrics;
mod preferences;
pub(crate) mod principal;
mod profiles;
mod query_commands;
pub(crate) use error::ApiError;
mod bot_commands;
mod cleanup_commands;
pub(crate) mod commands;
mod deadline_io;
pub(crate) mod history;
pub(crate) mod http_server;
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
    BotCatalogues(Arc<std::collections::BTreeSet<String>>),
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
impl App {
    pub(crate) fn password_permit(
        &self,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, crate::engine::EngineError> {
        self.password_jobs.clone().try_acquire_owned().map_err(|_| {
            metrics::increment(&self.admission.metrics.password_rejections);
            crate::engine::EngineError::Unavailable("Password workers busy; retry shortly.".into())
        })
    }
    pub fn io_permit(&self) -> Result<admission::IoPermit, String> {
        admission::io_permit(&self.io_jobs, &self.admission.metrics)
    }

    /// Capture or validate owned metadata with a short memory guard, using a
    /// worker only on contention. Database futures run outside this guard.
    pub(crate) async fn inspect_engine<T, E>(
        &self,
        inspect: impl FnOnce(&Engine, &Config) -> Result<T, E> + Send + 'static,
    ) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<crate::engine::EngineError> + Send + 'static,
    {
        match self.engine.try_read() {
            Ok(engine) => {
                return std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    engine.ensure_memory().map_err(E::from)?;
                    inspect(&engine, &self.config)
                }))
                .map_err(|_| {
                    E::from(crate::engine::EngineError::unavailable(
                        "Engine inspection failed",
                    ))
                })?;
            }
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(E::from(crate::engine::EngineError::unavailable(
                    "Engine state poisoned",
                )));
            }
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
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
        loop {
            let _io = self
                .io_jobs
                .clone()
                .acquire_many_owned(self.config.server.io_jobs)
                .await
                .map_err(EngineError::unavailable)?;
            let _password = self
                .password_jobs
                .clone()
                .acquire_many_owned(self.config.server.password_jobs)
                .await
                .map_err(EngineError::unavailable)?;
            if self.mutations.recovering() {
                drop(_password);
                drop(_io);
                self.mutations.wait_for_recovery().await;
                continue;
            }
            let checkpoint = self
                .inspect_engine(|engine, _| Ok::<_, EngineError>(engine.checkpoint_async()))
                .await?;
            checkpoint.await?;
            self.io_jobs.close();
            self.password_jobs.close();
            return Ok(());
        }
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
    let mut registry = metrics::Registry::new(&app.config.base_url);
    let bots = crate::bots::routes(&mut registry);
    let mut path = |path| registry.register("", path);
    let routes = Router::new()
        .merge(bots)
        .route(path("/api/avatars"), post(avatars::set))
        .route(path("/api/avatars/uploads"), post(avatars::allocate))
        .route(
            path("/api/avatars/uploads/{id}/complete"),
            post(avatars::complete),
        )
        .route(path("/api/avatars/{id}/info"), get(avatars::info))
        .route(path("/api/avatars/{id}/file"), get(avatars::download))
        .route(path("/api/media/config"), get(media::config))
        .route(path("/api/media/uploads"), post(media::allocate))
        .route(
            path("/api/media/uploads/{id}/complete"),
            post(media::complete),
        )
        .route(
            path("/api/media/uploads/{id}/abandon"),
            post(media::abandon),
        )
        .route(path("/api/media/{id}/file"), get(media::download))
        .route(path("/api/media/{id}/info"), get(media::info))
        .route(path("/api/login"), post(auth::login))
        .route(path("/api/logout"), post(auth::logout))
        .route(path("/api/me"), get(auth::me))
        .route(path("/api/session"), get(auth::session_status))
        .route(path("/api/history"), get(history::history))
        .route(path("/api/read"), post(history::read))
        .route(path("/api/color"), post(colors::set))
        .route(path("/api/preferences"), post(preferences::set))
        .route(
            path("/api/displayname"),
            post(display_names::set_display_name),
        )
        .route(path("/api/bot-commands"), get(bot_commands::get))
        .route(
            path("/api/health"),
            get(|| async { Json(json!({ "ok": true })) }),
        )
        .route(path("/ws"), get(socket::upgrade));
    let routes = if app.config.base_url == "/" {
        routes
    } else {
        Router::new().nest(app.config.base_url.trim_end_matches('/'), routes)
    };
    app.admission.metrics.register_routes(registry);
    routes
        .layer(axum::extract::DefaultBodyLimit::max(
            crate::limits::BROWSER_BODY_BYTES,
        ))
        .layer(middleware::from_fn_with_state(app.clone(), admission::http))
        .layer(middleware::from_fn_with_state(app.clone(), metrics::http))
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(middleware::from_fn_with_state(
            app.clone(),
            security::security,
        ))
        .layer(middleware::from_fn_with_state(
            app.clone(),
            metrics::observe,
        ))
        .layer(middleware::from_fn_with_state(
            app.clone(),
            crate::bots::contract,
        ))
        .with_state(app)
}
