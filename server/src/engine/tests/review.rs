use super::*;
use authorization::{Action, Group, Scope};
use tests::{engine, room_messages};

#[test]
fn room_metadata_read_does_not_project_a_global_section() {
    let mut e = engine();
    for input in [
        "/user set bob media.user_quota 128m @global",
        "/user set bob secret global-secret @global",
        "/grant eve #lobby x:policy.change",
    ] {
        e.execute(None, None, input).unwrap();
    }
    let output = e
        .read_command(Some("eve"), Some("lobby"), "/user get bob #lobby")
        .unwrap()
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert!(value["bob"].get("@global").is_none());
    assert!(value["bob"]["#lobby"].get("media.user_quota").is_none());
    assert!(!output.contains("global-secret"));
    assert!(value["bob"]["#lobby"].get("media.max_size").is_some());
}

#[test]
fn explicit_global_group_grants_are_symmetric_and_audit_once_per_command() {
    let mut e = engine();
    let members = e.data.rooms["lobby"].members.clone();
    for input in [
        "/grant bob admin @global",
        "/revoke bob admin @global",
        "/grant @all admin @global",
    ] {
        let audit = e.data.policy.audit.len();
        e.execute(None, None, input).unwrap();
        assert_eq!(e.data.policy.audit.len(), audit + 1);
        assert_eq!(e.data.rooms["lobby"].members, members);
        assert_eq!(e.is_admin("bob"), input.starts_with("/grant"));
    }
    assert!(e.is_admin("eve"));
}

#[test]
fn group_grants_cannot_bypass_room_invitation_authority() {
    let mut e = engine();
    e.provision("outside", "hash".into(), false, false).unwrap();
    for permission in ["/grant", "x:policy.change"] {
        e.execute(None, None, &format!("/grant bob #lobby {permission}"))
            .unwrap();
    }
    let room = e.room_scope("lobby").unwrap();
    assert!(!e.allows(Some("bob"), &room, Action::Invite));
    let before = e.data.clone();
    assert!(
        e.execute(Some("bob"), Some("lobby"), "/grant outside user #lobby")
            .unwrap_err()
            .contains("x:member.add")
    );
    assert!(e.data == before);
    e.execute(None, None, "/grant bob #lobby x:member.add")
        .unwrap();
    e.execute(Some("bob"), Some("lobby"), "/grant outside user #lobby")
        .unwrap();
    assert!(e.data.rooms["lobby"].members.contains("outside"));
    assert!(
        e.data
            .policy
            .assignments
            .iter()
            .any(|a| a.account_id == e.data.users["outside"].id
                && a.group == Group::User
                && a.scope == room)
    );
}

fn media_link(id: &str) -> String {
    format!("![file](/api/media/{id}/file)")
}
#[test]
fn attachment_errors_do_not_reveal_allocation_state_and_edit_checks_ownership_first() {
    let mut e = engine();
    let room = e.data.rooms["lobby"].id.clone();
    let mut media = e.allocate_media("alice", &room, "test.png", 8).unwrap();
    let missing = media_link(&uuid::Uuid::new_v4().to_string());
    let error = e.execute(Some("bob"), Some("lobby"), &missing).unwrap_err();
    assert_eq!(error.message(), "Upload not found.");
    assert_eq!(
        e.execute(Some("bob"), Some("lobby"), &media_link(&media.id))
            .unwrap_err(),
        error
    );
    media.ready = true;
    media.object_key = media.final_key();
    media.content_type = "image/png".into();
    e.finish_media("alice", &media).unwrap();
    assert_eq!(
        e.execute(Some("bob"), Some("lobby"), &media_link(&media.id))
            .unwrap_err(),
        error
    );
    e.execute(Some("alice"), Some("lobby"), "original").unwrap();
    let message = room_messages(&e, "lobby")[0].id.clone();
    assert_eq!(
        e.execute(
            Some("bob"),
            Some("lobby"),
            &format!("/edit {message} {missing}")
        )
        .unwrap_err()
        .message(),
        "You can only edit your own messages."
    );
    e.execute(Some("alice"), Some("lobby"), &media_link(&media.id))
        .unwrap();
    e.execute(Some("bob"), Some("lobby"), &media_link(&media.id))
        .unwrap();
    assert_eq!(room_messages(&e, "lobby").len(), 3);
}

