use super::*;
use authorization::{Action, Scope};

fn fixture() -> Engine {
    let mut e = Engine::open(Path::new(":memory:")).unwrap();
    for (name, admin) in [
        ("alice", true),
        ("bob", false),
        ("eve", false),
        ("mallory", true),
    ] {
        e.provision(name, "hash".into(), admin, false).unwrap();
    }
    e.execute(Some("alice"), None, "/new team").unwrap();
    e.execute(Some("alice"), None, "/add bob team").unwrap();
    e
}

#[test]
fn sudo_requires_both_global_grants_and_never_persists_elevation() {
    let mut e = fixture();
    assert!(e.execute(Some("bob"), None, "/sudo /new elevated").is_err());
    e.execute(None, None, "/grant bob @global /sudo").unwrap();
    assert!(e.execute(Some("bob"), None, "/sudo /new elevated").is_err());
    e.execute(None, None, "/revoke bob @global /sudo").unwrap();
    e.execute(None, None, "/grant bob @global x:command.sudo")
        .unwrap();
    assert!(e.execute(Some("bob"), None, "/sudo /new elevated").is_err());
    e.execute(None, None, "/grant bob team /sudo").unwrap();
    // A room command grant cannot authorize global elevation.
    assert!(
        e.execute(Some("bob"), Some("team"), "/sudo /new elevated")
            .is_err()
    );
    e.execute(None, None, "/grant bob @global /sudo").unwrap();
    let assignments = e.data.policy.assignments.clone();
    let grants = e.data.policy.grants.clone();
    assert!(
        e.execute(Some("bob"), None, "/sudo /whoami")
            .unwrap()
            .contains("Name: bob\nPermission: su")
    );
    assert!(e.data.policy.assignments == assignments);
    assert!(e.data.policy.grants == grants);
    assert!(!e.is_su(Some("bob")));
    e.execute(Some("bob"), Some("team"), "/sudo /new elevated")
        .unwrap();
    assert_eq!(e.data.rooms["elevated"].owner_id, e.data.users["bob"].id);
    assert_eq!(
        e.data.policy.audit.back().unwrap().actor_id,
        e.data.users["bob"].id
    );
    e.execute(
        Some("bob"),
        Some("team"),
        "/sudo /tell eve elevated message",
    )
    .unwrap();
    assert_eq!(
        e.data.private["bob:eve"].messages.back().unwrap().from,
        "bob"
    );
    assert!(!e.is_su(Some("bob")));
    assert!(e.execute(Some("bob"), None, "/new denied").is_err());
    for input in [
        "/sudo",
        "/sudo hello",
        "/sudo /",
        "/sudo /sudo /whoami",
        "/sudo /unknown",
    ] {
        let before = e.data.clone();
        assert!(e.execute(Some("bob"), None, input).is_err(), "{input}");
        assert!(e.data == before);
        assert!(!e.is_su(Some("bob")));
    }
    // A scoped action grant is insufficient even with the global command.
    e.execute(None, None, "/revoke bob @global x:command.sudo")
        .unwrap();
    e.execute(None, None, "/grant bob team x:command.sudo")
        .unwrap();
    assert!(
        e.execute(Some("bob"), Some("team"), "/sudo /whoami")
            .is_err()
    );
    e.execute(None, None, "/disable bob").unwrap();
    assert!(e.execute(Some("bob"), None, "/sudo /whoami").is_err());
}

