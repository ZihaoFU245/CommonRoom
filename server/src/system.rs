//! Grant-authorized operational reports. Collection uses existing counters and locks;
//! Markdown formatting runs only on demand, outside the engine guard.
use crate::{engine::EngineError, web::App};
use serde_json::{Value, json};

/// Receipts retain the authority used to issue the original report. Replay
/// cannot silently promote a plain command to sudo or bypass later revocation.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ReportAccess {
    // Preserve the stricter authority of receipts issued before global grants were supported.
    Su,
    Granted,
    Sudo,
}
impl ReportAccess {
    pub(crate) fn authorize(
        self,
        engine: &crate::engine::Engine,
        user: &str,
    ) -> Result<(), EngineError> {
        use crate::engine::authorization::{Action, Scope};
        match self {
            Self::Granted => engine.require_target_command(Some(user), &Scope::Server, "/system"),
            Self::Su if engine.is_su(Some(user)) => Ok(()),
            Self::Su => Err(EngineError::Forbidden(
                "Only su can read this system report.".into(),
            )),
            Self::Sudo => {
                engine.require_command(Some(user), None, "/sudo")?;
                engine.require_target_command(Some(user), &Scope::Server, "/sudo")?;
                engine.require(Some(user), &Scope::Server, Action::Sudo)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Help,
    Configs,
    Metrics,
    MetricsText,
}
impl Command {
    pub(crate) fn parse(input: &str) -> Result<Self, EngineError> {
        match input.split_whitespace().collect::<Vec<_>>().as_slice() {
            ["/system"] => Ok(Self::Help),
            ["/system", "configs"] => Ok(Self::Configs),
            ["/system", "metrics"] => Ok(Self::Metrics),
            ["/system", "metrics", "--txt"] => Ok(Self::MetricsText),
            _ => Err("Usage: /system [configs|metrics [--txt]]".into()),
        }
    }
    pub(crate) async fn execute(self, app: &App) -> Result<String, EngineError> {
        let history = app
            .inspect_engine(|engine, _| Ok::<_, EngineError>(engine.history_store().clone()))
            .await?;
        history.ensure_available_async().await?;
        match self {
            Self::Help => Ok("# System\n\n- `/system configs` — active configuration; changes require restart.\n- `/system metrics` — cumulative cache and database metrics since startup; add `--txt` for a shareable text code block.\n\nRequires a global `/system` command grant (included for su). Stdin and authorized `/sudo` are supported.".into()),
            Self::Configs => Ok(format!("# System configuration\n\nSettings take effect after restart. Credentials and database URLs remain environment-only.\n\n```json\n{}\n```", app.config.display().map_err(EngineError::Invalid)?)),
            Self::Metrics | Self::MetricsText => {
                let mut cache = history.stats_async().await?;
                let pools = cache.as_object_mut().and_then(|value| value.remove("pools")).unwrap_or(Value::Null);
                let snapshots = app.snapshots.lock().map_err(|_| EngineError::unavailable("Snapshot cache poisoned"))?.stats();
                let runtime = json!({"io_jobs_available":app.io_jobs.available_permits(),
                    "io_jobs_limit":app.config.server.io_jobs,
                    "password_jobs_available":app.password_jobs.available_permits(),
                    "password_jobs_limit":app.config.server.password_jobs,
                    "max_http_connections":app.config.server.max_http_connections,
                    "collected_at_unix_seconds":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs(),
                    "admission":app.admission.stats()?, "ordering":app.mutations.stats()});
                Ok(if self == Self::MetricsText {
                    render_metrics_text(&cache, &pools, &snapshots, &runtime)
                } else {
                    render_metrics(&cache, &pools, &snapshots, &runtime)
                })
            }
        }
    }
}

fn number(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}
fn ratio(hits: u64, misses: u64) -> String {
    let total = u128::from(hits) + u128::from(misses);
    if total == 0 {
        return "n/a (no lookups)".into();
    }
    let percent = (u128::from(hits) * 10_000 + total / 2) / total;
    format!("{}.{:02}%", percent / 100, percent % 100)
}
fn section(text: &mut String, title: &str, value: &Value, plain: bool) {
    if plain {
        text.push_str(&format!("\n\n{title}\n"));
    } else {
        text.push_str(&format!(
            "\n\n## {title}\n\n| Metric | Value |\n| --- | ---: |"
        ));
    }
    rows(text, "", value, plain);
}
fn rows(text: &mut String, prefix: &str, value: &Value, plain: bool) {
    match value {
        Value::Object(values) => {
            for (key, value) in values {
                rows(
                    text,
                    &if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    },
                    value,
                    plain,
                );
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                rows(text, &format!("{prefix}[{index}]"), value, plain);
            }
        }
        Value::Null => {}
        value if plain => text.push_str(&format!(
            "\n{prefix}: {}",
            value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string())
        )),
        value => text.push_str(&format!(
            "\n| `{prefix}` | {} |",
            value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string())
        )),
    }
}
// Derived values are computed only while rendering a report, never on request paths.
fn percentage(part: u64, total: u64) -> String {
    if total == 0 {
        return "n/a".into();
    }
    let value = (u128::from(part) * 10_000 + u128::from(total) / 2) / u128::from(total);
    format!("{}.{:02}%", value / 100, value % 100)
}
fn utilization(used: u64, limit: u64) -> String {
    format!("{} ({used} / {limit})", percentage(used, limit))
}
fn milliseconds(us: u64) -> String {
    format!("{:.3} ms", us as f64 / 1000.0)
}
fn maximum_ms(us: u64, samples: u64) -> String {
    if samples == 0 {
        "n/a".into()
    } else {
        milliseconds(us)
    }
}
fn average_ms(us: u64, count: u64) -> String {
    if count == 0 {
        "n/a".into()
    } else {
        format!("{:.3} ms", us as f64 / count as f64 / 1000.0)
    }
}
fn highest_shard_utilization(cache: &Value, used: &str, limit: &str) -> String {
    cache["shard_stats"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|shard| {
            let limit = number(shard, limit);
            (limit != 0).then_some((number(shard, used), limit))
        })
        .max_by(|(a, al), (b, bl)| {
            (u128::from(*a) * u128::from(*bl)).cmp(&(u128::from(*b) * u128::from(*al)))
        })
        .map_or_else(|| "n/a".into(), |(used, limit)| utilization(used, limit))
}
fn bucket_counts(route: &Value) -> Vec<u64> {
    (0..9)
        .map(|i| route["duration_buckets"][i].as_u64().unwrap_or(0))
        .collect()
}
fn aggregate_routes(routes: &Value) -> Value {
    let mut result = json!({"duration_buckets":vec![0u64;9]});
    if let Some(routes) = routes.as_object() {
        for route in routes.values() {
            for key in [
                "completed",
                "cancelled",
                "client_errors",
                "server_errors",
                "total_us",
            ] {
                result[key] = json!(number(&result, key).saturating_add(number(route, key)));
            }
            result["max_us"] = json!(number(&result, "max_us").max(number(route, "max_us")));
            for (index, count) in bucket_counts(route).iter().enumerate() {
                result["duration_buckets"][index] = json!(
                    result["duration_buckets"][index]
                        .as_u64()
                        .unwrap_or(0)
                        .saturating_add(*count)
                );
            }
        }
    }
    result
}
fn p95_bound(route: &Value) -> String {
    let buckets = bucket_counts(route);
    let total: u128 = buckets.iter().map(|n| u128::from(*n)).sum();
    if total == 0 {
        return "n/a".into();
    }
    let target = (total * 95).div_ceil(100);
    let mut seen = 0u128;
    for (index, count) in buckets.iter().enumerate() {
        seen += u128::from(*count);
        if seen >= target {
            return crate::web::metrics::DURATION_LIMITS_US
                .get(index)
                .map_or_else(|| ">5 s".into(), |us| format!("<= {}", milliseconds(*us)));
        }
    }
    "n/a".into()
}
fn render_routes(text: &mut String, routes: &Value, plain: bool) {
    if plain {
        text.push_str("\n\nHTTP route summary\n");
    } else {
        text.push_str("\n\n## HTTP route summary\n\n| Route | Completed | 4xx | 5xx | Cancelled | Average | p95 bound | Maximum |\n| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
    }
    let overall = aggregate_routes(routes);
    let mut entries = vec![("All routes", &overall)];
    if let Some(routes) = routes.as_object() {
        entries.extend(routes.iter().map(|(name, route)| (name.as_str(), route)));
    }
    for (name, route) in entries {
        let completed = number(route, "completed");
        let cancelled = number(route, "cancelled");
        let fields = [
            completed.to_string(),
            percentage(number(route, "client_errors"), completed),
            percentage(number(route, "server_errors"), completed),
            percentage(cancelled, completed.saturating_add(cancelled)),
            average_ms(number(route, "total_us"), completed),
            p95_bound(route),
            maximum_ms(number(route, "max_us"), completed),
        ];
        if plain {
            text.push_str(&format!("\n{name}: completed={}, 4xx={}, 5xx={}, cancelled={}, average={}, p95 bound={}, maximum={}",fields[0],fields[1],fields[2],fields[3],fields[4],fields[5],fields[6]));
        } else {
            text.push_str(&format!("\n| `{name}` | {} |", fields.join(" | ")));
        }
    }
}

fn render_metrics(cache: &Value, pools: &Value, snapshots: &Value, runtime: &Value) -> String {
    render_report(cache, pools, snapshots, runtime, false)
}
fn render_report(
    cache: &Value,
    pools: &Value,
    snapshots: &Value,
    runtime: &Value,
    plain: bool,
) -> String {
    let heading = if plain { "" } else { "# " };
    let emphasis = if plain { "" } else { "**" };
    let mut text = format!(
        "{heading}System metrics\n\nCumulative totals since server startup (uptime: {} seconds). Counters are approximate concurrent observations. Cache bytes are accounting budgets, not total process memory.\n\n- History page hit rate: {emphasis}{}{emphasis}. Coalesced waiters count as hits.\n- Unread cache hit rate: {emphasis}{}{emphasis}. Full/incremental counts describe database calculations after misses.\n- Snapshot reuse rate: {emphasis}{}{emphasis}. Reuse includes callers sharing an in-progress build.",
        number(cache, "uptime_seconds"),
        ratio(number(cache, "hits"), number(cache, "misses")),
        ratio(number(cache, "unread_hits"), number(cache, "unread_misses")),
        ratio(number(snapshots, "hits"), number(snapshots, "misses"))
    );
    let routes = &runtime["admission"]["http"]["routes"];
    let overall = aggregate_routes(routes);
    let completed = number(&overall, "completed");
    let client_errors = number(&overall, "client_errors");
    let server_errors = number(&overall, "server_errors");
    let cancelled = number(&overall, "cancelled");
    let successful = completed.saturating_sub(client_errors.saturating_add(server_errors));
    let buckets = bucket_counts(&overall);
    text.push_str(&format!("\n- HTTP responses: {emphasis}{}{emphasis} successful (non-4xx/5xx), {} client errors, {} server errors; {} cancelled among finished attempts.\n- Backend responses within 10 ms: {emphasis}{}{emphasis}; within 1 s: {emphasis}{}{emphasis}. {} completed responses.",
        percentage(successful, completed), percentage(client_errors, completed), percentage(server_errors, completed),
        percentage(cancelled, completed.saturating_add(cancelled)),
        percentage(buckets.iter().take(4).sum(), buckets.iter().sum()),
        percentage(buckets.iter().take(7).sum(), buckets.iter().sum()), completed));
    let mut usage = json!({
        "history.bytes":utilization(number(cache,"accounted_bytes"),number(cache,"byte_limit")),
        "history.messages":utilization(number(cache,"messages"),number(cache,"message_limit")),
        "history.pages":utilization(number(cache,"pages"),number(cache,"page_limit")),
        "unread.entries":utilization(number(cache,"unread_entries"),number(cache,"unread_entry_limit")),
        "snapshot.bytes":utilization(number(snapshots,"bytes"),number(snapshots,"byte_limit")),
        "snapshot.entries":utilization(number(snapshots,"entries"),number(snapshots,"entry_limit")),
        "io_jobs":utilization(number(runtime,"io_jobs_limit").saturating_sub(number(runtime,"io_jobs_available")),number(runtime,"io_jobs_limit")),
        "password_jobs":utilization(number(runtime,"password_jobs_limit").saturating_sub(number(runtime,"password_jobs_available")),number(runtime,"password_jobs_limit")),
        "http.connections":utilization(number(&runtime["admission"]["http"]["connections"],"active"),number(runtime,"max_http_connections"))
    });
    let admission = &runtime["admission"];
    for profile in ["browser", "bot"] {
        let limit = number(admission, &format!("{profile}_sockets_limit"));
        let available = number(admission, &format!("{profile}_sockets_available"));
        usage[format!("{profile}.sockets")] =
            json!(utilization(limit.saturating_sub(available), limit));
    }
    usage["http.connections_peak"] = json!(utilization(
        number(&admission["http"]["connections"], "peak"),
        number(runtime, "max_http_connections")
    ));
    usage["io_jobs_peak"] = json!(utilization(
        number(&admission["http"]["io_jobs"], "peak"),
        number(runtime, "io_jobs_limit")
    ));
    for (used, limit) in [
        ("bytes", "byte_limit"),
        ("messages", "message_limit"),
        ("pages", "page_limit"),
    ] {
        usage[format!("history.busiest_shard.{used}")] =
            json!(highest_shard_utilization(cache, used, limit));
    }
    section(&mut text, "Capacity utilization", &usage, plain);
    let events = &cache["events"];
    let mut behavior = json!({
        "Unread incremental calculations":ratio(number(cache,"unread_incremental"),number(cache,"unread_full")),
        "History byte evictions":number(events,"byte_evictions"),
        "History message evictions":number(events,"message_evictions"),
        "History page evictions":number(events,"page_evictions"),
        "History per-conversation evictions":number(events,"conversation_evictions"),
        "History oversized skips":number(events,"oversized_skips"),
        "Unread evictions":number(cache,"unread_evictions"),
        "Snapshot evictions":number(snapshots,"evictions"),
        "Snapshot incomplete builds":percentage(number(snapshots,"incomplete"),number(snapshots,"builds"))
    });
    for reason in ["cold_misses", "revision_misses", "range_misses"] {
        behavior[format!("History miss share.{reason}")] =
            json!(percentage(number(events, reason), number(cache, "misses")));
    }
    section(&mut text, "Cache behavior", &behavior, plain);
    let http = &runtime["admission"]["http"];
    let rejections = &http["rejections"];
    let mut reliability = json!({
        "HTTP request timeouts":number(http,"request_timeouts"),
        "HTTP body timeouts":number(http,"body_timeouts"),
        "HTTP header timeouts":number(&http["connections"],"header_timeouts"),
        "HTTP write timeouts":number(&http["connections"],"write_timeouts"),
        "Admission rejections (account / IO / password / socket)":format!("{} / {} / {} / {}",number(rejections,"account"),number(rejections,"io"),number(rejections,"password"),number(rejections,"socket")),
        "Ordering timeouts":number(&runtime["ordering"],"timeouts"),
        "Snapshot build success":percentage(number(snapshots,"builds").saturating_sub(number(snapshots,"failures")),number(snapshots,"builds"))
    });
    for operation in ["history_reads", "transactions", "cursor_advances"] {
        let counts = &pools["operations"][operation];
        reliability[format!("Database {operation} success")] =
            json!(ratio(number(counts, "succeeded"), number(counts, "failed")));
    }
    section(&mut text, "Reliability", &reliability, plain);
    let mut pool_summary = serde_json::Map::new();
    for name in ["readers", "writers"] {
        let pool = &pools[name];
        pool_summary.insert(
            format!("{name}.busy"),
            json!(utilization(
                number(pool, "open").saturating_sub(number(pool, "idle")),
                number(pool, "max")
            )),
        );
        pool_summary.insert(
            format!("{name}.checkout_average"),
            json!(average_ms(
                number(pool, "acquire_total_us"),
                number(pool, "acquisitions")
            )),
        );
        pool_summary.insert(
            format!("{name}.checkout_maximum"),
            json!(maximum_ms(
                number(pool, "acquire_max_us"),
                number(pool, "acquisitions")
            )),
        );
        pool_summary.insert(
            format!("{name}.checkout_errors"),
            json!(percentage(
                number(pool, "acquire_errors"),
                number(pool, "acquisitions")
            )),
        );
        pool_summary.insert(
            format!("{name}.checkout_timeouts"),
            json!(percentage(
                number(pool, "acquire_timeouts"),
                number(pool, "acquisitions")
            )),
        );
    }
    section(
        &mut text,
        "Database pools",
        &Value::Object(pool_summary),
        plain,
    );
    text.push_str("\n\nPool acquisition timing includes checkout health checks and connection creation. Cancelled acquisitions are excluded from completed-attempt totals.");
    let mut waits = serde_json::Map::new();
    for name in ["all", "conversations", "metadata", "audit"] {
        let counts = if name == "all" {
            &runtime["ordering"]
        } else {
            &runtime["ordering"][name]
        };
        let samples = number(counts, "completed");
        waits.insert(
            format!("{name}.average"),
            json!(average_ms(number(counts, "total_us"), samples)),
        );
        waits.insert(
            format!("{name}.maximum"),
            json!(maximum_ms(number(counts, "max_us"), samples)),
        );
        waits.insert(
            format!("{name}.timeouts"),
            json!(percentage(number(counts, "timeouts"), samples)),
        );
    }
    section(&mut text, "Ordering waits", &Value::Object(waits), plain);
    render_routes(&mut text, routes, plain);
    text.push_str("\n\nLatency measures backend response creation/upgrade, including body receipt, admission, security and awaited work. It excludes header receipt, proxy buffering before Rust, response transfer and subsequent S3 downloads. Histogram p95 is a bucket upper bound, not an exact percentile; n/a means no samples. Percentages use lifetime totals, not a recent window. Concurrent counter reads may briefly disagree.");
    section(&mut text, "History and unread caches", cache, plain);
    section(&mut text, "Snapshot cache", snapshots, plain);
    section(&mut text, "Database pool counters", pools, plain);
    section(&mut text, "Server admission", runtime, plain);
    text.push_str("\n\nHTTP duration buckets [0..8] are non-cumulative: <=0.5 ms, <=1 ms, <=5 ms, <=10 ms, <=50 ms, <=250 ms, <=1 s, <=5 s, >5 s. Cancelled requests have no completed duration sample. Ordering timings include timed-out completed attempts and exclude cancelled waits.");
    text.push_str("\n\nCollection is on demand; no periodic metrics logging runs. The report itself occupies request/job capacity. Host CPU, process RSS and end-to-end browser latency are not measured here.");
    text
}

fn render_metrics_text(cache: &Value, pools: &Value, snapshots: &Value, runtime: &Value) -> String {
    format!(
        "```text\n{}\n```",
        render_report(cache, pools, snapshots, runtime, true)
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
