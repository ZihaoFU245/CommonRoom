//! Account-scoped resource ownership. Dropping a lease releases its capacity.
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Profile {
    Browser,
    Bot,
}
struct Entry {
    id: u64,
    closed: watch::Sender<bool>,
    permit: Option<OwnedSemaphorePermit>,
}
#[derive(Default)]
struct State {
    serial: u64,
    sockets: HashMap<(String, Profile), VecDeque<Entry>>,
    http: HashMap<String, usize>,
    avatars: HashMap<String, (std::time::Instant, u32)>,
}
pub(crate) struct Admission {
    state: Mutex<State>,
    limits: crate::config::ServerConfig,
    pub(crate) metrics: Arc<super::metrics::Metrics>,
    browser: Arc<Semaphore>,
    bot: Arc<Semaphore>,
}
impl Default for Admission {
    fn default() -> Self {
        Self::new(crate::config::ServerConfig::default())
    }
}
impl Admission {
    pub fn new(limits: crate::config::ServerConfig) -> Self {
        Self {
            state: Mutex::default(),
            browser: Arc::new(Semaphore::new(
                usize::try_from(limits.browser_sockets).unwrap_or(128),
            )),
            bot: Arc::new(Semaphore::new(
                usize::try_from(limits.bot_sockets).unwrap_or(64),
            )),
            limits,
            metrics: Arc::default(),
        }
    }
    pub fn stats(&self) -> serde_json::Value {
        let active_http = self
            .state
            .lock()
            .map(|state| state.http.values().sum::<usize>())
            .unwrap_or(0);
        serde_json::json!({"http_active":active_http,
            "http_per_account_limit":self.limits.http_per_account,
            "browser_sockets_available":self.browser.available_permits(),
            "browser_sockets_limit":self.limits.browser_sockets,
            "bot_sockets_available":self.bot.available_permits(),
            "bot_sockets_limit":self.limits.bot_sockets,
            "sockets_per_account_limit":self.limits.sockets_per_account,
            "http":self.metrics.stats()})
    }
}
pub(crate) struct SocketLease {
    owner: Arc<Admission>,
    key: (String, Profile),
    id: u64,
    closed: watch::Receiver<bool>,
}
pub(crate) struct HttpLease {
    inherited: Option<Arc<HttpLease>>,
    owner: Arc<Admission>,
    account: String,
}
tokio::task_local! {
    static HTTP_LEASE: Arc<HttpLease>;
    static PRINCIPAL: super::principal::Resolved;
}
// A blocking job must retain the account reservation after its HTTP future is
// cancelled; retaining only the middleware guard would let cancelled work bypass it.
pub(crate) struct IoPermit {
    _global: OwnedSemaphorePermit,
    _account: Option<Arc<HttpLease>>,
    metrics: Arc<super::metrics::Metrics>,
}
pub(crate) fn io_permit(
    pool: &Arc<Semaphore>,
    metrics: &Arc<super::metrics::Metrics>,
) -> Result<IoPermit, String> {
    let global = pool.clone().try_acquire_owned().map_err(|_| {
        super::metrics::increment(&metrics.io_rejections);
        "Server busy; retry shortly."
    })?;
    super::metrics::increment(&metrics.io_accepted);
    let active = metrics
        .io_active
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        + 1;
    metrics
        .io_peak
        .fetch_max(active, std::sync::atomic::Ordering::Relaxed);
    Ok(IoPermit {
        metrics: metrics.clone(),
        _global: global,
        _account: HTTP_LEASE.try_with(Arc::clone).ok(),
    })
}
impl Drop for IoPermit {
    fn drop(&mut self) {
        self.metrics
            .io_active
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}
impl Admission {
    /// Self-service profile writes have a separate bounded account budget.
    pub(crate) fn profile_change(&self, account: &str) -> Result<(), super::ApiError> {
        self.profile_change_at(account, std::time::Instant::now())
    }
    fn profile_change_at(
        &self,
        account: &str,
        now: std::time::Instant,
    ) -> Result<(), super::ApiError> {
        use axum::http::StatusCode;
        let mut state = self.state.lock().map_err(|_| {
            super::error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Admission state unavailable; restart the service.",
            )
        })?;
        state
            .avatars
            .retain(|_, (at, _)| now.duration_since(*at).as_secs() < 10);
        if state.avatars.len() >= 4096 && !state.avatars.contains_key(account) {
            return Err(super::error(
                StatusCode::TOO_MANY_REQUESTS,
                "Profile changes busy; retry shortly.",
            ));
        }
        let entry = state.avatars.entry(account.into()).or_insert((now, 0));
        if entry.1 >= 4 {
            return Err(super::error(
                StatusCode::TOO_MANY_REQUESTS,
                "At most 4 profile changes per 10 seconds per account.",
            ));
        }
        entry.1 += 1;
        Ok(())
    }