#[test]
fn sudo_restores_authority_after_storage_failure_and_preserves_policy_updates() {
    let mut e = fixture();
    for permission in ["/sudo", "x:command.sudo"] {
        e.execute(None, None, &format!("/grant bob @global {permission}"))
            .unwrap();
    }
    let before = e.data.clone();
    e.db.execute_batch("CREATE TRIGGER reject_sudo BEFORE UPDATE ON state BEGIN SELECT RAISE(ABORT,'forced failure'); END;").unwrap();
    assert!(e.execute(Some("bob"), None, "/sudo /new rejected").is_err());
    assert!(e.data == before);
    assert!(!e.is_su(Some("bob")));
    e.db.execute_batch("DROP TRIGGER reject_sudo;").unwrap();
    e.execute(Some("bob"), None, "/sudo /grant eve @global w:room.create")
        .unwrap();
    assert!(e.allows(Some("eve"), &Scope::Server, Action::CreateRoom));
    assert!(!e.is_su(Some("bob")));
    // Revoking the caller's sudo grant applies after the current command.
    e.execute(Some("bob"), None, "/sudo /revoke bob @global /sudo")
        .unwrap();
    assert!(e.execute(Some("bob"), None, "/sudo /whoami").is_err());
}
#[test]
fn su_is_a_grant_group_and_only_su_can_assign_it() {
    let mut e = fixture();
    assert!(e.execute(Some("alice"), None, "/grant bob su").is_err());
    assert!(
        e.execute(
            Some("alice"),
            None,
            "/grant alice @global x:group.su.assign"
        )
        .is_err()
    );
    e.execute(None, None, "/grant bob su").unwrap();
    assert!(e.groups(Some("bob")).contains(&"su".into()));
    for action in Action::ALL {
        assert!(e.allows(Some("bob"), &Scope::Server, *action));
    }
    e.execute(Some("bob"), None, "/grant eve su").unwrap();
    assert!(e.is_su(Some("eve")));
    for command in ["/revoke bob su", "/disable bob", "/deleteuser bob"] {
        assert!(e.execute(Some("alice"), None, command).is_err());
    }
    assert!(
        e.provision_by(Some("alice"), "bob", "new".into(), false, true)
            .is_err()
    );
    e.execute(Some("eve"), None, "/revoke bob su").unwrap();
    assert!(!e.is_su(Some("bob")));
    assert!(e.execute(Some("bob"), None, "/grant alice su").is_err());
}
#[test]
fn creation_grants_ownership_without_exposing_room_to_other_admins() {
    let mut e = fixture();
    assert_eq!(
        e.execute(Some("eve"), None, "/rooms").unwrap(),
        "Your rooms:\nNone.\n\nPrivate conversations:\nNone."
    );
    assert!(e.execute(Some("bob"), None, "/new private").is_err());
    e.execute(None, None, "/grant bob @global w:room.create")
        .unwrap();
    assert!(e.execute(Some("bob"), None, "/new private").is_err());
    e.execute(None, None, "/grant bob @global /new").unwrap();
    e.execute(Some("bob"), None, "/new private").unwrap();
    assert_eq!(e.data.rooms["private"].owner_id, e.data.users["bob"].id);
    assert_eq!(
        e.execute(Some("bob"), None, "/rooms").unwrap(),
        "Your rooms:\n#private — owner: bob\n#team — owner: alice\n\nPrivate conversations:\nNone."
    );
    let scope = e.room_scope("private").unwrap();
    assert!(e.allows(Some("bob"), &scope, Action::Invite));
    for actor in ["alice", "mallory", "eve"] {
        assert!(e.execute(Some(actor), None, "/join private").is_err());
        assert!(e.execute(Some(actor), None, "/delete private").is_err());
        assert!(e.execute(Some(actor), None, "/add eve private").is_err());
        assert!(
            !e.snapshot(actor)
                .unwrap()
                .available_rooms
                .contains(&"private".into())
        );
        assert!(
            !e.snapshot(actor)
                .unwrap()
                .rooms
                .iter()
                .any(|r| r.name == "private")
        );
        assert!(e.history(actor, "private").is_err());
        assert!(
            !e.execute(Some(actor), None, "/rooms")
                .unwrap()
                .contains("#private")
        );
    }
    e.execute(Some("bob"), None, "/add eve private").unwrap();
    e.execute(Some("eve"), None, "/join private").unwrap();
    e.execute(Some("eve"), Some("private"), "allowed").unwrap();
    assert!(e.execute(Some("eve"), None, "/add alice private").is_err());
    e.execute(Some("bob"), None, "/tell eve hello").unwrap();
    assert!(
        e.execute(Some("bob"), None, "/rooms")
            .unwrap()
            .ends_with("Private conversations:\n@private:bob:eve")
    );
    assert!(
        e.execute(Some("alice"), None, "/rooms")
            .unwrap()
            .ends_with("Private conversations:\nNone.")
    );
}
#[test]
fn user_room_creator_can_delegate_invites_to_an_admin_and_revoke_them() {
    let mut e = fixture();
    for permission in ["w:room.create", "/new"] {
        e.execute(None, None, &format!("/grant bob @global {permission}"))
            .unwrap();
    }
    e.execute(Some("bob"), None, "/new private").unwrap();
    e.execute(Some("bob"), Some("private"), "/grant alice private /add")
        .unwrap();
    let before = e.data.clone();
    let error = e
        .execute(Some("alice"), None, "/add eve private")
        .unwrap_err();
    assert!(error.contains("x:member.add"));
    assert!(e.data == before);
    e.execute(
        Some("bob"),
        Some("private"),
        "/grant alice private x:member.add",
    )
    .unwrap();
    let before = e.data.clone();
    let error = e
        .execute(Some("alice"), None, "/add eve private")
        .unwrap_err();
    assert!(error.contains("every participant permission"));
    assert!(error.contains("Ask the room owner to add you"));
    assert!(e.data == before);
    e.execute(Some("bob"), Some("private"), "/add alice")
        .unwrap();
    e.execute(Some("alice"), Some("private"), "/add eve")
        .unwrap();
    e.execute(Some("eve"), None, "/join private").unwrap();
    e.execute(Some("eve"), Some("private"), "invited participant")
        .unwrap();
    for permission in ["x:member.add", "/add"] {
        e.execute(
            Some("bob"),
            Some("private"),
            &format!("/revoke alice private {permission}"),
        )
        .unwrap();
        let before = e.data.clone();
        assert!(
            e.execute(Some("alice"), Some("private"), "/add mallory")
                .is_err()
        );
        assert!(e.data == before);
        e.execute(
            Some("bob"),
            Some("private"),
            &format!("/grant alice private {permission}"),
        )
        .unwrap();
    }
    e.execute(Some("alice"), Some("private"), "/add mallory")
        .unwrap();
    let scope = e.room_scope("private").unwrap();
    assert!(!e.allows(Some("alice"), &scope, Action::PolicyWrite));
    assert!(!e.allows(Some("alice"), &scope, Action::Kick));
    assert!(!e.allows(Some("alice"), &scope, Action::DeleteRoom));
    assert!(!e.allows(Some("alice"), &scope, Action::Transfer));
    e.execute(Some("mallory"), None, "/new foreign").unwrap();
    let before = e.data.clone();
    assert!(
        e.execute(Some("alice"), Some("private"), "/grant eve private /add")
            .is_err()
    );
    assert!(
        e.execute(Some("alice"), Some("private"), "/add eve foreign")
            .is_err()
    );
    assert!(e.data == before);
}

#[test]
fn invitation_delegation_requires_current_ownership_and_cannot_expand_management() {
    let mut e = fixture();
    for permission in [
        "x:policy.change",
        "/grant",
        "/revoke",
        "x:member.add",
        "/add",
    ] {
        e.execute(None, None, &format!("/grant bob team {permission}"))
            .unwrap();
    }
    for permission in ["x:member.add", "/add"] {
        let before = e.data.clone();
        assert!(
            e.execute(
                Some("bob"),
                Some("team"),
                &format!("/grant eve team {permission}")
            )
            .is_err()
        );
        assert!(e.data == before);
        e.execute(
            Some("alice"),
            Some("team"),
            &format!("/grant eve team {permission}"),
        )
        .unwrap();
    }
    for permission in [
        "x:policy.change",
        "/grant",
        "x:member.remove",
        "/kick",
        "x:room.delete",
        "/delete",
        "x:room.owner.transfer",
        "/owner",
        "x:history.clean",
        "/clean",
        "w:room.create",
        "/new",
    ] {
        let before = e.data.clone();
        assert!(
            e.execute(
                Some("alice"),
                Some("team"),
                &format!("/grant eve team {permission}")
            )
            .is_err()
        );
        assert!(e.data == before);
    }
    e.execute(Some("alice"), Some("team"), "/owner mallory")
        .unwrap();
    // Give the former owner policy management again, so failure specifically
    // verifies the owner-only ceiling rather than a missing policy grant.
    for permission in [
        "x:policy.change",
        "/grant",
        "/revoke",
        "x:member.add",
        "/add",
    ] {
        e.execute(None, None, &format!("/grant alice team {permission}"))
            .unwrap();
    }
    for operation in ["/grant", "/revoke"] {
        for permission in ["x:member.add", "/add"] {
            let before = e.data.clone();
            assert!(
                e.execute(
                    Some("alice"),
                    Some("team"),
                    &format!("{operation} eve team {permission}")
                )
                .is_err()
            );
            assert!(e.data == before);
            e.execute(
                Some("mallory"),
                Some("team"),
                &format!("{operation} eve team {permission}"),
            )
            .unwrap();
        }
    }
}

