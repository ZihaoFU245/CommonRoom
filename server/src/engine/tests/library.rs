use super::tests::engine;
use super::*;
use database::LibraryAction;
fn run<T: Send>(
    future: impl std::future::Future<Output = Result<T, EngineError>> + Send,
) -> Result<T, EngineError> {
    database::block_on_storage(future)?
}

pub(crate) fn upload(engine: &Engine, user: &str) -> media::Media {
    let pending = engine
        .prepare_media_allocation(user, "@library", "sticker.png", 8, BTreeMap::new())
        .unwrap();
    run(pending.save()).unwrap();
    let mut media = pending.media;
    media.ready = true;
    media.content_type = "image/png".into();
    media.object_key = media.final_key();
    run(engine.media_store().ready(&media)).unwrap();
    media
}
fn change(
    engine: &Engine,
    user: &str,
    media: &media::Media,
    action: LibraryAction,
    source: Option<&str>,
    confirm: bool,
) -> Result<(), EngineError> {
    run(engine
        .media_store()
        .library_action(&database::LibraryWrite {
            account: engine.principal_id(Some(user)).unwrap(),
            id: &media.id,
            source,
            action,
            confirm_last: confirm,
            quota: engine.library_quota(user),
            config: &engine.media.config,
        }))
    .map(|_| ())
}
fn page(engine: &Engine, user: &str, owned: bool) -> database::LibraryPage {
    run(engine.media_store().library_page(&database::LibraryList {
        account: engine.principal_id(Some(user)).unwrap(),
        owned,
        recent: false,
        after: "",
        limit: 50,
        quota: engine.library_quota(user),
        conversations: &engine.library_read_scopes(user),
    }))
    .unwrap()
}

#[test]
fn library_upload_rejects_inactive_accounts_before_size_validation() {
    let mut engine = engine();
    engine.execute(None, None, "/disable bob").unwrap();
    for user in ["bob", "missing"] {
        assert!(matches!(
            engine.validate_library_upload(user, 0),
            Err(EngineError::Forbidden(_))
        ));
    }
    assert!(engine.validate_library_upload("alice", 0).is_err());
    assert!(engine.validate_library_upload("alice", 8).is_ok());
}

#[test]
fn library_multi_owner_multi_borrower_lifecycle_and_no_resurrection() {
    lifecycle(engine());
}

#[test]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
fn postgres_library_multi_owner_multi_borrower_lifecycle() {
    let fixture = database::tests::PostgresFixture::new();
    let mut engine = Engine::open_postgres(&fixture.url).unwrap();
    for (name, admin) in [("alice", true), ("bob", false)] {
        engine.provision(name, "hash".into(), admin, false).unwrap();
    }
    engine
        .execute(Some("alice"), None, "/new room lobby")
        .unwrap();
    engine
        .execute(Some("alice"), None, "/add bob #lobby")
        .unwrap();
    lifecycle(engine);
}

