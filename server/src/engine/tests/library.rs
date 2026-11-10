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
            before: None,
            after: None,
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

fn reorder_contract(engine: Engine) {
    let media: Vec<_> = (0..5).map(|_| upload(&engine, "alice")).collect();
    let ids: Vec<_> = media.iter().map(|item| item.id.clone()).collect();
    let foreign = upload(&engine, "bob");
    let store = engine.media_store();
    let request = database::LibraryWrite {
        account: engine.principal_id(Some("alice")).unwrap(),
        id: &ids[4],
        source: None,
        before: Some(&ids[0]),
        after: None,
        action: LibraryAction::Move,
        confirm_last: false,
        quota: engine.library_quota("alice"),
        config: &engine.media.config,
    };
    let order = || {
        page(&engine, "alice", false)
            .items
            .into_iter()
            .map(|item| item.media_id)
            .collect::<Vec<_>>()
    };
    assert!(run(store.library_action(&request)).unwrap());
    assert_eq!(
        order(),
        vec![
            ids[4].clone(),
            ids[0].clone(),
            ids[1].clone(),
            ids[2].clone(),
            ids[3].clone()
        ]
    );
    assert!(
        !run(store.library_action(&request)).unwrap(),
        "repeated drops are no-ops"
    );
    // The after anchor works at the end of a visible page without moving the
    // sticker past unseen rows elsewhere in the collection.
    run(store.library_action(&database::LibraryWrite {
        before: None,
        after: Some(&ids[1]),
        ..request
    }))
    .unwrap();
    assert_eq!(
        order(),
        vec![
            ids[0].clone(),
            ids[1].clone(),
            ids[4].clone(),
            ids[2].clone(),
            ids[3].clone()
        ]
    );
    let before = order();
    let postgres = engine.store.db.lock().unwrap().is_postgres();
    engine.store.db.lock().unwrap().execute_batch(if postgres {
        "CREATE FUNCTION reject_reorder_fn() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.position>0 AND OLD.position<0 THEN RAISE EXCEPTION 'injected reorder failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER reject_reorder BEFORE UPDATE OF position ON library_entries FOR EACH ROW EXECUTE FUNCTION reject_reorder_fn();"
    } else {
        "CREATE TRIGGER reject_reorder BEFORE UPDATE OF position ON library_entries WHEN NEW.position>0 AND OLD.position<0 BEGIN SELECT RAISE(ABORT,'injected reorder failure'); END;"
    }).unwrap();
    assert!(
        run(store.library_action(&database::LibraryWrite {
            before: None,
            ..request
        }))
        .is_err()
    );
    assert_eq!(
        order(),
        before,
        "failed rank shifts roll back the entire move"
    );
    engine
        .store
        .db
        .lock()
        .unwrap()
        .execute_batch(if postgres {
            "DROP TRIGGER reject_reorder ON library_entries; DROP FUNCTION reject_reorder_fn();"
        } else {
            "DROP TRIGGER reject_reorder"
        })
        .unwrap();
    assert!(
        run(store.library_action(&database::LibraryWrite {
            before: Some(&foreign.id),
            ..request
        }))
        .is_err()
    );
    assert_eq!(order(), before, "foreign anchors cannot change the order");
    assert!(
        run(store.library_action(&database::LibraryWrite {
            id: &foreign.id,
            ..request
        }))
        .is_err()
    );
    assert_eq!(page(&engine, "bob", false).items[0].media_id, foreign.id);
    // Exercise gaps left by removal and both shift directions under the unique index.
    change(
        &engine,
        "alice",
        &media[1],
        LibraryAction::Remove,
        None,
        false,
    )
    .unwrap();
    run(store.library_action(&database::LibraryWrite {
        id: &ids[0],
        before: None,
        ..request
    }))
    .unwrap();
    assert_eq!(
        order(),
        vec![
            ids[4].clone(),
            ids[2].clone(),
            ids[3].clone(),
            ids[0].clone()
        ]
    );
    run(store.library_action(&database::LibraryWrite {
        id: &ids[0],
        before: Some(&ids[4]),
        ..request
    }))
    .unwrap();
    assert_eq!(
        order(),
        vec![
            ids[0].clone(),
            ids[4].clone(),
            ids[2].clone(),
            ids[3].clone()
        ]
    );
}