#[test]
fn room_directory_lists_only_own_private_pairs_even_with_inspection_access() {
    let mut e = fixture();
    e.execute(Some("bob"), None, "/tell eve secret").unwrap();
    e.execute(Some("alice"), None, "/tell bob hello").unwrap();
    e.execute(None, None, "/grant alice @private:bob:eve r:message.read")
        .unwrap();
    for su in [false, true] {
        if su {
            e.execute(None, None, "/grant alice su").unwrap();
        }
        assert!(e.history("alice", "@private:bob:eve").is_ok());
        let before = e.data.clone();
        let output = e.execute(Some("alice"), None, "/rooms").unwrap();
        assert!(output.ends_with("Private conversations:\n@private:alice:bob"));
        assert!(!output.contains("@private:bob:eve"));
        assert!(e.data == before);
    }
    e.execute(None, None, "/revoke bob user").unwrap();
    let output = e.execute(Some("bob"), Some("team"), "/rooms").unwrap_err();
    assert!(output.contains("Command not permitted"));
    e.execute(None, None, "/grant bob @global /rooms").unwrap();
    assert!(
        e.execute(Some("bob"), None, "/rooms")
            .unwrap()
            .ends_with("Private conversations:\nNone.")
    );
}

#[test]
fn console_command_is_readonly_and_uses_the_selected_context_grant() {
    let mut e = fixture();
    e.execute(Some("bob"), None, "/tell eve hello").unwrap();
    let before = e.data.clone();
    for actor in ["alice", "bob", "mallory"] {
        for context in [None, Some("team"), Some("@direct:eve")] {
            // Alice and Mallory have no access to Bob's private pair; invocation
            // there remains subject to the usual context authorization.
            if context == Some("@direct:eve") && actor != "bob" {
                continue;
            }
            assert_eq!(
                e.execute(Some(actor), context, "/console").unwrap(),
                "Command view opened."
            );
        }
    }
    assert!(
        e.execute(Some("bob"), Some("team"), "/console extra")
            .is_err()
    );
    assert!(e.execute(None, None, "/console").is_err());
    assert!(e.data == before);
    e.execute(None, None, "/revoke bob user").unwrap();
    assert!(e.execute(Some("bob"), Some("team"), "/console").is_err());
    e.execute(None, None, "/grant bob team /console").unwrap();
    assert!(e.execute(Some("bob"), Some("team"), "/console").is_ok());
    // The Command view collects available scoped commands; actual operations
    // still require their own target command and action grants.
    assert!(e.execute(Some("bob"), None, "/console").is_ok());
    assert!(e.execute(Some("bob"), None, "/users").is_err());
    e.execute(None, None, "/revoke bob team /console").unwrap();
    assert!(e.execute(Some("bob"), Some("team"), "/console").is_err());
    assert!(e.execute(Some("bob"), None, "/console").is_err());
}

#[test]
fn global_scope_alias_and_command_hints_preserve_separate_permission_gates() {
    let mut e = fixture();
    let action = e
        .execute(None, None, "/grant bob @server w:room.create")
        .unwrap();
    assert!(action.contains("@global"));
    assert!(action.contains("matching command separately"));
    let error = e.execute(Some("bob"), None, "/new private").unwrap_err();
    assert!(error.contains("Missing command grant at @global"));
    let grant = e.execute(None, None, "/grant bob @global /new").unwrap();
    assert!(grant.contains("Requires: w:room.create at @global"));
    assert!(e.execute(Some("bob"), None, "/new private").is_ok());
    let original = e.data.clone();
    let global = e
        .execute(Some("bob"), None, "/permissions @global")
        .unwrap();
    let legacy = e
        .execute(Some("bob"), None, "/permissions @server")
        .unwrap();
    assert_eq!(global, legacy);
    assert!(e.data == original);
    e.execute(None, None, "/revoke bob @server w:room.create")
        .unwrap();
    let error = e.execute(Some("bob"), None, "/new denied").unwrap_err();
    assert!(error.contains("w:room.create at @global"));
    assert!(!e.data.rooms.contains_key("denied"));
    let scoped = e.execute(None, None, "/grant bob team /new").unwrap();
    assert!(scoped.contains("does not authorize that global operation"));
    assert!(
        e.execute(None, None, "/permissions @invalid")
            .unwrap_err()
            .contains("Unknown scope")
    );
}

