use super::media::Media;
use super::tests::{engine, room_messages};
use super::*;

fn ready(e: &Engine, user: &str, conversation: &str) -> Media {
    let mut media = e
        .allocate_media(user, conversation, "photo.png", 8)
        .unwrap();
    media.ready = true;
    media.object_key = media.final_key();
    media.content_type = "image/png".into();
    e.finish_media(user, &media).unwrap();
    media
}
fn link(media: &Media) -> String {
    format!(
        "![photo](https://example.com/commonroom/api/media/{}/file)",
        media.id
    )
}

#[test]
fn media_permissions_ownership_and_reference_transactions() {
    let mut e = engine();
    let room = e.data.rooms["lobby"].id.clone();
    e.execute(Some("alice"), None, "/new room other").unwrap();
    let other = e.data.rooms["other"].id.clone();
    assert!(e.allocate_media("bob", &other, "file", 8).is_err());
    for size in [0, e.media.config.max_size + 1] {
        assert!(e.allocate_media("alice", &room, "file", size).is_err());
    }
    let media = ready(&e, "alice", &room);
    assert!(e.media_owner("bob", &media.id).is_err());
    assert!(e.readable_media("alice", &media.id).is_err());
    let before = e.data.clone();
    assert!(
        e.execute(Some("bob"), Some("lobby"), &link(&media))
            .is_err()
    );
    assert!(e.data == before);
    assert!(room_messages(&e, "lobby").is_empty());
    assert!(
        e.execute(Some("alice"), Some("other"), &link(&media))
            .is_err()
    );
    e.execute(Some("alice"), Some("lobby"), &link(&media))
        .unwrap();
    assert!(e.readable_media("bob", &media.id).is_ok());
    // Once published, a reader can reuse an attachment inside its conversation.
    e.execute(Some("bob"), Some("lobby"), &link(&media))
        .unwrap();
    let first = room_messages(&e, "lobby")[0].id.clone();
    e.execute(Some("alice"), Some("lobby"), &format!("/retract {first}"))
        .unwrap();
    assert!(e.readable_media("bob", &media.id).is_ok());
    let second = room_messages(&e, "lobby")[0].id.clone();
    e.execute(
        Some("bob"),
        Some("lobby"),
        &format!("/edit {second} removed"),
    )
    .unwrap();
    assert!(!e.media_attached(&media.id).unwrap());
    assert!(e.readable_media("bob", &media.id).is_err());
}

#[test]
fn media_retention_revocation_and_cleanup_claims() {
    let mut e = engine();
    let room = e.data.rooms["lobby"].id.clone();
    let media = ready(&e, "alice", &room);
    e.execute(Some("alice"), Some("lobby"), &link(&media))
        .unwrap();
    e.execute(Some("alice"), Some("lobby"), "/kick bob #lobby")
        .unwrap();
    assert!(e.readable_media("bob", &media.id).is_err());
    e.set_limits(64, 64, 1).unwrap();
    e.execute(Some("alice"), Some("lobby"), "replace").unwrap();
    assert!(!e.media_attached(&media.id).unwrap());
    e.expired_media().unwrap();
    e.store
        .db
        .lock()
        .unwrap()
        .execute(
            "UPDATE media SET unreferenced=?1,created=?1,json=json_set(json,'$.created',?1)",
            [now() - e.media.config.pending_ttl - 1],
        )
        .unwrap();
    assert_eq!(e.expired_media().unwrap().len(), 1);
    // Deletion is claimed before network I/O; a concurrent send cannot revive it.
    assert!(
        e.execute(Some("alice"), Some("lobby"), &link(&media))
            .is_err()
    );
    assert!(e.media_owner("alice", &media.id).is_err());
    e.remove_media_record(&media.id).unwrap();
    assert!(e.stored_media(&media.id).unwrap().deleted);
}