fn lifecycle(mut engine: Engine) {
    engine
        .provision("carol", "hash".into(), false, false)
        .unwrap();
    let media = upload(&engine, "alice");
    assert_eq!(page(&engine, "alice", true).usage, 8);
    assert_eq!(page(&engine, "alice", false).items.len(), 1);
    assert!(engine.readable_media("bob", &media.id).is_err());
    assert!(change(&engine, "bob", &media, LibraryAction::Save, None, false).is_err());
    let text = format!("![sticker](/api/media/{}/file)", media.id);
    engine.execute(Some("alice"), Some("lobby"), &text).unwrap();
    let room = engine.data.rooms["lobby"].id.clone();
    assert!(engine.readable_media("bob", &media.id).is_ok());
    for _ in 0..2 {
        change(
            &engine,
            "bob",
            &media,
            LibraryAction::Save,
            Some(&room),
            false,
        )
        .unwrap();
        change(&engine, "bob", &media, LibraryAction::Acquire, None, false).unwrap();
    }
    assert_eq!(page(&engine, "bob", false).items.len(), 1);
    assert_eq!(page(&engine, "bob", true).usage, 8);
    assert_eq!(page(&engine, "bob", true).items[0].owner_count, 2);
    engine
        .execute(None, None, "/room add carol #lobby")
        .unwrap();
    // A second borrower has independent library access after source access is lost.
    change(
        &engine,
        "carol",
        &media,
        LibraryAction::Save,
        Some(&room),
        false,
    )
    .unwrap();
    engine
        .execute(Some("alice"), Some("lobby"), "/kick carol #lobby")
        .unwrap();
    assert_eq!(page(&engine, "carol", false).usage, 0);
    engine
        .execute(Some("alice"), Some("lobby"), "/kick bob #lobby")
        .unwrap();
    assert!(engine.readable_media("bob", &media.id).is_ok());
    let recents = |engine: &Engine| {
        run(engine.media_store().library_page(&database::LibraryList {
            account: engine.principal_id(Some("alice")).unwrap(),
            owned: false,
            recent: true,
            after: "",
            limit: 50,
            quota: engine.library_quota("alice"),
            conversations: &engine.library_read_scopes("alice"),
        }))
        .unwrap()
    };
    assert_eq!(recents(&engine).items.len(), 1);
    change(&engine, "alice", &media, LibraryAction::Remove, None, false).unwrap();
    assert!(recents(&engine).items.is_empty());
    assert!(page(&engine, "alice", false).items.is_empty());
    assert_eq!(page(&engine, "alice", true).items.len(), 1);
    change(
        &engine,
        "alice",
        &media,
        LibraryAction::Release,
        None,
        false,
    )
    .unwrap();
    assert_eq!(page(&engine, "alice", true).usage, 0);
    assert!(engine.readable_media("carol", &media.id).is_ok());
    assert!(change(&engine, "bob", &media, LibraryAction::Release, None, false).is_err());
    change(&engine, "bob", &media, LibraryAction::Release, None, true).unwrap();
    change(&engine, "bob", &media, LibraryAction::Release, None, true).unwrap();
    assert_eq!(page(&engine, "bob", true).usage, 0);
    assert!(engine.readable_media("carol", &media.id).is_err());
    assert!(!page(&engine, "carol", false).items[0].available);
    assert!(engine.execute(Some("alice"), Some("lobby"), &text).is_err());
    assert!(
        change(
            &engine,
            "carol",
            &media,
            LibraryAction::Acquire,
            None,
            false
        )
        .is_err()
    );
    assert!(change(&engine, "carol", &media, LibraryAction::Save, None, false).is_err());
    assert_eq!(engine.history("alice", "lobby").unwrap().messages.len(), 1);
    assert!(
        engine
            .expired_media()
            .unwrap()
            .iter()
            .any(|value| value.id == media.id)
    );
}

#[test]
fn library_limits_quota_backlog_and_account_deletion() {
    let mut engine = engine();
    engine.media.config.library_owned_limit = 1;
    engine.media.config.library_limit = 1;
    engine.media.config.library_cleanup_objects = 1;
    let media = upload(&engine, "alice");
    let pending = engine
        .prepare_media_allocation("alice", "@library", "second.png", 8, BTreeMap::new())
        .unwrap();
    assert!(run(pending.save()).is_err());
    let text = format!("![sticker](/api/media/{}/file)", media.id);
    engine.execute(Some("alice"), Some("lobby"), &text).unwrap();
    let room = engine.data.rooms["lobby"].id.clone();
    engine
        .execute(None, None, "/user set bob media.user_quota 7")
        .unwrap();
    change(
        &engine,
        "bob",
        &media,
        LibraryAction::Save,
        Some(&room),
        false,
    )
    .unwrap();
    assert!(change(&engine, "bob", &media, LibraryAction::Acquire, None, false).is_err());
    engine
        .execute(None, None, "/user set bob media.user_quota 16")
        .unwrap();
    change(&engine, "bob", &media, LibraryAction::Acquire, None, false).unwrap();
    engine.execute(None, None, "/user delete alice").unwrap();
    assert!(engine.readable_media("bob", &media.id).is_ok());
    assert_eq!(page(&engine, "bob", true).items[0].owner_count, 1);
    change(&engine, "bob", &media, LibraryAction::Release, None, true).unwrap();
    let page = page(&engine, "bob", false);
    assert!(page.uploads_paused);
    assert_eq!(page.usage, 0);
    let pending = engine
        .prepare_media_allocation("bob", "@library", "blocked.png", 8, BTreeMap::new())
        .unwrap();
    assert!(run(pending.save()).is_err());
}