#[test]
fn permission_output_keeps_assignment_privacy_and_conditional_access_visible() {
    let mut e = fixture();
    e.execute(Some("alice"), Some("team"), "original").unwrap();
    let id = e.data.rooms["team"].messages[0].id.clone();
    e.execute(Some("alice"), Some("team"), "/revoke bob user team")
        .unwrap();
    let reply = e
        .execute(Some("alice"), Some("team"), "/grant bob team /reply")
        .unwrap();
    assert!(reply.contains("r:message.read + w:message.create"));
    let scope = e.room_scope("team").unwrap();
    assert!(!e.allows(Some("bob"), &scope, Action::Read));
    assert!(!e.allows(Some("bob"), &scope, Action::Send));
    e.execute(
        Some("alice"),
        Some("team"),
        "/grant bob team r:message.read",
    )
    .unwrap();
    let error = e
        .execute(Some("bob"), Some("team"), &format!("/reply {id} denied"))
        .unwrap_err();
    assert!(error.contains("w:message.create at team"));
    e.execute(None, None, "/grant eve team w:message.create")
        .unwrap();
    e.execute(None, None, "/grant bob team x:history.clean 7d")
        .unwrap();
    let before = e.data.clone();
    let own = e
        .execute(Some("bob"), Some("team"), "/permissions")
        .unwrap();
    assert!(own.starts_with("# Access at #team\n"));
    assert!(own.contains("## Effective access"));
    assert!(own.contains("### Conditional actions"));
    assert!(own.contains("604800 seconds"));
    assert!(own.contains("Viewing assignments requires"));
    assert!(!own.contains("### eve"));
    let managed = e
        .execute(Some("alice"), Some("team"), "/permissions")
        .unwrap();
    assert!(managed.contains("## Direct grants at #team"));
    assert!(managed.contains("### eve"));
    assert!(managed.contains("## Group assignments at #team"));
    assert!(e.data == before);
}
#[test]
fn readonly_grants_filter_commands_content_members_and_unread_metadata() {
    let mut e = fixture();
    e.execute(Some("bob"), Some("team"), "original").unwrap();
    let id = e.data.rooms["team"].messages[0].id.clone();
    e.execute(Some("alice"), None, "/revoke bob user team")
        .unwrap();
    assert!(e.snapshot("bob").unwrap().rooms.is_empty());
    assert!(!e.snapshot("bob").unwrap().unread.contains_key("team"));
    for permission in ["r:message.read", "/history"] {
        e.execute(
            Some("alice"),
            Some("team"),
            &format!("/grant bob team {permission}"),
        )
        .unwrap();
    }
    let snapshot = e.snapshot("bob").unwrap();
    assert_eq!(snapshot.rooms[0].messages.len(), 1);
    assert!(snapshot.rooms[0].members.is_empty());
    assert!(
        snapshot.rooms[0]
            .commands
            .iter()
            .any(|c| c.name == "/history")
    );
    assert!(
        !snapshot.rooms[0]
            .commands
            .iter()
            .any(|c| c.name == "/reply")
    );
    e.execute(Some("bob"), Some("team"), "/history").unwrap();
    for input in [
        "denied".to_owned(),
        format!("/react {id} 👍"),
        format!("/retract {id}"),
        "/members".into(),
    ] {
        assert!(e.execute(Some("bob"), Some("team"), &input).is_err());
    }
    e.execute(
        Some("alice"),
        Some("team"),
        "/revoke bob team r:message.read",
    )
    .unwrap();
    assert!(e.history("bob", "team").is_err());
    assert!(e.snapshot("bob").unwrap().rooms.is_empty());
}
#[test]
fn permissions_and_command_gates_are_independent_and_target_scoped() {
    let mut e = fixture();
    e.execute(Some("mallory"), None, "/new other").unwrap();
    e.execute(None, None, "/grant bob team x:history.clean 7d")
        .unwrap();
    assert!(e.execute(Some("bob"), Some("team"), "/clean 7d").is_err());
    e.execute(None, None, "/grant bob team /clean").unwrap();
    assert!(e.execute(Some("bob"), Some("team"), "/clean 1d").is_err());
    e.execute(Some("bob"), Some("team"), "/clean 7d").unwrap();
    assert!(
        e.execute(Some("bob"), Some("other"), "/clean 7d team")
            .is_err()
    );
    assert!(
        e.execute(Some("bob"), Some("team"), "/clean 7d other")
            .is_err()
    );
    assert!(
        e.execute(Some("bob"), Some("team"), "/clean 7d @all")
            .is_err()
    );
    for permission in [
        "x:policy.change",
        "x:group.su.assign",
        "w:message.retract.any",
        "/grant",
        "x:account.password.reset",
    ] {
        assert!(
            e.execute(
                Some("alice"),
                Some("team"),
                &format!("/grant bob team {permission}")
            )
            .is_err()
        );
    }
    assert!(
        e.execute(Some("alice"), None, "/grant bob admin team")
            .is_err()
    );
    e.execute(None, None, "/grant bob admin team").unwrap();
    e.execute(Some("bob"), Some("team"), "/add eve").unwrap();
    assert!(e.execute(Some("bob"), Some("other"), "/add eve").is_err());
    assert!(
        e.execute(Some("bob"), None, "/user forbidden hash user")
            .is_err()
    );
}
#[test]
fn su_can_read_and_retract_other_private_messages_without_granting_admin_access() {
    let mut e = fixture();
    e.execute(Some("bob"), None, "/tell eve secret").unwrap();
    let id = e.data.private["bob:eve"].messages[0].id.clone();
    assert!(e.history("alice", "@private:bob:eve").is_err());
    assert!(
        !e.snapshot("alice")
            .unwrap()
            .rooms
            .iter()
            .any(|r| r.name.starts_with("@private:"))
    );
    e.execute(None, None, "/grant alice su").unwrap();
    assert_eq!(
        e.history("alice", "@private:bob:eve").unwrap().messages[0].text,
        "secret"
    );
    let snapshot = e.snapshot("alice").unwrap();
    assert!(snapshot.rooms.iter().any(|r| r.name == "@private:bob:eve"));
    assert!(snapshot.unread.contains_key("@private:bob:eve"));
    e.execute(
        Some("alice"),
        Some("@private:bob:eve"),
        &format!("/react {id} 👀"),
    )
    .unwrap();
    e.execute(
        Some("alice"),
        Some("@private:bob:eve"),
        &format!("/reply {id} moderation"),
    )
    .unwrap();
    assert_eq!(e.data.private["bob:eve"].messages.len(), 2);
    e.execute(
        Some("alice"),
        Some("@private:bob:eve"),
        &format!("/retract {id}"),
    )
    .unwrap();
    assert_eq!(e.data.private["bob:eve"].messages.len(), 1);
    assert!(e.data.private["bob:eve"].messages[0].reply.is_none());
    e.execute(None, None, "/revoke alice su").unwrap();
    assert!(e.history("alice", "@private:bob:eve").is_err());
    assert!(
        !e.snapshot("alice")
            .unwrap()
            .unread
            .contains_key("@private:bob:eve")
    );
}
#[test]
fn private_message_actions_cannot_escape_selected_conversation() {
    let mut e = fixture();
    e.execute(Some("bob"), None, "/tell alice first").unwrap();
    e.execute(Some("bob"), None, "/tell eve second").unwrap();
    let id = e.data.private["bob:eve"].messages[0].id.clone();
    for input in [
        format!("/retract {id}"),
        format!("/react {id} 👍"),
        format!("/reply {id} escaped"),
    ] {
        assert!(
            e.execute(Some("bob"), Some("@direct:alice"), &input)
                .is_err()
        );
    }
    e.execute(Some("bob"), Some("@direct:eve"), &format!("/retract {id}"))
        .unwrap();
}
#[test]
fn owner_transfer_and_deleted_resources_do_not_leave_inherited_grants() {
    let mut e = fixture();
    e.execute(Some("alice"), Some("team"), "/owner bob")
        .unwrap();
    assert_eq!(e.data.rooms["team"].owner_id, e.data.users["bob"].id);
    assert!(
        e.execute(Some("alice"), None, "/rooms")
            .unwrap()
            .contains("#team — owner: bob")
    );
    assert!(e.execute(Some("alice"), None, "/add eve team").is_err());
    assert!(e.execute(Some("bob"), Some("team"), "/leave").is_err());
    e.execute(Some("bob"), None, "/delete team").unwrap();
    e.execute(Some("alice"), None, "/new team").unwrap();
    assert!(e.execute(Some("bob"), None, "/join team").is_err());
    let old_id = e.data.users["bob"].id.clone();
    e.execute(Some("alice"), None, "/deleteuser bob").unwrap();
    e.provision("bob", "hash".into(), false, false).unwrap();
    assert_ne!(e.data.users["bob"].id, old_id);
    assert!(e.snapshot("bob").unwrap().rooms.is_empty());
    e.execute(None, None, "/disable alice").unwrap();
    assert!(
        e.execute(None, None, "/rooms")
            .unwrap()
            .contains("#team — owner: su")
    );
}
#[test]
fn policy_save_failures_preserve_compiled_permissions_and_creation_is_atomic() {
    let mut e = fixture();
    let before = e.data.clone();
    e.db.execute_batch("PRAGMA query_only=ON;").unwrap();
    for input in [
        "/grant bob su",
        "/grant bob @global w:room.create",
        "/owner bob team",
        "/new lost",
    ] {
        assert!(e.execute(None, None, input).is_err());
        assert!(e.data == before);
        assert!(!e.is_su(Some("bob")));
        assert!(!e.allows(Some("bob"), &Scope::Server, Action::CreateRoom));
    }
    assert!(!e.data.rooms.contains_key("lost"));
    e.db.execute_batch("PRAGMA query_only=OFF;").unwrap();
    e.execute(None, None, "/grant bob su").unwrap();
    let revision = e.data.policy.revision;
    e.execute(Some("bob"), Some("team"), "ordinary message")
        .unwrap();
    assert_eq!(e.data.policy.revision, revision);
    assert_eq!(e.authorization.revision, revision);
}
#[test]
#[ignore = "manual authorization microbenchmark"]
fn permission_lookup_benchmark() {
    let mut e = fixture();
    let id = e.data.users["bob"].id.clone();
    for n in 0..1000 {
        e.data.policy.grants.push(authorization::Grant {
            owner: false,
            subject: authorization::Subject::Account(id.clone()),
            scope: Scope::Room(format!("benchmark-{n}")),
            permissions: BTreeSet::from([Action::Read.name().into()]),
            minimum_age: 0,
        });
    }
    e.rebuild_authorization().unwrap();
    let scope = e.room_scope("team").unwrap();
    let start = std::time::Instant::now();
    for _ in 0..200_000 {
        assert!(std::hint::black_box(e.allows(
            Some("bob"),
            &scope,
            Action::Read
        )));
    }
    println!(
        "200000 indexed checks with 1000 additional grants: {:?}",
        start.elapsed()
    );
}