    pub(crate) fn socket(
        self: &Arc<Self>,
        account: String,
        profile: Profile,
    ) -> Result<SocketLease, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Admission state unavailable; restart the service.")?;
        let key = (account, profile);
        // Reclaim the oldest lease before obtaining capacity for its replacement.
        if let Some(entries) = state.sockets.get_mut(&key)
            && entries.len() >= usize::try_from(self.limits.sockets_per_account).unwrap_or(4)
            && let Some(mut entry) = entries.pop_front()
        {
            super::metrics::increment(&self.metrics.socket_replacements);
            let _ = entry.closed.send(true);
            entry.permit.take();
        }
        let pool = match profile {
            Profile::Browser => &self.browser,
            Profile::Bot => &self.bot,
        };
        let permit = pool.clone().try_acquire_owned().map_err(|_| {
            super::metrics::increment(&self.metrics.socket_rejections);
            "Connection limit reached."
        })?;
        state.serial = state.serial.wrapping_add(1);
        let id = state.serial;
        let (closed, receiver) = watch::channel(false);
        state
            .sockets
            .entry(key.clone())
            .or_default()
            .push_back(Entry {
                id,
                closed,
                permit: Some(permit),
            });
        Ok(SocketLease {
            owner: self.clone(),
            key,
            id,
            closed: receiver,
        })
    }
    pub(crate) fn http(self: &Arc<Self>, account: String) -> Result<HttpLease, String> {
        if let Ok(parent) = HTTP_LEASE.try_with(Arc::clone)
            && parent.account == account
            && Arc::ptr_eq(&parent.owner, self)
        {
            return Ok(HttpLease {
                owner: self.clone(),
                account,
                inherited: Some(parent),
            });
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| "Admission state unavailable; restart the service.")?;
        let count = state.http.entry(account.clone()).or_default();
        if *count >= usize::try_from(self.limits.http_per_account).unwrap_or(8) {
            super::metrics::increment(&self.metrics.account_rejections);
            return Err(format!(
                "Account busy; at most {} concurrent requests.",
                self.limits.http_per_account
            ));
        }
        *count += 1;
        Ok(HttpLease {
            inherited: None,
            owner: self.clone(),
            account,
        })
    }
}
impl SocketLease {
    pub(crate) async fn cancelled(&mut self) {
        if *self.closed.borrow() {
            return;
        }
        let _ = self.closed.changed().await;
    }
}
impl Drop for SocketLease {
    fn drop(&mut self) {
        if let Ok(mut state) = self.owner.state.lock()
            && let Some(entries) = state.sockets.get_mut(&self.key)
        {
            entries.retain(|entry| entry.id != self.id);
            if entries.is_empty() {
                state.sockets.remove(&self.key);
            }
        }
    }
}
impl Drop for HttpLease {
    fn drop(&mut self) {
        if self.inherited.is_some() {
            return;
        }
        if let Ok(mut state) = self.owner.state.lock()
            && let Some(count) = state.http.get_mut(&self.account)
        {
            *count -= 1;
            if *count == 0 {
                state.http.remove(&self.account);
            }
        }
    }
}

// Tokio task locals are not inherited by spawned work. Carry the HTTP lease
// explicitly so a command worker reuses its request's account reservation.
pub(crate) fn spawn<T: Send + 'static>(
    future: impl std::future::Future<Output = T> + Send + 'static,
) -> tokio::task::JoinHandle<T> {
    let lease = HTTP_LEASE.try_with(Arc::clone).ok();
    let principal = PRINCIPAL.try_with(Clone::clone).ok();
    tokio::spawn(async move {
        let future = async move {
            match principal {
                Some(principal) => PRINCIPAL.scope(principal, future).await,
                None => future.await,
            }
        };
        match lease {
            Some(lease) => HTTP_LEASE.scope(lease, future).await,
            None => future.await,
        }
    })
}

/// Detached accepted work owns its permit and request principal through completion.
pub(crate) fn spawn_io<T: Send + 'static, P: Send + 'static>(
    permit: P,
    future: impl std::future::Future<Output = T> + Send + 'static,
) -> tokio::task::JoinHandle<T> {
    spawn(async move {
        let _permit = permit;
        future.await
    })
}

pub(crate) fn resolved(
    app: &super::App,
    headers: &axum::http::HeaderMap,
) -> Option<super::principal::Resolved> {
    PRINCIPAL
        .try_with(|resolved| resolved.matches(app, headers).then(|| resolved.clone()))
        .ok()
        .flatten()
}

// Resolve once for account admission; handlers reuse this capture but still
// authenticate its credential kind and revalidate after checking storage health.
pub(crate) async fn http(
    axum::extract::State(app): axum::extract::State<super::App>,
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let path = request.uri().path();
    if !path.contains("/api/")
        || path.ends_with("/api/login")
        || path.ends_with("/api/health")
        || path.ends_with("/ws")
    {
        return next.run(request).await;
    }
    let headers = request.headers().clone();
    let resolved = app
        .inspect_engine({
            let app = app.clone();
            move |engine, _| {
                Ok::<_, crate::engine::EngineError>(super::principal::Resolved::capture(
                    &app, engine, headers,
                ))
            }
        })
        .await;
    let resolved = match resolved {
        Ok(Some(resolved)) => resolved,
        Ok(None) => return next.run(request).await,
        Err(_) => {
            return super::error(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "Admission worker unavailable.",
            )
            .into_response();
        }
    };
    let lease = match app.admission.http(resolved.principal.account().to_owned()) {
        Ok(lease) => lease,
        Err(message) => {
            return super::error(axum::http::StatusCode::TOO_MANY_REQUESTS, &message)
                .into_response();
        }
    };
    request.extensions_mut().insert(resolved.clone());
    HTTP_LEASE
        .scope(
            Arc::new(lease),
            PRINCIPAL.scope(resolved, next.run(request)),
        )
        .await
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
