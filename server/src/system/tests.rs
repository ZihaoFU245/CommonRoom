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
    assert!(text.contains("## Database pools"));
    assert!(text.contains("n/a (0 / 0)"));
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

#[test]
fn report_summarizes_outcomes_capacity_and_latency_without_losing_raw_counters() {
    let cache = json!({"hits":3,"misses":1,"accounted_bytes":256,"byte_limit":1024,"pages":2,"page_limit":8});
    let runtime = json!({"admission":{"http":{"routes":{
        "/api/example/{id}":{"completed":100,"client_errors":3,"server_errors":2,"cancelled":4,"total_us":200000,"max_us":20000,"duration_buckets":[10,20,40,20,10,0,0,0,0]}
    }}}});
    let report = render_metrics(&cache, &json!({}), &json!({}), &runtime);
    assert!(report.contains("**95.00%** successful"));
    assert!(report.contains("3.00% client errors, 2.00% server errors"));
    assert!(report.contains("3.85% cancelled"));
    assert!(report.contains("**90.00%**; within 1 s: **100.00%**"));
    assert!(report.contains("25.00% (256 / 1024)"));
    assert!(report.contains("| `/api/example/{id}` | 100 | 3.00% | 2.00% | 3.85% | 2.000 ms | <= 50.000 ms | 20.000 ms |"));
    assert!(
        report.contains("| `admission.http.routes./api/example/{id}.duration_buckets[0]` | 10 |")
    );
    let plain = render_metrics_text(&cache, &json!({}), &json!({}), &runtime);
    assert!(plain.contains("duration_buckets[0]: 10"));
    assert!(plain.contains("95.00% successful"));
    assert_eq!(p95_bound(&json!({})), "n/a");
    assert_eq!(
        p95_bound(&json!({"duration_buckets":[0,0,0,0,0,0,0,0,1]})),
        ">5 s"
    );
    assert_eq!(percentage(u64::MAX, u64::MAX), "100.00%");
}

#[test]
fn empty_latency_samples_are_unavailable_and_reports_include_every_counter() {
    let runtime = json!({"collected_at_unix_seconds":123,"io_jobs_limit":64,
        "admission":{"browser_sockets_limit":128,"browser_sockets_available":120,
            "bot_sockets_limit":64,"bot_sockets_available":64,
            "http":{"connections":{"peak":8},"io_jobs":{"peak":4},
                "routes":{"/cancelled":{"active":1,"cancelled":2,"completed":0,"max_us":0}}}},
        "ordering":{"completed":2,"total_us":100,"max_us":70,"timeouts":1}});
    let cache = json!({"misses":10,"events":{"range_misses":4,"byte_evictions":1},
        "shard_stats":[{"bytes":800,"byte_limit":1000},{"bytes":200,"byte_limit":1000}]});
    let pools = json!({"readers":{"acquisitions":0,"acquire_max_us":0}});
    for plain in [false, true] {
        let report = render_report(&cache, &pools, &json!({}), &runtime, plain);
        for label in [
            "History and unread caches",
            "Snapshot cache",
            "Database pool counters",
            "Server admission",
            "Cache behavior",
            "Ordering waits",
        ] {
            assert!(report.contains(label), "{label}");
        }
        assert!(report.contains("80.00% (800 / 1000)"));
        assert!(report.contains("6.25% (8 / 128)"));
        assert!(report.contains("6.25% (4 / 64)"));
        assert!(report.contains("40.00%"));
        assert!(report.contains("50.00%"));
        assert!(report.contains("0.070 ms"));
        assert!(report.contains("collected_at_unix_seconds"));
        assert!(report.contains("admission.http.routes./cancelled.active"));
        assert!(!report.contains("0.000 ms"));
    }
    assert_eq!(maximum_ms(0, 0), "n/a");
    assert_eq!(maximum_ms(0, 1), "0.000 ms");
    assert_eq!(
        highest_shard_utilization(&json!({}), "bytes", "byte_limit"),
        "n/a"
    );
}