#[test]
fn media_private_access_and_stable_identity() {
    let mut e = engine();
    e.execute(Some("alice"), None, "/tell bob first").unwrap();
    let id = e.data.private["alice:bob"].id.clone();
    let media = ready(&e, "alice", &id);
    e.execute(
        Some("alice"),
        Some("@direct:bob"),
        &format!("/tell bob {}", link(&media)),
    )
    .unwrap();
    assert!(e.readable_media("bob", &media.id).is_ok());
    assert!(e.readable_media("eve", &media.id).is_err());
    e.execute(Some("alice"), None, "/rename alicia").unwrap();
    assert!(e.media_owner("alicia", &media.id).is_ok());
    assert!(e.readable_media("bob", &media.id).is_ok());
}

#[test]
fn media_quota_and_failed_attachment_writes_are_atomic() {
    let mut e = engine();
    let room = e.data.rooms["lobby"].id.clone();
    let media = ready(&e, "alice", &room);
    e.store.db.lock().unwrap().execute_batch("CREATE TRIGGER fail_attachment BEFORE INSERT ON message_media BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(
        e.execute(Some("alice"), Some("lobby"), &link(&media))
            .is_err()
    );
    assert!(room_messages(&e, "lobby").is_empty());
    assert!(!e.media_attached(&media.id).unwrap());
    for _ in 0..8 {
        e.allocate_media("bob", &room, "file", 1).unwrap();
    }
    assert!(e.allocate_media("bob", &room, "file", 1).is_err());
}

#[test]
fn media_metadata_overrides_defaults_and_is_rechecked() {
    let mut e = super::tests::engine();
    let room = e.data.rooms["lobby"].id.clone();
    assert!(e.allocate_media("bob", &room, "file", 8).is_ok());
    e.execute(None, None, "/user set bob media.max_age 7d #lobby")
        .unwrap();
    e.execute(None, None, "/user set bob media.max_size 16 #lobby")
        .unwrap();
    assert!(e.allocate_media("bob", &room, "file", 17).is_err());
    let media = ready(&e, "bob", &room);
    assert_eq!(media.max_age, 604800);
    assert!(media.final_key().starts_with("media/7d/"));
    e.execute(None, None, "/revoke bob #lobby w:media.upload")
        .unwrap();
    assert!(e.media_owner("bob", &media.id).is_err());
    assert!(
        e.execute(Some("bob"), Some("lobby"), &link(&media))
            .is_err()
    );
    e.execute(None, None, "/grant bob #lobby w:media.upload")
        .unwrap();
    e.execute(None, None, "/user set bob media.max_size 8 #lobby")
        .unwrap();
    e.execute(None, None, "/user set bob media.max_age -1 #lobby")
        .unwrap();
    let permanent = ready(&e, "bob", &room);
    assert_eq!(permanent.max_age, -1);
    assert!(permanent.final_key().starts_with("media/permanent/"));
    e.execute(Some("bob"), Some("lobby"), &link(&permanent))
        .unwrap();
    assert_eq!(e.stored_media(&permanent.id).unwrap().expires, None);
    // Updating retention also applies to attempts to reuse existing media.
    e.execute(None, None, "/user set bob media.max_size 32 #lobby")
        .unwrap();
    e.execute(None, None, "/user set bob media.max_age 1d #lobby")
        .unwrap();
    assert!(
        e.media_allowance(
            "bob",
            &authorization::Scope::Room(room.clone()),
            16,
            Some(-1)
        )
        .is_err()
    );
    let sized = e.allocate_media("bob", &room, "large", 16).unwrap();
    assert_eq!(sized.max_age, 86400);
    assert!(
        e.execute(
            None,
            None,
            "/user set bob media.max_size 999999999999 #lobby"
        )
        .is_err()
    );
    assert!(
        e.execute(None, None, "/user set bob media.max_age 0d #lobby")
            .is_err()
    );
}

