use super::*;

#[test]
fn metadata_reset_restores_inheritance_and_clears_only_the_selected_branch() {
    let mut e = crate::engine::tests::engine();
    let room = e.room_scope("lobby").unwrap();
    let id = e.data.rooms["lobby"].id.clone();
    e.execute(None, None, "/user set bob media.max_size 10m @global")
        .unwrap();
    e.execute(None, None, "/user set bob media.max_size 5m #lobby")
        .unwrap();
    e.execute(None, None, "/user set bob label local #lobby")
        .unwrap();
    e.execute(None, None, "/user reset bob media.max_size #lobby")
        .unwrap();
    assert_eq!(
        e.metadata_value("bob", &room, "media.max_size"),
        Some("10485760")
    );
    assert_eq!(e.data.users["bob"].metadata[&id]["label"], "local");
    e.execute(None, Some("lobby"), "/user reset bob @all")
        .unwrap();
    assert!(!e.data.users["bob"].metadata.contains_key(&id));
    assert_eq!(
        e.data.users["bob"].metadata["@global"]["media.max_size"],
        "10485760"
    );
    e.execute(None, None, "/user set bob label local #lobby")
        .unwrap();
    e.execute(None, None, "/user set bob media.user_quota 512m")
        .unwrap();
    e.execute(None, Some("lobby"), "/user reset bob media.user_quota")
        .unwrap();
    assert!(!e.data.users["bob"].metadata["@global"].contains_key("media.user_quota"));
    e.execute(None, None, "/user reset bob @all @global")
        .unwrap();
    assert!(!e.data.users["bob"].metadata.contains_key("@global"));
    assert_eq!(e.data.users["bob"].metadata[&id]["label"], "local");
    let before = e.data.clone();
    for command in [
        "/user reset bob media.user_quota #lobby",
        "/user reset bob @all lobby",
        "/user reset bob media.unknown #lobby",
        "/user reset bob label #lobby extra",
    ] {
        assert!(e.execute(None, None, command).is_err(), "{command}");
        assert!(e.data == before);
    }
}

#[test]
fn bulk_metadata_reset_is_su_only_scoped_and_rolls_back_failed_writes() {
    let mut e = crate::engine::tests::engine();
    e.provision("outside", "hash".into(), false, false).unwrap();
    e.execute(None, None, "/user set @all label root @global")
        .unwrap();
    e.execute(None, None, "/user set @all label local #lobby")
        .unwrap();
    e.execute(None, None, "/user set @all media.user_quota 512m @global")
        .unwrap();
    let before = e.data.clone();
    for actor in ["alice", "bob"] {
        for command in [
            "/user reset bob label #lobby",
            "/user reset @all @all #lobby",
        ] {
            assert!(
                e.execute(Some(actor), Some("lobby"), command)
                    .unwrap_err()
                    .contains("Only su")
            );
            assert!(e.data == before);
        }
    }
    e.store.db.lock().unwrap().execute_batch("CREATE TRIGGER deny_reset BEFORE UPDATE ON entities WHEN NEW.kind='account' AND NEW.key='bob' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(
        e.execute(None, None, "/user reset @all @all #lobby")
            .is_err()
    );
    assert!(e.data == before);
    e.store
        .db
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER deny_reset;")
        .unwrap();
    let hash = e.data.users["bob"].hash.clone();
    e.execute(None, Some("lobby"), "/user reset @all @all")
        .unwrap();
    let id = &e.data.rooms["lobby"].id;
    assert!(
        e.data
            .users
            .values()
            .all(|user| !user.metadata.contains_key(id))
    );
    assert_eq!(e.data.users["outside"].metadata["@global"]["label"], "root");
    assert_eq!(e.data.users["bob"].hash, hash);
    e.execute(None, Some("lobby"), "/user reset @all media.user_quota")
        .unwrap();
    assert!(!e.data.users["bob"].metadata["@global"].contains_key("media.user_quota"));
    assert_eq!(
        e.data.users["outside"].metadata["@global"]["media.user_quota"],
        "536870912"
    );
}

#[test]
fn metadata_reset_persists_pruned_branches_without_touching_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    let mut e = Engine::open(&path).unwrap();
    e.provision("bob", "hash".into(), false, false).unwrap();
    e.execute(None, None, "/user set bob label root").unwrap();
    e.execute(None, None, "/user reset bob @all").unwrap();
    drop(e);
    let e = Engine::open(&path).unwrap();
    assert!(e.data.users["bob"].metadata.is_empty());
    assert_eq!(e.data.users["bob"].hash, "hash");
}