#[test]
fn rejected_and_preview_uploads_leave_no_private_directory_or_reservation() {
    let mut e = engine();
    let revision = e.revision;
    for (size, preview) in [(0, false), (8, true)] {
        let result =
            e.allocate_media_target("bob", "@direct:eve", "file", size, BTreeMap::new(), preview);
        assert_eq!(result.is_ok(), preview);
        assert!(e.data.private.is_empty());
        assert_eq!(e.revision, revision);
    }
    e.media.config.pending_limit = 0;
    assert!(
        e.allocate_media_target("bob", "@direct:eve", "file", 8, BTreeMap::new(), false)
            .is_err()
    );
    assert!(e.data.private.is_empty());
    e.media.config.pending_limit = 10;
    e.store.db.lock().unwrap().execute_batch("CREATE TRIGGER deny_pair BEFORE INSERT ON entities WHEN NEW.kind='private' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(
        e.allocate_media_target("bob", "@direct:eve", "file", 8, BTreeMap::new(), false)
            .is_err()
    );
    assert!(e.data.private.is_empty());
    let db = e.store.db.lock().unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM media", [], |r| r.get::<_, usize>(0))
            .unwrap(),
        0
    );
    drop(db);
    e.store
        .db
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER deny_pair;")
        .unwrap();
    let media = e
        .allocate_media_target("bob", "@direct:eve", "file", 8, BTreeMap::new(), false)
        .unwrap()
        .unwrap();
    assert_eq!(e.data.private["bob:eve"].id, media.conversation);
    assert_eq!(
        e.store
            .db
            .lock()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM entities WHERE kind='private'",
                [],
                |r| r.get::<_, usize>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
fn empty_cleanup_and_console_reads_do_not_write_or_advance_revision() {
    let mut e = engine();
    let revision = e.revision;
    let writes = e.store.db.lock().unwrap().total_changes();
    let audit = e.data.policy.audit.len();
    for input in ["/user ls", "/user get bob", "/room ls", "/clean 7d #lobby"] {
        e.execute(None, None, input).unwrap();
        assert_eq!(e.revision, revision);
        assert_eq!(e.store.db.lock().unwrap().total_changes(), writes);
        assert_eq!(e.data.policy.audit.len(), audit);
    }
}

#[test]
fn bulk_metadata_validates_key_caps_before_mutating_any_account() {
    let mut e = engine();
    let scope = e.data.rooms["lobby"].id.clone();
    e.data.users.get_mut("eve").unwrap().metadata.insert(
        scope,
        (0..32)
            .map(|i| (format!("key{i}"), "value".into()))
            .collect(),
    );
    let before = e.data.clone();
    let parts = ["/user", "set", "@all", "label", "test", "#lobby"];
    let context = CommandContext {
        actor: None,
        command: "/user",
        room: None,
        input: "",
        parts: &parts,
        view: None,
        scope: Scope::Server,
        markdown: false,
        author: "console",
    };
    assert!(
        e.apply_objects(&context)
            .unwrap_err()
            .contains("32 metadata keys")
    );
    assert!(e.data == before);
}

#[test]
fn scoped_snapshots_exclude_working_messages_and_unrelated_maps() {
    let mut e = engine();
    e.execute(Some("bob"), Some("lobby"), "cached body")
        .unwrap();
    let message = room_messages(&e, "lobby")[0].clone();
    e.working = Some(messages::WorkingMessage {
        reaction: None,
        conversation: e.data.rooms["lobby"].id.clone(),
        message,
    });
    let change = MetadataChange::keyed(
        &e.data,
        changes::DirtyKeys {
            rooms: ["lobby".into()].into(),
            ..Default::default()
        },
    );
    let view = change.view();
    assert!(view.rooms.unwrap()["lobby"] == e.data.rooms["lobby"]);
    assert!(view.users.is_none());
    assert!(view.private.is_none());
    assert!(view.sessions.is_none());
}

#[test]
fn cleanup_counts_use_existing_timestamp_index_and_count_multiple_references_once() {
    let mut e = engine();
    e.execute(Some("bob"), Some("lobby"), "message").unwrap();
    let id = e.data.rooms["lobby"].id.clone();
    let mut links = String::new();
    for _ in 0..2 {
        let mut media = e.allocate_media("bob", &id, "test.png", 8).unwrap();
        media.ready = true;
        media.object_key = media.final_key();
        media.content_type = "image/png".into();
        e.finish_media("bob", &media).unwrap();
        links.push_str(&media_link(&media.id));
    }
    e.execute(Some("bob"), Some("lobby"), &links).unwrap();
    let counts = e
        .cleanup_counts(
            &[id.clone(), "missing".into()],
            u64::try_from(i64::MAX).unwrap(),
        )
        .unwrap();
    assert_eq!(counts[&id], (2, 2));
    assert_eq!(counts["missing"], (0, 0));
    let db = e.store.db.lock().unwrap();
    let mut query = db
        .prepare(format!(
            "EXPLAIN QUERY PLAN {}",
            storage::CLEANUP_COUNTS_SQL
        ))
        .unwrap();
    let plan = query
        .query_map(
            params![serde_json::to_string(&[id]).unwrap(), i64::MAX],
            |r| r.get::<_, String>(3),
        )
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(
        plan.iter().any(|line| line.contains("messages_time")),
        "{plan:?}"
    );
}

#[test]
fn routing_normalizes_only_the_room_target() {
    assert_eq!(
        crate::commands::route("/room add #bob #lobby"),
        "/add #bob lobby"
    );
    assert_eq!(
        crate::commands::route("/room rename #lobby #new"),
        "/rename room lobby #new"
    );
    let mut e = engine();
    let before = e.data.clone();
    for input in ["/room add #bob #lobby", "/room rename #lobby #new"] {
        assert!(e.execute(None, None, input).is_err());
        assert!(e.data == before);
    }
}

#[test]
fn indexed_conversation_updates_follow_renames_deletion_and_private_access() {
    let mut e = engine();
    let room_id = e.data.rooms["lobby"].id.clone();
    e.execute(Some("alice"), Some("lobby"), "indexed room")
        .unwrap();
    e.execute(None, None, "/room rename #lobby renamed")
        .unwrap();
    assert_eq!(
        e.conversation_update("bob", &room_id).unwrap().unwrap()["view"],
        "renamed"
    );
    e.execute(None, None, "/room delete #renamed").unwrap();
    assert!(e.conversation_update("bob", &room_id).unwrap().is_none());
    e.execute(Some("alice"), None, "/tell bob indexed private")
        .unwrap();
    let private_id = e.data.private["alice:bob"].id.clone();
    assert_eq!(
        e.conversation_update("alice", &private_id)
            .unwrap()
            .unwrap()["view"],
        "@direct:bob"
    );
    assert!(e.conversation_update("eve", &private_id).unwrap().is_none());
    e.execute(None, None, "/user rename bob robert").unwrap();
    assert_eq!(
        e.conversation_update("alice", &private_id)
            .unwrap()
            .unwrap()["view"],
        "@direct:robert"
    );
}

#[test]
fn transport_identity_lookups_follow_disable_rename_and_replacement() {
    let mut e = engine();
    let id = e.active_account_id("bob").unwrap().to_owned();
    let hash = e.password_hash("bob").unwrap().to_owned();
    assert!(e.has_accounts());
    assert_eq!(e.active_account_name(&id), Some("bob"));
    e.execute(None, None, "/user disable bob").unwrap();
    assert_eq!(e.account_id("bob"), Some(id.as_str()));
    assert!(e.active_account_id("bob").is_none());
    assert!(e.active_account_name(&id).is_none());
    assert!(e.password_hash("bob").is_none());
    e.execute(None, None, "/user enable bob").unwrap();
    assert_eq!(e.password_hash("bob"), Some(hash.as_str()));
    e.execute(None, None, "/user rename bob robert").unwrap();
    assert!(e.account_id("bob").is_none());
    assert_eq!(e.active_account_name(&id), Some("robert"));
    e.execute(None, None, "/user delete robert").unwrap();
    assert!(e.active_account_name(&id).is_none());
    e.provision("robert", hash, false, false).unwrap();
    assert_ne!(e.active_account_id("robert"), Some(id.as_str()));
    assert!(e.active_account_name(&id).is_none());
}
