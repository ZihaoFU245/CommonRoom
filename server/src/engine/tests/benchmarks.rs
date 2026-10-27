use super::*;

#[test]
#[ignore = "manual persistence benchmark"]
fn mutation_persistence_benchmark() {
    for (rooms, history) in [(1, 100), (16, 1000)] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite");
        let mut e = Engine::open(&path).unwrap();
        e.provision("alice", "hash".into(), true, false).unwrap();
        e.set_limits(64, 64, history).unwrap();
        for n in 0..rooms {
            let room = format!("room{n}");
            let peer = format!("peer{n}");
            e.execute(Some("alice"), None, &format!("/new room {room}"))
                .unwrap();
            e.provision(&peer, "hash".into(), false, false).unwrap();
            for _ in 0..history {
                e.execute(Some("alice"), Some(&room), &"x".repeat(256))
                    .unwrap();
                e.execute(
                    Some("alice"),
                    None,
                    &format!("/tell {peer} {}", "x".repeat(256)),
                )
                .unwrap();
            }
        }
        e.store
            .db
            .lock()
            .unwrap()
            .execute_batch("PRAGMA wal_autocheckpoint=0;")
            .unwrap();
        e.checkpoint().unwrap();
        let changes = e.store.db.lock().unwrap().total_changes();
        let mut send = Vec::new();
        let mut reaction = Vec::new();
        let mut private_send = Vec::new();
        let mut private_reaction = Vec::new();
        for _ in 0..30 {
            let start = std::time::Instant::now();
            e.execute(Some("alice"), Some("room0"), "benchmark message")
                .unwrap();
            send.push(start.elapsed().as_micros());
            let id = super::tests::room_messages(&e, "room0")
                .back()
                .unwrap()
                .id
                .clone();
            let start = std::time::Instant::now();
            e.execute(Some("alice"), Some("room0"), &format!("/react {id} ok"))
                .unwrap();
            reaction.push(start.elapsed().as_micros());
            let start = std::time::Instant::now();
            e.execute(Some("alice"), None, "/tell peer0 benchmark private")
                .unwrap();
            private_send.push(start.elapsed().as_micros());
            let id = super::tests::private_messages(&e, "alice:peer0")
                .back()
                .unwrap()
                .id
                .clone();
            let start = std::time::Instant::now();
            e.execute(Some("alice"), None, &format!("/react {id} ok"))
                .unwrap();
            private_reaction.push(start.elapsed().as_micros());
        }
        let wal = std::fs::metadata(path.with_extension("sqlite-wal"))
            .unwrap()
            .len();
        let start = std::time::Instant::now();
        for _ in 0..30 {
            let _snapshot = e.snapshot("alice").unwrap();
        }
        let snapshots = start.elapsed().as_micros() / 30;
        let snapshot = e.snapshot("alice").unwrap();
        let start = std::time::Instant::now();
        for _ in 0..30 {
            let _bytes = serde_json::to_vec(&snapshot).unwrap();
        }
        let serialization = start.elapsed().as_micros() / 30;
        send.sort_unstable();
        reaction.sort_unstable();
        private_send.sort_unstable();
        private_reaction.sort_unstable();
        println!(
            "rooms={rooms} history={history} send_us_p50={} p95={} reaction_us_p50={} p95={} snapshot_us_mean={snapshots} serialization_us_mean={serialization} private_send_us_p50={} p95={} private_reaction_us_p50={} p95={} wal_bytes={wal} changed_rows={}",
            send[15],
            send[28],
            reaction[15],
            reaction[28],
            private_send[15],
            private_send[28],
            private_reaction[15],
            private_reaction[28],
            e.store.db.lock().unwrap().total_changes() - changes
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "manual 100-viewer / 30-room fan-out benchmark"]
async fn snapshot_fanout_shares_cold_reads_and_encoded_content() {
    let mut engine = Engine::open(Path::new(":memory:")).unwrap();
    engine.set_limits(128, 64, 1000).unwrap();
    engine
        .provision("alice", "hash".into(), true, false)
        .unwrap();
    for n in 0..100 {
        engine
            .provision(&format!("viewer{n}"), "hash".into(), false, false)
            .unwrap();
        engine
            .execute(None, None, &format!("/grant viewer{n} su"))
            .unwrap();
    }
    for n in 0..30 {
        let room = format!("room{n}");
        engine
            .execute(Some("alice"), None, &format!("/room new {room}"))
            .unwrap();
        for _ in 0..3 {
            engine
                .execute(Some("alice"), Some(&room), &"x".repeat(512))
                .unwrap();
        }
    }
    engine.history_store().reset_after_recovery().unwrap();
    for expected_misses in [30, 1] {
        let before = engine.history_store().stats().unwrap()["misses"]
            .as_u64()
            .unwrap();
        let plans: Vec<_> = (0..100)
            .map(|n| {
                engine
                    .snapshot_plan_for(&format!("viewer{n}"), false)
                    .unwrap()
            })
            .collect();
        let start = std::time::Instant::now();
        let snapshots: Vec<_> =
            futures_util::future::join_all(plans.into_iter().map(|plan| plan.finish_async()))
                .await
                .into_iter()
                .map(Result::unwrap)
                .map(|(snapshot, complete)| {
                    assert!(complete);
                    snapshot
                })
                .collect();
        for snapshot in &snapshots {
            serde_json::to_string(snapshot).unwrap();
        }
        let misses = engine.history_store().stats().unwrap()["misses"]
            .as_u64()
            .unwrap()
            - before;
        assert_eq!(misses, expected_misses);
        for room in &snapshots[0].rooms {
            assert_eq!(room.messages.encoded_range_count(), 1);
            for snapshot in &snapshots {
                assert!(
                    room.messages.shares_storage(
                        &snapshot
                            .rooms
                            .iter()
                            .find(|other| other.id == room.id)
                            .unwrap()
                            .messages
                    )
                );
            }
        }
        println!(
            "100 viewers / 30 rooms: {misses} page misses, {} ms including assembly and serialization",
            start.elapsed().as_millis()
        );
        engine
            .execute(Some("alice"), Some("room0"), "one new message")
            .unwrap();
    }
}
