use super::*;
#[test]
fn metrics_render_markdown_and_handle_empty_and_large_counters() {
    let literal = render_metrics_text(
        &json!({"label`|": "a | b`**c"}),
        &json!({}),
        &json!({}),
        &json!({}),
    );
    assert!(literal.contains("label`|: a | b`**c"));
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