#[test]
fn explicit_private_grants_apply_to_actions_and_ui_controls() {
    let mut e = fixture();
    e.execute(Some("bob"), None, "/tell eve secret").unwrap();
    let id = e.data.private["bob:eve"].messages[0].id.clone();
    for permission in [
        "r:message.read",
        "w:message.react",
        "w:message.create",
        "/react",
        "/reply",
    ] {
        e.execute(
            None,
            None,
            &format!("/grant mallory @private:bob:eve {permission}"),
        )
        .unwrap();
    }
    e.execute(
        Some("mallory"),
        Some("@private:bob:eve"),
        &format!("/react {id} agreed"),
    )
    .unwrap();
    e.execute(
        Some("mallory"),
        Some("@private:bob:eve"),
        &format!("/reply {id} permitted reply"),
    )
    .unwrap();
    let reply = e.data.private["bob:eve"]
        .messages
        .back()
        .unwrap()
        .id
        .clone();
    // The pair stays the same even when the original author is an inspector.
    e.execute(
        Some("bob"),
        Some("@direct:eve"),
        &format!("/react {reply} thanks"),
    )
    .unwrap();
    assert_eq!(e.data.private.len(), 1);
    assert_eq!(
        e.snapshot("bob").unwrap().direct.last().unwrap().private_id,
        e.data.private["bob:eve"].id
    );
    e.execute(None, None, "/grant bob @private:bob:eve r:message.metadata")
        .unwrap();
    let snapshot = e.snapshot("bob").unwrap();
    assert!(
        snapshot.private_access["@direct:eve"]
            .permissions
            .contains(&Action::Metadata.name().into())
    );
    assert!(
        !snapshot
            .private_permissions
            .contains(&Action::Metadata.name().into())
    );
    e.execute(None, None, "/grant bob @private:bob:eve /permissions")
        .unwrap();
    assert!(
        e.execute(Some("bob"), Some("@direct:eve"), "/permissions")
            .unwrap()
            .contains(Action::Metadata.name())
    );
    e.execute(
        None,
        None,
        "/revoke mallory @private:bob:eve r:message.read",
    )
    .unwrap();
    assert!(e.history("mallory", "@private:bob:eve").is_err());
    assert!(
        e.execute(
            Some("mallory"),
            Some("@private:bob:eve"),
            &format!("/react {id} denied")
        )
        .is_err()
    );
}
#[test]
fn account_scoped_grants_do_not_apply_to_other_accounts() {
    let mut e = fixture();
    e.execute(None, None, "/revoke bob user").unwrap();
    e.login("session".into(), "bob", "hash", None).unwrap();
    assert!(
        e.change_password("bob", "hash", "new".into(), "session")
            .is_err()
    );
    e.execute(None, None, "/grant bob @account:bob w:account.password.own")
        .unwrap();
    e.change_password("bob", "hash", "new".into(), "session")
        .unwrap();
    e.execute(
        None,
        None,
        "/grant alice @account:eve x:account.password.reset",
    )
    .unwrap();
    e.execute(None, None, "/grant alice @account:eve /reset")
        .unwrap();
    e.require_command(Some("alice"), None, "/reset").unwrap();
    e.provision_by(Some("alice"), "eve", "new".into(), false, true)
        .unwrap();
    assert!(
        e.provision_by(Some("alice"), "bob", "new".into(), false, true)
            .is_err()
    );
}
#[test]
fn private_read_revocation_removes_payload_contacts_and_unread_state() {
    let mut e = fixture();
    e.execute(Some("bob"), None, "/tell eve secret").unwrap();
    e.execute(None, None, "/revoke bob user").unwrap();
    let snapshot = e.snapshot("bob").unwrap();
    assert!(snapshot.direct.is_empty());
    assert!(snapshot.private_peers.is_empty());
    assert!(snapshot.private_access.is_empty());
    assert!(!snapshot.unread.contains_key("@direct:eve"));
    assert!(e.history("bob", "@direct:eve").is_err());
    for permission in ["r:message.read", "w:message.create", "/tell"] {
        e.execute(
            None,
            None,
            &format!("/grant bob @private:bob:eve {permission}"),
        )
        .unwrap();
    }
    e.execute(
        Some("bob"),
        Some("@direct:eve"),
        "/tell eve allowed in existing pair",
    )
    .unwrap();
    assert!(
        e.execute(Some("bob"), None, "/tell alice no new pair")
            .is_err()
    );
}

#[test]
fn command_view_cannot_borrow_a_command_grant_from_another_target() {
    let mut e = fixture();
    e.execute(Some("alice"), None, "/new other").unwrap();
    e.execute(Some("alice"), Some("other"), "target").unwrap();
    let id = e.data.rooms["other"].messages[0].id.clone();
    e.execute(None, None, "/grant bob other r:message.read")
        .unwrap();
    e.execute(None, None, "/grant bob other w:message.react")
        .unwrap();
    // Bob can invoke /react in team, but has no /react grant in other.
    assert!(
        e.execute(Some("bob"), Some("other"), &format!("/react {id} denied"))
            .is_err()
    );
    assert!(e.execute(Some("bob"), None, "/clean 1s other").is_err());
    e.execute(None, None, "/grant bob other x:history.clean")
        .unwrap();
    e.execute(None, None, "/grant bob team /clean").unwrap();
    assert!(e.execute(Some("bob"), None, "/clean 1s other").is_err());
    e.execute(None, None, "/grant bob other /clean").unwrap();
    e.execute(Some("bob"), None, "/clean 1s other").unwrap();
}

