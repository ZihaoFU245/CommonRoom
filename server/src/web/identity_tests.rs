use super::tests::command;
use super::*;
use crate::web::media::tests::app;

fn reject_identity(
    db: &crate::engine::database::Connection,
    postgres: bool,
    rename: bool,
    reject: bool,
) {
    db.execute_batch(match (postgres, rename, reject) {
        (true, true, true) => "CREATE FUNCTION reject_identity() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.kind='account' AND NEW.key='robert' THEN RAISE EXCEPTION 'injected'; END IF; RETURN NEW; END $$; CREATE TRIGGER reject_identity BEFORE INSERT ON entities FOR EACH ROW EXECUTE FUNCTION reject_identity();",
        (false, true, true) => "CREATE TRIGGER reject_identity BEFORE INSERT ON entities WHEN NEW.kind='account' AND NEW.key='robert' BEGIN SELECT RAISE(ABORT,'injected'); END;",
        (true, false, true) => "CREATE FUNCTION reject_identity() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected'; END $$; CREATE TRIGGER reject_identity BEFORE DELETE ON read_positions FOR EACH ROW EXECUTE FUNCTION reject_identity();",
        (false, false, true) => "CREATE TRIGGER reject_identity BEFORE DELETE ON read_positions BEGIN SELECT RAISE(ABORT,'injected'); END;",
        (true, true, false) => "DROP TRIGGER reject_identity ON entities; DROP FUNCTION reject_identity();",
        (true, false, false) => "DROP TRIGGER reject_identity ON read_positions; DROP FUNCTION reject_identity();",
        (false, _, false) => "DROP TRIGGER reject_identity;",
    }).unwrap();
}
async fn contract(mut app: App, db: crate::engine::database::Connection, postgres: bool) {
    app.io_jobs = Arc::new(tokio::sync::Semaphore::new(64));
    let (account, direct_id, media_id, sequence) = {
        let mut engine = app.engine().unwrap();
        engine.execute(None, None, "/grant alice su").unwrap();
        for room in ["shared", "reaction", "untouched"] {
            engine
                .execute(None, None, &format!("/room new {room}"))
                .unwrap();
        }
        for user in ["alice", "bob", "carol"] {
            engine
                .execute(None, None, &format!("/room add {user} #shared"))
                .unwrap();
        }
        engine
            .execute(None, None, "/room add bob #reaction")
            .unwrap();
        engine
            .execute(None, None, "/room owner bob #shared")
            .unwrap();
        let conversation = engine.data.rooms["shared"].id.clone();
        let mut media = engine
            .allocate_media("bob", &conversation, "photo.png", 8)
            .unwrap();
        media.ready = true;
        media.object_key = media.final_key();
        media.content_type = "image/png".into();
        engine.finish_media("bob", &media).unwrap();
        engine
            .execute(
                Some("bob"),
                Some("shared"),
                &format!("by bob ![photo](/api/media/{}/file)", media.id),
            )
            .unwrap();
        let source = engine.history("alice", "shared").unwrap().messages[0]
            .id
            .clone();
        engine
            .execute(
                Some("alice"),
                Some("shared"),
                &format!("/reply {source} @bob quoted"),
            )
            .unwrap();
        engine
            .execute(None, Some("reaction"), "reaction only")
            .unwrap();
        let reacting = engine.history("bob", "reaction").unwrap().messages[0]
            .id
            .clone();
        engine
            .execute(
                Some("bob"),
                Some("reaction"),
                &format!("/react {reacting} 👍"),
            )
            .unwrap();
        engine
            .execute(Some("bob"), None, "/tell carol private")
            .unwrap();
        let direct = engine.data.private["bob:carol"].id.clone();
        let through = engine.history("carol", "@direct:bob").unwrap().messages[0].sequence;
        engine.mark_read("carol", "@direct:bob", through).unwrap();
        let through = engine
            .history("bob", "shared")
            .unwrap()
            .messages
            .last()
            .unwrap()
            .sequence;
        engine.mark_read("bob", "shared", through).unwrap();
        engine
            .execute(None, None, "/bot new helper #shared")
            .unwrap();
        engine.execute(None, None, "/bot owner helper bob").unwrap();
        (
            engine.account_id("bob").unwrap().to_owned(),
            direct,
            media.id,
            engine.data.next_sequence,
        )
    };
    let mut notices = app.changes.subscribe();
    let before = app.engine_read().unwrap().data.clone();
    let revision = app.engine_read().unwrap().revision;
    reject_identity(&db, postgres, true, true);
    assert!(
        command(&app, "alice", None, "/user rename bob robert")
            .await
            .is_err()
    );
    assert!(app.engine_read().unwrap().data == before);
    assert_eq!(app.engine_read().unwrap().revision, revision);
    assert!(notices.try_recv().is_err());
    assert_eq!(
        app.engine_read().unwrap().session("bob").as_deref(),
        Some("bob")
    );
    let persisted: String = db
        .query_row(
            "SELECT json FROM messages ORDER BY sequence LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&persisted).unwrap()["from"],
        "bob"
    );
    reject_identity(&db, postgres, true, false);
    assert_eq!(
        command(&app, "alice", None, "/user rename bob robert")
            .await
            .unwrap(),
        "Renamed bob to robert."
    );
    assert!(matches!(notices.try_recv().unwrap(), Change::Metadata(_)));
    {
        let engine = app.engine_read().unwrap();
        assert_eq!(engine.account_id("robert"), Some(account.as_str()));
        assert_eq!(engine.session("bob").as_deref(), Some("robert"));
        assert_eq!(engine.data.private["carol:robert"].id, direct_id);
        assert_eq!(engine.data.rooms["shared"].owner_id, account);
        assert!(engine.data.rooms["shared"].members.contains("robert"));
        for room in ["shared", "reaction"] {
            assert_eq!(
                engine.data.rooms[room].revision,
                before.rooms[room].revision + 1
            );
        }
        assert_eq!(
            engine.data.rooms["untouched"].revision,
            before.rooms["untouched"].revision
        );
        let history = engine.history("robert", "shared").unwrap();
        assert_eq!(history.messages[0].from, "robert");
        assert_eq!(history.messages[0].author_id, account);
        assert_eq!(history.messages[1].reply.as_ref().unwrap().from, "robert");
        assert!(history.messages[1].mentions.contains("robert"));
        assert!(
            engine.history("robert", "reaction").unwrap().messages[0].reactions["👍"]
                .contains("robert")
        );
        assert_eq!(engine.unreads("robert").unwrap()["shared"].count, 0);
        assert_eq!(engine.unreads("carol").unwrap()["@direct:robert"].count, 0);
        assert_eq!(
            engine.bot_owner(&engine.data.users["helper"].id).as_deref(),
            Some("robert")
        );
        assert_eq!(engine.data.next_sequence, sequence);
    }
    let revision = app.engine_read().unwrap().revision;
    assert_eq!(
        command(&app, "alice", None, "/user rename robert robert")
            .await
            .unwrap(),
        "Username unchanged."
    );
    assert_eq!(app.engine_read().unwrap().revision, revision);
    assert!(notices.try_recv().is_err());
    let before = app.engine_read().unwrap().data.clone();
    reject_identity(&db, postgres, false, true);
    assert!(
        command(&app, "alice", None, "/user delete robert")
            .await
            .is_err()
    );
    assert!(app.engine_read().unwrap().data == before);
    assert_eq!(app.engine_read().unwrap().revision, revision);
    assert_eq!(
        app.engine_read().unwrap().session("bob").as_deref(),
        Some("robert")
    );
    assert!(notices.try_recv().is_err());
    let persisted: String = db
        .query_row(
            "SELECT json FROM messages ORDER BY sequence LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&persisted).unwrap()["from"],
        "robert"
    );
    reject_identity(&db, postgres, false, false);
    command(&app, "alice", None, "/user delete robert")
        .await
        .unwrap();
    assert!(matches!(notices.try_recv().unwrap(), Change::Metadata(_)));
    {
        let engine = app.engine_read().unwrap();
        assert!(engine.account_id("robert").is_none());
        assert!(engine.session("bob").is_none());
        assert!(!engine.data.private.contains_key("carol:robert"));
        assert_eq!(engine.data.rooms["shared"].owner_id, "console");
        let history = engine.history("alice", "shared").unwrap();
        assert_eq!(history.messages[0].from, "robert (deleted)");
        assert_eq!(history.messages[0].author_id, account);
        assert_eq!(
            history.messages[1].reply.as_ref().unwrap().from,
            "robert (deleted)"
        );
        assert!(!history.messages[1].mentions.contains("robert"));
        assert!(
            engine
                .bot_owner_id(&engine.data.users["helper"].id)
                .is_none()
        );
        assert_eq!(engine.data.next_sequence, sequence);
    }
    let store = app.engine_read().unwrap().media_store();
    let media = store.load(&media_id).await.unwrap();
    assert!(media.0.ready && media.1);
    assert_eq!(media.0.uploader, account);
    assert_eq!(db.query_row("SELECT count(*) FROM read_positions WHERE username='robert' OR conversation='dm:carol:robert'", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    command(
        &app,
        "alice",
        None,
        "/user new robert replacement-password user",
    )
    .await
    .unwrap();
    let engine = app.engine_read().unwrap();
    assert_ne!(engine.account_id("robert"), Some(account.as_str()));
    assert!(engine.session("bob").is_none());
    assert_eq!(
        engine.history("alice", "shared").unwrap().messages[0].author_id,
        account
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_account_identity_changes_keep_history_cursors_and_sessions_atomic() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("chat.sqlite");
    contract(
        app(Engine::open(&path).unwrap()),
        crate::engine::database::Connection::open(&path).unwrap(),
        false,
    )
    .await;
    let engine = Engine::open(&path).unwrap();
    assert_eq!(
        engine.history("alice", "shared").unwrap().messages[0].from,
        "robert (deleted)"
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_native_account_identity_changes_keep_history_cursors_and_sessions_atomic() {
    let fixture = crate::engine::database::tests::PostgresFixture::new();
    contract(
        app(Engine::open_postgres(&fixture.url).unwrap()),
        crate::engine::database::Connection::postgres(&fixture.url, false).unwrap(),
        true,
    )
    .await;
    let engine = Engine::open_postgres(&fixture.url).unwrap();
    assert_eq!(
        engine.history("alice", "shared").unwrap().messages[0].from,
        "robert (deleted)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_cancelled_identity_operations_release_engine_and_order_replacement() {
    for rename in [true, false] {
        let fixture = crate::engine::database::tests::PostgresFixture::new();
        let mut app = app(Engine::open_postgres(&fixture.url).unwrap());
        app.io_jobs = Arc::new(tokio::sync::Semaphore::new(64));
        let account = {
            let mut engine = app.engine().unwrap();
            engine.execute(None, None, "/grant alice su").unwrap();
            engine.execute(None, None, "/room new shared").unwrap();
            engine.execute(None, None, "/room add bob #shared").unwrap();
            engine
                .execute(Some("bob"), Some("shared"), "identity source")
                .unwrap();
            let sequence = engine.history("bob", "shared").unwrap().messages[0].sequence;
            engine.mark_read("bob", "shared", sequence).unwrap();
            engine.account_id("bob").unwrap().to_owned()
        };
        let blocker = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
        let hold = blocker.unchecked_transaction().unwrap();
        if rename {
            hold.execute_batch("LOCK TABLE message_accounts IN ACCESS EXCLUSIVE MODE")
                .unwrap();
        } else {
            hold.query_row(
                "SELECT username FROM read_positions WHERE username='bob' FOR UPDATE",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        }
        let mut notices = app.changes.subscribe();
        let changing = tokio::spawn({
            let app = app.clone();
            async move {
                command(
                    &app,
                    "alice",
                    None,
                    if rename {
                        "/user rename bob robert"
                    } else {
                        "/user delete bob"
                    },
                )
                .await
            }
        });
        let inspect = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
        loop {
            let waiting: bool = inspect.query_row(if rename {
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT conversation FROM message_accounts%')"
            } else {
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'DELETE FROM read_positions WHERE username%')"
            }, [], |row| row.get(0)).unwrap();
            if waiting {
                break;
            }
            if std::time::Instant::now() >= deadline || changing.is_finished() {
                hold.rollback().unwrap();
                let result = tokio::time::timeout(std::time::Duration::from_secs(2), changing)
                    .await
                    .unwrap()
                    .unwrap();
                assert!(
                    waiting,
                    "identity operation did not reach expected lock: {result:?}"
                );
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(app.engine.try_read().is_ok());
        assert_eq!(
            app.engine_read().unwrap().account_id("bob"),
            Some(account.as_str())
        );
        assert_eq!(
            app.engine_read().unwrap().session("bob").as_deref(),
            Some("bob")
        );
        assert!(notices.try_recv().is_err());
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_secs(1),
                command(&app, "bob", None, "/whoami")
            )
            .await
            .unwrap()
            .is_ok()
        );
        changing.abort();
        assert!(changing.await.unwrap_err().is_cancelled());
        let queued = tokio::spawn({
            let app = app.clone();
            async move {
                command(
                    &app,
                    "alice",
                    None,
                    if rename {
                        "/user disable robert"
                    } else {
                        "/user new bob replacement-password user"
                    },
                )
                .await
            }
        });
        tokio::task::yield_now().await;
        assert!(!queued.is_finished());
        hold.rollback().unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(4), queued)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let engine = app.engine_read().unwrap();
        if rename {
            assert_eq!(engine.account_id("robert"), Some(account.as_str()));
            assert!(engine.data.users["robert"].disabled);
        } else {
            assert_ne!(engine.account_id("bob"), Some(account.as_str()));
            assert_eq!(
                engine.history("alice", "shared").unwrap().messages[0].from,
                "bob (deleted)"
            );
        }
        assert!(engine.session("bob").is_none());
        assert!(matches!(notices.try_recv().unwrap(), Change::Metadata(_)));
        assert!(matches!(notices.try_recv().unwrap(), Change::Metadata(_)));
        assert!(notices.try_recv().is_err());
    }
}
