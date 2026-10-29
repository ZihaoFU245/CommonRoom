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
                let runtime = json!({"io_jobs_available":app.io_jobs.available_permits(),"password_jobs_available":app.password_jobs.available_permits()});
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
fn section(text: &mut String, title: &str, value: &Value) {
    text.push_str(&format!(
        "\n\n## {title}\n\n| Metric | Value |\n| --- | ---: |"
    ));
    rows(text, "", value);
}
fn rows(text: &mut String, prefix: &str, value: &Value) {
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
                );
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                rows(text, &format!("{prefix}[{index}]"), value);
            }
        }
        Value::Null => {}
        value => text.push_str(&format!(
            "\n| `{prefix}` | {} |",
            value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string())
        )),
    }
}
fn render_metrics(cache: &Value, pools: &Value, snapshots: &Value, runtime: &Value) -> String {
    let mut text = format!(
        "# System metrics\n\nCumulative totals since server startup (uptime: {} seconds). Counters are approximate concurrent observations. Cache bytes are accounting budgets, not total process memory.\n\n- History page hit rate: **{}**. Coalesced waiters count as hits.\n- Unread cache hit rate: **{}**. Full/incremental counts describe database calculations after misses.\n- Snapshot reuse rate: **{}**. Reuse includes callers sharing an in-progress build.",
        number(cache, "uptime_seconds"),
        ratio(number(cache, "hits"), number(cache, "misses")),
        ratio(number(cache, "unread_hits"), number(cache, "unread_misses")),
        ratio(number(snapshots, "hits"), number(snapshots, "misses"))
    );
    section(&mut text, "History and unread caches", cache);
    section(&mut text, "Snapshot cache", snapshots);
    section(&mut text, "Database pools", pools);
    text.push_str("\n\nPool acquisition timing includes checkout health checks and connection creation. Cancelled acquisitions are excluded from completed-attempt totals.");
    section(&mut text, "Server admission", runtime);
    text
}

fn render_metrics_text(cache: &Value, pools: &Value, snapshots: &Value, runtime: &Value) -> String {
    // Use the same complete report as the table view, with plain headings and key/value rows.
    let report = render_metrics(cache, pools, snapshots, runtime);
    let lines = report
        .lines()
        .filter_map(|line| {
            if line == "| Metric | Value |" || line == "| --- | ---: |" {
                None
            } else if let Some(row) = line
                .strip_prefix("| ")
                .and_then(|row| row.strip_suffix(" |"))
            {
                Some(row.replace('`', "").replace(" | ", ": "))
            } else {
                Some(line.trim_start_matches('#').trim_start().replace("**", ""))
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("```text\n{lines}\n```")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn metrics_render_markdown_and_handle_empty_and_large_counters() {
        assert_eq!(ratio(4752, 848), "84.86%");
        assert_eq!(ratio(0, 0), "n/a (no lookups)");
        assert_eq!(ratio(u64::MAX, u64::MAX), "50.00%");
        let text = render_metrics(
            &json!({"hits":4752,"misses":848,"uptime_seconds":12}),
            &json!({"readers":{"open":2}}),
            &json!({"hits":0,"misses":0}),
            &json!({}),
        );
        assert!(text.starts_with("# System metrics"));
        assert!(text.contains("**84.86%**"));
        assert!(text.contains("| `readers.open` | 2 |"));
        let plain = render_metrics_text(
            &json!({"hits":4752,"misses":848,"uptime_seconds":12}),
            &json!({"readers":{"open":2}}),
            &json!({"hits":0,"misses":0}),
            &json!({"io_jobs_available":64}),
        );
        assert!(plain.starts_with("```text\nSystem metrics"));
        assert!(plain.ends_with("\n```"));
        assert!(plain.contains("readers.open: 2"));
        assert!(plain.contains("io_jobs_available: 64"));
        assert!(plain.contains("84.86%"));
        assert!(!plain.contains('|'));
        assert_eq!(
            Command::parse("/system metrics --txt").unwrap(),
            Command::MetricsText
        );
        assert!(Command::parse("/system configs --txt").is_err());
        assert!(Command::parse("/system metrics extra").is_err());
        assert!(Command::parse("/configs").is_err());
    }
}