#[test]
fn metadata_tree_reads_resolve_sources_without_materializing_inheritance() {
    let mut e = crate::engine::tests::engine();
    e.execute(None, None, "/user set bob media.max_age 2d @global")
        .unwrap();
    e.execute(None, None, "/user set bob label root @global")
        .unwrap();
    let id = e.data.rooms["lobby"].id.clone();
    let get = |e: &Engine| -> serde_json::Value {
        serde_json::from_str(
            &e.read_command(Some("bob"), Some("lobby"), "/user get bob #lobby")
                .unwrap()
                .unwrap(),
        )
        .unwrap()
    };
    let before = e.data.clone();
    let initial = get(&e);
    assert_eq!(initial["bob"]["#lobby"]["media.max_age"]["value"], "172800");
    assert_eq!(
        initial["bob"]["#lobby"]["media.max_age"]["source"],
        "@global"
    );
    assert_eq!(
        initial["bob"]["#lobby"]["media.max_size"]["source"],
        "config"
    );
    assert_eq!(initial["bob"]["#lobby"]["label"]["value"], "root");
    assert!(initial["bob"]["#lobby"].get("media.user_quota").is_none());
    assert!(!e.data.users["bob"].metadata.contains_key(&id));
    assert!(e.data == before);
    e.execute(None, None, "/user set bob media.max_age 1d #lobby")
        .unwrap();
    let local = get(&e);
    assert_eq!(local["bob"]["#lobby"]["media.max_age"]["value"], "86400");
    assert_eq!(local["bob"]["#lobby"]["media.max_age"]["source"], "#lobby");
    assert_eq!(local["bob"]["@global"]["media.max_age"]["value"], "172800");
    e.execute(None, None, "/user set bob media.max_age @unset #lobby")
        .unwrap();
    assert_eq!(
        get(&e)["bob"]["#lobby"]["media.max_age"]["source"],
        "@global"
    );
    assert!(!e.data.users["bob"].metadata.contains_key(&id));
    e.execute(None, None, "/user set bob secret hidden @global")
        .unwrap();
    e.execute(None, None, "/grant eve #lobby x:policy.change")
        .unwrap();
    let restricted = e
        .read_command(Some("eve"), Some("lobby"), "/user get bob #lobby")
        .unwrap()
        .unwrap();
    assert!(!restricted.contains("hidden"));
}

#[test]
fn deleting_room_prunes_all_persisted_branches_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    let mut e = Engine::open(&path).unwrap();
    for name in ["alice", "bob"] {
        e.provision(name, "hash".into(), false, false).unwrap();
    }
    for room in ["a", "b"] {
        e.execute(None, None, &format!("/room new {room}")).unwrap();
        for name in ["alice", "bob"] {
            e.execute(None, None, &format!("/room add {name} #{room}"))
                .unwrap();
            e.execute(
                None,
                None,
                &format!("/user set {name} label {room} #{room}"),
            )
            .unwrap();
        }
    }
    e.execute(None, None, "/user set bob label root @global")
        .unwrap();
    let id = e.data.rooms["a"].id.clone();
    let other = e.data.rooms["b"].id.clone();
    e.execute(None, None, "/room kick bob #a").unwrap();
    e.store.db.lock().unwrap().execute_batch("CREATE TRIGGER deny_prune BEFORE UPDATE ON entities WHEN NEW.kind='account' AND NEW.key='bob' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    let before = e.data.clone();
    assert!(e.execute(None, None, "/room delete #a").is_err());
    assert!(e.data == before);
    e.store
        .db
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER deny_prune;")
        .unwrap();
    e.execute(None, None, "/room delete #a").unwrap();
    drop(e);
    let mut e = Engine::open(&path).unwrap();
    assert!(!e.data.rooms.contains_key("a"));
    for user in e.data.users.values() {
        assert!(!user.metadata.contains_key(&id));
        assert_eq!(user.metadata[&other]["label"], "b");
    }
    assert_eq!(e.data.users["bob"].metadata["@global"]["label"], "root");
    let db = e.store.db.lock().unwrap();
    let stale: u64 = db.query_row("SELECT count(*) FROM entities WHERE kind='account' AND json_type(json, '$.metadata.' || ?) IS NOT NULL", [&id], |row| row.get(0)).unwrap();
    assert_eq!(stale, 0);
    drop(db);
    e.execute(None, None, "/room new a").unwrap();
    assert_ne!(e.data.rooms["a"].id, id);
    assert!(
        !e.data.users["bob"]
            .metadata
            .contains_key(&e.data.rooms["a"].id)
    );
}

