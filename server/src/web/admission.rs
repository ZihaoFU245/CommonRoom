//! Account-scoped resource ownership. Dropping a lease releases its capacity.
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};

const SOCKETS_PER_ACCOUNT: usize = 4;
const HTTP_PER_ACCOUNT: usize = 8;

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
    browser: Arc<Semaphore>,
    bot: Arc<Semaphore>,
}
impl Default for Admission {
    fn default() -> Self {
        Self {
            state: Mutex::default(),
            browser: Arc::new(Semaphore::new(128)),
            bot: Arc::new(Semaphore::new(64)),
        }
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
}
// A blocking job must retain the account reservation after its HTTP future is
// cancelled; retaining only the middleware guard would let cancelled work bypass it.
pub(crate) struct IoPermit {
    _global: OwnedSemaphorePermit,
    _account: Option<Arc<HttpLease>>,
}
pub(crate) fn io_permit(pool: &Arc<Semaphore>) -> Result<IoPermit, String> {
    let global = pool
        .clone()
        .try_acquire_owned()
        .map_err(|_| "Server busy; retry shortly.")?;
    Ok(IoPermit {
        _global: global,
        _account: HTTP_LEASE.try_with(Arc::clone).ok(),
    })
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
            && entries.len() >= SOCKETS_PER_ACCOUNT
            && let Some(mut entry) = entries.pop_front()
        {
            let _ = entry.closed.send(true);
            entry.permit.take();
        }
        let pool = match profile {
            Profile::Browser => &self.browser,
            Profile::Bot => &self.bot,
        };
        let permit = pool
            .clone()
            .try_acquire_owned()
            .map_err(|_| "Connection limit reached.")?;
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
        if *count >= HTTP_PER_ACCOUNT {
            return Err("Account busy; at most 8 concurrent requests.".into());
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
    tokio::spawn(async move {
        match lease {
            Some(lease) => HTTP_LEASE.scope(lease, future).await,
            None => future.await,
        }
    })
}

// Authentication still belongs to each handler. This middleware only acquires
// fair capacity for credentials that currently identify a live stable account.
pub(crate) async fn http(
    axum::extract::State(app): axum::extract::State<super::App>,
    request: axum::extract::Request,
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
    let account = app
        .inspect_engine(
            move |engine, config| -> Result<Option<String>, crate::engine::EngineError> {
                if headers.contains_key(axum::http::header::AUTHORIZATION) {
                    return Ok(crate::bots::auth::Context::from_headers(engine, &headers)
                        .ok()
                        .map(|context| context.account));
                }
                Ok(super::auth::identity(engine, config, &headers)
                    .and_then(|name| engine.active_account_id(&name).map(str::to_owned)))
            },
        )
        .await;
    let _lease = match account {
        Ok(Some(account)) => match app.admission.http(account) {
            Ok(lease) => Some(lease),
            Err(message) => {
                return super::error(axum::http::StatusCode::TOO_MANY_REQUESTS, &message)
                    .into_response();
            }
        },
        Ok(None) => None,
        _ => {
            return super::error(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "Admission worker unavailable.",
            )
            .into_response();
        }
    };
    match _lease {
        Some(lease) => HTTP_LEASE.scope(Arc::new(lease), next.run(request)).await,
        None => next.run(request).await,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn spawned_commands_reuse_their_http_reservation() {
        let admission = Arc::new(Admission::default());
        let lease = Arc::new(admission.http("alice".into()).unwrap());
        let others: Vec<_> = (0..7)
            .map(|_| admission.http("alice".into()).unwrap())
            .collect();
        HTTP_LEASE
            .scope(lease, async {
                spawn({
                    let admission = admission.clone();
                    async move {
                        let nested = admission.http("alice".into()).unwrap();
                        assert_eq!(admission.state.lock().unwrap().http["alice"], 8);
                        drop(nested);
                        assert_eq!(admission.state.lock().unwrap().http["alice"], 8);
                    }
                })
                .await
                .unwrap();
            })
            .await;
        assert_eq!(admission.state.lock().unwrap().http["alice"], 7);
        drop(others);
        assert!(admission.state.lock().unwrap().http.is_empty());
    }
    #[test]
    fn avatar_budget_is_bounded_account_scoped_and_recovers_after_its_window() {
        let admission = Admission::default();
        let now = std::time::Instant::now();
        for _ in 0..4 {
            admission.profile_change_at("alice", now).unwrap();
        }
        assert_eq!(
            admission.profile_change_at("alice", now).unwrap_err().0,
            axum::http::StatusCode::TOO_MANY_REQUESTS
        );
        admission.profile_change_at("bob", now).unwrap();
        admission
            .profile_change_at("alice", now + std::time::Duration::from_secs(10))
            .unwrap();
        assert_eq!(admission.state.lock().unwrap().avatars.len(), 1);
        for i in 0..4095 {
            admission
                .profile_change_at(
                    &format!("account-{i}"),
                    now + std::time::Duration::from_secs(10),
                )
                .unwrap();
        }
        assert!(
            admission
                .profile_change_at("overflow", now + std::time::Duration::from_secs(10))
                .is_err()
        );
        assert_eq!(admission.state.lock().unwrap().avatars.len(), 4096);
    }
    #[tokio::test]
    async fn replacement_cancels_only_the_oldest_socket_and_keeps_capacity_bounded() {
        let admission = Arc::new(Admission::default());
        let mut oldest = admission.socket("alice".into(), Profile::Browser).unwrap();
        let others: Vec<_> = (0..3)
            .map(|_| admission.socket("alice".into(), Profile::Browser).unwrap())
            .collect();
        let replacement = admission.socket("alice".into(), Profile::Browser).unwrap();
        tokio::time::timeout(std::time::Duration::from_millis(50), oldest.cancelled())
            .await
            .unwrap();
        assert!(others.iter().all(|lease| !*lease.closed.borrow()));
        assert_eq!(admission.browser.available_permits(), 124);
        drop(oldest);
        assert_eq!(admission.browser.available_permits(), 124);
        drop(others);
        drop(replacement);
        assert_eq!(admission.browser.available_permits(), 128);
        assert!(admission.state.lock().unwrap().sockets.is_empty());
    }
    #[test]
    fn browser_and_bot_capacity_and_accounts_are_independent() {
        let admission = Arc::new(Admission::default());
        let browser: Vec<_> = (0..32)
            .flat_map(|n| (0..4).map(move |_| n))
            .map(|n| admission.socket(n.to_string(), Profile::Browser).unwrap())
            .collect();
        assert!(
            admission
                .socket("another".into(), Profile::Browser)
                .is_err()
        );
        let bot = admission.socket("another".into(), Profile::Bot).unwrap();
        assert_eq!(admission.bot.available_permits(), 63);
        drop(browser);
        drop(bot);
        assert!(admission.state.lock().unwrap().sockets.is_empty());
    }
    #[tokio::test]
    async fn cancelled_requests_retain_account_capacity_until_the_worker_finishes() {
        let app = crate::web::tests::login_test_app();
        let account = "alice-stable-id".to_owned();
        let lease = Arc::new(app.admission.http(account.clone()).unwrap());
        let (sent, received) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        let request = tokio::spawn({
            let app = app.clone();
            async move {
                HTTP_LEASE
                    .scope(lease, async {
                        let permit = app.io_permit().unwrap();
                        tokio::task::spawn_blocking(move || {
                            let _permit = permit;
                            let _ = sent.send(());
                            blocked.recv().unwrap();
                        })
                        .await
                        .unwrap();
                    })
                    .await;
            }
        });
        received.await.unwrap();
        request.abort();
        let _ = request.await;
        let others: Vec<_> = (0..7)
            .map(|_| app.admission.http(account.clone()).unwrap())
            .collect();
        assert!(app.admission.http(account.clone()).is_err());
        release.send(()).unwrap();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
        while app.admission.http(account.clone()).is_err() {
            assert!(tokio::time::Instant::now() < deadline);
            tokio::task::yield_now().await;
        }
        drop(others);
    }
    #[test]
    fn http_capacity_is_account_scoped_and_released_on_drop() {
        let admission = Arc::new(Admission::default());
        let mut leases: Vec<_> = (0..8)
            .map(|_| admission.http("alice".into()).unwrap())
            .collect();
        assert!(admission.http("alice".into()).is_err());
        let bob = admission.http("bob".into()).unwrap();
        leases.pop();
        assert!(admission.http("alice".into()).is_ok());
        drop(leases);
        drop(bob);
        assert!(admission.state.lock().unwrap().http.is_empty());
    }
}