#[test]
fn library_hidden_ownership_survives_cleanup_and_failed_retirement_rolls_back() {
    let engine = engine();
    let media = upload(&engine, "alice");
    change(&engine, "alice", &media, LibraryAction::Remove, None, false).unwrap();
    engine.prepare_media_cleanup().unwrap();
    assert!(
        !engine
            .expired_media()
            .unwrap()
            .iter()
            .any(|item| item.id == media.id)
    );
    engine.store.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_library_retirement BEFORE UPDATE OF deleting ON media WHEN NEW.deleting=1 BEGIN SELECT RAISE(ABORT,'injected retirement failure'); END;").unwrap();
    assert!(change(&engine, "alice", &media, LibraryAction::Release, None, true).is_err());
    let owned = page(&engine, "alice", true);
    assert_eq!(owned.usage, 8);
    assert_eq!(owned.items[0].owner_count, 1);
    assert_eq!(owned.cleanup_bytes, 0);
    assert!(engine.readable_media("alice", &media.id).is_ok());
    engine
        .store
        .db
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_library_retirement;")
        .unwrap();
    change(&engine, "alice", &media, LibraryAction::Release, None, true).unwrap();
    let retired = page(&engine, "alice", true);
    assert_eq!(retired.usage, 0);
    assert_eq!(retired.cleanup_bytes, 16);
    assert_eq!(retired.cleanup_objects, 2);
    let store = engine.media_store();
    run(store.cleanup_done(
        std::slice::from_ref(&media),
        std::slice::from_ref(&media.id),
    ))
    .unwrap();
    let cleaned = page(&engine, "alice", true);
    assert_eq!(cleaned.cleanup_bytes, 0);
    assert_eq!(cleaned.cleanup_objects, 0);
}

