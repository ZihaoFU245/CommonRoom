//! Best-effort, bounded browser telemetry. This path never takes a database job.
use super::{App, auth, security};
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, Method, StatusCode, header},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    net::IpAddr,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

pub(super) const PATH: &str = "/api/drop";
pub(super) const BODY_BYTES: usize = 2048;
pub(super) const BODY_TIME: Duration = Duration::from_secs(2);
const GLOBAL_RATE: u32 = 32;
const SOURCE_RATE: u32 = 8;
const IN_FLIGHT: usize = 4;
const ACCOUNT_INTERVAL: Duration = Duration::from_secs(30);
const DEDUP_WINDOW: Duration = Duration::from_secs(600);
const MAX_RECENT: usize = 8192;
const MAX_COUNT: u32 = 256;
const MAX_BATCH_COUNT: u32 = 1024;
const BUCKETS: usize = 8;
const V1_SERIES: usize = 38;
const COHORTS: usize = 48;
type Counts = [[u64; BUCKETS]; SERIES.len()];

// The versioned wire indices and duration bounds match web/src/api/clientMetrics.ts.
const SERIES: [(&str, bool); 65] = [
    ("upload.prepare", true),
    ("upload.allocate", true),
    ("upload.put", true),
    ("upload.verify", true),
    ("upload.ready", true),
    ("upload.failed_prepare", false),
    ("upload.failed_allocate", false),
    ("upload.failed_put", false),
    ("upload.failed_verify", false),
    ("upload.cancelled", false),
    ("upload.size_small", false),
    ("upload.size_medium", false),
    ("upload.size_large", false),
    ("chat.startup", true),
    ("chat.reconnect", true),
    ("chat.handshake", true),
    ("chat.sync", true),
    ("message.ack", true),
    ("message.failed", false),
    ("message.interrupted", false),
    ("history.reused", true),
    ("history.fetched", true),
    ("media.bytes_reused", true),
    ("media.bytes_fetched", true),
    ("media.lease_reused", false),
    ("media.lease_resolved", false),
    ("media_config.reused", false),
    ("media_config.joined", false),
    ("media_config.fetched", false),
    ("media.lease_joined", false),
    ("media.preview", true),
    ("media.preview_failed", false),
    ("history.failed", false),
    ("media_status.reused", false),
    ("media_status.joined", false),
    ("media_status.fetched", false),
    ("library.joined", false),
    ("library.fetched", false),
    ("first.chat", true),
    ("first.media", true),
    ("first.sticker", true),
    ("first.send", true),
    ("first.upload", true),
    ("error.render", false),
    ("error.script", false),
    ("error.promise", false),
    ("error.bootstrap", false),
    ("error.api_network", false),
    ("error.api_timeout", false),
    ("error.api_invalid_response", false),
    ("error.socket_failed", false),
    ("socket.disconnected", false),
    ("socket.resumed", false),
    ("socket.resynced", false),
    ("error.media_worker", false),
    ("error.sticker", false),
    ("error.playback", false),
    ("media.worker_ready", true),
    ("first.signed_in_chat", true),
    ("error.api_rejected", false),
    ("error.api_server", false),
    ("error.socket_invalid", false),
    ("media.playback_ready", true),
    ("error.upload_rejected", false),
    ("browser.resize_notice", false),
];
const LIMITS_MS: [u64; BUCKETS - 1] = [10, 50, 100, 250, 1_000, 5_000, 30_000];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Batch {
    v: u8,
    id: u64,
    rows: Vec<(usize, Vec<u16>)>,
    c: Option<u8>,
}
impl Batch {
    fn valid(&self) -> bool {
        let series_limit = match (self.v, self.c) {
            (1, None) => V1_SERIES,
            (2, Some(c)) if usize::from(c) < COHORTS => SERIES.len(),
            _ => return false,
        };
        self.id != 0
            && !self.rows.is_empty()
            && self.rows.len() <= series_limit
            && self.rows.windows(2).all(|pair| pair[0].0 < pair[1].0)
            && self.rows.iter().all(|(index, counts)| {
                SERIES[..series_limit]
                    .get(*index)
                    .is_some_and(|(_, timed)| {
                        counts.len() == if *timed { BUCKETS } else { 1 }
                            && (1..=MAX_COUNT)
                                .contains(&counts.iter().copied().map(u32::from).sum())
                    })
            })
            && self
                .rows
                .iter()
                .flat_map(|(_, counts)| counts)
                .copied()
                .map(u32::from)
                .sum::<u32>()
                <= MAX_BATCH_COUNT
    }
}

