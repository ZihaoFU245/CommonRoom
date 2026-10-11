use super::*;

#[test]
#[ignore = "manual persistence benchmark"]
fn mutation_persistence_benchmark() {
    for (rooms, history) in [(1, 100), (16, 1000)] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite");
        let mut e = super::tests::engine();
        e.set_limits(64, 64, history).unwrap();
        for n in 0..rooms {
            e.execute(Some("alice"), None, &format!("/new room room{n}"))
                .unwrap();
            let mut room = e.data.rooms[&format!("room{n}")].clone();
            for _ in 0..history {
                let m = e.message(Some("alice"), None, &"x".repeat(256)).unwrap();
                room.messages.push_back(m);
            }
            e.data.rooms.insert(format!("room{n}"), room);
            let peer = format!("peer{n}");
            e.provision(&peer, "hash".into(), false, false).unwrap();
            e.execute(Some("alice"), None, &format!("/tell {peer} seed"))
                .unwrap();
            let key = private_key("alice", &peer);
            let id = e.data.private[&key].id.clone();
            let mut messages = VecDeque::new();
            for _ in 0..history {
                let mut m = e
                    .message(Some("alice"), Some(&peer), &"x".repeat(256))
                    .unwrap();
                m.private_id.clone_from(&id);
                messages.push_back(m);
            }
            e.data.private.get_mut(&key).unwrap().messages = messages;
        }
        e.db = Engine::open(&path).unwrap().db;
        e.db.execute_batch("PRAGMA wal_autocheckpoint=0;").unwrap();
        e.save().unwrap();
        e.checkpoint().unwrap();
        let changes = e.db.total_changes();
        let mut send = Vec::new();
        let mut reaction = Vec::new();
        let mut private_send = Vec::new();
        let mut private_reaction = Vec::new();
        for _ in 0..30 {
            let start = std::time::Instant::now();
            e.execute(Some("alice"), Some("room0"), "benchmark message")
                .unwrap();
            send.push(start.elapsed().as_micros());
            let id = e.data.rooms["room0"].messages.back().unwrap().id.clone();
            let start = std::time::Instant::now();
            e.execute(Some("alice"), Some("room0"), &format!("/react {id} ok"))
                .unwrap();
            reaction.push(start.elapsed().as_micros());
            let start = std::time::Instant::now();
            e.execute(Some("alice"), None, "/tell peer0 benchmark private")
                .unwrap();
            private_send.push(start.elapsed().as_micros());
            let id = e.data.private["alice:peer0"]
                .messages
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
            e.db.total_changes() - changes
        );
    }
}
