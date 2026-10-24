use super::*;
fn sequences(page: &History) -> Vec<u64> {
    page.messages
        .iter()
        .map(|message| message.sequence)
        .collect()
}
#[test]
fn unrelated_conversations_do_not_wait_for_another_cache_shard() {
    let mut engine = super::super::tests::engine();
    let held = cache_shard(&engine.data.rooms["lobby"].id);
    let name = (0..32)
        .find_map(|n| {
            let name = format!("other{n}");
            engine
                .execute(Some("alice"), None, &format!("/room new {name}"))
                .unwrap();
            (cache_shard(&engine.data.rooms[&name].id) != held).then_some(name)
        })
        .unwrap();
    engine
        .execute(Some("alice"), Some(&name), "independent")
        .unwrap();
    let id = engine.data.rooms[&name].id.clone();
    let revision = engine.data.rooms[&name].revision;
    let store = engine.history_store().clone();
    let page_guard = store.cache[held].lock().unwrap();
    let unread_guard = store.unread_cache[held].lock().unwrap();
    let (sent, received) = std::sync::mpsc::channel();
    let thread = std::thread::spawn({
        let store = store.clone();
        move || {
            let page = store
                .page(&id, revision, HistoryRange::default(), "")
                .unwrap();
            let unread = store.unread(&id, "bob", 0, revision).unwrap();
            sent.send((page.messages.len(), unread.count)).unwrap();
        }
    });
    let result = received.recv_timeout(std::time::Duration::from_secs(1));
    drop(page_guard);
    drop(unread_guard);
    thread.join().unwrap();
    assert_eq!(result.unwrap(), (1, 1));
    let stats = store.stats().unwrap();
    assert!(stats["messages"].as_u64().unwrap() <= u64::try_from(CACHE_MESSAGES).unwrap());
    assert!(stats["accounted_bytes"].as_u64().unwrap() <= u64::try_from(CACHE_BYTES).unwrap());
}
#[test]
fn shared_latest_pages_reuse_payload_and_exact_byte_budgets() {
    let mut engine = super::super::tests::engine();
    for _ in 0..6 {
        engine
            .execute(Some("alice"), Some("lobby"), "Unicode 🦀 \"quoted\"")
            .unwrap();
    }
    let page = engine.history("bob", "lobby").unwrap();
    let latest = engine
        .history_page(
            "bob",
            "lobby",
            HistoryRange {
                limit: Some(3),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(Arc::ptr_eq(&page.messages.data, &latest.messages.data));
    assert_eq!(sequences(&latest), vec![4, 5, 6]);
    let budget = serde_json::to_vec(&latest.messages[2]).unwrap().len();
    let mut bytes = budget;
    let mut count = 100;
    let tail = latest.messages.bounded_tail(&mut count, &mut bytes);
    assert_eq!(tail.len(), 1);
    assert_eq!(tail[0].sequence, 6);
    assert_eq!(bytes, 0);
    assert!(Arc::ptr_eq(&tail.data, &page.messages.data));
    assert_eq!(
        serde_json::to_vec(&latest.messages).unwrap(),
        serde_json::to_vec(&latest.messages[..]).unwrap()
    );
}
#[test]
fn indexed_unread_cache_keeps_recency_and_replaces_cursor_revision_entries() {
    let mut cache = UnreadCache::default();
    let unread = Unread {
        count: 3,
        first: Some(1),
        through: 3,
        oldest: 1,
        revision: 1,
    };
    for index in 0..UNREAD_ENTRIES_PER_SHARD {
        cache.insert(&index.to_string(), "bob", 0, 1, unread.clone());
    }
    assert!(cache.get("0", "bob", 0, 1).is_some());
    cache.insert("new", "bob", 0, 1, unread.clone());
    assert!(cache.get("1", "bob", 0, 1).is_none());
    assert!(cache.get("0", "bob", 0, 1).is_some());
    cache.insert("0", "bob", 2, 2, unread);
    assert!(cache.get("0", "bob", 0, 1).is_none());
    assert!(cache.get("0", "bob", 2, 2).is_some());
    assert!(cache.get("0", "alice", 2, 2).is_none());
    assert_eq!(cache.recency.len(), UNREAD_ENTRIES_PER_SHARD);
    assert_eq!(
        cache
            .entries
            .values()
            .map(std::collections::HashMap::len)
            .sum::<usize>(),
        UNREAD_ENTRIES_PER_SHARD
    );
}
#[test]
fn pagination_matches_gapped_sequences_in_both_directions() {
    let mut engine = super::super::tests::engine();
    engine
        .execute(Some("alice"), None, "/new room other")
        .unwrap();
    for index in 1..=240 {
        let room = if index % 2 == 0 { "lobby" } else { "other" };
        engine
            .execute(Some("alice"), Some(room), "message")
            .unwrap();
    }
    let all: Vec<u64> = (2..=240).step_by(2).collect();
    let mut forward = Vec::new();
    let mut cursor = 0;
    loop {
        let page = engine
            .history_page(
                "bob",
                "lobby",
                HistoryRange {
                    after: Some(cursor),
                    limit: Some(17),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(page.has_before, cursor > 0);
        forward.extend(sequences(&page));
        if !page.has_after {
            break;
        }
        cursor = page.messages.last().unwrap().sequence;
    }
    assert_eq!(forward, all);
    let mut backward = Vec::new();
    let mut before = None;
    loop {
        let page = engine
            .history_page(
                "bob",
                "lobby",
                HistoryRange {
                    before,
                    limit: Some(17),
                    ..Default::default()
                },
            )
            .unwrap();
        let mut values = sequences(&page);
        values.extend(backward);
        backward = values;
        if !page.has_before {
            break;
        }
        before = Some(page.messages[0].sequence);
    }
    assert_eq!(backward, all);
    for (range, expected) in [
        (
            HistoryRange {
                start: Some(99),
                end: Some(105),
                ..Default::default()
            },
            vec![100, 102, 104],
        ),
        (
            HistoryRange {
                before: Some(2),
                ..Default::default()
            },
            vec![],
        ),
        (
            HistoryRange {
                after: Some(240),
                ..Default::default()
            },
            vec![],
        ),
        (
            HistoryRange {
                start: Some(100),
                end: Some(100),
                ..Default::default()
            },
            vec![100],
        ),
        (
            HistoryRange {
                around: Some(100),
                limit: Some(5),
                ..Default::default()
            },
            vec![96, 98, 100, 102, 104],
        ),
    ] {
        assert_eq!(
            sequences(&engine.history_page("bob", "lobby", range).unwrap()),
            expected
        );
    }
}
#[test]
fn empty_edge_pages_retain_the_reverse_paging_direction() {
    let mut engine = super::super::tests::engine();
    engine
        .execute(Some("alice"), Some("lobby"), "first")
        .unwrap();
    let older = engine
        .history_page(
            "bob",
            "lobby",
            HistoryRange {
                before: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(older.messages.is_empty());
    assert!(!older.has_before && older.has_after);
    let newer = engine
        .history_page(
            "bob",
            "lobby",
            HistoryRange {
                after: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(newer.messages.is_empty());
    assert!(newer.has_before && !newer.has_after);
}
#[test]
fn range_validation_rejects_ambiguous_and_unsafe_bounds() {
    for range in [
        HistoryRange {
            limit: Some(0),
            ..Default::default()
        },
        HistoryRange {
            limit: Some(101),
            ..Default::default()
        },
        HistoryRange {
            before: Some(10),
            after: Some(1),
            ..Default::default()
        },
        HistoryRange {
            start: Some(10),
            end: Some(9),
            ..Default::default()
        },
        HistoryRange {
            around: Some(5),
            end: Some(9),
            ..Default::default()
        },
        HistoryRange {
            after: Some(u64::MAX),
            ..Default::default()
        },
    ] {
        assert!(range.validate().is_err(), "{range:?}");
    }
    assert_eq!(HistoryRange::default().validate().unwrap(), 100);
    assert!(
        HistoryRange {
            end: Some(9_007_199_254_740_991),
            ..Default::default()
        }
        .validate()
        .is_ok()
    );
}
#[test]
fn avatar_visibility_reads_release_engine_and_discard_revoked_frames() {
    use axum::http::{HeaderMap, HeaderValue, header};
    for logout in [false, true] {
        let mut engine = super::super::tests::engine();
        engine.set_avatar_hosts(["example.com".into()].into());
        engine
            .set_avatar("alice", Some("https://example.com/a.png".into()), false)
            .unwrap();
        engine
            .execute(Some("alice"), Some("lobby"), "Retained author")
            .unwrap();
        engine
            .execute(None, None, "/revoke bob user @global")
            .unwrap();
        engine
            .execute(None, None, "/revoke bob user #lobby")
            .unwrap();
        engine
            .execute(None, None, "/grant bob #lobby r:message.read")
            .unwrap();
        engine
            .login("avatar-reader".into(), "bob", "test-hash", None)
            .unwrap();
        let owner = engine.active_account_id("alice").unwrap().to_owned();
        let store = engine.history_store().clone();
        let app = test_app(engine, 256);
        let guards: Vec<_> = store
            .readers
            .iter()
            .map(|reader| reader.lock().unwrap())
            .collect();
        let before = store.next.load(Ordering::Relaxed);
        let worker = std::thread::spawn({
            let app = app.clone();
            move || {
                let mut headers = HeaderMap::new();
                headers.insert(
                    header::COOKIE,
                    HeaderValue::from_static("chat_session=avatar-reader"),
                );
                crate::web::socket::avatar_frame(&app, &headers, &owner)
            }
        });
        let started = reader_started(&store, before);
        let wrote = if let Ok(mut engine) = app.engine.try_write() {
            if logout {
                engine.logout("avatar-reader").unwrap();
            } else {
                engine
                    .execute(None, None, "/revoke bob #lobby r:message.read")
                    .unwrap();
            }
            true
        } else {
            false
        };
        drop(guards);
        let result = worker.join().unwrap();
        assert!(
            started && wrote,
            "Avatar history waits must not retain the engine guard"
        );
        if logout {
            assert!(matches!(result, Err(EngineError::Forbidden(_))));
        } else {
            assert!(result.unwrap().is_none());
        }
        assert_eq!(store.stats().unwrap()["misses"], 0);
        assert_eq!(store.stats().unwrap()["unread_full"], 0);
    }
}

#[test]
fn notification_batches_release_engine_and_discard_revoked_content_and_cursors() {
    use axum::http::{HeaderMap, HeaderValue, header};
    for (logout, read_only) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut engine = super::super::tests::engine();
        engine.set_avatar_hosts(["example.com".into()].into());
        engine
            .set_avatar("alice", Some("https://example.com/a.png".into()), false)
            .unwrap();
        engine
            .execute(Some("alice"), Some("lobby"), "Retained author")
            .unwrap();
        engine
            .execute(None, None, "/revoke bob user @global")
            .unwrap();
        engine
            .execute(None, None, "/revoke bob user #lobby")
            .unwrap();
        engine
            .execute(None, None, "/grant bob #lobby r:message.read")
            .unwrap();
        engine
            .login("avatar-reader".into(), "bob", "test-hash", None)
            .unwrap();
        let conversation = engine.data.rooms["lobby"].id.clone();
        let store = engine.history_store().clone();
        let app = test_app(engine, 256);
        let guards: Vec<_> = store
            .readers
            .iter()
            .map(|reader| reader.lock().unwrap())
            .collect();
        let before = store.next.load(Ordering::Relaxed);
        let worker = std::thread::spawn({
            let app = app.clone();
            move || {
                let mut headers = HeaderMap::new();
                headers.insert(
                    header::COOKIE,
                    HeaderValue::from_static("chat_session=avatar-reader"),
                );
                let mut changes = vec![
                    crate::web::Change::Read("bob".into()),
                    crate::web::Change::Presence,
                ];
                if !read_only {
                    changes.insert(0, crate::web::Change::Conversation(conversation));
                }
                crate::web::updates::browser(&app, &headers, false, &changes)
            }
        });
        let started = reader_started(&store, before);
        let wrote = if let Ok(mut engine) = app.engine.try_write() {
            if logout {
                engine.logout("avatar-reader").unwrap();
            } else {
                engine
                    .execute(None, None, "/revoke bob #lobby r:message.read")
                    .unwrap();
            }
            true
        } else {
            false
        };
        drop(guards);
        let result = worker.join().unwrap();
        assert!(
            started && wrote,
            "Avatar history waits must not retain the engine guard"
        );
        if logout {
            assert!(matches!(result, Err(EngineError::Forbidden(_))));
        } else {
            let (_, frames) = result.unwrap();
            let frames: Vec<serde_json::Value> = frames
                .iter()
                .map(|frame| serde_json::from_str(frame).unwrap())
                .collect();
            assert!(!frames.iter().any(|frame| frame["kind"] == "conversation"));
            assert!(
                frames.iter().find(|frame| frame["kind"] == "read").unwrap()["unread"]
                    .as_object()
                    .unwrap()
                    .is_empty()
            );
        }
    }
}

#[test]
fn blocked_snapshot_reads_release_engine_and_never_deliver_revoked_authority() {
    use axum::http::{HeaderMap, HeaderValue, header};
    for (bot, logout, batched) in [
        (false, false, false),
        (false, true, false),
        (true, false, false),
        (true, true, false),
        (false, false, true),
        (false, true, true),
        (true, false, true),
        (true, true, true),
    ] {
        let mut engine = super::super::tests::engine();
        engine
            .execute(Some("alice"), Some("lobby"), "private history")
            .unwrap();
        engine
            .login("snapshot".into(), "bob", "test-hash", None)
            .unwrap();
        let secret = if bot {
            engine
                .execute(Some("alice"), Some("lobby"), "/bot new helper")
                .unwrap();
            Some(
                engine
                    .execute(Some("alice"), None, "/bot token new helper snapshot")
                    .unwrap()
                    .lines()
                    .last()
                    .unwrap()
                    .to_owned(),
            )
        } else {
            None
        };
        let store = engine.history_store().clone();
        let app = test_app(engine, 1);
        let context = secret.map(|secret| {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::AUTHORIZATION,
                format!("Bearer {secret}").parse().unwrap(),
            );
            crate::bots::auth::Context::authenticate(&app, &headers).unwrap()
        });
        let worker_context = context.clone();
        let guards: Vec<_> = store
            .readers
            .iter()
            .map(|reader| reader.lock().unwrap())
            .collect();
        let before = store.next.load(Ordering::Relaxed);
        let worker = std::thread::spawn({
            let app = app.clone();
            move || {
                let mut headers = HeaderMap::new();
                headers.insert(
                    header::COOKIE,
                    HeaderValue::from_static("chat_session=snapshot"),
                );
                if let Some(context) = worker_context {
                    if batched {
                        crate::bots::socket::render_changes(
                            &app,
                            &context,
                            &[crate::web::Change::All, crate::web::Change::Presence],
                        )
                        .map_err(EngineError::from)
                        .and_then(|(_, frames)| {
                            if !logout {
                                for frame in &frames {
                                    let frame: serde_json::Value =
                                        serde_json::from_str(frame).unwrap();
                                    if frame["kind"] == "presence" {
                                        assert!(frame["online"].as_array().unwrap().is_empty());
                                    }
                                }
                            }
                            frames
                                .into_iter()
                                .next()
                                .ok_or_else(|| EngineError::Internal("Missing baseline".into()))
                        })
                    } else {
                        crate::web::snapshots::bot_value(&app, &context).map_err(EngineError::from)
                    }
                } else if batched {
                    crate::web::updates::browser(
                        &app,
                        &headers,
                        false,
                        &[crate::web::Change::All, crate::web::Change::Presence],
                    )
                    .and_then(|(_, frames)| {
                        frames
                            .into_iter()
                            .next()
                            .ok_or_else(|| EngineError::Internal("Missing baseline".into()))
                    })
                } else {
                    crate::web::snapshots::value(&app, &headers, false)
                }
            }
        });
        let started = reader_started(&store, before);
        // Use try_write: a regression reports failure instead of deadlocking.
        let wrote = if let Ok(mut writer) = app.engine.try_write() {
            if let Some(context) = &context {
                if logout {
                    writer
                        .execute(
                            None,
                            None,
                            &format!("/bot token rm helper {}", context.credential),
                        )
                        .unwrap();
                } else {
                    writer
                        .execute(None, None, "/revoke helper #lobby r:message.read")
                        .unwrap();
                    if batched {
                        writer
                            .execute(None, None, "/revoke helper user @global")
                            .unwrap();
                    }
                }
            } else if logout {
                writer.logout("snapshot").unwrap();
            } else {
                writer
                    .execute(None, None, "/revoke bob user #lobby")
                    .unwrap();
            }
            true
        } else {
            false
        };
        drop(guards);
        let result = worker.join().unwrap();
        assert!(started, "snapshot never reached the blocked history reader");
        assert!(
            wrote,
            "snapshot retained the engine guard during history I/O"
        );
        if logout {
            assert!(result.is_err());
        } else {
            let snapshot: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
            if bot {
                assert!(snapshot["conversations"].as_array().unwrap().is_empty());
            } else {
                assert!(snapshot["rooms"].as_array().unwrap().is_empty());
                assert!(snapshot["unread"].as_object().unwrap().is_empty());
            }
            assert!(!snapshot.to_string().contains("private history"));
        }
    }
}
#[test]
fn incremental_unreads_match_sql_after_gaps_cursors_retention_and_mutations() {
    let mut engine = super::super::tests::engine();
    engine
        .execute(Some("alice"), None, "/room new gaps")
        .unwrap();
    let id = engine.data.rooms["lobby"].id.clone();
    let check = |engine: &Engine| {
        let unread = engine.unreads("bob").unwrap()["lobby"].clone();
        let read = engine.read_position("bob", "room:lobby");
        let db = engine.store.db.lock().unwrap();
        let expected: (usize,Option<u64>,u64,u64) = db.query_row(
                "SELECT count(*),min(sequence),coalesce((SELECT min(sequence) FROM messages WHERE conversation=?1),0),coalesce((SELECT max(sequence) FROM messages WHERE conversation=?1),0) FROM messages WHERE conversation=?1 AND sequence>?2 AND json_extract(json,'$.from')!='bob'",
                params![id,read], |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
            ).unwrap();
        assert_eq!(
            (unread.count, unread.first, unread.oldest, unread.through),
            expected
        );
    };
    check(&engine);
    let full = engine.history_store().stats().unwrap()["unread_full"]
        .as_u64()
        .unwrap();
    for n in 0..8 {
        engine.execute(Some("alice"), Some("gaps"), "gap").unwrap();
        engine
            .execute(
                Some(if n % 2 == 0 { "alice" } else { "bob" }),
                Some("lobby"),
                "message",
            )
            .unwrap();
        check(&engine);
    }
    assert_eq!(engine.history_store().stats().unwrap()["unread_full"], full);
    let page = engine.history("bob", "lobby").unwrap();
    engine
        .mark_read("bob", "lobby", page.messages[3].sequence)
        .unwrap();
    check(&engine);
    assert_eq!(engine.history_store().stats().unwrap()["unread_full"], full);
    assert!(
        engine.history_store().stats().unwrap()["unread_incremental"]
            .as_u64()
            .unwrap()
            >= 9
    );
    engine
        .execute(
            Some("alice"),
            Some("lobby"),
            &format!("/retract {}", page.messages[4].id),
        )
        .unwrap();
    check(&engine);
    assert!(
        engine.history_store().stats().unwrap()["unread_full"]
            .as_u64()
            .unwrap()
            > full
    );
    let target = engine.history("bob", "lobby").unwrap().messages[0]
        .id
        .clone();
    for command in [
        format!("/edit {target} changed"),
        format!("/react {target} ok"),
        "/user rename alicia".into(),
    ] {
        let before = engine.history_store().stats().unwrap()["unread_full"]
            .as_u64()
            .unwrap();
        engine
            .execute(Some("alice"), Some("lobby"), &command)
            .unwrap();
        check(&engine);
        assert!(
            engine.history_store().stats().unwrap()["unread_full"]
                .as_u64()
                .unwrap()
                > before
        );
    }
    engine
        .execute(Some("alicia"), None, "/user rename alice")
        .unwrap();
    check(&engine);
    engine.set_limits(64, 64, 2).unwrap();
    check(&engine);
    for _ in 0..3 {
        engine
            .execute(Some("alice"), Some("lobby"), "retention")
            .unwrap();
        check(&engine);
    }
    engine.execute(None, None, "/clean -1 #lobby").unwrap();
    check(&engine);
    engine
        .execute(Some("alice"), Some("lobby"), "after empty")
        .unwrap();
    check(&engine);
    engine.store.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_append BEFORE INSERT ON messages BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(
        engine
            .execute(Some("alice"), Some("lobby"), "failed")
            .is_err()
    );
    check(&engine);
}
#[test]
fn unread_and_cached_tails_follow_cursor_retraction_and_retention() {
    let mut engine = super::super::tests::engine();
    for author in ["alice", "bob", "alice", "bob", "alice"] {
        engine
            .execute(Some(author), Some("lobby"), "message")
            .unwrap();
    }
    let page = engine.history("bob", "lobby").unwrap();
    let small = engine
        .history_page(
            "bob",
            "lobby",
            HistoryRange {
                limit: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(sequences(&small), vec![4, 5]);
    assert!(small.has_before && !small.has_after);
    assert_eq!(engine.history_store().stats().unwrap()["hits"], 1);
    let unread = engine.unreads("bob").unwrap();
    assert_eq!(unread["lobby"].count, 3);
    assert_eq!(unread["lobby"].first, Some(1));
    engine.mark_read("bob", "lobby", 3).unwrap();
    assert!(!engine.mark_read("bob", "lobby", 1).unwrap());
    assert!(engine.mark_read("bob", "lobby", 99).is_err());
    assert_eq!(engine.unreads("bob").unwrap()["lobby"].count, 1);
    engine
        .execute(
            Some("alice"),
            Some("lobby"),
            &format!("/retract {}", page.messages[4].id),
        )
        .unwrap();
    assert_eq!(engine.unreads("bob").unwrap()["lobby"].count, 0);
    assert_eq!(
        sequences(&engine.history("bob", "lobby").unwrap()),
        vec![1, 2, 3, 4]
    );
    engine.set_limits(64, 64, 2).unwrap();
    let retained = engine.history("bob", "lobby").unwrap();
    assert_eq!(sequences(&retained), vec![3, 4]);
    assert!(!retained.has_before);
    assert_eq!(engine.unreads("bob").unwrap()["lobby"].oldest, 3);
}
#[test]
fn large_payload_pages_make_progress_and_around_reaches_anchor() {
    let mut engine = super::super::tests::engine();
    for _ in 0..4 {
        engine
            .execute(Some("alice"), Some("lobby"), "message")
            .unwrap();
    }
    // Legacy/imported rows may exceed today's input limit.
    engine
        .store
        .db
        .lock()
        .unwrap()
        .execute(
            "UPDATE messages SET json=json_set(json,'$.text',?1)",
            ["x".repeat(PAGE_BYTES)],
        )
        .unwrap();
    let first = engine
        .history_page(
            "bob",
            "lobby",
            HistoryRange {
                after: Some(0),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(sequences(&first), vec![1]);
    assert!(first.has_after);
    let next = engine
        .history_page(
            "bob",
            "lobby",
            HistoryRange {
                after: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(sequences(&next), vec![2]);
    let anchor = engine
        .history_page(
            "bob",
            "lobby",
            HistoryRange {
                around: Some(3),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(sequences(&anchor), vec![3]);
}
#[test]
fn each_cache_shard_enforces_its_share_of_bytes_pages_and_lru() {
    let mut engine = super::super::tests::engine();
    assert_eq!(engine.history_store().stats().unwrap()["messages"], 0);
    engine
        .execute(Some("alice"), Some("lobby"), "seed")
        .unwrap();
    let message = engine.history("alice", "lobby").unwrap().messages[0].clone();
    let make = |id: String, revision, count, bytes| CachedPage {
        conversation: id,
        revision,
        range: HistoryRange::default(),
        messages: MessagePage::new(vec![message.clone(); count], vec![1; count]),
        bytes,
        before: false,
        after: false,
    };
    let mut cache = Cache::default();
    for index in 0..120 {
        cache.insert(make(index.to_string(), 1, 100, 1024));
    }
    assert_eq!(cache.messages, CACHE_MESSAGES / CACHE_SHARDS);
    assert!(!cache.pages.iter().any(|p| p.conversation == "0"));
    cache.insert(make("119".into(), 2, 1, 1024));
    assert_eq!(
        cache
            .pages
            .iter()
            .filter(|p| p.conversation == "119")
            .count(),
        1
    );
    cache.insert(make("large".into(), 1, 1, CACHE_BYTES / CACHE_SHARDS));
    assert_eq!(cache.pages.len(), 1);
    for index in 0..300 {
        cache.insert(make(index.to_string(), 1, 0, 0));
    }
    assert_eq!(cache.pages.len(), 256 / CACHE_SHARDS);
    assert_eq!(cache.bytes, 0);
    assert_eq!(cache.messages, 0);
}
#[test]
fn account_rename_preserves_private_history_across_streaming_batches_and_name_reuse() {
    let mut engine = super::super::tests::engine();
    for index in 0..205 {
        engine
            .execute(Some("alice"), None, &format!("/tell bob batch {index}"))
            .unwrap();
    }
    let before = engine.history("bob", "@direct:alice").unwrap();
    let original_id = engine.data.private["alice:bob"].id.clone();
    engine.execute(Some("bob"), None, "/rename robert").unwrap();
    assert_eq!(engine.data.private["alice:robert"].id, original_id);
    let all = super::super::tests::private_messages(&engine, "alice:robert");
    assert_eq!(all.len(), 205);
    assert!(all.iter().all(
        |message| message.to.as_deref() == Some("robert") && message.private_id == original_id
    ));
    let after = engine.history("robert", "@direct:alice").unwrap();
    assert_eq!(sequences(&before), sequences(&after));
    assert!(after.revision > before.revision);
    engine
        .provision("bob", "replacement".into(), false, false)
        .unwrap();
    assert!(engine.history("bob", "@direct:alice").is_err());
    engine
        .execute(Some("alice"), None, "/tell bob new identity")
        .unwrap();
    assert_ne!(engine.data.private["alice:bob"].id, original_id);
    assert_eq!(
        engine
            .history("bob", "@direct:alice")
            .unwrap()
            .messages
            .len(),
        1
    );
    assert_eq!(
        engine
            .history("robert", "@direct:alice")
            .unwrap()
            .messages
            .len(),
        100
    );
}
#[test]
fn deep_range_query_uses_a_bounded_primary_key_seek() {
    let engine = super::super::tests::engine();
    engine
        .history_store()
        .read(|db| {
            let mut query = db
                .prepare(&format!("EXPLAIN QUERY PLAN {PAGE_ASC_SQL}"))
                .unwrap();
            let plans = query
                .query_map(
                    params![engine.data.rooms["lobby"].id, 1000, 1099, 100],
                    |row| row.get::<_, String>(3),
                )
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .join(" ");
            assert!(plans.contains("SEARCH messages"), "{plans}");
            assert!(
                plans.contains("sequence>?") && plans.contains("sequence<?"),
                "{plans}"
            );
            assert!(!plans.contains("TEMP B-TREE"), "{plans}");
            Ok(())
        })
        .unwrap();
}
#[test]
fn four_reader_connections_execute_simultaneously() {
    let engine = super::super::tests::engine();
    let (started, ready) = std::sync::mpsc::channel();
    let mut releases = Vec::new();
    let mut readers = Vec::new();
    for _ in 0..4 {
        let store = engine.history_store().clone();
        let started = started.clone();
        let (release, wait) = std::sync::mpsc::channel();
        releases.push(release);
        readers.push(std::thread::spawn(move || {
            store.read(|db| {
                started.send(()).unwrap();
                wait.recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
                db.query_row("SELECT count(*) FROM entities", [], |row| {
                    row.get::<_, usize>(0)
                })
                .map_err(EngineError::unavailable)
            })
        }));
    }
    let overlapped = (0..4).all(|_| {
        ready
            .recv_timeout(std::time::Duration::from_secs(2))
            .is_ok()
    });
    for release in releases {
        let _ = release.send(());
    }
    for reader in readers {
        assert!(reader.join().unwrap().unwrap() > 0);
    }
    assert!(
        overlapped,
        "All four reader jobs must enter before any is released."
    );
}

#[test]
fn pages_are_independent_bounded_and_revision_checked() {
    let mut engine = super::super::tests::engine();
    engine.set_limits(64, 64, 2000).unwrap();
    for index in 1..=1200 {
        engine
            .execute(Some("alice"), Some("lobby"), &format!("message {index}"))
            .unwrap();
    }
    assert!(engine.working.is_none());
    let range = HistoryRange {
        start: Some(1000),
        end: Some(1099),
        ..Default::default()
    };
    let page = engine.history_page("alice", "lobby", range).unwrap();
    assert_eq!(page.messages.len(), 100);
    assert_eq!(page.messages[0].sequence, 1000);
    assert_eq!(page.messages[99].sequence, 1099);
    assert!(page.has_before && page.has_after);
    let stats = engine.history_store().stats().unwrap();
    assert_eq!(stats["messages"], 100);
    engine.history_page("alice", "lobby", range).unwrap();
    assert_eq!(engine.history_store().stats().unwrap()["hits"], 1);
    let id = page.messages[0].id.clone();
    engine
        .execute(Some("bob"), Some("lobby"), &format!("/react {id} ok"))
        .unwrap();
    let updated = engine.history_page("alice", "lobby", range).unwrap();
    assert!(updated.messages[0].reactions["ok"].contains("bob"));
    assert!(engine.history_page("eve", "missing", range).is_err());
    for start in (1..1200).step_by(100) {
        engine
            .history_page(
                "alice",
                "lobby",
                HistoryRange {
                    start: Some(start),
                    ..Default::default()
                },
            )
            .unwrap();
    }
    assert!(
        engine.history_store().stats().unwrap()["messages"]
            .as_u64()
            .unwrap()
            <= 1000
    );
}
#[test]
fn file_readers_can_overlap_a_wal_writer() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("chat.sqlite");
    let mut engine = Engine::open(&path).unwrap();
    engine
        .provision("alice", "hash".into(), true, false)
        .unwrap();
    engine
        .execute(Some("alice"), None, "/new room room")
        .unwrap();
    engine
        .execute(Some("alice"), Some("room"), "committed")
        .unwrap();
    let id = engine.data.rooms["room"].id.clone();
    let store = engine.history_store().clone();
    let mut writer = engine.store.db.lock().unwrap();
    let tx = writer.transaction().unwrap();
    tx.execute("UPDATE counters SET sequence=sequence+1", [])
        .unwrap();
    let revision = engine.data.rooms["room"].revision;
    let reader = std::thread::spawn(move || {
        store
            .page(&id, revision, HistoryRange::default(), "room")
            .unwrap()
    });
    assert_eq!(reader.join().unwrap().messages[0].text, "committed");
    tx.rollback().unwrap();
}

fn test_app(engine: Engine, capacity: usize) -> crate::web::App {
    let (changes, _) = tokio::sync::broadcast::channel(capacity);
    crate::web::App {
        engine: Arc::new(std::sync::RwLock::new(engine)),
        config: Arc::new(crate::config::Config::default()),
        changes,
        media: None,
        attempts: Default::default(),
        admission: Default::default(),
        io_jobs: Arc::new(tokio::sync::Semaphore::new(64)),
        password_jobs: Arc::new(tokio::sync::Semaphore::new(2)),
        snapshots: Default::default(),
        bot_runtimes: Default::default(),
        stopping: Default::default(),
        shutdown: Default::default(),
    }
}

fn reader_started(store: &HistoryStore, before: usize) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while store.next.load(Ordering::Relaxed) == before && std::time::Instant::now() < deadline {
        std::thread::yield_now();
    }
    store.next.load(Ordering::Relaxed) > before
}

#[test]
fn unread_batches_lease_one_reader_and_reuse_warm_entries() {
    let mut engine = super::super::tests::engine();
    for room in ["lobby", "other", "third"] {
        if room != "lobby" {
            engine
                .execute(None, None, &format!("/room new {room}"))
                .unwrap();
        }
        engine.execute(None, Some(room), "unread").unwrap();
    }
    let requests: Vec<_> = engine
        .data
        .rooms
        .values()
        .map(|r| (r.id.as_str(), 0, r.revision))
        .collect();
    let store = engine.history_store();
    let before = store.next.load(Ordering::Relaxed);
    for result in store.unread_batch("bob", requests.iter().copied()).unwrap() {
        assert_eq!(result.unwrap().count, 1);
    }
    assert_eq!(store.next.load(Ordering::Relaxed) - before, 1);
    for result in store.unread_batch("bob", requests.iter().copied()).unwrap() {
        assert_eq!(result.unwrap().count, 1);
    }
    assert_eq!(store.next.load(Ordering::Relaxed) - before, 1);
}

#[test]
fn wire_size_matches_serialized_unicode_quotes_controls_and_reactions() {
    let mut engine = super::super::tests::engine();
    engine
        .execute(Some("alice"), Some("lobby"), "Unicode 🦀 \"quote\"\n\t\\")
        .unwrap();
    let id = engine.history("bob", "lobby").unwrap().messages[0]
        .id
        .clone();
    engine
        .execute(Some("bob"), Some("lobby"), &format!("/react {id} 🙂"))
        .unwrap();
    let message = &engine.history("bob", "lobby").unwrap().messages[0];
    assert_eq!(
        wire_size(message).unwrap(),
        serde_json::to_vec(message).unwrap().len()
    );
}