#[test]
fn verified_unsent_uploads_reserve_slots_until_publication() {
    let mut e = engine();
    e.media.config.pending_limit = 1;
    let room = e.data.rooms["lobby"].id.clone();
    let media = ready(&e, "alice", &room);
    assert!(e.allocate_media("alice", &room, "file", 1).is_err());
    e.execute(Some("alice"), Some("lobby"), &link(&media))
        .unwrap();
    let first_expiry = e.stored_media(&media.id).unwrap().expires.unwrap();
    assert!((now() + 604799..=now() + 604801).contains(&first_expiry));
    assert!(e.allocate_media("alice", &room, "file", 1).is_ok());
    e.execute(Some("alice"), Some("lobby"), &link(&media))
        .unwrap();
    assert_eq!(
        e.stored_media(&media.id).unwrap().expires,
        Some(first_expiry)
    );
}

#[test]
fn published_media_expires_while_referenced_and_surrounding_text_remains_editable() {
    let mut e = engine();
    let room = e.data.rooms["lobby"].id.clone();
    let media = ready(&e, "alice", &room);
    e.execute(Some("alice"), Some("lobby"), &link(&media))
        .unwrap();
    e.store
        .db
        .lock()
        .unwrap()
        .execute(
            "UPDATE media SET expires=?2,json=json_set(json,'$.expires',?2) WHERE id=?1",
            params![media.id, now() - 1],
        )
        .unwrap();
    assert!(e.readable_media("alice", &media.id).is_err());
    assert!(
        e.execute(Some("alice"), Some("lobby"), &link(&media))
            .is_err()
    );
    let id = room_messages(&e, "lobby")[0].id.clone();
    e.execute(
        Some("alice"),
        Some("lobby"),
        &format!("/edit {id} caption {}", link(&media)),
    )
    .unwrap();
    assert_eq!(e.expired_media().unwrap().len(), 1);
    e.remove_media_record(&media.id).unwrap();
    assert!(e.stored_media(&media.id).unwrap().deleted);
    assert!(e.media_attached(&media.id).unwrap());
    assert!(e.readable_media("alice", &media.id).is_err());
}

#[test]
fn cancelled_uploads_cannot_be_published_and_cleanup_does_not_starve() {
    let mut e = engine();
    let room = e.data.rooms["lobby"].id.clone();
    let first = ready(&e, "alice", &room);
    let second = ready(&e, "alice", &room);
    assert!(e.abandon_media("bob", &first.id).is_err());
    e.abandon_media("alice", &first.id).unwrap();
    e.abandon_media("alice", &second.id).unwrap();
    e.media.config.cleanup_batch = 1;
    let batch = e.expired_media().unwrap();
    assert_eq!(batch.len(), 1);
    let next = e.expired_media().unwrap();
    assert_ne!(batch[0].id, next[0].id);
    assert!(
        e.execute(Some("alice"), Some("lobby"), &link(&first))
            .is_err()
    );
}

#[test]
fn media_can_be_the_first_private_message() {
    let mut e = engine();
    let id = e
        .resolve_media_conversation("alice", "@direct:bob")
        .unwrap();
    assert!(e.working.is_none());
    let media = ready(&e, "alice", &id);
    e.execute(Some("alice"), None, &format!("/tell bob {}", link(&media)))
        .unwrap();
    assert!(e.readable_media("bob", &media.id).is_ok());
    let other = e
        .resolve_media_conversation("eve", "@direct:alice")
        .unwrap();
    assert!(e.allocate_media("eve", &other, "file", 8).is_ok());
    assert!(e.allocate_media("bob", &other, "file", 8).is_err());
}

#[test]
fn room_managers_cannot_change_media_metadata_but_can_revoke_uploads() {
    let mut e = super::tests::engine();
    e.execute(None, None, "/user set alice media.max_size 16 #lobby")
        .unwrap();
    e.execute(None, None, "/user set alice media.max_age 1d #lobby")
        .unwrap();
    e.execute(None, None, "/user set bob media.max_size 8 #lobby")
        .unwrap();
    e.execute(None, None, "/user set bob media.max_age 1d #lobby")
        .unwrap();
    for command in [
        "/user set bob media.max_size 32 #lobby",
        "/user set bob media.max_age 7d #lobby",
        "/user set bob media.max_age -1 #lobby",
        "/user set bob media.max_age @unset #lobby",
        "/user set bob media.max_size @unset #lobby",
    ] {
        assert!(e.execute(Some("alice"), Some("lobby"), command).is_err());
    }
    assert!(
        e.execute(
            Some("alice"),
            Some("lobby"),
            "/user set bob media.max_size 16 #lobby"
        )
        .is_err()
    );
    e.execute(
        None,
        Some("lobby"),
        "/user set bob media.max_size 16 #lobby",
    )
    .unwrap();
    e.execute(
        Some("alice"),
        Some("lobby"),
        "/revoke bob #lobby w:media.upload",
    )
    .unwrap();
    assert!(
        e.allocate_media("bob", &e.data.rooms["lobby"].id, "file", 8)
            .is_err()
    );
}