#[test]
fn library_pages_and_recent_ids_are_bounded_without_duplicate_charges() {
    let mut engine = engine();
    engine.media.config.library_recent_limit = 2;
    let media: Vec<_> = (0..3).map(|_| upload(&engine, "alice")).collect();
    let account = engine.active_account_id("alice").unwrap();
    let list = database::LibraryList {
        account,
        owned: false,
        recent: false,
        after: "",
        limit: 1,
        quota: engine.library_quota("alice"),
        conversations: &[],
    };
    let store = engine.media_store();
    let first = run(store.library_page(&list)).unwrap();
    assert_eq!(first.items.len(), 1);
    let second = run(store.library_page(&database::LibraryList {
        after: first.next_cursor.as_deref().unwrap(),
        ..list
    }))
    .unwrap();
    assert_eq!(second.items.len(), 1);
    assert_eq!(first.items[0].media_id, media[0].id);
    assert_eq!(second.items[0].media_id, media[1].id);
    let third = run(store.library_page(&database::LibraryList {
        after: second.next_cursor.as_deref().unwrap(),
        ..list
    }))
    .unwrap();
    assert_eq!(third.items.len(), 1);
    assert_eq!(third.items[0].media_id, media[2].id);
    assert!(third.next_cursor.is_none());
    assert_eq!(first.usage, 24);
    for asset in &media {
        for _ in 0..2 {
            engine
                .execute(
                    Some("alice"),
                    Some("lobby"),
                    &format!("![sticker](/api/media/{}/file)", asset.id),
                )
                .unwrap();
        }
    }
    let recent = run(store.library_page(&database::LibraryList {
        account: engine.active_account_id("alice").unwrap(),
        owned: false,
        recent: true,
        after: "",
        limit: 50,
        quota: engine.library_quota("alice"),
        conversations: &engine.library_read_scopes("alice"),
    }))
    .unwrap();
    assert_eq!(recent.items.len(), 2);
    assert!(recent.next_cursor.is_none());
    assert_ne!(recent.items[0].media_id, recent.items[1].media_id);
    assert_eq!(recent.usage, 24);
    engine.media.config.library_limit = 3;
    let pending = engine
        .prepare_media_allocation("alice", "@library", "full.png", 8, BTreeMap::new())
        .unwrap();
    assert!(run(pending.save()).is_err());
    change(
        &engine,
        "alice",
        &media[0],
        LibraryAction::Remove,
        None,
        false,
    )
    .unwrap();
    let pending = engine
        .prepare_media_allocation("alice", "@library", "reserved.png", 8, BTreeMap::new())
        .unwrap();
    run(pending.save()).unwrap();
    let excess = engine
        .prepare_media_allocation("alice", "@library", "overflow.png", 8, BTreeMap::new())
        .unwrap();
    assert!(
        run(excess.save()).is_err(),
        "an unfinished upload already reserves an entry slot"
    );
}

fn concurrent_quota(mut engine: Engine) {
    let first = upload(&engine, "alice");
    let second = upload(&engine, "alice");
    for media in [&first, &second] {
        engine
            .execute(
                Some("alice"),
                Some("lobby"),
                &format!("![sticker](/api/media/{}/file)", media.id),
            )
            .unwrap();
        change(
            &engine,
            "bob",
            media,
            LibraryAction::Save,
            Some(&engine.data.rooms["lobby"].id),
            false,
        )
        .unwrap();
    }
    engine
        .execute(None, None, "/user set bob media.user_quota 8")
        .unwrap();
    let config = engine.media.config.clone();
    let account = engine.active_account_id("bob").unwrap().to_owned();
    let a = database::LibraryWrite {
        account: &account,
        id: &first.id,
        source: None,
        action: LibraryAction::Acquire,
        confirm_last: false,
        quota: 8,
        config: &config,
    };
    let b = database::LibraryWrite {
        id: &second.id,
        ..a
    };
    let store = engine.media_store();
    let (a, b) = run(async {
        Ok(tokio::join!(
            store.library_action(&a),
            store.library_action(&b)
        ))
    })
    .unwrap();
    assert_ne!(
        a.is_ok(),
        b.is_ok(),
        "only one acquisition fits the remaining quota"
    );
    assert_eq!(page(&engine, "bob", true).usage, 8);
    assert_eq!(page(&engine, "bob", true).items.len(), 1);
}
#[test]
fn library_concurrent_acquisitions_share_account_quota() {
    concurrent_quota(engine());
}
#[test]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
fn postgres_library_concurrent_acquisitions_share_account_quota() {
    let fixture = database::tests::PostgresFixture::new();
    let mut engine = Engine::open_postgres(&fixture.url).unwrap();
    for (name, admin) in [("alice", true), ("bob", false)] {
        engine.provision(name, "hash".into(), admin, false).unwrap();
    }
    engine
        .execute(Some("alice"), None, "/room new lobby")
        .unwrap();
    engine
        .execute(Some("alice"), None, "/add bob #lobby")
        .unwrap();
    concurrent_quota(engine);
}