struct Recent {
    time: Instant,
    account: String,
    id: u64,
}
#[derive(Clone, Copy)]
struct CohortTotals {
    counts: Counts,
    batches: u64,
}

struct Totals {
    counts: Counts,
    cohorts: [Option<Box<CohortTotals>>; COHORTS],
    newest: HashMap<String, Instant>,
    ids: std::collections::HashSet<(String, u64)>,
    recent: VecDeque<Recent>,
    accepted: u64,
    duplicates: u64,
    throttled: u64,
}
impl Default for Totals {
    fn default() -> Self {
        Self {
            counts: [[0; BUCKETS]; SERIES.len()],
            cohorts: std::array::from_fn(|_| None),
            newest: HashMap::new(),
            ids: std::collections::HashSet::new(),
            recent: VecDeque::new(),
            accepted: 0,
            duplicates: 0,
            throttled: 0,
        }
    }
}

impl Totals {
    fn accept(&mut self, account: &str, batch: Batch, now: Instant) -> StatusCode {
        while self
            .recent
            .front()
            .is_some_and(|entry| now.duration_since(entry.time) >= DEDUP_WINDOW)
        {
            let Some(Recent { time, account, id }) = self.recent.pop_front() else {
                break;
            };
            if self.newest.get(&account) == Some(&time) {
                self.newest.remove(&account);
            }
            self.ids.remove(&(account, id));
        }
        let key = (account.to_owned(), batch.id);
        if self.ids.contains(&key) {
            self.duplicates += 1;
            return StatusCode::NO_CONTENT;
        }
        if self.recent.len() >= MAX_RECENT
            || self
                .newest
                .get(account)
                .is_some_and(|previous| now.duration_since(*previous) < ACCOUNT_INTERVAL)
        {
            self.throttled += 1;
            return StatusCode::TOO_MANY_REQUESTS;
        }
        self.ids.insert(key);
        self.newest.insert(account.to_owned(), now);
        self.recent.push_back(Recent {
            time: now,
            account: account.to_owned(),
            id: batch.id,
        });
        let cohort =
            self.cohorts[usize::from(batch.c.unwrap_or_default())].get_or_insert_with(|| {
                Box::new(CohortTotals {
                    counts: [[0; BUCKETS]; SERIES.len()],
                    batches: 0,
                })
            });
        cohort.batches = cohort.batches.saturating_add(1);
        for (index, counts) in batch.rows {
            for (bucket, count) in counts.into_iter().enumerate() {
                let count = u64::from(count);
                let overall = &mut self.counts[index][bucket];
                *overall = overall.saturating_add(count);
                let grouped = &mut cohort.counts[index][bucket];
                *grouped = grouped.saturating_add(count);
            }
        }
        self.accepted += 1;
        StatusCode::NO_CONTENT
    }
}

struct Window {
    start: Instant,
    requests: u32,
    sources: HashMap<IpAddr, u32>,
}

pub(crate) struct ClientMetrics {
    ingress: Mutex<Window>,
    in_flight: tokio::sync::Semaphore,
    ingress_rejections: AtomicU64,
    totals: Mutex<Totals>,
}
impl Default for ClientMetrics {
    fn default() -> Self {
        Self {
            ingress: Mutex::new(Window {
                start: Instant::now(),
                requests: 0,
                sources: HashMap::new(),
            }),
            in_flight: tokio::sync::Semaphore::new(IN_FLIGHT),
            ingress_rejections: AtomicU64::new(0),
            totals: Mutex::default(),
        }
    }
}