#[test]
fn room_upload_defaults_can_be_revoked_without_removing_text_or_read_access() {
    let mut e = engine();
    let room = e.data.rooms["lobby"].id.clone();
    assert_eq!(
        e.allocate_media("eve", &room, "file", 8).unwrap().max_age,
        e.media.config.max_age
    );
    let published = ready(&e, "bob", &room);
    e.execute(Some("bob"), Some("lobby"), &link(&published))
        .unwrap();
    let pending = ready(&e, "bob", &room);
    e.execute(Some("alice"), None, "/new room other").unwrap();
    e.execute(Some("alice"), Some("other"), "/add bob #other")
        .unwrap();
    e.execute(None, None, "/grant bob @global w:media.upload")
        .unwrap();
    let revision = e.data.policy.revision;
    e.execute(
        Some("alice"),
        Some("lobby"),
        "/revoke bob #lobby w:media.upload",
    )
    .unwrap();
    assert!(e.data.policy.revision > revision);
    assert!(
        !e.effective(Some("bob"), &authorization::Scope::Room(room.clone()))
            .has(authorization::Action::MediaUpload)
    );
    assert!(e.allocate_media("bob", &room, "file", 8).is_err());
    assert!(e.media_owner("bob", &pending.id).is_err());
    assert!(
        e.execute(Some("bob"), Some("lobby"), &link(&pending))
            .is_err()
    );
    e.execute(Some("bob"), Some("lobby"), "text still works")
        .unwrap();
    e.execute(None, None, "/revoke bob @global w:media.upload")
        .unwrap();
    let private = e
        .resolve_media_conversation("bob", "@direct:alice")
        .unwrap();
    let direct = ready(&e, "bob", &private);
    e.execute(Some("bob"), None, &format!("/tell alice {}", link(&direct)))
        .unwrap();
    assert!(e.readable_media("alice", &direct.id).is_ok());
    assert!(e.readable_media("bob", &published.id).is_ok());
    assert!(
        e.allocate_media("bob", &e.data.rooms["other"].id, "file", 8)
            .is_ok()
    );
    e.execute(Some("alice"), Some("lobby"), "/grant bob user #lobby")
        .unwrap();
    assert!(e.allocate_media("bob", &room, "file", 8).is_err());
    assert!(
        e.execute(
            Some("eve"),
            Some("lobby"),
            "/grant bob #lobby w:media.upload"
        )
        .is_err()
    );
    e.execute(
        Some("alice"),
        Some("lobby"),
        "/grant bob #lobby w:media.upload",
    )
    .unwrap();
    assert!(e.allocate_media("bob", &room, "file", 8).is_ok());
}

