use super::*;
use crate::web::media::tests::app;

async fn command(app: &App, view: Option<&str>, text: &str) -> Result<String, String> {
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::COOKIE,
        "chat_session=bob".parse().unwrap(),
    );
    execute_input(
        app,
        &headers,
        "bob",
        &Input {
            id: 1,
            room: view.map(str::to_owned),
            text: text.into(),
            markdown: false,
        },
    )
    .await
    .map(|(reply, change)| {
        assert!(change.is_none());
        reply
    })
}
fn setup(engine: Engine) -> App {
    let mut app = app(engine);
    app.io_jobs = Arc::new(tokio::sync::Semaphore::new(64));
    {
        let mut engine = app.engine().unwrap();
        engine.execute(None, None, "/room new secret").unwrap();
        engine
            .execute(None, None, "/room add alice #secret")
            .unwrap();
        engine
            .execute(None, Some("secret"), "source message")
            .unwrap();
        engine.execute(None, None, "/revoke bob user").unwrap();
        for permission in ["/sudo", "x:command.sudo"] {
            engine
                .execute(None, None, &format!("/grant bob @global {permission}"))
                .unwrap();
        }
    }
    app
}
async fn contract(app: App, db: crate::engine::database::Connection, postgres: bool) {
    let source = app
        .engine_read()
        .unwrap()
        .history("alice", "secret")
        .unwrap()
        .messages[0]
        .clone();
    assert!(
        command(
            &app,
            Some("secret"),
            &format!("/reply {} denied", source.id)
        )
        .await
        .is_err()
    );
    command(&app, None, "/sudo /clean -1 #secret --dry-run")
        .await
        .unwrap();
    assert_eq!(
        app.engine_read()
            .unwrap()
            .history("alice", "secret")
            .unwrap()
            .messages
            .len(),
        1
    );
    let sequence = app.engine_read().unwrap().data.next_sequence;
    let revision = app.engine_read().unwrap().revision;
    let mut notices = app.changes.subscribe();
    db.execute_batch(if postgres {
        "CREATE FUNCTION reject_elevated_message() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected'; END $$; CREATE TRIGGER reject_elevated_message BEFORE INSERT ON messages FOR EACH ROW EXECUTE FUNCTION reject_elevated_message();"
    } else {
        "CREATE TRIGGER reject_elevated_message BEFORE INSERT ON messages BEGIN SELECT RAISE(ABORT,'injected'); END;"
    }).unwrap();
    assert!(
        command(
            &app,
            Some("secret"),
            &format!("/sudo /reply {} rollback", source.id)
        )
        .await
        .is_err()
    );
    {
        let engine = app.engine_read().unwrap();
        assert_eq!(engine.data.next_sequence, sequence);
        assert_eq!(engine.revision, revision);
        assert!(!engine.is_su(Some("bob")));
        assert_eq!(engine.history("alice", "secret").unwrap().messages.len(), 1);
    }
    assert!(notices.try_recv().is_err());
    assert_eq!(
        db.query_row("SELECT sequence FROM counters WHERE id=1", [], |row| row
            .get::<_, u64>(0))
            .unwrap(),
        sequence
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM messages", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    db.execute_batch(if postgres {
        "DROP TRIGGER reject_elevated_message ON messages; DROP FUNCTION reject_elevated_message();"
    } else {
        "DROP TRIGGER reject_elevated_message;"
    })
    .unwrap();
    command(
        &app,
        Some("secret"),
        &format!("/sudo /reply {} elevated reply", source.id),
    )
    .await
    .unwrap();
    command(
        &app,
        Some("secret"),
        &format!("/sudo /react {} 👍", source.id),
    )
    .await
    .unwrap();
    command(
        &app,
        None,
        &format!("/sudo /edit {} elevated edit", source.id),
    )
    .await
    .unwrap();
    assert!(
        command(&app, None, "/sudo /tell carol elevated direct")
            .await
            .unwrap()
            .contains("carol")
    );
    {
        let engine = app.engine_read().unwrap();
        let history = engine.history("alice", "secret").unwrap();
        assert_eq!(history.messages[0].text, "elevated edit");
        assert_eq!(history.messages[0].author_id, source.author_id);
        assert!(history.messages[0].reactions["👍"].contains("bob"));
        assert_eq!(history.messages[1].from, "bob");
        assert_eq!(history.messages[1].reply.as_ref().unwrap().id, source.id);
        assert!(!engine.is_su(Some("bob")));
    }
    command(&app, None, &format!("/sudo /retract {}", source.id))
        .await
        .unwrap();
    assert_eq!(
        app.engine_read()
            .unwrap()
            .history("alice", "secret")
            .unwrap()
            .messages
            .len(),
        1
    );
    let attachment = {
        let mut engine = app.engine().unwrap();
        let conversation = engine.data.rooms["secret"].id.clone();
        let mut media = engine
            .allocate_media("alice", &conversation, "photo.png", 8)
            .unwrap();
        media.ready = true;
        media.object_key = media.final_key();
        media.content_type = "image/png".into();
        engine.finish_media("alice", &media).unwrap();
        let attachment = format!("![photo](/api/media/{}/file)", media.id);
        engine
            .execute(Some("alice"), Some("secret"), &attachment)
            .unwrap();
        attachment
    };
    let latest = app
        .engine_read()
        .unwrap()
        .history("alice", "secret")
        .unwrap()
        .messages
        .last()
        .unwrap()
        .id
        .clone();
    command(
        &app,
        Some("secret"),
        &format!("/sudo /reply {latest} {attachment}"),
    )
    .await
    .unwrap();
    assert!(!app.engine_read().unwrap().is_su(Some("bob")));
    command(&app, None, "/sudo /clean -1 #secret")
        .await
        .unwrap();
    let engine = app.engine_read().unwrap();
    assert!(
        engine
            .history("alice", "secret")
            .unwrap()
            .messages
            .is_empty()
    );
    assert!(!engine.is_su(Some("bob")));
    assert_eq!(
        engine
            .data
            .policy
            .audit
            .iter()
            .filter(|audit| audit.operation == "/clean" || audit.operation == "/retract")
            .count(),
        2
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_elevated_writes_preserve_authority_and_atomicity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    contract(
        setup(Engine::open(&path).unwrap()),
        crate::engine::database::Connection::open(&path).unwrap(),
        false,
    )
    .await;
    let engine = Engine::open(&path).unwrap();
    assert!(
        engine
            .history("alice", "secret")
            .unwrap()
            .messages
            .is_empty()
    );
    assert!(!engine.is_su(Some("bob")));
    assert_eq!(
        engine.history("carol", "@direct:bob").unwrap().messages[0].text,
        "elevated direct"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_native_elevated_writes_preserve_authority_and_atomicity() {
    let fixture = crate::engine::database::tests::PostgresFixture::new();
    contract(
        setup(Engine::open_postgres(&fixture.url).unwrap()),
        crate::engine::database::Connection::postgres(&fixture.url, false).unwrap(),
        true,
    )
    .await;
    let engine = Engine::open_postgres(&fixture.url).unwrap();
    assert!(
        engine
            .history("alice", "secret")
            .unwrap()
            .messages
            .is_empty()
    );
    assert!(!engine.is_su(Some("bob")));
    assert_eq!(
        engine.history("carol", "@direct:bob").unwrap().messages[0].text,
        "elevated direct"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_elevated_lookup_releases_authority_and_rechecks_sudo_revocation() {
    let fixture = crate::engine::database::tests::PostgresFixture::new();
    let app = setup(Engine::open_postgres(&fixture.url).unwrap());
    let source = app
        .engine_read()
        .unwrap()
        .history("alice", "secret")
        .unwrap()
        .messages[0]
        .clone();
    let blocker = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
    let hold = blocker.unchecked_transaction().unwrap();
    hold.execute_batch("LOCK TABLE messages IN ACCESS EXCLUSIVE MODE")
        .unwrap();
    let changing = tokio::spawn({
        let app = app.clone();
        let id = source.id.clone();
        async move { command(&app, None, &format!("/sudo /edit {id} forbidden edit")).await }
    });
    let inspect = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let waiting: bool = inspect.query_row("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%FROM messages%')", [], |row| row.get(0)).unwrap();
        if waiting {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "lookup did not reach reader"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(app.engine.try_read().is_ok());
    assert!(!app.engine_read().unwrap().is_su(Some("bob")));
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        tokio::task::spawn_blocking({
            let app = app.clone();
            move || {
                app.engine()
                    .unwrap()
                    .execute(None, None, "/revoke bob @global x:command.sudo")
            }
        }),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    hold.rollback().unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(3), changing)
        .await
        .unwrap()
        .unwrap();
    assert!(result.is_err(), "revoked sudo edit committed");
    let engine = app.engine_read().unwrap();
    assert!(!engine.is_su(Some("bob")));
    assert_eq!(
        engine.history("alice", "secret").unwrap().messages[0].text,
        source.text
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_cancelled_elevated_writes_publish_before_queued_revocation() {
    for cleanup in [false, true] {
        let fixture = crate::engine::database::tests::PostgresFixture::new();
        let app = setup(Engine::open_postgres(&fixture.url).unwrap());
        let source = app
            .engine_read()
            .unwrap()
            .history("alice", "secret")
            .unwrap()
            .messages[0]
            .clone();
        let blocker = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
        let hold = blocker.unchecked_transaction().unwrap();
        if cleanup {
            hold.query_row(
                "SELECT key FROM entities WHERE kind='room' AND key='secret' FOR UPDATE",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        } else {
            hold.query_row(
                "SELECT sequence FROM counters WHERE id=1 FOR UPDATE",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        }
        let mut notices = app.changes.subscribe();
        let changing = tokio::spawn({
            let app = app.clone();
            async move {
                command(
                    &app,
                    Some("secret"),
                    &if cleanup {
                        "/sudo /clean -1 #secret".into()
                    } else {
                        format!("/sudo /reply {} accepted reply", source.id)
                    },
                )
                .await
            }
        });
        let inspect = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let waiting: bool = inspect.query_row(if cleanup {
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%entities%')"
            } else {
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'UPDATE counters%')"
            }, [], |row| row.get(0)).unwrap();
            if waiting {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "elevated write did not reach transaction"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(app.engine.try_read().is_ok());
        assert!(!app.engine_read().unwrap().is_su(Some("bob")));
        assert!(notices.try_recv().is_err());
        changing.abort();
        assert!(changing.await.unwrap_err().is_cancelled());
        let revoke = tokio::task::spawn_blocking({
            let app = app.clone();
            move || {
                app.engine()
                    .unwrap()
                    .execute(None, None, "/revoke bob @global x:command.sudo")
            }
        });
        hold.rollback().unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), revoke)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let engine = app.engine_read().unwrap();
        let history = engine.history("alice", "secret").unwrap();
        assert!(!engine.is_su(Some("bob")));
        if cleanup {
            assert!(history.messages.is_empty());
            assert!(
                engine
                    .data
                    .policy
                    .audit
                    .iter()
                    .any(|audit| audit.operation == "/clean")
            );
        } else {
            assert_eq!(history.messages.len(), 2);
            assert_eq!(history.messages[1].text, "accepted reply");
            assert_eq!(history.messages[1].from, "bob");
        }
        assert!(matches!(
            notices.try_recv().unwrap(),
            Change::Conversation(_)
        ));
        assert!(notices.try_recv().is_err());
    }
}

#[tokio::test]
async fn global_message_actions_reuse_the_lookup_when_the_revision_is_unchanged() {
    let app = setup(Engine::open(std::path::Path::new(":memory:")).unwrap());
    let (message, history) = {
        let engine = app.engine_read().unwrap();
        (
            engine.history("alice", "secret").unwrap().messages[0]
                .id
                .clone(),
            engine.history_store().clone(),
        )
    };
    let before = history.stats().unwrap()["pools"]["operations"]["history_reads"]["succeeded"]
        .as_u64()
        .unwrap();
    command(&app, None, &format!("/sudo /edit {message} replaced once"))
        .await
        .unwrap();
    let after = history.stats().unwrap()["pools"]["operations"]["history_reads"]["succeeded"]
        .as_u64()
        .unwrap();
    assert_eq!(
        after - before,
        1,
        "one message lookup, without a second database read"
    );
    assert_eq!(
        app.engine_read()
            .unwrap()
            .history("alice", "secret")
            .unwrap()
            .messages[0]
            .text,
        "replaced once"
    );
}

#[tokio::test]
async fn queued_global_edit_refreshes_a_reply_scrubbed_after_the_early_lookup() {
    let app = setup(Engine::open(std::path::Path::new(":memory:")).unwrap());
    let (quoted, source, history, id) = {
        let mut engine = app.engine().unwrap();
        let quoted = engine.history("alice", "secret").unwrap().messages[0]
            .id
            .clone();
        engine
            .execute(
                Some("alice"),
                Some("secret"),
                &format!("/reply {quoted} original reply"),
            )
            .unwrap();
        let page = engine.history("alice", "secret").unwrap();
        (
            quoted,
            page.messages[1].id.clone(),
            engine.history_store().clone(),
            engine.data.rooms["secret"].id.clone(),
        )
    };
    let ordering = app.mutations.conversation(id.clone()).await.unwrap();
    let before = history.stats().unwrap()["pools"]["operations"]["history_reads"]["succeeded"]
        .as_u64()
        .unwrap();
    let queued = tokio::spawn({
        let app = app.clone();
        let source = source.clone();
        async move {
            command(
                &app,
                None,
                &format!("/sudo /edit {source} replacement reply"),
            )
            .await
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while history.stats().unwrap()["pools"]["operations"]["history_reads"]["succeeded"]
            .as_u64()
            .unwrap()
            == before
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!queued.is_finished());
    let original = history.locate_async(&quoted).await.unwrap().unwrap().1;
    let _audit = app.mutations.audit.lock().await;
    let pending = app
        .with_conversation(&ordering, move |engine| {
            let plan = engine.action_plan(None, None, &format!("/retract {quoted}"))?;
            engine.prepare_retraction(None, None, &plan.action, &id, &original)
        })
        .await
        .unwrap();
    pending.save().await.unwrap();
    app.with_conversation(&ordering, move |engine| engine.publish_history(pending))
        .await
        .unwrap();
    drop(ordering);
    queued.await.unwrap().unwrap();
    let page = app
        .engine_read()
        .unwrap()
        .history("alice", "secret")
        .unwrap();
    assert_eq!(page.messages.len(), 1);
    assert_eq!(page.messages[0].text, "replacement reply");
    assert!(
        page.messages[0].reply.is_none(),
        "an early cached lookup must not resurrect a deleted quote"
    );
}
