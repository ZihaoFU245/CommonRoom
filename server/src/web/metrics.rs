//! Bounded operational counters; no client identifiers, raw URLs or message bodies.
use serde_json::{Value, json};
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicU64, Ordering},
};

#[derive(Default)]
pub(crate) struct Metrics {
    pub accepted_connections: AtomicU64,
    pub active_connections: AtomicU64,
    pub peak_connections: AtomicU64,
    pub connection_errors: AtomicU64,
    pub header_timeouts: AtomicU64,
    pub write_timeouts: AtomicU64,
    pub body_timeouts: AtomicU64,
    pub body_rejections: AtomicU64,
    pub request_timeouts: AtomicU64,
    pub account_rejections: AtomicU64,
    pub io_rejections: AtomicU64,
    pub io_accepted: AtomicU64,
    pub io_active: AtomicU64,
    pub io_peak: AtomicU64,
    pub password_rejections: AtomicU64,
    pub socket_rejections: AtomicU64,
    pub socket_replacements: AtomicU64,
    requests: OnceLock<std::collections::BTreeMap<String, Arc<Route>>>,
    unmatched: Arc<Route>,
}
#[derive(Default)]
struct Route {
    active: AtomicU64,
    completed: AtomicU64,
    cancelled: AtomicU64,
    client_errors: AtomicU64,
    server_errors: AtomicU64,
    total_us: AtomicU64,
    max_us: AtomicU64,
    // Non-cumulative buckets: <=10ms, <=50ms, <=250ms, <=1s, <=5s, >5s.
    duration_buckets: [AtomicU64; 6],
}
impl Route {
    fn snapshot(&self) -> Option<Value> {
        let active = self.active.load(Ordering::Relaxed);
        let completed = self.completed.load(Ordering::Relaxed);
        let cancelled = self.cancelled.load(Ordering::Relaxed);
        if active == 0 && completed == 0 && cancelled == 0 {
            return None;
        }
        Some(
            json!({"active":active,"completed":completed,"cancelled":cancelled,
            "client_errors":self.client_errors.load(Ordering::Relaxed),
            "server_errors":self.server_errors.load(Ordering::Relaxed),
            "total_us":self.total_us.load(Ordering::Relaxed),
            "max_us":self.max_us.load(Ordering::Relaxed),
            "duration_buckets":self.duration_buckets.each_ref().map(|bucket| bucket.load(Ordering::Relaxed))}),
        )
    }
}