#[test]
fn room_media_restrictions_survive_restart_rename_and_owner_transfer() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    let mut e = Engine::open(&path).unwrap();
    for (user, admin) in [("alice", true), ("bob", false), ("eve", false)] {
        e.provision(user, "hash".into(), admin, false).unwrap();
    }
    e.execute(Some("alice"), None, "/new room room").unwrap();
    e.execute(Some("alice"), Some("room"), "/add bob").unwrap();
    e.execute(Some("alice"), Some("room"), "/add eve").unwrap();
    let room = e.data.rooms["room"].id.clone();
    e.execute(
        Some("alice"),
        Some("room"),
        "/revoke bob #room w:media.upload",
    )
    .unwrap();
    drop(e);
    let mut e = Engine::open(&path).unwrap();
    assert!(e.allocate_media("bob", &room, "file", 8).is_err());
    e.execute(Some("bob"), None, "/rename bobby").unwrap();
    e.execute(Some("alice"), None, "/rename room room renamed")
        .unwrap();
    assert!(e.allocate_media("bobby", &room, "file", 8).is_err());
    e.execute(Some("alice"), Some("renamed"), "/owner eve #renamed")
        .unwrap();
    assert!(e.allocate_media("bobby", &room, "file", 8).is_err());
    e.execute(
        Some("eve"),
        Some("renamed"),
        "/grant bobby #renamed w:media.upload",
    )
    .unwrap();
    assert!(e.allocate_media("bobby", &room, "file", 8).is_ok());
    e.execute(
        Some("eve"),
        Some("renamed"),
        "/revoke bobby #renamed w:media.upload",
    )
    .unwrap();
    e.execute(None, None, "/rm user bobby").unwrap();
    assert!(e.data.policy.media_revocations.is_empty());
    e.provision("bobby", "hash".into(), false, false).unwrap();
    e.execute(Some("eve"), Some("renamed"), "/add bobby")
        .unwrap();
    assert!(e.allocate_media("bobby", &room, "file", 8).is_ok());
    e.execute(
        Some("eve"),
        Some("renamed"),
        "/revoke bobby #renamed w:media.upload",
    )
    .unwrap();
    e.execute(Some("eve"), None, "/rm room renamed").unwrap();
    assert!(e.data.policy.media_revocations.is_empty());
}

#[test]
fn failed_room_media_revocation_and_restore_leave_compiled_authority_unchanged() {
    let mut e = engine();
    let room = e.data.rooms["lobby"].id.clone();
    for blocked in [false, true] {
        if blocked {
            e.execute(
                Some("alice"),
                Some("lobby"),
                "/revoke bob #lobby w:media.upload",
            )
            .unwrap();
        }
        let before = e.data.clone();
        e.store.db.lock().unwrap().execute_batch("CREATE TRIGGER fail_media_policy BEFORE UPDATE ON state BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        let command = if blocked {
            "/grant bob #lobby w:media.upload"
        } else {
            "/revoke bob #lobby w:media.upload"
        };
        assert!(e.execute(Some("alice"), Some("lobby"), command).is_err());
        assert!(e.data == before);
        assert_eq!(e.allocate_media("bob", &room, "file", 8).is_err(), blocked);
        e.store
            .db
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER fail_media_policy;")
            .unwrap();
    }
}

