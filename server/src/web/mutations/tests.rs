use super::*;
use std::time::Duration;
#[tokio::test]
async fn uncontended_memory_phases_stay_on_the_async_thread() {
    let app = crate::web::tests::login_test_app();
    let thread = std::thread::current().id();
    let actual: Result<_, EngineError> = app
        .inspect_engine(|_, _| Ok(std::thread::current().id()))
        .await;
    assert_eq!(actual.unwrap(), thread);
    let permit = app
        .mutations
        .conversation("memory-only".into())
        .await
        .unwrap();
    let actual: Result<_, EngineError> = app
        .with_conversation(&permit, |_| Ok(std::thread::current().id()))
        .await;
    assert_eq!(actual.unwrap(), thread);
}

#[tokio::test]
#[ignore = "manual memory inspection scheduling benchmark"]
async fn inspection_scheduling_benchmark() {
    use futures_util::FutureExt;
    use std::time::Instant;
    let app = crate::web::tests::login_test_app();
    let start = Instant::now();
    for _ in 0..10_000 {
        let result: Result<(), EngineError> = app.inspect_engine(|_, _| Ok(())).await;
        result.unwrap();
    }
    println!(
        "uncontended inspection: {} ns per call",
        start.elapsed().as_nanos() / 10_000
    );
    let start = Instant::now();
    for _ in 0..100 {
        let mut inspection =
            Box::pin(app.inspect_engine::<_, EngineError>(|_, _| Ok(std::thread::current().id())));
        {
            let _writer = app.engine.write().unwrap();
            assert!(inspection.as_mut().now_or_never().is_none());
        }
        assert_ne!(inspection.await.unwrap(), std::thread::current().id());
    }
    println!(
        "contention fallback: {} us per call including worker dispatch",
        start.elapsed().as_micros() / 100
    );
}

#[tokio::test]
async fn cancelled_conversation_acquisitions_remove_weak_entries() {
    let coordinator = Arc::new(Coordinator::default());
    let metadata = coordinator.metadata().await;
    for index in 0..600 {
        let mut acquisition = Box::pin(coordinator.conversation(format!("cancelled-{index}")));
        assert!(futures_util::poll!(&mut acquisition).is_pending());
        // Cancellation must release ordering entries before the fence is granted.
        drop(acquisition);
    }
    assert!(coordinator.conversations.lock().unwrap().is_empty());
    drop(metadata);
    let acquired = coordinator.conversation("live".into()).await.unwrap();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(10),
            coordinator.conversation("live".into())
        )
        .await
        .is_err()
    );
    drop(acquired);
    assert!(coordinator.conversation("live".into()).await.is_ok());
}

#[tokio::test]
async fn contended_read_panic_returns_unavailable_without_poisoning() {
    let app = crate::web::tests::login_test_app();
    use futures_util::FutureExt;
    let mut inspection =
        Box::pin(app.inspect_engine::<(), EngineError>(|_, _| panic!("injected read panic")));
    {
        let _writer = app.engine.write().unwrap();
        assert!(inspection.as_mut().now_or_never().is_none());
    }
    assert!(matches!(inspection.await, Err(EngineError::Unavailable(_))));
    assert!(!app.engine.is_poisoned());
    let subsequent: Result<(), EngineError> = app.inspect_engine(|_, _| Ok(())).await;
    assert!(subsequent.is_ok());
}

