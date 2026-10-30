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

#[tokio::test]
async fn spawned_handlers_reuse_capture_and_recheck_revoked_sessions() {
    let app = crate::web::media::tests::app(
        crate::engine::Engine::open(std::path::Path::new(":memory:")).unwrap(),
    );
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("cookie", "chat_session=alice".parse().unwrap());
    let capture = app
        .inspect_engine({
            let app = app.clone();
            let headers = headers.clone();
            move |engine, _| {
                Ok::<_, crate::engine::EngineError>(
                    super::super::principal::Resolved::capture(&app, engine, headers).unwrap(),
                )
            }
        })
        .await
        .unwrap();
    PRINCIPAL.scope(capture, async move {
        spawn_io((), async move {
            assert!(resolved(&app, &headers).is_some());
            let identity = super::super::auth::authenticated_identity(&app, &headers).await.unwrap();
            assert_eq!(identity.user, "alice");
            app.engine().unwrap().logout("alice").unwrap();
            assert!(matches!(super::super::auth::authenticated_identity(&app, &headers).await, Err(error) if error.0 == axum::http::StatusCode::UNAUTHORIZED));
        }).await.unwrap();
    }).await;
}