#[test]
fn media_reference_removal_tracks_orphans_and_columns_override_legacy_json() {
    let mut e = engine();
    let room = e.data.rooms["lobby"].id.clone();
    let media = ready(&e, "alice", &room);
    e.execute(Some("alice"), Some("lobby"), &link(&media))
        .unwrap();
    e.abandon_media("alice", &media.id).unwrap(); // Idempotent no-op after publication.
    assert!(e.readable_media("bob", &media.id).is_ok());
    let message = e.history("alice", "lobby").unwrap().messages[0].id.clone();
    e.execute(Some("alice"), Some("lobby"), &format!("/retract {message}"))
        .unwrap();
    let unreferenced: Option<u64> = e
        .store
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT unreferenced FROM media WHERE id=?1",
            [&media.id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(unreferenced.is_some());
    e.abandon_media("alice", &media.id).unwrap();
    assert!(!e.stored_media(&media.id).unwrap().deleting);
    e.store.db.lock().unwrap().execute("UPDATE media SET deleting=1,json=json_set(json,'$.deleting',json('false'),'$.ready',json('false')) WHERE id=?1",[&media.id]).unwrap();
    let restored = e.stored_media(&media.id).unwrap();
    assert!(restored.deleting && restored.ready);
}

#[test]
fn restart_cleanup_preserves_shared_references_and_releases_quota_only_after_delete() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("chat.sqlite");
    let mut e = Engine::open(&path).unwrap();
    e.provision("alice", "hash".into(), true, false).unwrap();
    e.execute(Some("alice"), None, "/new room files").unwrap();
    let room = e.data.rooms["files"].id.clone();
    let live = ready(&e, "alice", &room);
    let orphan = ready(&e, "alice", &room);
    let pending = e.allocate_media("alice", &room, "pending", 8).unwrap();
    e.execute(Some("alice"), Some("files"), &link(&live))
        .unwrap();
    e.execute(Some("alice"), Some("files"), &link(&live))
        .unwrap();
    let first = room_messages(&e, "files")[0].id.clone();
    e.execute(Some("alice"), Some("files"), &format!("/retract {first}"))
        .unwrap();
    drop(e);
    let mut e = Engine::open(&path).unwrap();
    e.media.config.user_quota = 24;
    e.prepare_media_cleanup().unwrap();
    let claimed = e.expired_media().unwrap();
    assert_eq!(claimed.len(), 2);
    assert!(claimed.iter().any(|m| m.id == orphan.id));
    assert!(claimed.iter().any(|m| m.id == pending.id));
    assert!(e.readable_media("alice", &live.id).is_ok());
    // Failed storage deletion leaves both the claim and all byte reservations.
    assert!(e.allocate_media("alice", &room, "blocked", 1).is_err());
    e.prepare_media_cleanup().unwrap();
    assert_eq!(e.expired_media().unwrap().len(), 2);
    e.remove_media_record(&orphan.id).unwrap();
    assert!(e.allocate_media("alice", &room, "reclaimed", 8).is_ok());
    // New, unsent uploads in this process keep their normal pending grace.
    assert!(!e.expired_media().unwrap().iter().any(|m| m.id == live.id));
    let last = room_messages(&e, "files")[0].id.clone();
    e.execute(
        Some("alice"),
        Some("files"),
        &format!("/edit {last} removed"),
    )
    .unwrap();
    assert!(e.expired_media().unwrap().iter().any(|m| m.id == live.id));
    // Tombstones must keep staging recovery alive for a late in-flight PUT.
    assert!(
        !e.media_staging_cleanup(now().saturating_add(1))
            .unwrap()
            .contains(&orphan.id)
    );
    e.store
        .db
        .lock()
        .unwrap()
        .execute(
            "UPDATE media SET created=?2 WHERE id=?1",
            params![orphan.id, now() - e.media.config.pending_ttl - 1],
        )
        .unwrap();
    assert!(
        e.media_staging_cleanup(now().saturating_add(1))
            .unwrap()
            .contains(&orphan.id)
    );
}

#[test]
fn media_download_tickets_recheck_owner_permissions_expiry_and_shared_sql_changes() {
    let mut e = engine();
    let room = e.data.rooms["lobby"].id.clone();
    let media = ready(&e, "alice", &room);
    e.execute(Some("alice"), Some("lobby"), &link(&media))
        .unwrap();
    let mut ticket = e.prepare_media_download("bob", &media.id).unwrap();
    assert!(e.validate_media_download("bob", &ticket).is_ok());
    assert!(e.validate_media_download("alice", &ticket).is_err());
    let expiry = ticket.media.expires;
    ticket.media.expires = Some(now());
    assert!(e.validate_media_download("bob", &ticket).is_err());
    ticket.media.expires = expiry;
    let revision = e.revision;
    e.store
        .db
        .lock()
        .unwrap()
        .execute("UPDATE media SET deleting=1 WHERE id=?1", [&media.id])
        .unwrap();
    assert_eq!(e.revision, revision);
    assert!(e.validate_media_download("bob", &ticket).is_err());
    e.store
        .db
        .lock()
        .unwrap()
        .execute("UPDATE media SET deleting=0 WHERE id=?1", [&media.id])
        .unwrap();
    let ticket = e.prepare_media_download("bob", &media.id).unwrap();
    e.execute(None, None, "/room kick bob #lobby").unwrap();
    assert!(e.validate_media_download("bob", &ticket).is_err());
}