#[test]
fn console_identity_cannot_be_claimed_by_an_account_with_the_same_display_name() {
    let mut e = fixture();
    e.provision("console", "hash".into(), false, false).unwrap();
    e.execute(Some("alice"), None, "/add console team").unwrap();
    e.execute(None, Some("team"), "stdin message").unwrap();
    let message = &e.data.rooms["team"].messages[0];
    assert_eq!(message.author_id, "console");
    assert_ne!(message.author_id, e.data.users["console"].id);
    let id = message.id.clone();
    assert!(
        e.execute(Some("console"), Some("team"), &format!("/retract {id}"))
            .is_err()
    );
}

#[test]
fn combined_grant_syntax_is_unambiguous_and_preserves_additive_rules() {
    let mut e = fixture();
    e.execute(None, None, "/new su").unwrap();
    e.execute(None, None, "/grant bob su r:message.read")
        .unwrap();
    assert!(!e.is_su(Some("bob")));
    assert!(e.history("bob", "su").is_ok());
    e.execute(None, None, "/revoke bob su r:message.read")
        .unwrap();
    assert!(e.history("bob", "su").is_err());
    e.execute(None, None, "/grant bob su").unwrap();
    assert!(e.is_su(Some("bob")));
    e.execute(None, None, "/revoke bob su").unwrap();
    e.execute(
        Some("alice"),
        Some("team"),
        "/revoke bob team w:message.create",
    )
    .unwrap();
    e.execute(Some("bob"), Some("team"), "group still grants writes")
        .unwrap();
    e.execute(Some("alice"), Some("team"), "/revoke bob user team")
        .unwrap();
    assert!(e.execute(Some("bob"), Some("team"), "denied").is_err());
    for input in [
        "/grant bob team unknown",
        "/grant bob team r:message.read 1d",
        "/revoke bob team r:message.read 7d",
        "/grant bob nonexistent r:message.read",
        "/grant missing team r:message.read",
        "/permit bob team r:message.read",
        "/unpermit bob team r:message.read",
    ] {
        let before = e.data.clone();
        assert!(e.execute(None, None, input).is_err(), "{input}");
        assert!(e.data == before, "rejected command changed state: {input}");
    }
}
#[test]
fn manuals_are_public_readonly_and_do_not_authorize_commands() {
    let mut e = fixture();
    let before = e.data.clone();
    let revision = e.revision;
    for topic in [
        "",
        "grant",
        "/grant",
        "revoke",
        "permissions",
        "groups",
        "scopes",
        "ownership",
        "man",
        "history",
        "reset",
    ] {
        let manual = e
            .execute(Some("eve"), None, &format!("/man {topic}"))
            .unwrap();
        assert!(!manual.is_empty());
        assert!(e.data == before);
        assert_eq!(e.revision, revision);
    }
    for action in Action::ALL {
        assert!(
            e.execute(Some("eve"), None, "/man permissions")
                .unwrap()
                .contains(action.name())
        );
    }
    assert!(e.execute(Some("eve"), None, "/grant eve su").is_err());
    assert!(
        e.execute(Some("eve"), None, "/man unknown")
            .unwrap_err()
            .contains("No manual")
    );
    assert!(e.execute(Some("eve"), None, "/man grant extra").is_err());
    assert!(
        e.execute(None, None, "/man grant")
            .unwrap()
            .contains("Only an existing su")
    );
}
#[test]
fn invitation_and_user_group_assignment_cannot_amplify_partial_authority() {
    let mut e = fixture();
    e.execute(Some("alice"), Some("team"), "/revoke bob user team")
        .unwrap();
    for permission in [
        "r:message.read",
        "x:member.add",
        "x:policy.change",
        "/add",
        "/grant",
    ] {
        e.execute(None, None, &format!("/grant bob team {permission}"))
            .unwrap();
    }
    let before = e.data.clone();
    for command in [
        "/add bob",
        "/add eve",
        "/grant bob user team",
        "/grant eve user team",
        "/grant bob team w:message.create",
    ] {
        assert!(
            e.execute(Some("bob"), Some("team"), command).is_err(),
            "{command}"
        );
        assert!(e.data == before);
    }
    e.execute(Some("alice"), Some("team"), "/grant eve user team")
        .unwrap();
    e.execute(Some("eve"), Some("team"), "authorized participant")
        .unwrap();
}
#[test]
fn every_action_has_explicit_defaults_for_each_group_and_resource_boundary() {
    let mut e = fixture();
    e.execute(Some("bob"), None, "/tell eve secret").unwrap();
    e.execute(None, None, "/grant eve su").unwrap();
    let user = BTreeSet::from([
        "r:account.list",
        "w:account.password.own",
        "w:account.rename.own",
        "w:private.create",
    ]);
    let admin = BTreeSet::from([
        "r:account.list",
        "w:account.password.own",
        "w:account.rename.own",
        "w:private.create",
        "w:room.create",
        "x:account.create",
        "x:account.disable",
        "x:account.enable",
        "x:account.delete",
        "x:group.admin.assign",
        "r:server.config",
        "r:policy.read",
    ]);
    let member = BTreeSet::from([
        "r:room.discover",
        "r:message.read",
        "r:member.list",
        "w:message.create",
        "w:message.react",
        "w:message.retract.own",
        "x:room.join",
    ]);
    let manager = BTreeSet::from([
        "r:room.discover",
        "r:message.metadata",
        "r:policy.read",
        "x:policy.change",
        "x:member.add",
        "x:member.remove",
        "x:room.join",
        "x:room.delete",
        "x:history.clean",
        "x:room.owner.transfer",
    ]);
    let room = e.room_scope("team").unwrap();
    let private = e.private_scope("bob:eve");
    let account = Scope::Account(e.data.users["bob"].id.clone());
    let unknown = Scope::Room("unavailable".into());
    for actor in [
        Some("alice"),
        Some("bob"),
        Some("mallory"),
        Some("eve"),
        None,
        Some("missing"),
    ] {
        for scope in [
            Scope::Server,
            room.clone(),
            private.clone(),
            account.clone(),
            unknown.clone(),
        ] {
            let mut expected: BTreeSet<&str> = BTreeSet::new();
            match actor {
                Some("alice" | "mallory") => expected.extend(admin.iter().copied()),
                Some("bob") => expected.extend(user.iter().copied()),
                _ => (),
            }
            if scope == room {
                if actor == Some("alice") {
                    expected.extend(member.iter().copied());
                    expected.extend(manager.iter().copied());
                }
                if actor == Some("bob") {
                    expected.extend(member.iter().copied());
                }
            }
            if scope == private && actor == Some("bob") {
                expected.extend(member.iter().copied());
            }
            for action in Action::ALL {
                let allowed =
                    actor.is_none() || actor == Some("eve") || expected.contains(&action.name());
                assert_eq!(
                    e.allows(actor, &scope, *action),
                    allowed,
                    "actor={actor:?}, scope={scope:?}, action={}",
                    action.name()
                );
            }
        }
    }
}
#[test]
fn disabled_superusers_have_no_action_or_command_authority() {
    let mut e = fixture();
    e.execute(None, None, "/grant bob su").unwrap();
    e.execute(None, None, "/disable bob").unwrap();
    for action in Action::ALL {
        assert!(!e.allows(Some("bob"), &Scope::Server, *action));
    }
    for command in crate::commands::available(true, false) {
        assert!(e.require_command(Some("bob"), None, command.name).is_err());
    }
    assert!(e.snapshot("bob").is_none());
    assert!(e.execute(Some("bob"), None, "/grant alice su").is_err());
}
#[test]
fn explicit_reset_authority_cannot_manage_a_superuser_account() {
    let mut e = fixture();
    e.execute(None, None, "/grant bob su").unwrap();
    e.execute(None, None, "/grant alice @global x:account.password.reset")
        .unwrap();
    let before = e.data.clone();
    assert!(
        e.provision_by(Some("alice"), "bob", "new".into(), false, true)
            .unwrap_err()
            .contains("Only su")
    );
    assert!(
        e.execute(Some("alice"), None, "/grant bob admin")
            .unwrap_err()
            .contains("Only su")
    );
    assert!(e.data == before);
}

