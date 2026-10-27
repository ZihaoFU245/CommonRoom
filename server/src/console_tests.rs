use super::*;
use crate::{engine::Engine, web::media::tests::app};
use std::sync::Arc;

async fn command(app: &App, input: &str) -> Result<String, EngineError> {
    execute(app, input.into())
        .await
        .map(|output| output.map_or_else(String::new, |output| output.reply))
}
async fn contract(mut app: App, db: crate::engine::database::Connection, postgres: bool) {
    app.io_jobs = Arc::new(tokio::sync::Semaphore::new(64));
    assert!(execute(&app, "  ".into()).await.unwrap().is_none());
    assert_eq!(
        command(&app, "/help").await.unwrap(),
        commands::help(true, true)
    );
    command(&app, "/user new dave strong-console-password user")
        .await
        .unwrap();
    assert!(crate::engine::verify_password(
        "strong-console-password",
        &app.engine_read().unwrap().data.users["dave"].hash
    ));
    command(&app, "/room new #console-room").await.unwrap();
    command(&app, "/room add alice #console-room")
        .await
        .unwrap();
    command(&app, "/room add bob #console-room").await.unwrap();
    let source = {
        let mut engine = app.engine().unwrap();
        engine
            .execute(Some("alice"), Some("console-room"), "console source")
            .unwrap();
        engine.history("alice", "console-room").unwrap().messages[0].clone()
    };
    assert!(
        command(&app, "/history #console-room")
            .await
            .unwrap()
            .contains("console source")
    );
    assert!(command(&app, "/configs").await.is_ok());
    assert!(command(&app, "/configs unexpected").await.is_err());
    assert!(
        command(&app, "/tell alice forbidden console direct")
            .await
            .unwrap_err()
            .contains("user account")
    );
    assert!(
        command(
            &app,
            &format!("/reply {} forbidden console reply", source.id)
        )
        .await
        .unwrap_err()
        .contains("user account")
    );
    assert!(
        command(&app, &format!("/react {} 👍", source.id))
            .await
            .unwrap_err()
            .contains("user account")
    );
    assert!(command(&app, "plain stdin text").await.is_err());
    command(&app, &format!("/edit {} console edit", source.id))
        .await
        .unwrap();
    {
        let engine = app.engine_read().unwrap();
        let message = &engine.history("alice", "console-room").unwrap().messages[0];
        assert_eq!(message.text, "console edit");
        assert_eq!(message.author_id, source.author_id);
        assert_eq!(message.from, "alice");
        assert!(engine.account_id("console").is_none());
    }
    let (private_source, media_source, attachment) = {
        let mut engine = app.engine().unwrap();
        engine
            .execute(Some("alice"), None, "/tell bob console private source")
            .unwrap();
        let private = engine.history("alice", "@direct:bob").unwrap().messages[0].clone();
        let conversation = engine.data.rooms["console-room"].id.clone();
        let mut media = engine
            .allocate_media("alice", &conversation, "console.png", 8)
            .unwrap();
        media.ready = true;
        media.object_key = media.final_key();
        media.content_type = "image/png".into();
        engine.finish_media("alice", &media).unwrap();
        let attachment = format!("![console](/api/media/{}/file)", media.id);
        engine
            .execute(Some("alice"), Some("console-room"), &attachment)
            .unwrap();
        let message = engine
            .history("alice", "console-room")
            .unwrap()
            .messages
            .last()
            .unwrap()
            .id
            .clone();
        (private, message, attachment)
    };
    command(
        &app,
        &format!("/edit {} console private edit", private_source.id),
    )
    .await
    .unwrap();
    assert_eq!(
        app.engine_read()
            .unwrap()
            .history("alice", "@direct:bob")
            .unwrap()
            .messages[0]
            .author_id,
        private_source.author_id
    );
    command(&app, &format!("/retract {}", private_source.id))
        .await
        .unwrap();
    assert!(
        app.engine_read()
            .unwrap()
            .history("alice", "@direct:bob")
            .unwrap()
            .messages
            .is_empty()
    );
    command(&app, &format!("/edit {media_source} {attachment} retained"))
        .await
        .unwrap();
    assert!(
        command(
            &app,
            &format!(
                "/edit {media_source} ![new](/api/media/{}/file)",
                uuid::Uuid::new_v4()
            )
        )
        .await
        .unwrap_err()
        .contains("Media uploads require an account")
    );
    let before = app.engine_read().unwrap().data.clone();
    let revision = app.engine_read().unwrap().revision;
    let mut notices = app.changes.subscribe();
    db.execute_batch(if postgres {
        "CREATE FUNCTION reject_console_metadata() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected'; END $$; CREATE TRIGGER reject_console_metadata BEFORE UPDATE ON state FOR EACH ROW EXECUTE FUNCTION reject_console_metadata();"
    } else { "CREATE TRIGGER reject_console_metadata BEFORE UPDATE ON state BEGIN SELECT RAISE(ABORT,'injected'); END;" }).unwrap();
    assert!(command(&app, "/room new #rolled-back").await.is_err());
    assert!(app.engine_read().unwrap().data == before);
    assert_eq!(app.engine_read().unwrap().revision, revision);
    assert!(notices.try_recv().is_err());
    db.execute_batch(if postgres {
        "DROP TRIGGER reject_console_metadata ON state; DROP FUNCTION reject_console_metadata();"
    } else {
        "DROP TRIGGER reject_console_metadata;"
    })
    .unwrap();
    command(&app, "/user rename dave david").await.unwrap();
    command(&app, "/user reset david password replaced-console-password")
        .await
        .unwrap();
    command(&app, "/bot new helper #console-room")
        .await
        .unwrap();
    let issued = execute(&app, "/sudo /bot token new helper console".into())
        .await
        .unwrap()
        .unwrap();
    assert!(issued.secret);
    let secret = issued.reply.lines().last().unwrap();
    assert!(secret.starts_with("bot_"));
    assert!(
        !serde_json::to_string(&app.engine_read().unwrap().data)
            .unwrap()
            .contains(secret)
    );
    let revision = app.engine_read().unwrap().revision;
    command(&app, "/clean -1 #console-room --dry-run")
        .await
        .unwrap();
    assert_eq!(app.engine_read().unwrap().revision, revision);
    command(&app, &format!("/sudo /retract {}", source.id))
        .await
        .unwrap();
    {
        let mut engine = app.engine().unwrap();
        engine
            .execute(Some("alice"), Some("console-room"), "cleanup source")
            .unwrap();
    }
    command(&app, "/clean -1 @global").await.unwrap();
    let engine = app.engine_read().unwrap();
    assert!(
        engine
            .history("alice", "console-room")
            .unwrap()
            .messages
            .is_empty()
    );
    assert!(
        engine
            .data
            .policy
            .audit
            .iter()
            .any(|entry| entry.actor_id == "console" && entry.operation == "/clean")
    );
    assert!(crate::engine::verify_password(
        "replaced-console-password",
        &engine.data.users["david"].hash
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_console_preserves_trusted_operations_secrets_and_atomicity() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("chat.sqlite");
    contract(
        app(Engine::open(&path).unwrap()),
        crate::engine::database::Connection::open(&path).unwrap(),
        false,
    )
    .await;
    let engine = Engine::open(&path).unwrap();
    assert!(engine.data.users.contains_key("david"));
    assert!(
        engine
            .history("alice", "console-room")
            .unwrap()
            .messages
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_native_console_preserves_trusted_operations_secrets_and_atomicity() {
    let fixture = crate::engine::database::tests::PostgresFixture::new();
    contract(
        app(Engine::open_postgres(&fixture.url).unwrap()),
        crate::engine::database::Connection::postgres(&fixture.url, false).unwrap(),
        true,
    )
    .await;
    let engine = Engine::open_postgres(&fixture.url).unwrap();
    assert!(engine.data.users.contains_key("david"));
    assert!(
        engine
            .history("alice", "console-room")
            .unwrap()
            .messages
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_cancelled_console_jobs_drain_before_shutdown_checkpoint() {
    let fixture = crate::engine::database::tests::PostgresFixture::new();
    let mut app = app(Engine::open_postgres(&fixture.url).unwrap());
    app.io_jobs = Arc::new(tokio::sync::Semaphore::new(64));
    let blocker = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
    let hold = blocker.unchecked_transaction().unwrap();
    hold.query_row("SELECT id FROM state WHERE id=1 FOR UPDATE", [], |row| {
        row.get::<_, i64>(0)
    })
    .unwrap();
    let changing = tokio::spawn({
        let app = app.clone();
        async move {
            execute(
                &app,
                "/user new accepted accepted-console-password user".into(),
            )
            .await
            .map(|output| output.is_some())
        }
    });
    let inspect = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
    loop {
        let waiting: bool = inspect.query_row("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'INSERT INTO state%')", [], |row| row.get(0)).unwrap();
        if waiting {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "console did not reach metadata write"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(app.engine.try_read().is_ok());
    assert!(
        !app.engine_read()
            .unwrap()
            .data
            .users
            .contains_key("accepted")
    );
    changing.abort();
    assert!(changing.await.unwrap_err().is_cancelled());
    app.stopping
        .store(true, std::sync::atomic::Ordering::SeqCst);
    app.shutdown.notify_waiters();
    let checkpoint = tokio::spawn({
        let app = app.clone();
        async move { app.checkpoint_after_jobs().await }
    });
    tokio::task::yield_now().await;
    assert!(!checkpoint.is_finished());
    hold.rollback().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(4), checkpoint)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        app.engine_read()
            .unwrap()
            .data
            .users
            .contains_key("accepted")
    );
    assert!(app.io_jobs.is_closed());
    assert!(app.password_jobs.is_closed());
    assert!(
        execute(&app, "/room new rejected-after-stop".into())
            .await
            .is_err()
    );
}