#[test]
fn metadata_all_includes_effective_defaults_and_quota_shorthand_is_account_wide() {
    let mut e = crate::engine::tests::engine();
    e.execute(None, None, "/user set bob secret global-only")
        .unwrap();
    let get = |e: &Engine, command: &str| -> serde_json::Value {
        serde_json::from_str(
            &e.read_command(Some("bob"), Some("lobby"), command)
                .unwrap()
                .unwrap(),
        )
        .unwrap()
    };
    let initial = get(&e, "/user get bob @all");
    assert_eq!(
        initial["bob"]["@global"]["media.user_quota"]["value"],
        "1073741824"
    );
    assert_eq!(
        initial["bob"]["@global"]["media.max_size"]["value"],
        "26214400"
    );
    assert_eq!(initial["bob"]["@global"]["secret"]["value"], "global-only");
    e.execute(None, Some("lobby"), "/user set bob media.user_quota 1g")
        .unwrap();
    assert_eq!(
        e.data.users["bob"].metadata["@global"]["media.user_quota"],
        "1073741824"
    );
    e.execute(None, Some("lobby"), "/user set bob media.user_quota 512m")
        .unwrap();
    assert_eq!(
        get(&e, "/user get bob @all")["bob"]["@global"]["media.user_quota"]["value"],
        "536870912"
    );
    e.execute(None, Some("lobby"), "/user set bob media.max_size 512k")
        .unwrap();
    assert_eq!(
        get(&e, "/user get bob @all #lobby")["bob"]["#lobby"]["media.max_size"]["value"],
        "524288"
    );
    e.provision("outside", "hash".into(), false, false).unwrap();
    e.execute(None, Some("lobby"), "/user set @all media.user_quota 128m")
        .unwrap();
    assert!(e.data.users["outside"].metadata.is_empty());
    let selected: serde_json::Value = serde_json::from_str(
        &e.execute(None, Some("lobby"), "/user get @all media.user_quota")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(selected.as_object().unwrap().len(), 3);
    assert!(selected.get("outside").is_none());
    e.execute(None, None, "/user set @all media.user_quota 256m")
        .unwrap();
    let all: serde_json::Value =
        serde_json::from_str(&e.execute(None, None, "/user get @all @all").unwrap()).unwrap();
    for name in ["alice", "bob", "eve"] {
        assert_eq!(
            all[name]["@global"]["media.user_quota"]["value"],
            "268435456"
        );
    }
    e.execute(None, None, "/user set bob media.user_quota @unset")
        .unwrap();
    assert_eq!(
        get(&e, "/user get bob media.user_quota")["bob"]["@global"]["media.user_quota"]["value"],
        "1073741824"
    );
    for value in ["0g", "2g", "1.5g", "18446744073709551615g"] {
        assert!(
            e.execute(
                None,
                None,
                &format!("/user set bob media.user_quota {value}")
            )
            .is_err()
        );
    }
}

#[test]
fn every_explicit_room_scope_requires_hash_and_omitted_scopes_still_work() {
    let mut e = crate::engine::tests::engine();
    let before = e.data.clone();
    for command in [
        "/grant bob lobby w:message.react",
        "/revoke bob lobby w:message.react",
        "/grant bob user lobby",
        "/revoke bob user lobby",
        "/grant @all lobby /reply",
        "/permissions lobby",
        "/user ls lobby",
        "/user ls room",
        "/user get bob @all lobby",
        "/user set bob label test lobby",
        "/bot ls lobby",
        "/clean 7d lobby",
        "/room add bob lobby",
        "/room join lobby",
        "/room rename lobby renamed",
    ] {
        assert!(
            e.execute(None, Some("lobby"), command)
                .unwrap_err()
                .contains("#name"),
            "{command}"
        );
        assert!(e.data == before);
    }
    for command in [
        "/user ls",
        "/user get bob @all",
        "/permissions",
        "/room add bob",
        "/clean 7d #lobby",
        "/user ls #lobby",
        "/permissions #lobby",
    ] {
        assert!(
            e.execute(Some("alice"), Some("lobby"), command).is_ok(),
            "{command}"
        );
    }
}