#[test]
fn disabled_superusers_remain_protected_from_account_management() {
    let mut e = fixture();
    e.execute(None, None, "/grant bob su").unwrap();
    for permission in ["x:account.password.reset", "/reset"] {
        e.execute(None, None, &format!("/grant alice @global {permission}"))
            .unwrap();
    }
    e.execute(None, None, "/disable bob").unwrap();
    let before = e.data.clone();
    for command in ["/enable bob", "/disable bob", "/deleteuser bob"] {
        let error = e.execute(Some("alice"), None, command).unwrap_err();
        assert!(error.contains("Only su"), "{command}: {error}");
        assert!(e.data == before);
    }
    let error = e
        .provision_by(Some("alice"), "bob", "takeover".into(), false, true)
        .unwrap_err();
    assert!(error.contains("Only su"));
    assert!(e.data == before);

    // Disabled accounts retain their assignments, but cannot act as su.
    assert!(!e.is_su(Some("bob")));
    // Stdin remains able to recover or delete a disabled superuser.
    e.provision("bob", "recovered".into(), false, true).unwrap();
    e.execute(None, None, "/enable bob").unwrap();
    assert!(e.is_su(Some("bob")));
    assert_eq!(e.data.users["bob"].hash, "recovered");
    e.execute(None, None, "/disable bob").unwrap();
    e.execute(None, None, "/deleteuser bob").unwrap();
    assert!(!e.data.users.contains_key("bob"));

    // Ordinary disabled accounts remain manageable by an authorized admin.
    e.execute(None, None, "/disable eve").unwrap();
    e.provision_by(Some("alice"), "eve", "reset".into(), false, true)
        .unwrap();
    e.execute(Some("alice"), None, "/enable eve").unwrap();
    assert!(e.active("eve"));
}

