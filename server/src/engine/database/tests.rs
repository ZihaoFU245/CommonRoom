//! PostgreSQL tests use unique schemas inside an explicitly supplied test DB.
use super::*;
use crate::engine::Engine;

pub(crate) struct PostgresFixture {
    pub(crate) url: String,
    admin: Connection,
    schema: String,
}
impl PostgresFixture {
    pub(crate) fn new() -> Self {
        let raw = std::env::var("CHAT_TEST_POSTGRES_URL")
            .expect("Set CHAT_TEST_POSTGRES_URL to an isolated PostgreSQL test database");
        let admin = Connection::postgres(&raw, false).unwrap();
        let schema = format!("chat_test_{}", uuid::Uuid::new_v4().simple());
        admin
            .execute_batch(&format!("CREATE SCHEMA {schema}"))
            .unwrap();
        let mut url = url::Url::parse(&raw).unwrap();
        url.query_pairs_mut()
            .append_pair("options", &format!("-csearch_path={schema}"));
        Self {
            url: url.into(),
            admin,
            schema,
        }
    }
}
impl Drop for PostgresFixture {
    fn drop(&mut self) {
        let result = self
            .admin
            .execute_batch(&format!("DROP SCHEMA {} CASCADE", self.schema));
        if !std::thread::panicking() {
            result.unwrap();
        }
    }
}

#[test]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
fn postgres_backend_contract() {
    let fixture = PostgresFixture::new();
    let mut e = Engine::open_postgres(&fixture.url).unwrap();
    assert!(Engine::open_postgres(&fixture.url).is_err());
    for (name, admin) in [("alice", true), ("bob", false)] {
        e.provision(name, "test-hash".into(), admin, false).unwrap();
    }
    e.execute(Some("alice"), None, "/new room lobby").unwrap();
    e.execute(Some("alice"), None, "/add bob #lobby").unwrap();
    e.execute(
        Some("alice"),
        Some("lobby"),
        "Unicode 👩‍💻 你好 @bob \0 embedded",
    )
    .unwrap();
    let first = e.history("bob", "lobby").unwrap().messages[0].clone();
    e.execute(
        Some("bob"),
        Some("lobby"),
        &format!("/reply {} reply", first.id),
    )
    .unwrap();
    e.execute(
        Some("bob"),
        Some("lobby"),
        &format!("/react {} 👍", first.id),
    )
    .unwrap();
    assert!(e.history("alice", "lobby").unwrap().messages[0].reactions["👍"].contains("bob"));
    e.execute(
        Some("alice"),
        Some("lobby"),
        &format!("/edit {} edited", first.id),
    )
    .unwrap();
    e.execute(Some("alice"), None, "/tell bob private").unwrap();
    e.login("session".into(), "bob", "test-hash", None).unwrap();
    let room = e.data.rooms["lobby"].id.clone();
    e.store_read_position("bob", "room:lobby", first.sequence)
        .unwrap();
    e.store_read_position("bob", "room:lobby", 0).unwrap();
    assert_eq!(e.store.db.lock().unwrap().query_row::<u64,_,_>("SELECT sequence FROM read_positions WHERE username='bob' AND conversation='room:lobby'",[],|r|r.get(0)).unwrap(),first.sequence);
    e.execute(None, None, "/user rename bob robert").unwrap();
    assert!(e.history("robert", "lobby").unwrap().messages[0].reactions["👍"].contains("robert"));
    assert_eq!(e.session("session").as_deref(), Some("robert"));
    let sequence = e.data.next_sequence;
    let revision = e.data.rooms["lobby"].revision;
    let snapshots = e.snapshot("robert").unwrap();
    assert!(!snapshots.rooms.is_empty());
    let unread = e
        .history_store()
        .unread(&room, "robert", 0, revision)
        .unwrap();
    assert_eq!(unread.count, 1);
    drop(e);
    let mut e = Engine::open_postgres(&fixture.url).unwrap();
    assert_eq!(e.data.next_sequence, sequence);
    assert_eq!(e.data.rooms["lobby"].revision, revision);
    assert_eq!(e.session("session").as_deref(), Some("robert"));
    assert_eq!(e.history("robert", "lobby").unwrap().messages.len(), 2);
    e.execute(
        Some("alice"),
        Some("lobby"),
        &format!("/retract {}", first.id),
    )
    .unwrap();
    assert!(
        e.history("robert", "lobby").unwrap().messages[0]
            .reply
            .is_none()
    );
    e.set_limits(64, 64, 1).unwrap();
    e.execute(Some("alice"), Some("lobby"), "retained").unwrap();
    assert_eq!(e.history("robert", "lobby").unwrap().messages.len(), 1);
    e.execute(None, None, "/clean -1 #lobby").unwrap();
    assert!(e.history("robert", "lobby").unwrap().messages.is_empty());
    e.execute(None, None, "/user delete robert").unwrap();
    assert!(e.session("session").is_none());
    drop(e);
}