impl ClientMetrics {
    pub(super) fn admit(&self, source: Option<IpAddr>) -> Option<tokio::sync::SemaphorePermit<'_>> {
        let permit = self.in_flight.try_acquire().ok();
        if permit.is_some()
            && let Ok(mut ingress) = self.ingress.try_lock()
        {
            let now = Instant::now();
            if now.duration_since(ingress.start) >= Duration::from_secs(1) {
                ingress.start = now;
                ingress.requests = 0;
                ingress.sources.clear();
            }
            if ingress.requests < GLOBAL_RATE {
                let source_allowed = source.is_none_or(|source| {
                    let count = ingress.sources.entry(source).or_default();
                    if *count >= SOURCE_RATE {
                        return false;
                    }
                    *count += 1;
                    true
                });
                if source_allowed {
                    ingress.requests += 1;
                    return permit;
                }
            }
        }
        super::metrics::increment(&self.ingress_rejections);
        None
    }

    pub(crate) fn stats(&self) -> Value {
        let (accepted, duplicates, throttled, series, cohorts) = {
            let Ok(totals) = self.totals.lock() else {
                return Value::Null;
            };
            (
                totals.accepted,
                totals.duplicates,
                totals.throttled,
                totals.counts,
                totals
                    .cohorts
                    .iter()
                    .enumerate()
                    .filter_map(|(index, entry)| {
                        entry.as_deref().copied().map(|entry| (index, entry))
                    })
                    .collect::<Vec<_>>(),
            )
        };
        let mut stats = serde_json::Map::new();
        stats.insert("batches.accepted".into(), json!(accepted));
        stats.insert("batches.duplicate".into(), json!(duplicates));
        stats.insert(
            "batches.account_or_dedup_capacity_limited".into(),
            json!(throttled),
        );
        stats.insert(
            "batches.ingress_limited".into(),
            json!(self.ingress_rejections.load(Relaxed)),
        );
        for ((name, timed), counts) in SERIES.iter().zip(series) {
            let count = counts.iter().copied().fold(0u64, u64::saturating_add);
            stats.insert(format!("{name}.count"), json!(count));
            if *timed && count != 0 {
                let target = (u128::from(count) * 95).div_ceil(100);
                let mut seen = 0u128;
                let bucket = counts
                    .iter()
                    .position(|&n| {
                        seen += u128::from(n);
                        seen >= target
                    })
                    .unwrap_or(BUCKETS - 1);
                let bound = LIMITS_MS
                    .get(bucket)
                    .map_or_else(|| "> 30000 ms".to_owned(), |limit| format!("<= {limit} ms"));
                stats.insert(format!("{name}.p95_bound"), json!(bound));
                stats.insert(format!("{name}.buckets"), json!(counts));
            }
        }
        for (name, hits, misses) in [
            ("history", 20, &[21][..]),
            ("media.bytes", 22, &[23][..]),
            ("media.lease", 24, &[25, 29][..]),
            ("media_config", 26, &[27, 28][..]),
            ("media_status", 33, &[34, 35][..]),
            ("library", 36, &[37][..]),
        ] {
            let count = |index: usize| {
                series[index]
                    .iter()
                    .copied()
                    .fold(0u64, u64::saturating_add)
            };
            let hits = count(hits);
            let total = u128::from(hits)
                + misses
                    .iter()
                    .map(|&index| u128::from(count(index)))
                    .sum::<u128>();
            let rate = (u128::from(hits) * 10_000 + total / 2)
                .checked_div(total)
                .map_or_else(
                    || "n/a".into(),
                    |percent| format!("{}.{:02}%", percent / 100, percent % 100),
                );
            stats.insert(format!("{name}.reuse_rate"), json!(rate));
        }
        stats.insert(
            "summary".into(),
            json!({
                "milestones": milestones(&series, true),
                "timings": timings(&series),
                "errors": errors(&series),
                "clients": cohorts.into_iter().map(|(index, entry)| {
                    json!({
                        "name":cohort_name(index),
                        "batches":entry.batches,
                        "milestones":milestones(&entry.counts, false),
                        "errors":errors(&entry.counts)
                    })
                }).collect::<Vec<_>>()
            }),
        );
        Value::Object(stats)
    }
}

fn cohort_name(index: usize) -> String {
    let engine = ["Unknown engine", "Chromium", "WebKit", "Gecko"][index / 12];
    let device = ["Unknown device", "Desktop", "Phone", "Tablet"][(index / 3) % 4];
    let mode = ["Unknown app mode", "Browser tab", "Installed app"][index % 3];
    format!("{device} · {engine} · {mode}")
}

fn duration_row(label: &str, counts: &[u64; BUCKETS]) -> Value {
    let count = counts.iter().copied().fold(0, u64::saturating_add);
    let bound = |percent: u128| {
        if count == 0 {
            return "—".to_owned();
        }
        let target = (u128::from(count) * percent).div_ceil(100);
        let mut observed = 0u128;
        match counts
            .iter()
            .position(|&value| {
                observed += u128::from(value);
                observed >= target
            })
            .and_then(|index| LIMITS_MS.get(index))
        {
            Some(ms) if *ms >= 1_000 => format!("≤ {} s", ms / 1_000),
            Some(ms) => format!("≤ {ms} ms"),
            None => "> 30 s".to_owned(),
        }
    };
    json!([label, count, bound(50), bound(95)])
}