/// Built alongside the actual router, then frozen before it serves traffic.
/// Returning the original path keeps metrics and routing in one declaration.
pub(crate) struct Registry {
    base: String,
    routes: std::collections::BTreeMap<String, Arc<Route>>,
}
impl Registry {
    pub fn new(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_owned(),
            routes: Default::default(),
        }
    }
    pub fn register(&mut self, prefix: &str, path: &'static str) -> &'static str {
        self.routes
            .entry(format!("{}{prefix}{path}", self.base))
            .or_default();
        path
    }
}
pub(crate) fn increment(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}
fn add_duration(counter: &AtomicU64, duration_us: u64) {
    let mut current = counter.load(Ordering::Relaxed);
    loop {
        match counter.compare_exchange_weak(
            current,
            current.saturating_add(duration_us),
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return,
            Err(updated) => current = updated,
        }
    }
}
pub(crate) struct Connection(Arc<Metrics>);
impl Metrics {
    pub fn connection(self: &Arc<Self>) -> Connection {
        increment(&self.accepted_connections);
        let active = self.active_connections.fetch_add(1, Ordering::Relaxed) + 1;
        self.peak_connections.fetch_max(active, Ordering::Relaxed);
        Connection(self.clone())
    }
    pub fn register_routes(&self, registry: Registry) {
        // Router clones/rebuilds with the same App retain their counters. The
        // production route set and base path are fixed for this App's lifetime.
        self.requests.get_or_init(|| registry.routes);
    }
    pub fn request(&self, route: &str) -> Request {
        let route = self
            .requests
            .get()
            .and_then(|routes| routes.get(route))
            .unwrap_or(&self.unmatched)
            .clone();
        increment(&route.active);
        Request {
            route,
            start: std::time::Instant::now(),
            status: None,
        }
    }
    pub fn stats(&self) -> Value {
        let mut routes = self
            .requests
            .get()
            .into_iter()
            .flat_map(|routes| routes.iter())
            .filter_map(|(name, route)| route.snapshot().map(|snapshot| (name.clone(), snapshot)))
            .collect::<serde_json::Map<_, _>>();
        if let Some(snapshot) = self.unmatched.snapshot() {
            routes.insert("unmatched".into(), snapshot);
        }
        json!({
            "connections": {"accepted": self.accepted_connections.load(Ordering::Relaxed),
                "active":self.active_connections.load(Ordering::Relaxed),
                "peak":self.peak_connections.load(Ordering::Relaxed),
                "errors":self.connection_errors.load(Ordering::Relaxed),
                "header_timeouts":self.header_timeouts.load(Ordering::Relaxed),
                "write_timeouts":self.write_timeouts.load(Ordering::Relaxed)},
            "body_timeouts":self.body_timeouts.load(Ordering::Relaxed),
            "body_rejections":self.body_rejections.load(Ordering::Relaxed),
            "request_timeouts":self.request_timeouts.load(Ordering::Relaxed),
            "rejections":{"account":self.account_rejections.load(Ordering::Relaxed),
                "io":self.io_rejections.load(Ordering::Relaxed),
                "password":self.password_rejections.load(Ordering::Relaxed),
                "socket":self.socket_rejections.load(Ordering::Relaxed)},
            "socket_replacements":self.socket_replacements.load(Ordering::Relaxed),
            "io_jobs":{"accepted":self.io_accepted.load(Ordering::Relaxed),
                "active":self.io_active.load(Ordering::Relaxed),"peak":self.io_peak.load(Ordering::Relaxed)},
            "routes":routes
        })
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.0.active_connections.fetch_sub(1, Ordering::Relaxed);
    }
}
pub(crate) struct Request {
    route: Arc<Route>,
    start: std::time::Instant,
    pub status: Option<axum::http::StatusCode>,
}
impl Drop for Request {
    fn drop(&mut self) {
        let route = &self.route;
        if let Some(status) = self.status {
            if status.is_client_error() {
                increment(&route.client_errors);
            }
            if status.is_server_error() {
                increment(&route.server_errors);
            }
            let us = u64::try_from(self.start.elapsed().as_micros()).unwrap_or(u64::MAX);
            add_duration(&route.total_us, us);
            route.max_us.fetch_max(us, Ordering::Relaxed);
            let bucket = [10_000, 50_000, 250_000, 1_000_000, 5_000_000]
                .iter()
                .position(|limit| us <= *limit)
                .unwrap_or(5);
            increment(&route.duration_buckets[bucket]);
            increment(&route.completed);
        } else {
            increment(&route.cancelled);
        }
        route.active.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Observe ingress/security failures as well as completed handler responses.
pub(crate) async fn observe(
    axum::extract::State(app): axum::extract::State<super::App>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let metrics = &app.admission.metrics;
    let route = request
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map_or("unmatched", |path| path.as_str());
    let mut measurement = metrics.request(route);
    let response = next.run(request).await;
    measurement.status = Some(response.status());
    response
}

/// Bound body reception absolutely, including clients that trickle bytes forever.
/// Route labels come from Axum's static route patterns, never raw request URLs.
pub(crate) async fn http(
    axum::extract::State(app): axum::extract::State<super::App>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::{http::StatusCode, response::IntoResponse};
    use std::time::Duration;
    let metrics = &app.admission.metrics;
    let bot_base = format!("{}api/bot/v1", app.config.base_url);
    let bot =
        request.uri().path() == bot_base || request.uri().path().starts_with(&(bot_base + "/"));
    let body_limit = if bot {
        crate::limits::BOT_BODY_BYTES
    } else {
        crate::limits::BROWSER_BODY_BYTES
    };
    let result = tokio::time::timeout(
        Duration::from_secs(app.config.server.request_timeout_seconds),
        async {
            let (parts, body) = request.into_parts();
            let body = match tokio::time::timeout(
                Duration::from_secs(app.config.server.body_timeout_seconds),
                axum::body::to_bytes(body, body_limit),
            )
            .await
            {
                Err(_) => {
                    increment(&metrics.body_timeouts);
                    let mut response = super::error(
                        StatusCode::REQUEST_TIMEOUT,
                        "Request body deadline exceeded.",
                    )
                    .into_response();
                    response.headers_mut().insert(
                        axum::http::header::CONNECTION,
                        axum::http::HeaderValue::from_static("close"),
                    );
                    return response;
                }
                Ok(Err(error)) => {
                    use std::error::Error;
                    let too_large = error
                        .source()
                        .is_some_and(|source| source.is::<http_body_util::LengthLimitError>());
                    increment(&metrics.body_rejections);
                    let status = if too_large {
                        StatusCode::PAYLOAD_TOO_LARGE
                    } else {
                        StatusCode::BAD_REQUEST
                    };
                    let mut response =
                        super::error(status, "Request body rejected.").into_response();
                    response.headers_mut().insert(
                        axum::http::header::CONNECTION,
                        axum::http::HeaderValue::from_static("close"),
                    );
                    return response;
                }
                Ok(Ok(body)) => body,
            };
            next.run(axum::extract::Request::from_parts(
                parts,
                axum::body::Body::from(body),
            ))
            .await
        },
    )
    .await;
    result.unwrap_or_else(|_| {
        increment(&metrics.request_timeouts);
        super::error(StatusCode::REQUEST_TIMEOUT, "Request deadline exceeded.").into_response()
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