fn populate(e: &mut Engine) -> crate::bots::models::MessageInput {
    for (name, admin) in [("alice", true), ("bob", false)] {
        e.provision(name, "test-hash".into(), admin, false).unwrap();
    }
    e.execute(None, None, "/room new lobby").unwrap();
    for name in ["alice", "bob"] {
        e.execute(None, None, &format!("/room add {name} #lobby"))
            .unwrap();
    }
    e.execute(
        Some("alice"),
        Some("lobby"),
        "Unicode 👩‍💻 @bob \0 zero \\\"quoted\"",
    )
    .unwrap();
    let first = e.history("bob", "lobby").unwrap().messages[0].clone();
    e.execute(
        Some("bob"),
        Some("lobby"),
        &format!("/reply {} reply \0 text", first.id),
    )
    .unwrap();
    e.execute(
        Some("bob"),
        Some("lobby"),
        &format!("/react {} 👍", first.id),
    )
    .unwrap();
    e.execute(Some("alice"), None, "/tell bob private").unwrap();
    e.login("persisted-session".into(), "bob", "test-hash", None)
        .unwrap();
    e.mark_read("bob", "lobby", first.sequence).unwrap();
    e.record_auth_event(&crate::engine::AuthEvent::new(
        Some("bob"),
        None,
        crate::engine::AuthOutcome::Success,
    ))
    .unwrap();
    e.execute(None, Some("lobby"), "/bot new helper").unwrap();
    let issuer = e
        .execute(None, None, "/bot token new helper transfer-issuer")
        .unwrap()
        .lines()
        .last()
        .unwrap()
        .to_owned();
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::AUTHORIZATION,
        format!("Bearer {issuer}").parse().unwrap(),
    );
    let context = crate::bots::auth::Context::from_headers(e, &headers).unwrap();
    e.issue_bot_credential(
        &context,
        crate::bots::models::CredentialInput {
            label: "transfer-worker".into(),
            expires: crate::bots::now() + 600,
            conversations: Some([e.data.rooms["lobby"].id.clone()].into()),
            operations: ["read".into(), "send".into()].into(),
        },
    )
    .unwrap();
    let request = crate::bots::models::MessageInput {
        conversation_id: e.data.rooms["lobby"].id.clone(),
        text: "bot answer".into(),
        reply_to: Some(first.id),
        markdown: true,
    };
    e.send_bot_message("helper", &request, Some("send-key"))
        .unwrap();
    let account = e.data.users["helper"].id.clone();
    e.bot_command_once(&account, "command-key", "digest", |e| {
        e.execute(None, None, "/room new created")
            .map_err(crate::bots::error::Error::storage)
    })
    .unwrap();
    let room = e.data.rooms["lobby"].id.clone();
    let mut media = e.allocate_media("alice", &room, "photo.png", 8).unwrap();
    media.ready = true;
    media.object_key = media.final_key();
    media.content_type = "image/png".into();
    e.finish_media("alice", &media).unwrap();
    e.execute(
        Some("alice"),
        Some("lobby"),
        &format!("![file](/api/media/{}/file)", media.id),
    )
    .unwrap();
    e.execute(None, None, "/user set alice custom Unicodevalue")
        .unwrap();
    let mut avatar = e.allocate_avatar("alice", 8).unwrap();
    avatar.ready = true;
    avatar.object_key = avatar.final_key();
    avatar.content_type = "image/png".into();
    e.finish_avatar("alice", &avatar).unwrap();
    request
}