fn milestones(counts: &Counts, show_empty: bool) -> Vec<Value> {
    duration_rows(
        counts,
        &[
            (38, "First chat display"),
            (58, "First chat after sign-in"),
            (39, "First chat image displayed"),
            (40, "First sticker displayed"),
            (41, "First message acknowledged"),
            (42, "First attachment ready"),
        ],
        show_empty,
    )
}

fn timings(counts: &Counts) -> Vec<Value> {
    duration_rows(
        counts,
        &[
            (13, "Chat socket ready"),
            (15, "Socket handshake"),
            (16, "Initial socket sync"),
            (14, "Reconnect"),
            (17, "Message acknowledgement"),
            (20, "History from memory"),
            (21, "History fetched"),
            (22, "Media served from cache"),
            (23, "Media fetched"),
            (30, "Managed image load"),
            (57, "Media worker ready"),
            (62, "Play to media playback"),
            (0, "Prepare upload"),
            (1, "Allocate upload"),
            (2, "Transfer upload"),
            (3, "Verify upload"),
            (4, "Attachment ready"),
        ],
        false,
    )
}

fn duration_rows(counts: &Counts, labels: &[(usize, &str)], show_empty: bool) -> Vec<Value> {
    labels
        .iter()
        .filter(|(index, _)| show_empty || counts[*index].iter().any(|&count| count > 0))
        .map(|&(index, label)| duration_row(label, &counts[index]))
        .collect()
}

fn errors(counts: &Counts) -> Vec<Value> {
    [
        (43, "App render failure"),
        (44, "Uncaught script error"),
        (64, "Browser deferred resize notifications"),
        (45, "Unhandled promise rejection"),
        (46, "Chat bootstrap failure"),
        (47, "API network failure"),
        (48, "API timeout"),
        (59, "API request rejected (4xx except 401)"),
        (60, "API server error (5xx)"),
        (49, "Invalid API response"),
        (50, "Socket closed before ready"),
        (61, "Invalid socket message"),
        (18, "Message send failed"),
        (19, "Message send interrupted"),
        (32, "History load failed"),
        (54, "Media worker failed"),
        (31, "Managed image failed"),
        (55, "Sticker load failed"),
        (56, "Media playback failed"),
        (63, "Upload rejected for file size"),
        (5, "Upload preparation failed"),
        (6, "Upload allocation failed"),
        (7, "Upload transfer failed"),
        (8, "Upload verification failed"),
    ]
    .into_iter()
    .filter(|&(index, _)| counts[index][0] > 0)
    .map(|(index, label)| json!([label, counts[index][0]]))
    .collect()
}

pub(super) fn is_route(app: &App, path: &str) -> bool {
    path.strip_prefix(app.config.base_url.trim_end_matches('/')) == Some(PATH)
}

pub(super) fn is_drop(app: &App, method: &Method, path: &str) -> bool {
    *method == Method::POST && is_route(app, path)
}