#[test]
fn library_reorder_is_atomic_account_scoped_and_preserves_unseen_rows() {
    reorder_contract(engine());
}

#[test]
fn library_reorder_survives_sqlite_restart() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("order.sqlite");
    let mut engine = Engine::open(&path).unwrap();
    engine
        .provision("alice", "hash".into(), true, false)
        .unwrap();
    let first = upload(&engine, "alice");
    let second = upload(&engine, "alice");
    run(engine
        .media_store()
        .library_action(&database::LibraryWrite {
            account: engine.active_account_id("alice").unwrap(),
            id: &second.id,
            action: LibraryAction::Move,
            source: None,
            before: Some(&first.id),
            after: None,
            quota: engine.library_quota("alice"),
            config: &engine.media.config,
            confirm_last: false,
        }))
    .unwrap();
    drop(engine);
    let reopened = Engine::open(&path).unwrap();
    let saved = page(&reopened, "alice", false);
    assert_eq!(saved.items[0].media_id, second.id);
    assert_eq!(saved.items[1].media_id, first.id);
}

#[test]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
fn postgres_library_reorder_is_atomic_account_scoped_and_preserves_unseen_rows() {
    let fixture = database::tests::PostgresFixture::new();
    let mut engine = Engine::open_postgres(&fixture.url).unwrap();
    for user in ["alice", "bob"] {
        engine.provision(user, "hash".into(), true, false).unwrap();
    }
    reorder_contract(engine);
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
    assert!(engine.readable_media("carol", &media.id).is_ok());
    assert!(page(&engine, "carol", false).items[0].available);
    assert!(
        engine
            .expired_media()
            .unwrap()
            .iter()
            .all(|value| value.id != media.id)
    );
    // Administrative deletion remains an explicit global revocation.
    change(&engine, "alice", &media, LibraryAction::Delete, None, false).unwrap();
    assert!(engine.readable_media("carol", &media.id).is_err());
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
    assert!(!page(&engine, "bob", false).uploads_paused);
    assert!(engine.readable_media("bob", &media.id).is_ok());
    change(&engine, "bob", &media, LibraryAction::Delete, None, false).unwrap();
    let page = page(&engine, "bob", false);
    assert!(page.uploads_paused);
    assert_eq!(page.usage, 0);
    let pending = engine
        .prepare_media_allocation("bob", "@library", "blocked.png", 8, BTreeMap::new())
        .unwrap();
    assert!(run(pending.save()).is_err());
}