#[test]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
fn postgres_sqlite_migration_preserves_every_table_and_retries() {
    assert_eq!(SQLITE_SCHEMA_VERSION, 20);
    assert_eq!(POSTGRES_SCHEMA_VERSION, 1);
    let fixture = PostgresFixture::new();
    let source = tempfile::tempdir().unwrap();
    let path = source.path().join("chat.sqlite");
    let mut e = Engine::open(&path).unwrap();
    let request = populate(&mut e);
    assert!(Engine::migrate_sqlite(&path, &fixture.url).is_err()); // active owner
    let expected_data = e.data.clone();
    let expected_cursors = e.store.read_positions.clone();
    let expected_history = serde_json::to_value(e.history("bob", "lobby").unwrap()).unwrap();
    drop(e);
    let original = std::fs::read(&path).unwrap();
    {
        let source = Connection::open_with_access(&path, SqliteAccess::ReadOnly).unwrap();
        assert_eq!(
            source
                .query_row::<u32, _, _>("PRAGMA user_version", [], |r| r.get(0))
                .unwrap(),
            SQLITE_SCHEMA_VERSION
        );
    }
    let report = Engine::migrate_sqlite(&path, &fixture.url).unwrap();
    assert_eq!(report.rows.len(), 14);
    assert!(report.rows.values().all(|rows| *rows > 0));
    assert_eq!(std::fs::read(&path).unwrap(), original);
    let mut pg = Engine::open_postgres(&fixture.url).unwrap();
    assert_eq!(
        pg.store
            .db
            .lock()
            .unwrap()
            .query_row::<u32, _, _>("SELECT version FROM chat_schema", [], |r| r.get(0),)
            .unwrap(),
        POSTGRES_SCHEMA_VERSION
    );
    assert!(pg.data == expected_data);
    assert_eq!(pg.store.read_positions, expected_cursors);
    assert_eq!(
        serde_json::to_value(pg.history("bob", "lobby").unwrap()).unwrap(),
        expected_history
    );
    assert_eq!(pg.session("persisted-session").as_deref(), Some("bob"));
    assert!(
        pg.send_bot_message("helper", &request, Some("send-key"))
            .unwrap()
            .replayed
    );
    let helper = pg.data.users["helper"].id.clone();
    let mut executed = false;
    assert!(
        pg.bot_command_once(&helper, "command-key", "digest", |_| {
            executed = true;
            Ok("unexpected".into())
        })
        .unwrap()
        .replayed
    );
    assert!(!executed);
    pg.record_auth_event(&crate::engine::AuthEvent::new(
        Some("alice"),
        None,
        crate::engine::AuthOutcome::Logout,
    ))
    .unwrap();
    pg.execute(None, None, "/room rename #created renamed")
        .unwrap();
    pg.execute(Some("alice"), Some("lobby"), "new post-migration message")
        .unwrap();
    assert!(pg.data.next_sequence > expected_data.next_sequence);
    pg.execute(
        Some("alice"),
        Some("lobby"),
        &format!("/retract {}", request.reply_to.as_ref().unwrap()),
    )
    .unwrap();
    assert!(
        pg.history("bob", "lobby")
            .unwrap()
            .messages
            .iter()
            .any(|message| message.text.contains('\0') && message.reply.is_none())
    );
    let revision = pg.data.rooms["lobby"].revision;
    drop(pg);
    assert!(Engine::migrate_sqlite(&path, &fixture.url).is_err()); // never merge/overwrite
    let pg = Engine::open_postgres(&fixture.url).unwrap();
    assert_eq!(pg.data.rooms["lobby"].revision, revision);
    drop(pg);
}