pub(super) fn eligible(app: &App, headers: &HeaderMap) -> Result<(), StatusCode> {
    security::check_origin(app, headers).map_err(|_| StatusCode::FORBIDDEN)?;
    if app.config.client_metrics_sample_percent() == 0 {
        return Err(StatusCode::FORBIDDEN);
    }
    if auth::token(&app.config, headers).is_none() {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    if !matches!(content_type, Some("text/plain" | "application/json")) {
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    Ok(())
}

pub(super) async fn drop_batch(
    State(app): State<App>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    // Telemetry only changes unsaved counters. Decline on contention instead of
    // dispatching a blocking job or adding work to the authentication DB pool.
    let Ok(batch) = serde_json::from_slice::<Batch>(&body) else {
        return StatusCode::BAD_REQUEST;
    };
    if !batch.valid() {
        return StatusCode::BAD_REQUEST;
    }
    if app.stopping.load(Relaxed) {
        return StatusCode::SERVICE_UNAVAILABLE;
    }
    let Ok(mut totals) = app.admission.client.totals.try_lock() else {
        return StatusCode::TOO_MANY_REQUESTS;
    };
    let Ok(engine) = app.engine.try_read() else {
        return StatusCode::SERVICE_UNAVAILABLE;
    };
    if engine.ensure_memory().is_err() {
        return StatusCode::SERVICE_UNAVAILABLE;
    }
    let Some(user) = auth::identity(&engine, &app.config, &headers) else {
        return StatusCode::UNAUTHORIZED;
    };
    let Some(account) = engine.active_account_id(&user) else {
        return StatusCode::UNAUTHORIZED;
    };
    totals.accept(account, batch, Instant::now())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::web::{router, tests::login_test_app};
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    #[tokio::test]
    async fn drop_authenticates_bounds_and_deduplicates_batches() {
        let mut app = login_test_app();
        std::sync::Arc::make_mut(&mut app.config).base_url = "/chat/".into();
        let token = "metrics-session";
        {
            let mut engine = app.engine_api().unwrap();
            let hash = engine.password_hash("alice").unwrap().to_owned();
            engine.login(token.into(), "alice", &hash, None).unwrap();
        }
        let routes = router(app.clone());
        let post = |body: String, origin: &str, session: &str| {
            routes.clone().oneshot(
                Request::post("/chat/api/drop")
                    .header(header::ORIGIN, origin)
                    .header(header::COOKIE, format!("chat_session={session}"))
                    .header(header::CONTENT_TYPE, "text/plain;charset=UTF-8")
                    .body(Body::from(body))
                    .unwrap(),
            )
        };
        let origin = "http://localhost:5173";
        let batch = json!({"v":1,"id":42,"rows":[[20,[1,0,0,0,0,0,0,0]],[22,[2,0,0,0,0,0,0,0]],[23,[0,1,0,0,0,0,0,0]]]});
        assert_eq!(
            post(batch.to_string(), "https://example.invalid", token)
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            post(batch.to_string(), origin, "invalid")
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            post(" ".repeat(BODY_BYTES + 1), origin, token)
                .await
                .unwrap()
                .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        for rows in [
            json!([[5, [1]], [5, [1]]]),
            json!([[5, [257]]]),
            json!([[20, [1]]]),
            json!([[99, [1]]]),
            json!([[0, [256, 1, 0, 0, 0, 0, 0, 0]]]),
            json!([[5, [256]], [6, [256]], [7, [256]], [8, [256]], [9, [1]]]),
        ] {
            let invalid = json!({"v":1,"id":41,"rows":rows});
            assert_eq!(
                post(invalid.to_string(), origin, token)
                    .await
                    .unwrap()
                    .status(),
                StatusCode::BAD_REQUEST
            );
        }
        for _ in 0..2 {
            assert_eq!(
                post(batch.to_string(), origin, token)
                    .await
                    .unwrap()
                    .status(),
                StatusCode::NO_CONTENT
            );
        }
        let mut next = batch;
        next["id"] = json!(43);
        assert_eq!(
            post(next.to_string(), origin, token)
                .await
                .unwrap()
                .status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        let stats = app.admission.client.stats();
        assert_eq!(stats["batches.accepted"], 1);
        assert_eq!(stats["batches.duplicate"], 1);
        assert_eq!(stats["media.bytes.reuse_rate"], "66.67%");
        assert_eq!(stats["history.reused.p95_bound"], "<= 10 ms");
        assert!(
            app.admission.metrics.stats()["routes"]
                .as_object()
                .unwrap()
                .is_empty()
        );
        let report = crate::system::Command::Metrics.execute(&app).await.unwrap();
        assert!(report.contains("## Client metrics"));
        assert!(report.contains("| Media bytes | 2 | 3 | 66.67% |"));
        let report = crate::system::Command::MetricsText
            .execute(&app)
            .await
            .unwrap();
        assert!(report.contains("media.bytes_reused.count: 2"));

        app.engine_api().unwrap().logout(token).unwrap();
        assert_eq!(
            post(next.to_string(), origin, token)
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn v2_client_milestones_and_failures_render_with_cohort_and_samples() {
        let app = login_test_app();
        let service = router(app.clone());
        {
            let mut engine = app.engine_api().unwrap();
            let hash = engine.password_hash("alice").unwrap().to_owned();
            engine
                .login("client-v2".into(), "alice", &hash, None)
                .unwrap();
        }
        let rows = json!([
            [38, [0, 0, 1, 0, 1, 0, 0, 0]],
            [39, [0, 0, 0, 1, 0, 0, 0, 0]],
            [40, [0, 0, 0, 0, 0, 1, 0, 0]],
            [41, [0, 1, 0, 0, 0, 0, 0, 0]],
            [42, [0, 0, 0, 0, 1, 0, 0, 0]],
            [48, [3]],
            [56, [1]]
        ]);
        let post = |body: Value| {
            service.clone().oneshot(
                Request::post("/api/drop")
                    .header(header::ORIGIN, "http://localhost:5173")
                    .header(header::COOKIE, "chat_session=client-v2")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
        };
        for invalid in [
            json!({"v":1,"id":20,"rows":rows}),
            json!({"v":2,"id":20,"rows":rows}),
            json!({"v":2,"id":20,"c":48,"rows":rows}),
            json!({"v":2,"id":20,"c":31,"rows":rows,"user_agent":"must be rejected"}),
        ] {
            assert_eq!(
                post(invalid).await.unwrap().status(),
                StatusCode::BAD_REQUEST
            );
        }
        let valid = json!({"v":2,"id":20,"c":31,"rows":rows});
        assert_eq!(
            post(valid.clone()).await.unwrap().status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(post(valid).await.unwrap().status(), StatusCode::NO_CONTENT);
        let report = crate::system::Command::Metrics.execute(&app).await.unwrap();
        assert!(report.contains("| First chat display | 2 | ≤ 100 ms | ≤ 1 s |"));
        assert!(report.contains("| First sticker displayed | 1 | ≤ 5 s | ≤ 5 s |"));
        assert!(report.contains("| Phone · WebKit · Browser tab | API timeout | 3 |"));
        assert!(report.contains(
            "| Phone · WebKit · Browser tab | First attachment ready | 1 | ≤ 1 s | ≤ 1 s |"
        ));
        assert!(report.contains("| Duplicate batches ignored | 1 |"));
        let plain = crate::system::Command::MetricsText
            .execute(&app)
            .await
            .unwrap();
        assert!(plain.contains("Phone · WebKit · Browser tab  ·  Media playback failed  ·  1"));
        assert!(plain.contains("first.chat.buckets[2]: 1"));
    }

    #[test]
    fn zero_percent_disables_batch_intake() {
        let mut app = login_test_app();
        std::sync::Arc::make_mut(&mut app.config)
            .server
            .client_metrics_sample_percent = Some(0);
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, "http://localhost:5173".parse().unwrap());
        assert_eq!(eligible(&app, &headers), Err(StatusCode::FORBIDDEN));
    }

    #[test]
    fn ingress_has_independent_concurrency_and_rate_caps() {
        let metrics = ClientMetrics::default();
        let permits: Vec<_> = (0..IN_FLIGHT)
            .map(|_| metrics.admit(None).unwrap())
            .collect();
        assert!(metrics.admit(None).is_none());
        drop(permits);
        for _ in u32::try_from(IN_FLIGHT).unwrap()..GLOBAL_RATE {
            drop(metrics.admit(None).unwrap());
        }
        assert!(metrics.admit(None).is_none());
        metrics.ingress.lock().unwrap().start -= Duration::from_secs(1);
        let peer = "192.0.2.10".parse().unwrap();
        for _ in 0..SOURCE_RATE {
            assert!(metrics.admit(Some(peer)).is_some());
        }
        assert!(metrics.admit(Some(peer)).is_none());
        assert!(metrics.admit(Some("192.0.2.11".parse().unwrap())).is_some());
    }

    #[test]
    fn dedup_window_survives_intervening_batches_and_releases_state() {
        let mut totals = Totals::default();
        let batch = |id| Batch {
            v: 1,
            id,
            c: None,
            rows: vec![(22, vec![1, 0, 0, 0, 0, 0, 0, 0])],
        };
        let now = Instant::now();
        assert_eq!(
            totals.accept("account", batch(1), now),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            totals.accept("account", batch(2), now + ACCOUNT_INTERVAL),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            totals.accept("account", batch(1), now + 2 * ACCOUNT_INTERVAL),
            StatusCode::NO_CONTENT
        );
        assert_eq!(totals.counts[22][0], 2);
        assert_eq!(
            totals.accept("other", batch(3), now + 2 * DEDUP_WINDOW),
            StatusCode::NO_CONTENT
        );
        assert_eq!(totals.ids.len(), 1);
        assert_eq!(totals.newest.len(), 1);
        assert_eq!(totals.recent.len(), 1);
    }
}