#[test]
fn library_hidden_ownership_survives_cleanup_and_failed_release_rolls_back() {
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
    engine.store.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_library_retirement BEFORE UPDATE OF expires ON media WHEN NEW.expires IS NOT NULL BEGIN SELECT RAISE(ABORT,'injected release failure'); END;").unwrap();
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
    assert_eq!(page(&engine, "alice", true).usage, 0);
    assert_eq!(page(&engine, "alice", true).cleanup_bytes, 0);
    change(&engine, "alice", &media, LibraryAction::Delete, None, false).unwrap();
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
        before: None,
        after: None,
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

#[test]
fn recent_page_filters_inaccessible_entries_before_the_limit() {
    let engine = engine();
    let first = upload(&engine, "alice");
    let second = upload(&engine, "alice");
    let third = upload(&engine, "alice");
    let account = engine.principal_id(Some("alice")).unwrap();
    let db = engine.store.db.lock().unwrap();
    for (index, media) in [&first, &second, &third].iter().enumerate() {
        db.execute(
            "INSERT INTO library_recents(account_id,media_id,used) VALUES(?1,?2,?3)",
            &[
                &account as &dyn database::Encode,
                &media.id,
                &i64::try_from(index).unwrap(),
            ][..],
        )
        .unwrap();
    }
    db.execute("DELETE FROM library_owners WHERE media_id=?1", [&third.id])
        .unwrap();
    db.execute("DELETE FROM library_entries WHERE media_id=?1", [&third.id])
        .unwrap();
    drop(db);
    let mut config = engine.media.config.clone();
    config.library_recent_limit = 2;
    let store = MediaStore {
        database: engine.store.db.clone(),
        config,
    };
    let recent = run(store.library_page(&database::LibraryList {
        account,
        owned: false,
        recent: true,
        after: "",
        limit: 2,
        quota: engine.library_quota("alice"),
        conversations: &[],
    }))
    .unwrap();
    assert_eq!(recent.items.len(), 2);
    assert!(recent.items.iter().all(|item| item.media_id != third.id));
}

#[test]
fn unavailable_library_entries_cannot_pin_cleaned_tombstones() {
    let engine = engine();
    let media = upload(&engine, "alice");
    change(&engine, "alice", &media, LibraryAction::Release, None, true).unwrap();
    assert_eq!(page(&engine, "alice", false).items.len(), 1);
    set_expiry(&engine, &media, media::timestamp() - 1);
    assert!(
        engine
            .expired_media()
            .unwrap()
            .iter()
            .any(|row| row.id == media.id)
    );
    engine
        .store
        .db
        .media_write_blocking(&database::MediaWrite::CleanupDone {
            records: &[(media.id.clone(), true)],
            staging: &[],
            tombstones_before: media::timestamp() + 1,
        })
        .unwrap();
    assert!(engine.stored_media(&media.id).is_err());
    assert!(page(&engine, "alice", false).items.is_empty());
}

fn set_expiry(engine: &Engine, media: &media::Media, expires: u64) {
    engine
        .store
        .db
        .lock()
        .unwrap()
        .execute(
            "UPDATE media SET expires=$2 WHERE id=$1",
            database::params![&media.id, i64::try_from(expires).unwrap()],
        )
        .unwrap();
}

fn release_retention(mut engine: Engine) {
    let media = upload(&engine, "alice");
    let text = format!("![sticker](/api/media/{}/file)", media.id);
    engine.execute(Some("alice"), Some("lobby"), &text).unwrap();
    let room = engine.data.rooms["lobby"].id.clone();
    change(
        &engine,
        "bob",
        &media,
        LibraryAction::Save,
        Some(&room),
        false,
    )
    .unwrap();
    let before = media::timestamp();
    change(&engine, "alice", &media, LibraryAction::Release, None, true).unwrap();
    let store = engine.media_store();
    let released = run(store.load(&media.id)).unwrap().0;
    let deadline = released.expires.unwrap();
    assert!((before + 7 * 86400..=media::timestamp() + 7 * 86400).contains(&deadline));
    assert!(!released.deleting);
    assert_eq!(released.object_key, media.object_key);
    assert_eq!(released.final_key(), media.final_key());
    assert!(engine.readable_media("bob", &media.id).is_ok());
    change(&engine, "alice", &media, LibraryAction::Release, None, true).unwrap();
    engine.prepare_media_cleanup().unwrap();
    assert_eq!(
        run(store.load(&media.id)).unwrap().0.expires,
        Some(deadline)
    );
    assert!(
        engine
            .expired_media()
            .unwrap()
            .iter()
            .all(|row| row.id != media.id)
    );
    // Reuse and a saved collection alone never renew the grace period.
    engine.execute(Some("bob"), Some("lobby"), &text).unwrap();
    assert_eq!(
        run(store.load(&media.id)).unwrap().0.expires,
        Some(deadline)
    );
    engine
        .execute(None, None, "/user set bob media.user_quota 7")
        .unwrap();
    assert!(change(&engine, "bob", &media, LibraryAction::Acquire, None, false).is_err());
    assert_eq!(
        run(store.load(&media.id)).unwrap().0.expires,
        Some(deadline)
    );
    engine
        .execute(None, None, "/user set bob media.user_quota 16")
        .unwrap();
    change(&engine, "bob", &media, LibraryAction::Acquire, None, false).unwrap();
    assert_eq!(run(store.load(&media.id)).unwrap().0.expires, None);
    assert_eq!(page(&engine, "bob", true).usage, 8);
    change(&engine, "bob", &media, LibraryAction::Release, None, true).unwrap();
    set_expiry(&engine, &media, media::timestamp());
    assert!(engine.readable_media("bob", &media.id).is_err());
    assert!(engine.media_info("bob", &media.id).is_ok());
    assert!(page(&engine, "bob", false).items.is_empty());
    for action in [LibraryAction::Acquire, LibraryAction::Save] {
        assert!(change(&engine, "bob", &media, action, None, false).is_err());
    }
    assert!(engine.execute(Some("bob"), Some("lobby"), &text).is_err());
    let expired = engine.expired_media().unwrap();
    assert!(
        expired.iter().any(|row| row.id == media.id),
        "message references cannot block expiry cleanup"
    );
    run(store.cleanup_done(&expired, std::slice::from_ref(&media.id))).unwrap();
    assert!(run(store.load(&media.id)).unwrap().0.deleted);
    assert!(change(&engine, "bob", &media, LibraryAction::Acquire, None, false).is_err());

    // A never-sent sticker also gets the full grace period, not orphan cleanup.
    let unused = upload(&engine, "bob");
    change(&engine, "bob", &unused, LibraryAction::Release, None, true).unwrap();
    engine.prepare_media_cleanup().unwrap();
    assert!(
        engine
            .expired_media()
            .unwrap()
            .iter()
            .all(|row| row.id != unused.id)
    );
    assert!(engine.readable_media("bob", &unused.id).is_ok());

    // Account deletion follows the same last-owner rule.
    let orphan = upload(&engine, "alice");
    engine.execute(None, None, "/user delete alice").unwrap();
    let orphan = run(store.load(&orphan.id)).unwrap().0;
    assert!(
        orphan
            .expires
            .is_some_and(|expiry| expiry > media::timestamp())
    );
    assert!(!orphan.deleting);
}

#[test]
fn library_release_retention_preserves_grace_and_blocks_expired_reacquisition() {
    release_retention(engine());
}

#[test]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
fn postgres_library_release_retention_preserves_grace_and_blocks_expired_reacquisition() {
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
    release_retention(engine);
}

fn legacy_retention(mut engine: Engine) {
    let media = upload(&engine, "alice");
    let text = format!("![sticker](/api/media/{}/file)", media.id);
    engine.execute(Some("alice"), Some("lobby"), &text).unwrap();
    // Simulate an ownerless permanent asset written by the previous version.
    engine
        .store
        .db
        .lock()
        .unwrap()
        .execute("DELETE FROM library_owners WHERE media_id=$1", [&media.id])
        .unwrap();
    engine.prepare_media_cleanup().unwrap();
    let store = engine.media_store();
    let deadline = run(store.load(&media.id)).unwrap().0.expires.unwrap();
    assert!(deadline >= media::timestamp() + 7 * 86400 - 1);
    engine.prepare_media_cleanup().unwrap();
    assert_eq!(
        run(store.load(&media.id)).unwrap().0.expires,
        Some(deadline)
    );
    assert!(
        engine
            .expired_media()
            .unwrap()
            .iter()
            .all(|row| row.id != media.id)
    );
    let owned = upload(&engine, "alice");
    engine.prepare_media_cleanup().unwrap();
    assert_eq!(run(store.load(&owned.id)).unwrap().0.expires, None);
}

#[test]
fn library_startup_assigns_legacy_abandoned_assets_one_grace_period() {
    legacy_retention(engine());
}

#[test]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
fn postgres_library_startup_assigns_legacy_abandoned_assets_one_grace_period() {
    let fixture = database::tests::PostgresFixture::new();
    let mut engine = Engine::open_postgres(&fixture.url).unwrap();
    engine
        .provision("alice", "hash".into(), true, false)
        .unwrap();
    engine
        .execute(Some("alice"), None, "/new room lobby")
        .unwrap();
    legacy_retention(engine);
}

fn reacquire_cleanup_race(engine: Engine) {
    let media = upload(&engine, "alice");
    change(&engine, "alice", &media, LibraryAction::Release, None, true).unwrap();
    let store = engine.media_store();
    let deadline = run(store.load(&media.id)).unwrap().0.expires.unwrap();
    let request = database::LibraryWrite {
        account: engine.principal_id(Some("alice")).unwrap(),
        id: &media.id,
        source: None,
        before: None,
        after: None,
        action: LibraryAction::Acquire,
        confirm_last: false,
        quota: engine.library_quota("alice"),
        config: &engine.media.config,
    };
    // A cleanup snapshot at the deadline races an accepted pre-expiry acquire.
    // Whichever durable row lock wins must prevent the opposite outcome.
    let cleanup = database::MediaWrite::ClaimCleanup {
        at: deadline,
        pending_before: 0,
        attempted_before: u64::MAX,
        limit: 32,
    };
    let (acquired, claimed) = run(async {
        Ok(tokio::join!(
            store.library_action(&request),
            engine.store.db.media_write(&cleanup)
        ))
    })
    .unwrap();
    let claimed = claimed.unwrap();
    let current = run(store.load(&media.id)).unwrap().0;
    if acquired.is_ok() {
        assert!(claimed.media.iter().all(|row| row.id != media.id));
        assert!(current.available_at(media::timestamp()));
        assert_eq!(current.expires, None);
    } else {
        assert!(claimed.media.iter().any(|row| row.id == media.id));
        assert!(current.deleting);
        assert!(
            change(
                &engine,
                "alice",
                &media,
                LibraryAction::Acquire,
                None,
                false
            )
            .is_err()
        );
    }
}

#[test]
fn library_reacquisition_and_cleanup_claim_have_one_durable_winner() {
    reacquire_cleanup_race(engine());
}

#[test]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
fn postgres_library_reacquisition_and_cleanup_claim_have_one_durable_winner() {
    let fixture = database::tests::PostgresFixture::new();
    let mut engine = Engine::open_postgres(&fixture.url).unwrap();
    engine
        .provision("alice", "hash".into(), true, false)
        .unwrap();
    reacquire_cleanup_race(engine);
}

fn collection_ownership_policy(mut engine: Engine) {
    let first = upload(&engine, "alice");
    let text = format!("![sticker](/api/media/{}/file)", first.id);
    engine.execute(Some("alice"), Some("lobby"), &text).unwrap();
    let room = engine.data.rooms["lobby"].id.clone();
    change(
        &engine,
        "bob",
        &first,
        LibraryAction::Save,
        Some(&room),
        false,
    )
    .unwrap();
    assert_eq!(
        page(&engine, "bob", false).usage,
        0,
        "owned-by-others saves stay borrowed"
    );
    assert!(!page(&engine, "bob", false).items[0].owned);
    change(&engine, "alice", &first, LibraryAction::Save, None, false).unwrap();
    assert_eq!(
        page(&engine, "alice", true).usage,
        8,
        "saving one's own asset never adds a charge"
    );
    change(&engine, "alice", &first, LibraryAction::Release, None, true).unwrap();
    let store = engine.media_store();
    let expiry = run(store.load(&first.id)).unwrap().0.expires;
    engine
        .execute(None, None, "/user set bob media.user_quota 7")
        .unwrap();
    assert!(change(&engine, "bob", &first, LibraryAction::Save, None, false).is_err());
    assert_eq!(run(store.load(&first.id)).unwrap().0.expires, expiry);
    engine
        .execute(None, None, "/user set bob media.user_quota 16")
        .unwrap();
    // A previously borrowed entry can now be kept through the same save action.
    for _ in 0..2 {
        change(&engine, "bob", &first, LibraryAction::Save, None, false).unwrap();
    }
    assert_eq!(run(store.load(&first.id)).unwrap().0.expires, None);
    assert_eq!(page(&engine, "bob", false).usage, 8);
    assert_eq!(page(&engine, "bob", false).items[0].owner_count, 1);
    let second = upload(&engine, "alice");
    let text = format!("![sticker](/api/media/{}/file)", second.id);
    engine.execute(Some("alice"), Some("lobby"), &text).unwrap();
    change(
        &engine,
        "alice",
        &second,
        LibraryAction::Release,
        None,
        true,
    )
    .unwrap();
    let expiry = run(store.load(&second.id)).unwrap().0.expires;
    engine.media.config.library_limit = 1;
    assert!(
        change(
            &engine,
            "bob",
            &second,
            LibraryAction::Save,
            Some(&room),
            false
        )
        .is_err()
    );
    assert_eq!(
        run(store.load(&second.id)).unwrap().0.expires,
        expiry,
        "collection-limit failure rolls back ownership and expiry restoration"
    );
    assert_eq!(page(&engine, "bob", true).usage, 8);
    change(&engine, "bob", &first, LibraryAction::Release, None, true).unwrap();
    set_expiry(&engine, &first, media::timestamp());
    assert!(page(&engine, "bob", false).items.is_empty());
    // Expired hidden entries consume neither collection slots nor page limits.
    change(
        &engine,
        "bob",
        &second,
        LibraryAction::Save,
        Some(&room),
        false,
    )
    .unwrap();
    let list = run(store.library_page(&database::LibraryList {
        account: engine.principal_id(Some("bob")).unwrap(),
        owned: false,
        recent: false,
        after: "",
        limit: 1,
        quota: engine.library_quota("bob"),
        conversations: &[],
    }))
    .unwrap();
    assert_eq!(list.items.len(), 1);
    assert_eq!(list.items[0].media_id, second.id);
    assert!(list.next_cursor.is_none());
    assert!(list.items[0].owned);
}

#[test]
fn library_save_acquires_only_dangling_assets_and_hides_expired_entries() {
    collection_ownership_policy(engine());
}

#[test]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
fn postgres_library_save_acquires_only_dangling_assets_and_hides_expired_entries() {
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
    collection_ownership_policy(engine);
}

fn metadata_media_usage_contract(mut engine: Engine) {
    let inspect = |engine: &Engine, user: &str| -> u64 {
        let command = format!("/user get {user} media.usage @global");
        let result: serde_json::Value = serde_json::from_str(
            &engine
                .read_command(Some("alice"), None, &command)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        result[user]["@global"]["media.usage"]["value"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap()
    };
    let media = upload(&engine, "alice");
    assert_eq!(inspect(&engine, "alice"), 8);
    assert_eq!(inspect(&engine, "bob"), 0);
    let room = engine.data.rooms["lobby"].id.clone();
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
        &media,
        LibraryAction::Acquire,
        Some(&room),
        false,
    )
    .unwrap();
    assert_eq!(inspect(&engine, "bob"), 8);
    let pending = engine
        .prepare_media_allocation("bob", &room, "pending.png", 5, BTreeMap::new())
        .unwrap();
    run(pending.save()).unwrap();
    assert_eq!(inspect(&engine, "bob"), 13);
    // A lowered quota does not cap the reported usage or delete existing media.
    engine
        .execute(None, None, "/user set bob media.user_quota 7")
        .unwrap();
    assert_eq!(inspect(&engine, "bob"), 13);
    let retiring = pending.media;
    run(engine
        .media_store()
        .abandon(&retiring.id, &retiring.uploader))
    .unwrap();
    assert_eq!(
        inspect(&engine, "bob"),
        13,
        "attachment bytes stay charged during cleanup"
    );
    run(engine.media_store().cleanup_done(&[retiring], &[])).unwrap();
    assert_eq!(inspect(&engine, "bob"), 8);
    change(&engine, "bob", &media, LibraryAction::Release, None, false).unwrap();
    assert_eq!(inspect(&engine, "bob"), 0);
    assert_eq!(inspect(&engine, "alice"), 8);
}

#[test]
fn library_metadata_media_usage_tracks_live_quota_accounting() {
    metadata_media_usage_contract(engine());
}

#[test]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
fn postgres_library_metadata_media_usage_tracks_live_quota_accounting() {
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
    metadata_media_usage_contract(engine);
}