#[test]
fn every_registered_command_and_content_endpoint_denies_accounts_without_grants() {
    let mut e = fixture();
    e.execute(Some("bob"), Some("team"), "owned message")
        .unwrap();
    e.execute(Some("bob"), None, "/tell eve private").unwrap();
    let id = e.data.rooms["team"].messages[0].id.clone();
    let sequence = e.data.rooms["team"].messages[0].sequence;
    e.execute(None, None, "/revoke bob user team").unwrap();
    e.execute(None, None, "/revoke bob user").unwrap();
    let probes = [
        "/sudo /whoami".into(),
        "/help".into(),
        "/console".into(),
        "/man grant".into(),
        "/permissions team".into(),
        "/debug on".into(),
        "/whoami".into(),
        "/rename renamed".into(),
        "/passwd old new".into(),
        "/users".into(),
        "/user created password user".into(),
        "/reset eve password".into(),
        "/enable eve".into(),
        "/disable eve".into(),
        "/deleteuser eve".into(),
        "/grant bob su".into(),
        "/revoke eve su".into(),
        "/rooms".into(),
        "/members team".into(),
        "/join team".into(),
        "/leave team".into(),
        "/new created".into(),
        "/add eve team".into(),
        "/kick alice team".into(),
        "/delete team".into(),
        "/tell eve denied".into(),
        format!("/retract {id}"),
        format!("/react {id} reaction"),
        format!("/reply {id} reply"),
        "/history".into(),
        "/configs".into(),
        "/clean 7d team".into(),
        "/owner bob team".into(),
        "/clear".into(),
        "/logout".into(),
        // Agent commands: a plain user must be denied every one of them, and
        // an owner must still be confined to the agents they own.
        "/agent helper sk-denied".into(),
        "/agent-key sk-denied".into(),
        "/agent-reply auto".into(),
        "/agent-name renamed".into(),
        "/agent-prompt denied personality".into(),
        "/agent-provider openrouter".into(),
        "/agent-base-url https://gateway.example.com/v1".into(),
        "/agent-model some/model".into(),
        "/agent-search on".into(),
        "/agent-search-key tvly-dev-denied".into(),
        "/agent-sources never".into(),
        "/agent-config".into(),
        "/agent-remove".into(),
    ];
    let catalog: BTreeSet<_> = crate::commands::available(true, false)
        .iter()
        .map(|c| c.name.to_owned())
        .collect();
    let tested: BTreeSet<_> = probes
        .iter()
        .map(|text: &String| text.split_whitespace().next().unwrap().to_owned())
        .collect();
    assert_eq!(
        tested, catalog,
        "Every new command needs an explicit denial probe."
    );
    let before = e.data.clone();
    let cursors = e.read_positions.clone();
    for context in [None, Some("team"), Some("@direct:eve")] {
        for text in &probes {
            assert!(
                e.execute(Some("bob"), context, text).is_err(),
                "context={context:?}, text={text}"
            );
            assert!(e.data == before);
        }
    }
    for view in ["team", "@direct:eve", "@private:bob:eve"] {
        assert!(e.history("bob", view).is_err());
        assert!(e.mark_read("bob", view, sequence).is_err());
    }
    assert_eq!(e.read_positions, cursors);
    let snapshot = e.snapshot("bob").unwrap();
    assert!(snapshot.rooms.is_empty() && snapshot.direct.is_empty() && snapshot.unread.is_empty());
    assert!(snapshot.commands.is_empty());
}
#[test]
fn unicode_names_and_rename_preserve_identity_history_sessions_and_cursors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    let id;
    let private_id;
    let sequence;
    {
        let mut e = Engine::open(&path).unwrap();
        e.provision("管理员", "hash".into(), true, false).unwrap();
        e.provision("用户🙂", "hash".into(), false, false).unwrap();
        e.execute(Some("管理员"), None, "/new 测试").unwrap();
        e.execute(Some("管理员"), None, "/add 用户🙂 测试").unwrap();
        e.execute(Some("用户🙂"), Some("测试"), "hello @管理员")
            .unwrap();
        let message_id = e.data.rooms["测试"].messages[0].id.clone();
        e.execute(
            Some("管理员"),
            Some("测试"),
            &format!("/reply {message_id} @用户🙂"),
        )
        .unwrap();
        e.execute(
            Some("用户🙂"),
            Some("测试"),
            &format!("/react {message_id} 👍"),
        )
        .unwrap();
        e.execute(Some("管理员"), None, "/tell 用户🙂 hello")
            .unwrap();
        sequence = e.data.private["用户🙂:管理员"].messages[0].sequence;
        private_id = e.data.private["用户🙂:管理员"].id.clone();
        e.mark_read("用户🙂", "@direct:管理员", sequence).unwrap();
        e.login("session".into(), "用户🙂", "hash", None).unwrap();
        id = e.data.users["用户🙂"].id.clone();
        e.execute(Some("用户🙂"), None, "/rename 新名字🚀").unwrap();
        assert_eq!(e.data.users["新名字🚀"].id, id);
        assert_eq!(e.session("session").as_deref(), Some("新名字🚀"));
        assert!(e.data.rooms["测试"].members.contains("新名字🚀"));
        assert_eq!(e.data.rooms["测试"].messages[0].from, "新名字🚀");
        assert_eq!(
            e.data.rooms["测试"].messages[1]
                .reply
                .as_ref()
                .unwrap()
                .from,
            "新名字🚀"
        );
        assert!(e.data.rooms["测试"].messages[0].reactions["👍"].contains("新名字🚀"));
        assert!(
            e.data.rooms["测试"].messages[1]
                .mentions
                .contains("新名字🚀")
        );
        assert_eq!(e.data.private["新名字🚀:管理员"].id, private_id);
        assert_eq!(
            e.data.private["新名字🚀:管理员"].messages[0].to.as_deref(),
            Some("新名字🚀")
        );
        assert_eq!(e.read_positions["新名字🚀"]["dm:新名字🚀:管理员"], sequence);
        e.execute(Some("新名字🚀"), None, "/tell 管理员 renamed")
            .unwrap();
        e.provision("用户🙂", "replacement".into(), false, false)
            .unwrap();
        assert!(e.snapshot("用户🙂").unwrap().direct.is_empty());
    }
    let e = Engine::open(&path).unwrap();
    assert_eq!(e.data.users["新名字🚀"].id, id);
    assert_eq!(e.session("session").as_deref(), Some("新名字🚀"));
    assert_eq!(e.read_positions["新名字🚀"]["dm:新名字🚀:管理员"], sequence);
    assert_eq!(e.data.private["新名字🚀:管理员"].id, private_id);
}

#[test]
fn rename_requires_own_or_any_action_and_target_command_and_protects_su() {
    let mut e = fixture();
    assert!(e.execute(Some("bob"), None, "/rename eve stolen").is_err());
    assert!(
        e.execute(Some("alice"), None, "/rename bob stolen")
            .is_err()
    );
    for name in ["alice", "bad:name", "@direct", "/bad", "", "x.y"] {
        assert!(e.rename_user(Some("bob"), "bob", name).is_err());
    }
    assert!(valid_name(&"界".repeat(32)));
    assert!(!valid_name(&"界".repeat(33)));
    assert!(!valid_name("bad\u{0085}name"));
    assert!(!valid_name("bad\u{0000}name"));
    e.execute(None, None, "/grant alice @account:bob x:account.rename.any")
        .unwrap();
    e.execute(Some("alice"), None, "/rename bob 重命名")
        .unwrap();
    e.execute(None, None, "/grant eve su").unwrap();
    e.execute(None, None, "/grant alice @account:eve x:account.rename.any")
        .unwrap();
    assert!(
        e.execute(Some("alice"), None, "/rename eve protected")
            .is_err()
    );
    e.execute(None, None, "/rename eve 超级用户").unwrap();
    e.execute(None, None, "/revoke 重命名 user").unwrap();
    e.execute(
        None,
        None,
        "/grant 重命名 @account:重命名 w:account.rename.own",
    )
    .unwrap();
    assert!(e.execute(Some("重命名"), None, "/rename denied").is_err());
    e.execute(None, None, "/grant 重命名 @account:重命名 /rename")
        .unwrap();
    let access = e.snapshot("重命名").unwrap().account_access;
    assert!(access.permissions.contains(&"w:account.rename.own".into()));
    assert!(
        access
            .commands
            .iter()
            .any(|command| command.name == "/rename")
    );
    assert!(
        !access
            .commands
            .iter()
            .any(|command| command.name == "/passwd")
    );
    e.execute(Some("重命名"), None, "/rename allowed").unwrap();
}

#[test]
fn rename_storage_failure_rolls_back_account_and_read_positions() {
    let mut e = fixture();
    e.execute(Some("eve"), None, "/tell bob incoming").unwrap();
    let sequence = e.data.private["bob:eve"].messages[0].sequence;
    e.mark_read("bob", "@direct:eve", sequence).unwrap();
    let before = e.data.clone();
    let positions = e.read_positions.clone();
    e.db.execute_batch("CREATE TRIGGER fail_rename BEFORE UPDATE ON state BEGIN SELECT RAISE(FAIL, 'injected'); END;").unwrap();
    assert!(e.execute(Some("bob"), None, "/rename 改名").is_err());
    assert!(e.data == before);
    assert_eq!(e.read_positions, positions);
    e.db.execute_batch("DROP TRIGGER fail_rename;").unwrap();
    e.execute(Some("bob"), None, "/rename 改名").unwrap();
}
