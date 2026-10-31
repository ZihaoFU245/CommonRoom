use super::*;
use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    extract::Request,
    http::StatusCode,
    middleware,
    routing::post,
};
use std::time::Duration;
use tower::ServiceExt;

fn app() -> super::super::App {
    let mut app = crate::web::tests::login_test_app();
    let mut config = (*app.config).clone();
    config.server.body_timeout_seconds = 1;
    config.server.request_timeout_seconds = 2;
    app.config = Arc::new(config);
    app
}
fn router(app: super::super::App) -> Router {
    let mut registry = Registry::new("/");
    registry.register("", "/body");
    registry.register("", "/slow");
    app.admission.metrics.register_routes(registry);
    Router::new()
        .route("/body", post(|body: Bytes| async move { body }))
        .route(
            "/slow",
            post(|| async {
                tokio::time::sleep(Duration::from_secs(10)).await;
                "late"
            }),
        )
        .layer(middleware::from_fn_with_state(app.clone(), http))
        .layer(middleware::from_fn_with_state(app, observe))
}
#[tokio::test]
async fn trickling_body_has_absolute_deadline_and_other_requests_progress() {
    let app = app();
    let router = router(app.clone());
    let stream = futures_util::stream::unfold((), |()| async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        Some((Ok::<_, std::io::Error>(Bytes::from_static(b"x")), ()))
    });
    let slow = tokio::spawn(
        router.clone().oneshot(
            Request::post("/body")
                .body(Body::from_stream(stream))
                .unwrap(),
        ),
    );
    let response = router
        .clone()
        .oneshot(Request::post("/body").body(Body::from("ok")).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(to_bytes(response.into_body(), 64).await.unwrap(), "ok");
    let response = tokio::time::timeout(Duration::from_secs(3), slow)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    assert_eq!(response.headers()["connection"], "close");
    assert_eq!(app.admission.metrics.stats()["body_timeouts"], 1);
    assert_eq!(app.admission.metrics.stats()["request_timeouts"], 0);
    assert_eq!(
        app.admission.metrics.stats()["routes"]["/body"]["active"],
        0
    );
}
#[tokio::test]
async fn oversized_bodies_and_handler_deadlines_are_distinguished() {
    let app = app();
    let router = router(app.clone());
    let oversized = router
        .clone()
        .oneshot(
            Request::post("/body")
                .body(Body::from(vec![0; 4097]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let slow = router
        .clone()
        .oneshot(Request::post("/slow").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(slow.status(), StatusCode::REQUEST_TIMEOUT);
    let stats = app.admission.metrics.stats();
    assert_eq!(stats["body_rejections"], 1);
    assert_eq!(stats["request_timeouts"], 1);
    assert_eq!(stats["routes"]["/slow"]["completed"], 1);
    assert_eq!(stats["routes"]["/slow"]["client_errors"], 1);
    for index in 0..50 {
        let response = router
            .clone()
            .oneshot(
                Request::get(format!("/unknown/{index}?secret=test"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    assert_eq!(
        app.admission.metrics.stats()["routes"]
            .as_object()
            .unwrap()
            .len(),
        3
    );
    assert!(!app.admission.metrics.stats().to_string().contains("secret"));
}
#[tokio::test]
async fn cancelled_requests_are_counted_and_release_active_measurements() {
    let app = app();
    let mut request =
        Box::pin(router(app.clone()).oneshot(Request::post("/slow").body(Body::empty()).unwrap()));
    assert!(futures_util::poll!(&mut request).is_pending());
    assert_eq!(
        app.admission.metrics.stats()["routes"]["/slow"]["active"],
        1
    );
    drop(request);
    let stats = app.admission.metrics.stats();
    assert_eq!(stats["routes"]["/slow"]["active"], 0);
    assert_eq!(stats["routes"]["/slow"]["cancelled"], 1);
    assert_eq!(stats["routes"]["/slow"]["completed"], 0);
}

#[tokio::test]
async fn bot_body_budget_preserves_unicode_under_a_base_path() {
    let mut app = app();
    let mut config = (*app.config).clone();
    config.base_url = "/chat/".into();
    app.config = Arc::new(config);
    let router = Router::new()
        .route(
            "/chat/api/bot/v1/body",
            post(|body: Bytes| async move { body }),
        )
        .layer(middleware::from_fn_with_state(app, http));
    for (size, status) in [
        (16000, StatusCode::OK),
        (32768, StatusCode::OK),
        (32769, StatusCode::PAYLOAD_TOO_LARGE),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::post("/chat/api/bot/v1/body")
                    .body(Body::from(vec![b'x'; size]))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
}

#[test]
fn concurrent_updates_and_snapshots_preserve_every_request() {
    let metrics = Arc::new(Metrics::default());
    let mut registry = Registry::new("/");
    registry.register("", "/requests/{id}");
    metrics.register_routes(registry);
    let barrier = Arc::new(std::sync::Barrier::new(5));
    let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let metrics = metrics.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let pending = metrics.request("/requests/{id}");
                barrier.wait();
                barrier.wait();
                for index in 0..2000 {
                    let mut request = metrics.request("/requests/{id}");
                    request.status = match index % 4 {
                        0 => Some(StatusCode::OK),
                        1 => Some(StatusCode::BAD_REQUEST),
                        2 => Some(StatusCode::SERVICE_UNAVAILABLE),
                        _ => None,
                    };
                }
                drop(pending);
            })
        })
        .collect();
    barrier.wait();
    assert_eq!(metrics.stats()["routes"]["/requests/{id}"]["active"], 4);
    let reader = std::thread::spawn({
        let metrics = metrics.clone();
        let finished = finished.clone();
        move || {
            while !finished.load(Ordering::Relaxed) {
                assert_eq!(metrics.stats()["routes"].as_object().unwrap().len(), 1);
            }
        }
    });
    barrier.wait();
    for worker in workers {
        worker.join().unwrap();
    }
    finished.store(true, Ordering::Relaxed);
    reader.join().unwrap();
    let stats = metrics.stats();
    let route = &stats["routes"]["/requests/{id}"];
    assert_eq!(route["active"], 0);
    assert_eq!(route["completed"], 6000);
    assert_eq!(route["cancelled"], 2004);
    assert_eq!(route["client_errors"], 2000);
    assert_eq!(route["server_errors"], 2000);
    assert_eq!(
        route["duration_buckets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap())
            .sum::<u64>(),
        6000
    );
    assert!(route["total_us"].as_u64().unwrap() >= route["max_us"].as_u64().unwrap());
}

#[tokio::test]
async fn registered_parameter_routes_keep_their_base_path_and_bound_unknown_labels() {
    let mut app = app();
    let mut config = (*app.config).clone();
    config.base_url = "/chat/".into();
    app.config = Arc::new(config);
    let mut registry = Registry::new("/chat/");
    let path = registry.register("/api", "/items/{id}");
    app.admission.metrics.register_routes(registry);
    let router = Router::new()
        .nest(
            "/chat/api",
            Router::new().route(path, post(|| async { "ok" })),
        )
        .layer(middleware::from_fn_with_state(app.clone(), observe));
    for index in 0..100 {
        let response = router
            .clone()
            .oneshot(
                Request::post(format!("/chat/api/items/{index}?secret=hidden"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    for index in 0..100 {
        let response = router
            .clone()
            .oneshot(
                Request::post(format!("/chat/api/missing/{index}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    let stats = app.admission.metrics.stats();
    assert_eq!(stats["routes"]["/chat/api/items/{id}"]["completed"], 100);
    assert_eq!(stats["routes"]["unmatched"]["completed"], 100);
    assert_eq!(stats["routes"].as_object().unwrap().len(), 2);
    assert!(!stats.to_string().contains("secret"));
}