fn recent_order(mut engine: Engine) {
    engine.media.config.library_recent_limit = 2;
    let mut assets: Vec<_> = (0..3).map(|_| upload(&engine, "alice")).collect();
    assets.sort_by(|a, b| a.id.cmp(&b.id));
    let account = engine.active_account_id("alice").unwrap().to_owned();
    // Legacy timestamps can tie or be ahead of the current wall clock. Neither
    // may cause a newly committed use to sort behind an older retained use.
    for asset in &assets[..2] {
        engine
            .store
            .db
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO library_recents(account_id,media_id,used) VALUES($1,$2,$3)",
                database::params![&account, &asset.id, 9_000_000_000i64],
            )
            .unwrap();
    }
    for (index, expected) in [(2, [2, 0]), (1, [1, 2]), (2, [2, 1])] {
        engine
            .execute(
                Some("alice"),
                Some("lobby"),
                &format!("![sticker](/api/media/{}/file)", assets[index].id),
            )
            .unwrap();
        let recent = run(engine.media_store().library_page(&database::LibraryList {
            account: &account,
            owned: false,
            recent: true,
            after: "",
            limit: 50,
            quota: engine.library_quota("alice"),
            conversations: &engine.library_read_scopes("alice"),
        }))
        .unwrap();
        assert_eq!(
            recent
                .items
                .iter()
                .map(|item| item.media_id.as_str())
                .collect::<Vec<_>>(),
            expected.map(|index| assets[index].id.as_str())
        );
    }
}

#[test]
fn library_recent_order_advances_past_legacy_timestamp_ties() {
    recent_order(engine());
}

#[test]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
fn postgres_library_recent_order_advances_past_legacy_timestamp_ties() {
    let fixture = database::tests::PostgresFixture::new();
    let mut engine = Engine::open_postgres(&fixture.url).unwrap();
    engine
        .provision("alice", "hash".into(), true, false)
        .unwrap();
    engine
        .execute(Some("alice"), None, "/new room lobby")
        .unwrap();
    recent_order(engine);
}

fn collection_order_contract(engine: Engine) {
    let media: Vec<_> = (0..4).map(|_| upload(&engine, "alice")).collect();
    let initial = page(&engine, "alice", false);
    assert_eq!(
        initial
            .items
            .iter()
            .map(|item| &item.media_id)
            .collect::<Vec<_>>(),
        media.iter().map(|item| &item.id).collect::<Vec<_>>()
    );
    let list = database::LibraryList {
        account: engine.active_account_id("alice").unwrap(),
        owned: false,
        recent: false,
        after: "",
        limit: 1,
        quota: engine.library_quota("alice"),
        conversations: &[],
    };
    let store = engine.media_store();
    let first = run(store.library_page(&list)).unwrap();
    change(
        &engine,
        "alice",
        &media[0],
        LibraryAction::Remove,
        None,
        false,
    )
    .unwrap();
    let next = run(store.library_page(&database::LibraryList {
        after: first.next_cursor.as_deref().unwrap(),
        ..list
    }))
    .unwrap();
    assert_eq!(
        next.items[0].media_id, media[1].id,
        "deleting the cursor row cannot break continuation"
    );
    change(
        &engine,
        "alice",
        &media[0],
        LibraryAction::Save,
        None,
        false,
    )
    .unwrap();
    let reordered = page(&engine, "alice", false);
    assert_eq!(
        reordered.items.last().unwrap().media_id,
        media[0].id,
        "saving an older asset appends it to the collection"
    );
    assert!(
        run(store.library_page(&database::LibraryList {
            after: "invalid",
            ..list
        }))
        .is_err()
    );
}

#[test]
fn library_collection_order_appends_uploads_and_saves_and_survives_cursor_deletion() {
    collection_order_contract(engine());
}

#[test]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
fn postgres_library_collection_order_appends_uploads_and_saves() {
    let fixture = database::tests::PostgresFixture::new();
    let mut engine = Engine::open_postgres(&fixture.url).unwrap();
    engine
        .provision("alice", "hash".into(), true, false)
        .unwrap();
    collection_order_contract(engine);
}