#[test]
fn migration_rejects_unsupported_source_without_connecting_or_rewriting() {
    let source = tempfile::tempdir().unwrap();
    let path = source.path().join("chat.sqlite");
    assert!(Engine::migrate_sqlite(&path, "invalid-target").is_err());
    assert!(!path.exists());
    for version in [18, 19, 21] {
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE IF NOT EXISTS marker(value TEXT);")
            .unwrap();
        db.execute_batch(&format!("PRAGMA user_version={version}"))
            .unwrap();
        drop(db);
        let before = std::fs::read(&path).unwrap();
        assert!(
            Engine::migrate_sqlite(&path, "invalid-target")
                .unwrap_err()
                .contains("v20")
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}

#[test]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
fn postgres_schema_rejection_and_connection_loss_fail_closed() {
    let fixture = PostgresFixture::new();
    let db = Connection::postgres(&fixture.url, false).unwrap();
    db.execute_batch(
        "CREATE TABLE unrelated(value TEXT); INSERT INTO unrelated VALUES('preserved');",
    )
    .unwrap();
    assert!(Engine::open_postgres(&fixture.url).is_err());
    assert_eq!(
        db.query_row::<String, _, _>("SELECT value FROM unrelated", [], |r| r.get(0))
            .unwrap(),
        "preserved"
    );
    db.execute_batch("DROP TABLE unrelated; CREATE TABLE chat_schema(id BIGINT,version BIGINT); INSERT INTO chat_schema VALUES(1,18);").unwrap();
    assert!(Engine::open_postgres(&fixture.url).is_err());
    assert_eq!(
        db.query_row::<u32, _, _>("SELECT version FROM chat_schema", [], |r| r.get(0))
            .unwrap(),
        18
    );
    db.execute_batch("DROP TABLE chat_schema").unwrap();
    let mut e = Engine::open_postgres(&fixture.url).unwrap();
    populate(&mut e);
    let before = e.data.clone();
    // Ownership is held on a dedicated session, independent of write leases.
    let pid = e.store.db.ownership_pid().unwrap();
    assert!(
        db.query_row::<bool, _, _>("SELECT pg_terminate_backend($1)", [pid], |r| r.get(0))
            .unwrap()
    );
    assert!(
        e.execute(Some("alice"), Some("lobby"), "must not commit")
            .is_err()
    );
    assert!(e.data == before);
    assert!(e.ensure_recovered().is_err());
    drop(e);
    let e = Engine::open_postgres(&fixture.url).unwrap();
    assert!(e.data == before);
    assert!(
        !e.history("alice", "lobby")
            .unwrap()
            .messages
            .iter()
            .any(|m| m.text == "must not commit")
    );
    drop(e);
}

#[test]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
fn postgres_failed_transfer_leaves_empty_target_and_unchanged_source() {
    let fixture = PostgresFixture::new();
    let source = tempfile::tempdir().unwrap();
    let path = source.path().join("chat.sqlite");
    {
        let mut e = Engine::open(&path).unwrap();
        populate(&mut e);
        e.store
            .db
            .lock()
            .unwrap()
            .execute("UPDATE retention SET count=count+1", [])
            .unwrap();
    }
    let original = std::fs::read(&path).unwrap();
    assert!(
        Engine::migrate_sqlite(&path, &fixture.url)
            .unwrap_err()
            .contains("retention")
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    let destination = Connection::postgres(&fixture.url, false).unwrap();
    assert!(
        !destination
            .query_row::<bool, _, _>("SELECT to_regclass('chat_schema') IS NOT NULL", [], |r| r
                .get(0))
            .unwrap()
    );
    // A failed verification must release the PostgreSQL ownership lock as well.
    {
        let e = Engine::open(&path).unwrap();
        e.store
            .db
            .lock()
            .unwrap()
            .execute("UPDATE retention SET count=count-1", [])
            .unwrap();
    }
    assert!(Engine::migrate_sqlite(&path, &fixture.url).is_ok());
}

#[test]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
fn postgres_media_receipts_and_failures_are_atomic() {
    let fixture = PostgresFixture::new();
    let mut e = Engine::open_postgres(&fixture.url).unwrap();
    let request = populate(&mut e);
    let room = e.data.rooms["lobby"].id.clone();
    let id: String = e
        .store
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT id FROM media WHERE conversation!='@avatar'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let ticket = e.prepare_media_download("bob", &id).unwrap();
    e.validate_media_download("bob", &ticket).unwrap();
    e.store
        .db
        .lock()
        .unwrap()
        .execute("UPDATE media SET deleting=1 WHERE id=$1", [&id])
        .unwrap();
    assert!(e.validate_media_download("bob", &ticket).is_err());
    e.store
        .db
        .lock()
        .unwrap()
        .execute("UPDATE media SET deleting=0 WHERE id=$1", [&id])
        .unwrap();
    let before = e.data.clone();
    let helper = e.data.users["helper"].id.clone();
    e.store.db.lock().unwrap().execute_batch("CREATE FUNCTION reject_receipt() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected'; END; $$; CREATE TRIGGER reject_command BEFORE INSERT ON bot_command_receipts FOR EACH ROW EXECUTE FUNCTION reject_receipt(); CREATE TRIGGER reject_send BEFORE INSERT ON bot_receipts FOR EACH ROW EXECUTE FUNCTION reject_receipt();").unwrap();
    assert!(
        e.bot_command_once(&helper, "fail", "digest", |e| e
            .execute(None, None, "/room new rollback")
            .map_err(crate::bots::error::Error::storage))
            .is_err()
    );
    assert!(e.data == before);
    assert!(
        e.send_bot_message("helper", &request, Some("failure"))
            .is_err()
    );
    assert!(e.data == before);
    assert!(e.bot_command_receipt(&helper, "fail").unwrap().is_none());
    e.store.db.lock().unwrap().execute_batch("DROP TRIGGER reject_command ON bot_command_receipts; DROP TRIGGER reject_send ON bot_receipts; DROP FUNCTION reject_receipt();").unwrap();
    assert!(
        !e.send_bot_message("helper", &request, Some("failure"))
            .unwrap()
            .replayed
    );
    let readers = e.history_store().clone();
    readers
        .read(|reader| {
            let snapshot = reader.unchecked_transaction()?;
            let before: u64 =
                snapshot.query_row("SELECT count(*) FROM messages", [], |r| r.get(0))?;
            e.execute(Some("alice"), Some("lobby"), "concurrent append")?;
            let after: u64 =
                snapshot.query_row("SELECT count(*) FROM messages", [], |r| r.get(0))?;
            assert_eq!(before, after); // repeatable snapshot across separate statements
            Ok(())
        })
        .unwrap();
    e.execute(None, None, "/clean -1 #lobby").unwrap();
    assert!(e.readable_media("bob", &id).is_err());
    let expired = e.expired_media_before(i64::MAX.unsigned_abs()).unwrap();
    assert!(expired.iter().any(|m| m.id == id));
    e.remove_media_record(&id).unwrap();
    e.media_staging_cleanup(i64::MAX.unsigned_abs()).unwrap();
    e.prepare_media_cleanup().unwrap();
    let avatar_id: String = e
        .store
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT id FROM media WHERE conversation='@avatar'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(e.readable_avatar("alice", &avatar_id).is_ok());
    assert_eq!(
        e.history_store()
            .unread(&room, "bob", 0, e.data.rooms["lobby"].revision)
            .unwrap()
            .count,
        0
    );
    drop(e);
}

#[test]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
fn postgres_pagination_and_incremental_unread_match_sql_oracle() {
    use crate::engine::HistoryRange;
    let fixture = PostgresFixture::new();
    let mut e = Engine::open_postgres(&fixture.url).unwrap();
    populate(&mut e);
    for i in 0..140 {
        e.execute(Some("alice"), Some("lobby"), &format!("page {i}"))
            .unwrap();
    }
    let first = e
        .history_page(
            "bob",
            "lobby",
            HistoryRange {
                start: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(first.messages.len(), 100);
    assert!(first.has_after);
    let last = first.messages.last().unwrap().sequence;
    let second = e
        .history_page(
            "bob",
            "lobby",
            HistoryRange {
                after: Some(last),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(second.messages.len(), 44);
    assert!(!second.has_after);
    let anchor = first.messages[80].sequence;
    let around = e
        .history_page(
            "bob",
            "lobby",
            HistoryRange {
                around: Some(anchor),
                limit: Some(7),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(around.messages.len(), 7);
    assert!(around.messages.iter().any(|m| m.sequence == anchor));
    let preceding = e
        .history_page(
            "bob",
            "lobby",
            HistoryRange {
                before: Some(anchor),
                limit: Some(10),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(preceding.messages.iter().all(|m| m.sequence < anchor));
    assert_eq!(preceding.messages.len(), 10);
    let initial = e.unreads("bob").unwrap()["lobby"].count;
    assert_eq!(initial, 142); // the initial read cursor already covers the first message
    e.mark_read("bob", "lobby", anchor).unwrap();
    let remaining = e.unreads("bob").unwrap()["lobby"].count;
    e.execute(Some("alice"), Some("lobby"), "new incoming")
        .unwrap();
    assert_eq!(e.unreads("bob").unwrap()["lobby"].count, remaining + 1);
    let id = &e.data.rooms["lobby"].id;
    let oracle:usize=e.store.db.lock().unwrap().query_row("SELECT count(*) FROM messages WHERE conversation=$1 AND sequence>$2 AND projection->>'from'!='bob'",params![id,anchor],|r|r.get(0)).unwrap();
    assert_eq!(e.unreads("bob").unwrap()["lobby"].count, oracle);
    let snapshot = e.snapshot("bob").unwrap();
    assert!(!snapshot.rooms.is_empty());
    drop(e);
}
