use super::*;
use crate::engine::tests::engine;

#[test]
fn object_commands_preserve_room_and_account_identity_and_metadata() {
    let mut e = engine();
    let id = e.data.rooms["lobby"].id.clone();
    assert!(
        e.execute(None, None, "/user rename room lobby wrong-family")
            .is_err()
    );
    assert!(e.data.rooms.contains_key("lobby"));
    e.execute(None, Some("lobby"), "/user set bob label member")
        .unwrap();
    assert_eq!(
        e.execute(Some("bob"), Some("lobby"), "/user ls").unwrap(),
        "alice\nbob\neve"
    );
    assert!(
        e.execute(Some("bob"), None, "/room ls")
            .unwrap()
            .contains("#lobby")
    );
    e.execute(Some("alice"), Some("lobby"), "/room rename #lobby renamed")
        .unwrap();
    e.execute(Some("bob"), Some("renamed"), "/user rename bobby")
        .unwrap();
    assert_eq!(e.data.rooms["renamed"].id, id);
    assert_eq!(e.data.users["bobby"].metadata[&id]["label"], "member");
    e.execute(Some("alice"), None, "/room delete #renamed")
        .unwrap();
    assert!(!e.data.users["bobby"].metadata.contains_key(&id));
}
#[test]
fn metadata_bulk_selection_is_scoped_private_and_atomic() {
    let mut e = engine();
    e.provision("outside", "hash".into(), false, false).unwrap();
    e.execute(None, Some("lobby"), "/user set @all label room-member")
        .unwrap();
    let all: serde_json::Value = serde_json::from_str(
        &e.execute(Some("alice"), None, "/user get @all #lobby")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(all.as_object().unwrap().len(), 3);
    assert_eq!(all["bob"]["#lobby"]["label"]["value"], "room-member");
    assert!(e.data.users["outside"].metadata.is_empty());
    let before = e.data.clone();
    assert!(
        e.execute(Some("bob"), Some("lobby"), "/user get @all #lobby")
            .is_err()
    );
    assert!(
        e.execute(Some("bob"), Some("lobby"), "/user set @all label denied")
            .is_err()
    );
    assert!(
        e.execute(Some("alice"), None, "/user set outside label denied #lobby")
            .is_err()
    );
    assert!(e.data == before);
    assert!(
        e.read_command(Some("bob"), Some("lobby"), "/user get bob")
            .unwrap()
            .is_ok()
    );
    e.execute(None, None, "/grant eve su").unwrap();
    let before = e.data.clone();
    assert!(
        e.execute(Some("alice"), None, "/user set @all label denied #lobby")
            .is_err()
    );
    assert!(e.data == before);
}
#[test]
fn disabled_superuser_metadata_remains_protected_from_admin_bulk_and_direct_writes() {
    let mut e = engine();
    e.execute(None, None, "/grant eve su").unwrap();
    e.execute(None, None, "/user disable eve").unwrap();
    let before = e.data.clone();
    for input in ["/user set eve label denied", "/user set @all label denied"] {
        assert!(
            e.execute(Some("alice"), None, input)
                .unwrap_err()
                .contains("Only su")
        );
        assert!(e.data == before);
    }
    e.execute(None, None, "/user set eve label recovery")
        .unwrap();
}
#[test]
fn metadata_and_grants_bulk_writes_rollback_on_storage_failure() {
    let mut e = engine();
    let before = e.data.clone();
    e.store.db.lock().unwrap().execute_batch("CREATE TRIGGER deny_metadata BEFORE UPDATE ON entities WHEN NEW.kind='account' AND NEW.key='eve' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(e.execute(None, None, "/user set @all label test").is_err());
    assert!(e.data == before);
    e.store.db.lock().unwrap().execute_batch("DROP TRIGGER deny_metadata; CREATE TRIGGER deny_policy BEFORE UPDATE ON state BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(
        e.execute(None, None, "/grant @all #lobby r:message.metadata")
            .is_err()
    );
    assert!(e.data == before);
}
#[test]
fn bulk_grants_and_revokes_check_every_target_and_last_admin() {
    let mut e = engine();
    e.execute(Some("alice"), None, "/grant @all #lobby w:message.react")
        .unwrap();
    assert!(
        e.data
            .policy
            .grants
            .iter()
            .filter(|g| g.permissions.contains("w:message.react"))
            .count()
            >= 3
    );
    e.execute(Some("alice"), None, "/revoke @all #lobby w:message.react")
        .unwrap();
    assert!(
        !e.data
            .policy
            .grants
            .iter()
            .any(|g| g.permissions.contains("w:message.react"))
    );
    e.provision("otheradmin", "hash".into(), true, false)
        .unwrap();
    let before = e.data.clone();
    assert!(
        e.execute(Some("alice"), None, "/revoke @all admin")
            .is_err()
    );
    assert!(e.data == before);
    // The owner is among @all, so participant-group removal must reject all.
    assert!(
        e.execute(Some("alice"), None, "/revoke @all user #lobby")
            .is_err()
    );
    assert!(e.data == before);
    e.execute(None, None, "/grant eve su").unwrap();
    let before = e.data.clone();
    assert!(
        e.execute(Some("alice"), None, "/grant @all #lobby w:message.react")
            .is_err()
    );
    assert!(e.data == before);
}
#[test]
fn metadata_limits_inherit_validate_and_enforce_account_quota() {
    let mut e = engine();
    let room = e.room_scope("lobby").unwrap();
    let id = e.data.rooms["lobby"].id.clone();
    e.execute(None, None, "/user set bob media.max_size 16")
        .unwrap();
    e.execute(None, None, "/user set bob media.max_age 1d #lobby")
        .unwrap();
    e.execute(None, None, "/user set bob media.user_quota 20")
        .unwrap();
    assert_eq!(e.media_allowance("bob", &room, 16, None).unwrap(), 86400);
    assert!(e.allocate_media("bob", &id, "file", 17).is_err());
    e.allocate_media("bob", &id, "file", 16).unwrap();
    assert!(e.allocate_media("bob", &id, "file", 8).is_err());
    e.execute(None, None, "/user set bob media.max_size 8 #lobby")
        .unwrap();
    assert!(e.media_allowance("bob", &room, 9, None).is_err());
    e.execute(None, None, "/user set bob media.max_size @unset #lobby")
        .unwrap();
    assert!(e.media_allowance("bob", &room, 9, None).is_ok());
    for input in [
        "/user set bob media.max_size 0",
        "/user set bob media.max_age 0d",
        "/user set bob media.user_quota 8 #lobby",
        "/user set bob media.unknown 1",
        "/user set bob media.unknown @unset",
        "/user set bob media.user_quota @unset #lobby",
        "/grant bob #lobby w:media.upload 16 7d",
    ] {
        let before = e.data.clone();
        assert!(e.execute(None, None, input).is_err(), "{input}");
        assert!(e.data == before);
    }
}
#[test]
fn account_scoped_metadata_reads_apply_only_to_the_named_target_and_writes_need_su() {
    let mut e = engine();
    e.execute(None, None, "/revoke bob user").unwrap();
    for permission in ["/user", "r:account.metadata", "x:account.metadata"] {
        e.execute(
            None,
            None,
            &format!("/grant bob @account:alice {permission}"),
        )
        .unwrap();
    }
    e.execute(None, None, "/user set alice label authorized")
        .unwrap();
    assert!(
        e.execute(Some("bob"), None, "/user set alice label denied")
            .unwrap_err()
            .contains("Only su")
    );
    assert!(
        e.read_command(Some("bob"), None, "/user get alice")
            .unwrap()
            .unwrap()
            .contains("authorized")
    );
    assert!(e.execute(Some("bob"), None, "/user get eve").is_err());
    assert!(
        e.execute(Some("bob"), None, "/user set eve label denied")
            .is_err()
    );
}
#[test]
fn metadata_persists_and_read_commands_do_not_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    let mut e = Engine::open(&path).unwrap();
    e.provision("alice", "hash".into(), true, false).unwrap();
    e.execute(None, None, "/user set alice label saved")
        .unwrap();
    drop(e);
    let e = Engine::open(&path).unwrap();
    let writes = e.store.db.lock().unwrap().total_changes();
    let result = e
        .read_command(Some("alice"), None, "/user get @all")
        .unwrap()
        .unwrap();
    assert!(result.contains("saved"));
    assert!(!result.contains("hash"));
    assert_eq!(e.store.db.lock().unwrap().total_changes(), writes);
}
#[test]
fn room_owner_can_invite_with_implicit_or_explicit_scope_and_errors_use_public_syntax() {
    let mut e = crate::engine::tests::engine();
    e.execute(Some("alice"), None, "/room new #support")
        .unwrap();
    for (view, command, member) in [
        (Some("support"), "/room add bob", "bob"),
        (None, "/room add eve #support", "eve"),
    ] {
        e.execute(Some("alice"), view, command).unwrap();
        assert!(e.data.rooms["support"].members.contains(member));
        assert!(e.execute(Some(member), Some("support"), "/history").is_ok());
    }
    for (command, expected) in [
        ("/room add", "Usage: /room add user [#room]"),
        ("/room kick", "Usage: /room kick user [#room]"),
        ("/room join", "Usage: /room join #room"),
        ("/room owner", "Usage: /room owner user [#room]"),
        ("/room new", "Usage: /room new #name"),
        ("/room delete", "Usage: /room delete #name"),
    ] {
        assert_eq!(
            e.execute(Some("alice"), Some("support"), command)
                .unwrap_err()
                .message(),
            expected
        );
    }
    assert_eq!(
        e.execute(Some("alice"), None, "/room add bob")
            .unwrap_err()
            .message(),
        "Specify a room."
    );
    e.execute(Some("alice"), Some("support"), "/room kick bob")
        .unwrap();
    assert!(!e.data.rooms["support"].members.contains("bob"));
    assert!(e.execute(Some("bob"), Some("support"), "/history").is_err());
}
