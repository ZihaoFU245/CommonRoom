use super::*;
#[test]
fn admins_and_room_managers_can_read_but_only_su_can_change_metadata() {
    let mut e = crate::engine::tests::engine();
    e.execute(None, Some("lobby"), "/user set @all label seeded")
        .unwrap();
    assert!(e.allows(Some("alice"), &Scope::Server, Action::AccountMetadataRead));
    assert!(!e.allows(Some("alice"), &Scope::Server, Action::AccountMetadataWrite));
    assert!(
        e.execute(Some("alice"), Some("lobby"), "/user get @all")
            .unwrap()
            .contains("seeded")
    );
    e.execute(None, None, "/grant bob admin #lobby").unwrap();
    assert!(
        e.execute(Some("bob"), Some("lobby"), "/user get eve")
            .unwrap()
            .contains("seeded")
    );
    // Explicit or previously stored write flags cannot bypass the su boundary.
    e.execute(None, None, "/grant alice @global x:account.metadata")
        .unwrap();
    let before = e.data.clone();
    let writes = e.store.db.lock().unwrap().total_changes();
    for actor in ["alice", "bob"] {
        for command in [
            "/user set bob label changed",
            "/user set @all label changed #lobby",
            "/user set bob label @unset #lobby",
            "/user set bob media.user_quota 1g",
        ] {
            assert!(
                e.execute(Some(actor), Some("lobby"), command)
                    .unwrap_err()
                    .contains("Only su"),
                "{actor}: {command}"
            );
            assert!(e.data == before);
            assert_eq!(e.store.db.lock().unwrap().total_changes(), writes);
        }
    }
    e.execute(None, None, "/grant bob su").unwrap();
    e.execute(Some("bob"), Some("lobby"), "/user set @all label changed")
        .unwrap();
    assert!(
        e.execute(Some("bob"), Some("lobby"), "/user get alice")
            .unwrap()
            .contains("changed")
    );
}