#[tokio::test]
async fn shutdown_yields_drain_permits_to_queued_metadata_recovery() {
    let mut app =
        crate::web::media::tests::app(Engine::open(std::path::Path::new(":memory:")).unwrap());
    app.io_jobs = Arc::new(tokio::sync::Semaphore::new(
        usize::try_from(super::super::IO_JOBS).unwrap(),
    ));
    let id = {
        let mut engine = app.engine().unwrap();
        engine
            .execute(Some("alice"), None, "/room new recovered")
            .unwrap();
        engine.data.rooms["recovered"].id.clone()
    };
    let permit = app.mutations.conversation(id.clone()).await.unwrap();
    let pending = app
        .with_conversation(&permit, |engine| {
            engine.prepare_browser_message("alice", Some("recovered"), "durable", false)
        })
        .await
        .unwrap();
    let commit = pending.save(None).await.unwrap();
    app.engine
        .write()
        .unwrap()
        .data
        .rooms
        .get_mut("recovered")
        .unwrap()
        .id = uuid::Uuid::new_v4().to_string();
    let occupied = app
        .io_jobs
        .clone()
        .acquire_many_owned(super::super::IO_JOBS)
        .await
        .unwrap();
    app.stopping
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let shutdown = tokio::spawn({
        let app = app.clone();
        async move { app.checkpoint_after_jobs().await }
    });
    tokio::task::yield_now().await;
    assert!(
        app.with_conversation(&permit, move |engine| engine
            .publish_message(pending, commit))
            .await
            .is_err()
    );
    drop(permit);
    drop(occupied);
    tokio::time::timeout(Duration::from_secs(2), shutdown)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(app.io_jobs.is_closed());
    assert_eq!(app.engine_read().unwrap().data.rooms["recovered"].id, id);
}

#[tokio::test]
async fn committed_publication_mismatch_reloads_metadata_and_resumes_service() {
    recovery_contract(
        Engine::open(std::path::Path::new(":memory:")).unwrap(),
        false,
    )
    .await;
}
#[tokio::test]
async fn failed_authoritative_reload_remains_unavailable() {
    recovery_contract(
        Engine::open(std::path::Path::new(":memory:")).unwrap(),
        true,
    )
    .await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_committed_publication_mismatch_recovers_authoritative_metadata() {
    let fixture = crate::engine::database::tests::PostgresFixture::new();
    recovery_contract(Engine::open_postgres(&fixture.url).unwrap(), false).await;
}
async fn recovery_contract(engine: Engine, corrupt_database: bool) {
    let app = crate::web::media::tests::app(engine);
    let (id, database) = {
        let mut engine = app.engine().unwrap();
        engine
            .execute(Some("alice"), None, "/room new recovered")
            .unwrap();
        (
            engine.data.rooms["recovered"].id.clone(),
            engine.auth_store(),
        )
    };
    let permit = app.mutations.conversation(id.clone()).await.unwrap();
    let pending = app
        .with_conversation(&permit, |engine| {
            engine.prepare_browser_message("alice", Some("recovered"), "durable message", false)
        })
        .await
        .unwrap();
    let commit = pending.save(None).await.unwrap();
    {
        let mut engine = app.engine.write().unwrap();
        engine.data.rooms.get_mut("recovered").unwrap().id = uuid::Uuid::new_v4().to_string();
        if corrupt_database {
            engine.test_corrupt_recovery_metadata();
        }
    }
    let mut changes = app.changes.subscribe();
    let result = app
        .with_conversation(&permit, move |engine| {
            engine.publish_message(pending, commit)
        })
        .await;
    assert!(result.is_err());
    drop(permit);
    tokio::time::timeout(Duration::from_secs(2), async {
        while app
            .mutations
            .recovering
            .load(std::sync::atomic::Ordering::Acquire)
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let restored: Result<_, EngineError> = app
        .inspect_engine(|engine, _| Ok(engine.data.rooms["recovered"].id.clone()))
        .await;
    if corrupt_database {
        assert!(restored.is_err());
        assert!(changes.try_recv().is_err());
    } else {
        assert_eq!(restored.unwrap(), id);
        assert_eq!(changes.try_recv().unwrap(), crate::web::Change::All);
        database.health().await.unwrap();
        let permit = app.mutations.conversation(id).await.unwrap();
        let page = app
            .with_conversation(&permit, |engine| {
                engine.prepare_browser_message("alice", Some("recovered"), "after recovery", false)
            })
            .await
            .unwrap();
        let commit = page.save(None).await.unwrap();
        app.with_conversation(&permit, move |engine| engine.publish_message(page, commit))
            .await
            .unwrap();
    }
}
#[tokio::test]
async fn panicking_ordered_mutation_fails_closed_after_poisoning_the_engine() {
    let app = crate::web::tests::login_test_app();
    let permit = app
        .mutations
        .conversation("panic-test".into())
        .await
        .unwrap();
    let result: Result<(), EngineError> = app
        .with_conversation(&permit, |_engine| {
            // Inject an unexpected failure through the actual guarded worker.
            panic!("injected mutation panic")
        })
        .await;
    assert!(matches!(result, Err(EngineError::Unavailable(_))));
    assert!(app.engine.is_poisoned());
    let next: Result<(), EngineError> = app.with_conversation(&permit, |_engine| Ok(())).await;
    assert!(matches!(next, Err(EngineError::Unavailable(_))));
    let read: Result<(), EngineError> = app.inspect_engine(|_, _| Ok(())).await;
    assert!(matches!(read, Err(EngineError::Unavailable(_))));
}
#[tokio::test]
async fn bulk_conversations_do_not_deadlock_a_queued_metadata_writer() {
    let coordinator = Arc::new(Coordinator::default());
    let one = coordinator.conversation("b".into()).await.unwrap();
    let (entered, entered_rx) = tokio::sync::oneshot::channel();
    let (release, release_rx) = tokio::sync::oneshot::channel();
    let writer = tokio::spawn({
        let coordinator = coordinator.clone();
        async move {
            let metadata = coordinator.metadata.clone().write_owned().await;
            entered.send(()).unwrap();
            release_rx.await.unwrap();
            drop(metadata);
        }
    });
    tokio::task::yield_now().await;
    let bulk = tokio::spawn({
        let coordinator = coordinator.clone();
        async move {
            coordinator
                .conversations(vec!["b".into(), "a".into(), "a".into()])
                .await
                .unwrap()
        }
    });
    tokio::task::yield_now().await;
    drop(one);
    tokio::time::timeout(Duration::from_millis(100), entered_rx)
        .await
        .unwrap()
        .unwrap();
    assert!(!bulk.is_finished());
    release.send(()).unwrap();
    writer.await.unwrap();
    let bulk = tokio::time::timeout(Duration::from_millis(100), bulk)
        .await
        .unwrap()
        .unwrap();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(10),
            coordinator.conversations(vec!["a".into(), "b".into()])
        )
        .await
        .is_err()
    );
    drop(bulk);
    let reversed = coordinator
        .conversations(vec!["b".into(), "a".into()])
        .await
        .unwrap();
    drop(reversed);
    assert!(coordinator.conversations.lock().unwrap().is_empty());
}
#[tokio::test]
async fn conversations_order_independently_and_metadata_waits_for_publication() {
    let coordinator = Arc::new(Coordinator::default());
    let one = coordinator.conversation("one".into()).await.unwrap();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(10),
            coordinator.conversation("one".into())
        )
        .await
        .is_err()
    );
    let two = tokio::time::timeout(
        Duration::from_millis(50),
        coordinator.conversation("two".into()),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(10),
            coordinator.metadata.clone().write_owned()
        )
        .await
        .is_err()
    );
    drop(one);
    drop(two);
    let metadata = coordinator.metadata.clone().write_owned().await;
    assert!(coordinator.conversations.lock().unwrap().is_empty());
    assert!(
        tokio::time::timeout(
            Duration::from_millis(10),
            coordinator.conversation("cancelled".into())
        )
        .await
        .is_err()
    );
    drop(metadata);
    let next = coordinator.conversation("next".into()).await.unwrap();
    drop(next);
    assert!(coordinator.conversations.lock().unwrap().is_empty());
}
