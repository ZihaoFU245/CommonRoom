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
    e.execute(Some("alice"), None, "/new room team").unwrap();
    e.execute(Some("alice"), None, "/add bob #team").unwrap();
    e
}

#[test]
fn policy_inspection_requires_resource_authority_and_hides_scope_existence() {
    let mut e = fixture();
    e.execute(Some("bob"), None, "/tell eve secret").unwrap();
    e.execute(Some("mallory"), None, "/new room hidden")
        .unwrap();
    for actor in ["alice", "bob"] {
        for (existing, missing) in [
            ("@private:bob:eve", "@private:alice:mallory"),
            ("@account:eve", "@account:missing"),
            ("#hidden", "#missing"),
        ] {
            // Bob can inspect his own effective private access, but not its assignments.
            if actor == "bob" && existing.starts_with("@private:") {
                let output = e
                    .execute(Some(actor), None, &format!("/permissions {existing}"))
                    .unwrap();
                assert!(!output.contains("## Direct grants"));
            } else {
                let error = e
                    .execute(Some(actor), None, &format!("/permissions {existing}"))
                    .unwrap_err();
                assert_eq!(
                    error,
                    e.execute(Some(actor), None, &format!("/permissions {missing}"))
                        .unwrap_err()
                );
            }
            for command in ["/grant", "/revoke"] {
                let error = e
                    .execute(
                        Some(actor),
                        Some("team"),
                        &format!("{command} eve {existing} r:message.read"),
                    )
                    .unwrap_err();
                assert_eq!(
                    error,
                    e.execute(
                        Some(actor),
                        Some("team"),
                        &format!("{command} eve {missing} r:message.read")
                    )
                    .unwrap_err()
                );
            }
        }
    }
    // Global admin access may show effective rights in a readable room, but
    // only local policy readers can inspect other accounts' assignments.
    let output = e
        .execute(Some("bob"), Some("team"), "/permissions")
        .unwrap();
    assert!(!output.contains("## Direct grants"));
    assert!(
        e.execute(Some("alice"), Some("team"), "/permissions")
            .unwrap()
            .contains("## Direct grants")
    );
    e.execute(None, None, "/grant mallory user #team").unwrap();
    assert!(
        !e.execute(Some("mallory"), None, "/permissions #team")
            .unwrap()
            .contains("## Direct grants")
    );
    for scope in ["@private:bob:eve", "@account:eve", "#hidden"] {
        e.execute(None, None, &format!("/grant alice {scope} r:policy.read"))
            .unwrap();
        assert!(
            e.execute(Some("alice"), None, &format!("/permissions {scope}"))
                .unwrap()
                .contains("## Direct grants")
        );
        assert!(
            e.execute(None, None, &format!("/permissions {scope}"))
                .unwrap()
                .contains("## Direct grants")
        );
    }
}

#[test]
fn room_owner_participant_group_requires_transfer_or_su_before_revocation() {
    let mut e = fixture();
    for permission in ["x:policy.change", "/revoke", "/rm"] {
        e.execute(None, None, &format!("/grant bob #team {permission}"))
            .unwrap();
    }
    for actor in [Some("bob"), Some("alice")] {
        for command in ["/revoke", "/rm group"] {
            let before = e.data.clone();
            let revision = e.revision;
            let error = e
                .execute(actor, Some("team"), &format!("{command} alice user #team"))
                .unwrap_err();
            assert!(
                error.contains("Transfer room ownership"),
                "{actor:?} {command}: {error}"
            );
            assert!(e.data == before);
            assert_eq!(e.revision, revision);
            assert!(e.history("alice", "team").is_ok());
            e.execute(Some("alice"), Some("team"), "owner still has access")
                .unwrap();
        }
    }
    e.execute(Some("alice"), Some("team"), "/owner bob")
        .unwrap();
    e.execute(Some("bob"), Some("team"), "/revoke alice user #team")
        .unwrap();
    assert!(e.history("alice", "team").is_err());
    assert!(e.history("bob", "team").is_ok());
}

#[test]
fn su_and_sudo_can_revoke_owner_participant_access_and_delete_owned_rooms() {
    for command in ["/revoke", "/rm group"] {
        for (actor, prefix) in [(None, ""), (Some("mallory"), ""), (Some("bob"), "/sudo ")] {
            let mut e = fixture();
            e.execute(None, None, "/grant mallory su").unwrap();
            for permission in ["/sudo", "x:command.sudo"] {
                e.execute(None, None, &format!("/grant bob @global {permission}"))
                    .unwrap();
            }
            let owner_id = e.data.users["alice"].id.clone();
            e.execute(actor, None, &format!("{prefix}{command} alice user #team"))
                .unwrap();
            assert_eq!(e.data.rooms["team"].owner_id, owner_id);
            assert!(e.history("alice", "team").is_err());
            assert!(!e.is_su(Some("bob")));
            e.execute(actor, None, &format!("{prefix}/rm room team"))
                .unwrap();
            assert!(!e.data.rooms.contains_key("team"));
        }
    }
}

#[test]
fn sudo_authority_is_cleared_even_when_policy_rebuilding_fails() {
    let mut e = fixture();
    for permission in ["/sudo", "x:command.sudo"] {
        e.execute(None, None, &format!("/grant bob @global {permission}"))
            .unwrap();
    }
    let before = e.data.clone();
    let result = e.with_command_authority(Some("bob"), None, true, |engine| {
        engine.data.policy.grants.push(authorization::Grant {
            owner: false,
            subject: authorization::Subject::Account(engine.data.users["bob"].id.clone()),
            scope: Scope::Server,
            permissions: BTreeSet::from(["x:unknown.permission".into()]),
        });
        Ok(())
    });
    assert!(result.is_err());
    assert!(!e.is_su(Some("bob")));
    for action in Action::ALL {
        assert!(!e.allows(Some("bob"), &Scope::Server, *action));
    }
    e.data = before;
    e.rebuild_authorization().unwrap();
    assert!(!e.is_su(Some("bob")));
    assert!(e.allows(Some("bob"), &Scope::Server, Action::Sudo));
}

#[test]
fn sudo_requires_both_global_grants_and_never_persists_elevation() {
    let mut e = fixture();
    assert!(
        e.execute(Some("bob"), None, "/sudo /new room elevated")
            .is_err()
    );
    e.execute(None, None, "/grant bob @global /sudo").unwrap();
    assert!(
        e.execute(Some("bob"), None, "/sudo /new room elevated")
            .is_err()
    );
    e.execute(None, None, "/revoke bob @global /sudo").unwrap();
    e.execute(None, None, "/grant bob @global x:command.sudo")
        .unwrap();
    assert!(
        e.execute(Some("bob"), None, "/sudo /new room elevated")
            .is_err()
    );
    e.execute(None, None, "/grant bob #team /sudo").unwrap();
    // A room command grant cannot authorize global elevation.
    assert!(
        e.execute(Some("bob"), Some("team"), "/sudo /new room elevated")
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
    e.execute(Some("bob"), Some("team"), "/sudo /new room elevated")
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
        super::tests::private_messages(&e, "bob:eve")
            .back()
            .unwrap()
            .from,
        "bob"
    );
    assert!(!e.is_su(Some("bob")));
    assert!(e.execute(Some("bob"), None, "/new room denied").is_err());
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
    e.execute(None, None, "/grant bob #team x:command.sudo")
        .unwrap();
    assert!(
        e.execute(Some("bob"), Some("team"), "/sudo /whoami")
            .is_err()
    );
    e.execute(None, None, "/disable bob").unwrap();
    assert!(e.execute(Some("bob"), None, "/sudo /whoami").is_err());
}

#[test]
fn sudo_covers_every_engine_command_without_borrowing_another_identity() {
    let probes = [
        "/help",
        "/man sudo",
        "/permissions @audit",
        "/whoami",
        "/console",
        "/debug on",
        "/user ls",
        "/room ls",
        "/permissions @groups",
        "/user ls #team",
        "/room new created",
        "/room add eve #team",
        "/room kick eve #team",
        "/room join #team",
        "/room leave #team",
        "/room owner eve #team",
        "/grant eve su",
        "/revoke eve su",
        "/revoke eve user #team",
        "/room delete #team",
        "/user delete eve",
        "/user enable eve",
        "/user disable eve",
        "/user rename renamed",
        "/user rename eve renamed",
        "/room rename #team renamed",
        "/tell eve sudo message",
        "/react MESSAGE reaction",
        "/reply MESSAGE sudo reply",
        "/edit MESSAGE sudo edit",
        "/retract MESSAGE",
        "/history",
        "/clean 1s @all",
        "/bot new created #team",
        "/bot ls #team",
        "/bot info helper #team",
        "/bot rm helper @global",
        "/bot token new helper test",
        "/bot token ls helper",
        "/bot token rm helper TOKEN",
    ];
    let tested: BTreeSet<_> = probes
        .iter()
        .map(|text| text.split_whitespace().next().unwrap())
        .collect();
    let engine_commands: BTreeSet<_> = crate::commands::available(true, false)
        .iter()
        .map(|command| command.name)
        .filter(|name| {
            ![
                "/sudo", "/configs", "/passwd", "/reset", "/clear", "/logout",
            ]
            .contains(name)
        })
        .collect();
    assert_eq!(
        tested, engine_commands,
        "Every engine command needs a sudo probe."
    );
    for probe in probes {
        let mut e = fixture();
        e.execute(Some("alice"), Some("team"), "original").unwrap();
        let original = super::tests::room_messages(&e, "team")[0].clone();
        e.execute(None, None, "/room add eve #team").unwrap();
        e.execute(None, None, "/bot new helper #team").unwrap();
        e.execute(None, None, "/bot token new helper fixture")
            .unwrap();
        let token = e.data.bots[&e.data.users["helper"].id]
            .tokens
            .keys()
            .next()
            .unwrap()
            .clone();
        e.execute(None, None, "/grant eve su").unwrap();
        e.execute(None, None, "/revoke bob user #team").unwrap();
        e.execute(None, None, "/revoke bob user").unwrap();
        for permission in ["/sudo", "x:command.sudo"] {
            e.execute(None, None, &format!("/grant bob @global {permission}"))
                .unwrap();
        }
        let caller_id = e.data.users["bob"].id.clone();
        let audit_len = e.data.policy.audit.len();
        let command = probe
            .replace("MESSAGE", &original.id)
            .replace("TOKEN", &token);
        assert!(
            e.execute(Some("bob"), Some("team"), &command).is_err(),
            "ordinary {command}"
        );
        let result = e.execute(Some("bob"), Some("team"), &format!("/sudo {command}"));
        assert!(result.is_ok(), "{command}: {result:?}");
        let reply = result.unwrap();
        let caller = if probe == "/user rename renamed" {
            "renamed"
        } else {
            "bob"
        };
        assert_eq!(e.data.users[caller].id, caller_id, "{command}");
        assert!(!e.is_su(Some(caller)), "{command}");
        if e.data.policy.audit.len() > audit_len {
            assert_eq!(
                e.data.policy.audit.back().unwrap().actor_id,
                caller_id,
                "{command}"
            );
        }
        match probe {
            "/whoami" => assert_eq!(reply, "Name: bob\nPermission: su"),
            "/room new created" => assert_eq!(e.data.rooms["created"].owner_id, caller_id),
            "/react MESSAGE reaction" => assert!(
                super::tests::room_messages(&e, "team")[0].reactions["reaction"].contains("bob")
            ),
            "/reply MESSAGE sudo reply" => {
                let messages = super::tests::room_messages(&e, "team");
                assert_eq!(messages.back().unwrap().from, "bob");
                assert_eq!(messages.back().unwrap().author_id, caller_id);
            }
            "/tell eve sudo message" => {
                let messages = super::tests::private_messages(&e, "bob:eve");
                assert_eq!(messages.back().unwrap().from, "bob");
                assert_eq!(messages.back().unwrap().author_id, caller_id);
            }
            "/edit MESSAGE sudo edit" => assert_eq!(
                super::tests::room_messages(&e, "team")[0].author_id,
                original.author_id
            ),
            _ => {}
        }
    }
}

#[test]
fn sudo_join_persists_the_callers_membership_without_persisting_elevation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    let mut e = Engine::open(&path).unwrap();
    for (name, admin) in [("alice", true), ("bob", false)] {
        e.provision(name, "hash".into(), admin, false).unwrap();
    }
    e.execute(Some("alice"), None, "/new room team").unwrap();
    for permission in ["/sudo", "x:command.sudo"] {
        e.execute(None, None, &format!("/grant bob @global {permission}"))
            .unwrap();
    }
    assert!(e.execute(Some("bob"), None, "/join #team").is_err());
    assert_eq!(
        e.execute(Some("bob"), None, "/sudo /join #team").unwrap(),
        "Joined #team."
    );
    assert!(e.data.rooms["team"].members.contains("bob"));
    assert!(!e.is_su(Some("bob")));
    let scope = e.room_scope("team").unwrap();
    assert!(e.allows(Some("bob"), &scope, Action::Send));
    assert!(!e.allows(Some("bob"), &scope, Action::Invite));
    assert_eq!(e.data.rooms["team"].owner_id, e.data.users["alice"].id);
    let before = e.data.clone();
    assert_eq!(
        e.execute(Some("bob"), None, "/sudo /join #team").unwrap(),
        e.execute(Some("bob"), None, "/join #team").unwrap()
    );
    assert!(e.data == before);
    drop(e);
    let mut e = Engine::open(&path).unwrap();
    assert!(e.data.rooms["team"].members.contains("bob"));
    e.execute(None, None, "/revoke bob @global x:command.sudo")
        .unwrap();
    e.execute(Some("bob"), Some("team"), "joined participant")
        .unwrap();
    e.execute(Some("bob"), None, "/leave #team").unwrap();
    assert!(e.execute(Some("bob"), None, "/join #team").is_err());
}

#[test]
fn joining_as_su_saves_membership_but_ordinary_readers_do_not_gain_grants() {
    let mut e = fixture();
    for permission in ["/join", "x:room.join", "r:message.read"] {
        e.execute(None, None, &format!("/grant eve #team {permission}"))
            .unwrap();
    }
    let before = e.data.clone();
    e.execute(Some("eve"), None, "/join #team").unwrap();
    assert!(e.data == before);
    assert!(!e.allows(Some("eve"), &e.room_scope("team").unwrap(), Action::Send));
    e.execute(None, None, "/grant eve su").unwrap();
    e.execute(Some("eve"), None, "/join #team").unwrap();
    e.execute(None, None, "/revoke eve su").unwrap();
    assert!(e.data.rooms["team"].members.contains("eve"));
    e.execute(Some("eve"), Some("team"), "participant after su revocation")
        .unwrap();
}

#[test]
fn sudo_join_storage_failure_restores_membership_and_authority() {
    let mut e = fixture();
    for permission in ["/sudo", "x:command.sudo"] {
        e.execute(None, None, &format!("/grant eve @global {permission}"))
            .unwrap();
    }
    let before = e.data.clone();
    let revision = e.revision;
    e.store.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_join BEFORE UPDATE ON state BEGIN SELECT RAISE(ABORT,'forced failure'); END;").unwrap();
    assert!(
        e.execute(Some("eve"), None, "/sudo /join #team")
            .unwrap_err()
            .contains("Storage unavailable")
    );
    assert!(e.data == before);
    assert_eq!(e.revision, revision);
    assert!(!e.is_su(Some("eve")));
    assert!(e.history("eve", "team").is_err());
}

#[test]
fn sudo_restores_authority_after_storage_failure_and_preserves_policy_updates() {
    let mut e = fixture();
    for permission in ["/sudo", "x:command.sudo"] {
        e.execute(None, None, &format!("/grant bob @global {permission}"))
            .unwrap();
    }
    let before = e.data.clone();
    e.store.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_sudo BEFORE UPDATE ON state BEGIN SELECT RAISE(ABORT,'forced failure'); END;").unwrap();
    assert!(
        e.execute(Some("bob"), None, "/sudo /new room rejected")
            .is_err()
    );
    assert!(e.data == before);
    assert!(!e.is_su(Some("bob")));
    e.store
        .db
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_sudo;")
        .unwrap();
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
    for command in ["/revoke bob su", "/disable bob", "/rm user bob"] {
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
        e.execute(Some("eve"), None, "/ls room").unwrap(),
        "Your rooms:\nNone.\n\nPrivate conversations:\nNone."
    );
    assert!(e.execute(Some("bob"), None, "/new room private").is_err());
    e.execute(None, None, "/grant bob @global w:room.create")
        .unwrap();
    assert!(e.execute(Some("bob"), None, "/new room private").is_err());
    e.execute(None, None, "/grant bob @global /new").unwrap();
    e.execute(Some("bob"), None, "/new room private").unwrap();
    assert_eq!(e.data.rooms["private"].owner_id, e.data.users["bob"].id);
    assert_eq!(
        e.execute(Some("bob"), None, "/ls room").unwrap(),
        "Your rooms:\n#private — owner: bob\n#team — owner: alice\n\nPrivate conversations:\nNone."
    );
    let scope = e.room_scope("private").unwrap();
    assert!(e.allows(Some("bob"), &scope, Action::Invite));
    for actor in ["alice", "mallory", "eve"] {
        assert!(e.execute(Some(actor), None, "/join #private").is_err());
        assert!(e.execute(Some(actor), None, "/rm room private").is_err());
        assert!(e.execute(Some(actor), None, "/add eve #private").is_err());
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
            !e.execute(Some(actor), None, "/ls room")
                .unwrap()
                .contains("#private")
        );
    }
    e.execute(Some("bob"), None, "/add eve #private").unwrap();
    e.execute(Some("eve"), None, "/join #private").unwrap();
    e.execute(Some("eve"), Some("private"), "allowed").unwrap();
    assert!(e.execute(Some("eve"), None, "/add alice #private").is_err());
    e.execute(Some("bob"), None, "/tell eve hello").unwrap();
    assert!(
        e.execute(Some("bob"), None, "/ls room")
            .unwrap()
            .ends_with("Private conversations:\n@private:bob:eve")
    );
    assert!(
        e.execute(Some("alice"), None, "/ls room")
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
    e.execute(Some("bob"), None, "/new room private").unwrap();
    e.execute(Some("bob"), Some("private"), "/grant alice #private /add")
        .unwrap();
    let before = e.data.clone();
    let error = e
        .execute(Some("alice"), None, "/add eve #private")
        .unwrap_err();
    assert!(error.contains("x:member.add"));
    assert!(e.data == before);
    e.execute(
        Some("bob"),
        Some("private"),
        "/grant alice #private x:member.add",
    )
    .unwrap();
    let before = e.data.clone();
    let error = e
        .execute(Some("alice"), None, "/add eve #private")
        .unwrap_err();
    assert!(error.contains("every participant permission"));
    assert!(error.contains("Ask the room owner to add you"));
    assert!(e.data == before);
    e.execute(Some("bob"), Some("private"), "/add alice")
        .unwrap();
    e.execute(Some("alice"), Some("private"), "/add eve")
        .unwrap();
    e.execute(Some("eve"), None, "/join #private").unwrap();
    e.execute(Some("eve"), Some("private"), "invited participant")
        .unwrap();
    for permission in ["x:member.add", "/add"] {
        e.execute(
            Some("bob"),
            Some("private"),
            &format!("/revoke alice #private {permission}"),
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
            &format!("/grant alice #private {permission}"),
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
    e.execute(Some("mallory"), None, "/new room foreign")
        .unwrap();
    let before = e.data.clone();
    assert!(
        e.execute(Some("alice"), Some("private"), "/grant eve #private /add")
            .is_err()
    );
    assert!(
        e.execute(Some("alice"), Some("private"), "/add eve #foreign")
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
        e.execute(None, None, &format!("/grant bob #team {permission}"))
            .unwrap();
    }
    for permission in ["x:member.add", "/add"] {
        let before = e.data.clone();
        assert!(
            e.execute(
                Some("bob"),
                Some("team"),
                &format!("/grant eve #team {permission}")
            )
            .is_err()
        );
        assert!(e.data == before);
        e.execute(
            Some("alice"),
            Some("team"),
            &format!("/grant eve #team {permission}"),
        )
        .unwrap();
    }
    for permission in [
        "x:policy.change",
        "/grant",
        "x:member.remove",
        "/kick",
        "x:room.delete",
        "x:room.rename",
        "/rm",
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
                &format!("/grant eve #team {permission}")
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
        e.execute(None, None, &format!("/grant alice #team {permission}"))
            .unwrap();
    }
    for operation in ["/grant", "/revoke"] {
        for permission in ["x:member.add", "/add"] {
            let before = e.data.clone();
            assert!(
                e.execute(
                    Some("alice"),
                    Some("team"),
                    &format!("{operation} eve #team {permission}")
                )
                .is_err()
            );
            assert!(e.data == before);
            e.execute(
                Some("mallory"),
                Some("team"),
                &format!("{operation} eve #team {permission}"),
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
        let output = e.execute(Some("alice"), None, "/ls room").unwrap();
        assert!(output.ends_with("Private conversations:\n@private:alice:bob"));
        assert!(!output.contains("@private:bob:eve"));
        assert!(e.data == before);
    }
    e.execute(None, None, "/revoke bob user").unwrap();
    let output = e
        .execute(Some("bob"), Some("team"), "/ls room")
        .unwrap_err();
    assert!(output.contains("Command not permitted"));
    e.execute(None, None, "/grant bob @global /ls").unwrap();
    assert!(
        e.execute(Some("bob"), None, "/ls room")
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
    e.execute(None, None, "/grant bob #team /console").unwrap();
    assert!(e.execute(Some("bob"), Some("team"), "/console").is_ok());
    // The Command view collects available scoped commands; actual operations
    // still require their own target command and action grants.
    assert!(e.execute(Some("bob"), None, "/console").is_ok());
    assert!(e.execute(Some("bob"), None, "/ls user").is_err());
    e.execute(None, None, "/revoke bob #team /console").unwrap();
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
    let error = e
        .execute(Some("bob"), None, "/new room private")
        .unwrap_err();
    assert!(error.contains("Missing command grant at @global"));
    let grant = e.execute(None, None, "/grant bob @global /new").unwrap();
    assert!(grant.contains("Requires: w:room.create or x:account.create at @global"));
    assert!(e.execute(Some("bob"), None, "/new room private").is_ok());
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
    let error = e
        .execute(Some("bob"), None, "/new room denied")
        .unwrap_err();
    assert!(error.contains("w:room.create at @global"));
    assert!(!e.data.rooms.contains_key("denied"));
    let scoped = e.execute(None, None, "/grant bob #team /new").unwrap();
    assert!(scoped.contains("does not authorize that global operation"));
    assert!(
        e.execute(None, None, "/permissions @invalid")
            .unwrap_err()
            .contains("Unknown scope")
    );
}

#[test]
fn permission_output_keeps_assignment_privacy_and_limits_separate() {
    let mut e = fixture();
    e.execute(Some("alice"), Some("team"), "original").unwrap();
    let id = super::tests::room_messages(&e, "team")[0].id.clone();
    e.execute(Some("alice"), Some("team"), "/revoke bob user #team")
        .unwrap();
    let reply = e
        .execute(Some("alice"), Some("team"), "/grant bob #team /reply")
        .unwrap();
    assert!(reply.contains("r:message.read + w:message.create"));
    let scope = e.room_scope("team").unwrap();
    assert!(!e.allows(Some("bob"), &scope, Action::Read));
    assert!(!e.allows(Some("bob"), &scope, Action::Send));
    e.execute(
        Some("alice"),
        Some("team"),
        "/grant bob #team r:message.read",
    )
    .unwrap();
    let error = e
        .execute(Some("bob"), Some("team"), &format!("/reply {id} denied"))
        .unwrap_err();
    assert!(error.contains("w:message.create at team"));
    e.execute(None, None, "/grant eve #team w:message.create")
        .unwrap();
    e.execute(None, None, "/grant bob #team x:history.clean")
        .unwrap();
    e.execute(None, None, "/user set bob history.clean_min_age 7d #team")
        .unwrap();
    let before = e.data.clone();
    let own = e
        .execute(Some("bob"), Some("team"), "/permissions")
        .unwrap();
    assert!(own.starts_with("# Access at #team\n"));
    assert!(own.contains("## Effective access"));
    assert!(!own.contains("Conditional actions"));
    assert!(!own.contains("604800"));
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
    let id = super::tests::room_messages(&e, "team")[0].id.clone();
    e.execute(Some("alice"), None, "/revoke bob user #team")
        .unwrap();
    assert!(e.snapshot("bob").unwrap().rooms.is_empty());
    assert!(!e.snapshot("bob").unwrap().unread.contains_key("team"));
    for permission in ["r:message.read", "/history"] {
        e.execute(
            Some("alice"),
            Some("team"),
            &format!("/grant bob #team {permission}"),
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
        "/revoke bob #team r:message.read",
    )
    .unwrap();
    assert!(e.history("bob", "team").is_err());
    assert!(e.snapshot("bob").unwrap().rooms.is_empty());
}
#[test]
fn permissions_and_command_gates_are_independent_and_target_scoped() {
    let mut e = fixture();
    e.execute(Some("mallory"), None, "/new room other").unwrap();
    e.execute(None, None, "/grant bob #team x:history.clean")
        .unwrap();
    e.execute(None, None, "/user set bob history.clean_min_age 7d #team")
        .unwrap();
    assert!(
        e.execute(Some("bob"), Some("team"), "/clean 7d #team")
            .is_err()
    );
    e.execute(None, None, "/grant bob #team /clean").unwrap();
    assert!(
        e.execute(Some("bob"), Some("team"), "/clean 1d #team")
            .is_err()
    );
    e.execute(Some("bob"), Some("team"), "/clean 7d #team")
        .unwrap();
    assert!(
        e.execute(Some("bob"), Some("other"), "/clean 7d #team")
            .is_err()
    );
    assert!(
        e.execute(Some("bob"), Some("team"), "/clean 7d #other")
            .is_err()
    );
    assert!(
        e.execute(Some("bob"), Some("team"), "/clean 7d @all")
            .is_err()
    );
    for permission in [
        "x:policy.change",
        "w:message.retract.any",
        "/grant",
        "x:account.password.reset",
    ] {
        assert!(
            e.execute(
                Some("alice"),
                Some("team"),
                &format!("/grant bob #team {permission}")
            )
            .is_err()
        );
    }
    assert!(
        e.execute(Some("alice"), None, "/grant bob admin #team")
            .is_err()
    );
    e.execute(None, None, "/grant bob admin #team").unwrap();
    e.execute(Some("bob"), Some("team"), "/add eve").unwrap();
    assert!(e.execute(Some("bob"), Some("other"), "/add eve").is_err());
    assert!(
        e.execute(Some("bob"), None, "/new user forbidden hash user")
            .is_err()
    );
}
#[test]
fn su_can_read_and_retract_other_private_messages_without_granting_admin_access() {
    let mut e = fixture();
    e.execute(Some("bob"), None, "/tell eve secret").unwrap();
    let id = super::tests::private_messages(&e, "bob:eve")[0].id.clone();
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
    assert_eq!(super::tests::private_messages(&e, "bob:eve").len(), 2);
    e.execute(
        Some("alice"),
        Some("@private:bob:eve"),
        &format!("/retract {id}"),
    )
    .unwrap();
    assert_eq!(super::tests::private_messages(&e, "bob:eve").len(), 1);
    assert!(
        super::tests::private_messages(&e, "bob:eve")[0]
            .reply
            .is_none()
    );
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
    let id = super::tests::private_messages(&e, "bob:eve")[0].id.clone();
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
        e.execute(Some("alice"), None, "/ls room")
            .unwrap()
            .contains("#team — owner: bob")
    );
    assert!(e.execute(Some("alice"), None, "/add eve #team").is_err());
    assert!(e.execute(Some("bob"), Some("team"), "/leave").is_err());
    e.execute(Some("bob"), None, "/rm room team").unwrap();
    e.execute(Some("alice"), None, "/new room team").unwrap();
    assert!(e.execute(Some("bob"), None, "/join #team").is_err());
    let old_id = e.data.users["bob"].id.clone();
    e.execute(Some("alice"), None, "/rm user bob").unwrap();
    e.provision("bob", "hash".into(), false, false).unwrap();
    assert_ne!(e.data.users["bob"].id, old_id);
    assert!(e.snapshot("bob").unwrap().rooms.is_empty());
    e.execute(None, None, "/disable alice").unwrap();
    assert!(
        e.execute(None, None, "/ls room")
            .unwrap()
            .contains("#team — owner: su")
    );
}
#[test]
fn policy_save_failures_preserve_compiled_permissions_and_creation_is_atomic() {
    let mut e = fixture();
    let before = e.data.clone();
    e.store
        .db
        .lock()
        .unwrap()
        .execute_batch("PRAGMA query_only=ON;")
        .unwrap();
    for input in [
        "/grant bob su",
        "/grant bob @global w:room.create",
        "/owner bob #team",
        "/new room lost",
    ] {
        assert!(e.execute(None, None, input).is_err());
        assert!(e.data == before);
        assert!(!e.is_su(Some("bob")));
        assert!(!e.allows(Some("bob"), &Scope::Server, Action::CreateRoom));
    }
    assert!(!e.data.rooms.contains_key("lost"));
    e.store
        .db
        .lock()
        .unwrap()
        .execute_batch("PRAGMA query_only=OFF;")
        .unwrap();
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
    let id = super::tests::private_messages(&e, "bob:eve")[0].id.clone();
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
    let reply = super::tests::private_messages(&e, "bob:eve")
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
    e.execute(Some("alice"), None, "/new room other").unwrap();
    e.execute(Some("alice"), Some("other"), "target").unwrap();
    let id = super::tests::room_messages(&e, "other")[0].id.clone();
    e.execute(None, None, "/grant bob #other r:message.read")
        .unwrap();
    e.execute(None, None, "/grant bob #other w:message.react")
        .unwrap();
    // Bob can invoke /react in team, but has no /react grant in other.
    assert!(
        e.execute(Some("bob"), Some("other"), &format!("/react {id} denied"))
            .is_err()
    );
    assert!(e.execute(Some("bob"), None, "/clean 1s #other").is_err());
    e.execute(None, None, "/grant bob #other x:history.clean")
        .unwrap();
    e.execute(None, None, "/grant bob #team /clean").unwrap();
    assert!(e.execute(Some("bob"), None, "/clean 1s #other").is_err());
    e.execute(None, None, "/grant bob #other /clean").unwrap();
    e.execute(None, None, "/user set bob history.clean_min_age -1 @global")
        .unwrap();
    e.execute(Some("bob"), None, "/clean 1s #other").unwrap();
}

#[test]
fn console_identity_cannot_be_claimed_by_an_account_with_the_same_display_name() {
    let mut e = fixture();
    e.provision("console", "hash".into(), false, false).unwrap();
    e.execute(Some("alice"), None, "/add console #team")
        .unwrap();
    e.execute(None, Some("team"), "stdin message").unwrap();
    let message = &super::tests::room_messages(&e, "team")[0];
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
    e.execute(None, None, "/new room su").unwrap();
    e.execute(None, None, "/grant bob #su r:message.read")
        .unwrap();
    assert!(!e.is_su(Some("bob")));
    assert!(e.history("bob", "su").is_ok());
    e.execute(None, None, "/revoke bob #su r:message.read")
        .unwrap();
    assert!(e.history("bob", "su").is_err());
    e.execute(None, None, "/grant bob su").unwrap();
    assert!(e.is_su(Some("bob")));
    e.execute(None, None, "/revoke bob su").unwrap();
    e.execute(
        Some("alice"),
        Some("team"),
        "/revoke bob #team w:message.create",
    )
    .unwrap();
    e.execute(Some("bob"), Some("team"), "group still grants writes")
        .unwrap();
    e.execute(Some("alice"), Some("team"), "/revoke bob user #team")
        .unwrap();
    assert!(e.execute(Some("bob"), Some("team"), "denied").is_err());
    for input in [
        "/grant bob #team unknown",
        "/grant bob #team r:message.read 1d",
        "/revoke bob #team r:message.read 7d",
        "/grant bob #nonexistent r:message.read",
        "/grant missing #team r:message.read",
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
        "user",
        "room",
        "retract",
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
    for (topic, details) in [
        ("user", "/user get @all"),
        ("room", "/room rename"),
        ("retract", "retained replies lose their quote preview"),
    ] {
        let manual = e
            .execute(Some("eve"), None, &format!("/man {topic}"))
            .unwrap();
        assert!(manual.contains(details), "missing behavior in /man {topic}");
    }
    let new_error = e.execute(None, None, "/new unknown").unwrap_err();
    assert!(new_error.contains("/room new"));
    assert!(!new_error.contains("/rm"));
    let rm_error = e.execute(None, None, "/rm unknown").unwrap_err();
    assert!(rm_error.contains("/revoke user group"));
    assert!(!rm_error.contains("/new"));
    assert!(e.data == before);
    assert_eq!(e.revision, revision);
    assert!(
        e.execute(None, None, "/man grant")
            .unwrap()
            .contains("Only an existing su")
    );
}
#[test]
fn invitation_and_user_group_assignment_cannot_amplify_partial_authority() {
    let mut e = fixture();
    e.execute(Some("alice"), Some("team"), "/revoke bob user #team")
        .unwrap();
    for permission in [
        "r:message.read",
        "x:member.add",
        "x:policy.change",
        "/add",
        "/grant",
    ] {
        e.execute(None, None, &format!("/grant bob #team {permission}"))
            .unwrap();
    }
    let before = e.data.clone();
    for command in [
        "/add bob",
        "/add eve",
        "/grant bob user #team",
        "/grant eve user #team",
        "/grant bob #team w:message.create",
    ] {
        assert!(
            e.execute(Some("bob"), Some("team"), command).is_err(),
            "{command}"
        );
        assert!(e.data == before);
    }
    e.execute(Some("alice"), Some("team"), "/grant eve user #team")
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
        "w:account.color.own",
        "w:private.create",
    ]);
    let admin = BTreeSet::from([
        "r:account.metadata",
        "r:account.list",
        "w:account.password.own",
        "w:account.rename.own",
        "w:account.color.own",
        "w:private.create",
        "w:room.create",
        "x:account.create",
        "x:account.disable",
        "x:account.enable",
        "x:account.delete",
        "x:group.admin.assign",
        "x:bot.credentials",
        "r:bot.config",
        "w:bot.config",
        "r:server.config",
        "r:policy.read",
    ]);
    let member = BTreeSet::from([
        "r:room.discover",
        "r:message.read",
        "r:member.list",
        "w:message.create",
        "w:media.upload",
        "w:message.react",
        "w:message.retract.own",
        "w:message.edit.own",
        "w:account.color.own",
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
        "x:room.rename",
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
                    expected.insert("w:account.color.any");
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
    for command in ["/enable bob", "/disable bob", "/rm user bob"] {
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
    e.execute(None, None, "/rm user bob").unwrap();
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
    let id = super::tests::room_messages(&e, "team")[0].id.clone();
    let sequence = super::tests::room_messages(&e, "team")[0].sequence;
    e.execute(None, None, "/revoke bob user #team").unwrap();
    e.execute(None, None, "/revoke bob user").unwrap();
    let probes = [
        "/sudo /whoami".into(),
        "/help".into(),
        "/bot ls @global".into(),
        "/console".into(),
        "/man grant".into(),
        "/permissions #team".into(),
        "/debug on".into(),
        "/whoami".into(),
        "/user rename renamed".into(),
        "/user passwd old new".into(),
        "/user ls".into(),
        "/user new created password user".into(),
        "/user reset eve password password".into(),
        "/user enable eve".into(),
        "/user disable eve".into(),
        "/user delete eve".into(),
        "/grant bob su".into(),
        "/revoke eve su".into(),
        "/room ls".into(),
        "/user ls #team".into(),
        "/room join #team".into(),
        "/room leave #team".into(),
        "/room new created".into(),
        "/room add eve #team".into(),
        "/room kick alice #team".into(),
        "/room delete #team".into(),
        "/tell eve denied".into(),
        format!("/retract {id}"),
        format!("/react {id} reaction"),
        format!("/reply {id} reply"),
        format!("/edit {id} edited"),
        "/history".into(),
        "/configs".into(),
        "/clean 7d #team".into(),
        "/room owner bob #team".into(),
        "/clear".into(),
        "/logout".into(),
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
    let cursors = e.store.read_positions.clone();
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
    assert_eq!(e.store.read_positions, cursors);
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
        e.execute(Some("管理员"), None, "/new room 测试").unwrap();
        e.execute(Some("管理员"), None, "/add 用户🙂 #测试")
            .unwrap();
        e.execute(Some("用户🙂"), Some("测试"), "hello @管理员")
            .unwrap();
        let message_id = super::tests::room_messages(&e, "测试")[0].id.clone();
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
        sequence = super::tests::private_messages(&e, "用户🙂:管理员")[0].sequence;
        private_id = e.data.private["用户🙂:管理员"].id.clone();
        e.mark_read("用户🙂", "@direct:管理员", sequence).unwrap();
        e.login("session".into(), "用户🙂", "hash", None).unwrap();
        id = e.data.users["用户🙂"].id.clone();
        e.execute(Some("用户🙂"), None, "/rename 新名字🚀").unwrap();
        assert_eq!(e.data.users["新名字🚀"].id, id);
        assert_eq!(e.session("session").as_deref(), Some("新名字🚀"));
        assert!(e.data.rooms["测试"].members.contains("新名字🚀"));
        assert_eq!(super::tests::room_messages(&e, "测试")[0].from, "新名字🚀");
        assert_eq!(
            super::tests::room_messages(&e, "测试")[1]
                .reply
                .as_ref()
                .unwrap()
                .from,
            "新名字🚀"
        );
        assert!(super::tests::room_messages(&e, "测试")[0].reactions["👍"].contains("新名字🚀"));
        assert!(
            super::tests::room_messages(&e, "测试")[1]
                .mentions
                .contains("新名字🚀")
        );
        assert_eq!(e.data.private["新名字🚀:管理员"].id, private_id);
        assert_eq!(
            super::tests::private_messages(&e, "新名字🚀:管理员")[0]
                .to
                .as_deref(),
            Some("新名字🚀")
        );
        assert_eq!(
            e.store.read_positions["新名字🚀"]["dm:新名字🚀:管理员"],
            sequence
        );
        e.execute(Some("新名字🚀"), None, "/tell 管理员 renamed")
            .unwrap();
        e.provision("用户🙂", "replacement".into(), false, false)
            .unwrap();
        assert!(e.snapshot("用户🙂").unwrap().direct.is_empty());
    }
    let e = Engine::open(&path).unwrap();
    assert_eq!(e.data.users["新名字🚀"].id, id);
    assert_eq!(e.session("session").as_deref(), Some("新名字🚀"));
    assert_eq!(
        e.store.read_positions["新名字🚀"]["dm:新名字🚀:管理员"],
        sequence
    );
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
        assert!(e.rename_user(Some("bob"), "bob", name, "/rename").is_err());
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
    e.execute(None, None, "/grant 重命名 @account:重命名 /user")
        .unwrap();
    let access = e.snapshot("重命名").unwrap().account_access;
    assert!(access.permissions.contains(&"w:account.rename.own".into()));
    assert!(
        access
            .commands
            .iter()
            .any(|command| command.name == "/user")
    );
    assert!(
        !access
            .commands
            .iter()
            .any(|command| command.name == "/passwd")
    );
    e.execute(Some("重命名"), None, "/user rename allowed")
        .unwrap();
}

#[test]
fn rename_storage_failure_rolls_back_account_and_read_positions() {
    let mut e = fixture();
    e.execute(Some("eve"), None, "/tell bob incoming").unwrap();
    let sequence = super::tests::private_messages(&e, "bob:eve")[0].sequence;
    e.mark_read("bob", "@direct:eve", sequence).unwrap();
    let before = e.data.clone();
    let positions = e.store.read_positions.clone();
    e.store.db.lock().unwrap().execute_batch("CREATE TRIGGER fail_rename BEFORE INSERT ON audit_entries BEGIN SELECT RAISE(FAIL, 'injected'); END;").unwrap();
    assert!(e.execute(Some("bob"), None, "/rename 改名").is_err());
    assert!(e.data == before);
    assert_eq!(e.store.read_positions, positions);
    e.store
        .db
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER fail_rename;")
        .unwrap();
    e.execute(Some("bob"), None, "/rename 改名").unwrap();
}

#[test]
fn room_rename_checks_scoped_actions_context_and_target_commands() {
    let mut e = fixture();
    e.execute(Some("alice"), None, "/new room other").unwrap();
    for actor in ["bob", "eve", "mallory"] {
        let before = e.data.clone();
        assert!(
            e.execute(Some(actor), None, "/rename room team denied")
                .is_err()
        );
        assert!(e.data == before);
    }
    e.execute(None, None, "/revoke bob user").unwrap();
    e.execute(None, None, "/grant bob #team x:room.rename")
        .unwrap();
    assert!(
        e.execute(Some("bob"), None, "/rename room team denied")
            .is_err()
    );
    e.execute(None, None, "/grant bob #other /rename").unwrap();
    assert!(
        e.execute(Some("bob"), Some("other"), "/rename room team denied")
            .unwrap_err()
            .contains("on this target")
    );
    e.execute(None, None, "/grant bob #team /rename").unwrap();
    assert!(
        e.execute(Some("bob"), Some("@direct:eve"), "/rename room team denied")
            .unwrap_err()
            .contains("in this context")
    );
    e.execute(Some("bob"), Some("other"), "/rename room team renamed")
        .unwrap();
    assert!(
        e.execute(Some("bob"), Some("renamed"), "/rename room other stolen")
            .is_err()
    );
    e.execute(None, None, "/revoke bob #renamed /rename")
        .unwrap();
    assert!(
        e.execute(Some("bob"), Some("other"), "/rename room renamed denied")
            .is_err()
    );
    e.execute(None, None, "/grant bob #renamed /rename")
        .unwrap();
    e.execute(None, None, "/revoke bob #renamed x:room.rename")
        .unwrap();
    assert!(
        e.execute(Some("bob"), None, "/rename room renamed denied")
            .is_err()
    );
    // Account rename authority cannot substitute for room management.
    e.execute(None, None, "/grant bob @global x:account.rename.any")
        .unwrap();
    assert!(
        e.execute(Some("bob"), None, "/rename room renamed denied")
            .is_err()
    );
    e.execute(None, None, "/grant bob #renamed x:room.rename")
        .unwrap();
    e.execute(None, None, "/disable bob").unwrap();
    assert!(
        e.execute(Some("bob"), None, "/rename room renamed denied")
            .unwrap_err()
            .contains("Account unavailable")
    );
}

#[test]
fn room_rename_follows_management_bundles_ownership_and_sudo() {
    let mut e = fixture();
    e.execute(Some("alice"), Some("team"), "/rename room team owned")
        .unwrap();
    e.execute(Some("alice"), Some("owned"), "/owner bob")
        .unwrap();
    assert!(
        e.execute(Some("alice"), None, "/rename room owned denied")
            .is_err()
    );
    e.execute(Some("bob"), Some("owned"), "/rename room owned transferred")
        .unwrap();
    e.execute(None, None, "/grant mallory admin #transferred")
        .unwrap();
    e.execute(Some("mallory"), None, "/rename room transferred managed")
        .unwrap();
    // Room owners cannot delegate a management action outside the participant bundle.
    assert!(
        e.execute(
            Some("bob"),
            Some("managed"),
            "/grant eve #managed x:room.rename"
        )
        .is_err()
    );
    e.execute(Some("alice"), None, "/new room foreign").unwrap();
    for permission in ["/sudo", "x:command.sudo"] {
        e.execute(None, None, &format!("/grant bob @global {permission}"))
            .unwrap();
    }
    let owner = e.data.rooms["foreign"].owner_id.clone();
    e.execute(Some("bob"), None, "/sudo /rename room foreign elevated")
        .unwrap();
    assert_eq!(e.data.rooms["elevated"].owner_id, owner);
    assert_eq!(
        e.data.policy.audit.back().unwrap().actor_id,
        e.data.users["bob"].id
    );
    assert!(!e.is_su(Some("bob")));
    assert!(
        e.execute(Some("bob"), None, "/rename room elevated denied")
            .is_err()
    );
    e.execute(None, None, "/grant eve su").unwrap();
    e.execute(Some("eve"), None, "/rename room elevated superuser")
        .unwrap();
    e.execute(None, None, "/rename room superuser console")
        .unwrap();
}

#[test]
fn rename_explicit_targets_validation_and_noops_preserve_account_shortcuts() {
    let mut e = fixture();
    e.provision("room", "hash".into(), false, false).unwrap();
    e.execute(Some("room"), None, "/rename user room user")
        .unwrap();
    // Legacy target names may themselves be the new subcommand words.
    e.execute(None, None, "/rename user room").unwrap();
    e.execute(Some("bob"), None, "/rename user bob robert")
        .unwrap();
    e.execute(Some("robert"), None, "/rename bob").unwrap();
    e.execute(None, None, "/grant eve su").unwrap();
    e.execute(None, None, "/grant alice @account:eve x:account.rename.any")
        .unwrap();
    assert!(
        e.execute(Some("alice"), None, "/rename user eve denied")
            .unwrap_err()
            .contains("Only su")
    );
    e.execute(Some("alice"), None, "/new room other").unwrap();
    let before = e.data.clone();
    let revision = e.revision;
    for input in [
        "/rename",
        "/rename room",
        "/rename room team new extra",
        "/rename unknown team new",
        "/rename room missing new",
        "/rename room team other",
        "/rename room team @private",
        "/rename room team bad:name",
    ] {
        assert!(e.execute(None, None, input).is_err(), "{input}");
        assert!(e.data == before);
        assert_eq!(e.revision, revision);
    }
    for name in [
        "",
        "bad name",
        "bad\u{0000}name",
        "bad\u{0085}name",
        &"界".repeat(33),
    ] {
        assert!(
            e.rename_room(Some("alice"), "team", name, "/rename")
                .is_err()
        );
        assert!(e.data == before);
    }
    e.store.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_noop BEFORE UPDATE ON state BEGIN SELECT RAISE(ABORT,'no writes'); END;").unwrap();
    assert_eq!(
        e.execute(Some("alice"), None, "/rename room team team")
            .unwrap(),
        "Room name unchanged."
    );
    assert!(e.data == before);
    assert_eq!(e.revision, revision);
    e.store
        .db
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_noop;")
        .unwrap();
    // Room and account namespaces are independent.
    e.execute(Some("alice"), None, "/rename room team bob")
        .unwrap();
    assert!(e.data.users.contains_key("bob"));
}

#[test]
fn room_rename_preserves_history_grants_cursors_and_identity_across_restart_and_name_reuse() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    let mut e = Engine::open(&path).unwrap();
    for (name, admin) in [("alice", true), ("bob", false)] {
        e.provision(name, "hash".into(), admin, false).unwrap();
    }
    e.execute(Some("alice"), None, "/new room team").unwrap();
    e.execute(Some("alice"), None, "/add bob #team").unwrap();
    e.execute(Some("alice"), Some("team"), "hello @bob")
        .unwrap();
    let message = e.history("bob", "team").unwrap().messages[0].clone();
    e.execute(
        Some("bob"),
        Some("team"),
        &format!("/react {} 👍", message.id),
    )
    .unwrap();
    e.execute(
        Some("bob"),
        Some("team"),
        &format!("/reply {} reply", message.id),
    )
    .unwrap();
    e.mark_read("bob", "team", message.sequence).unwrap();
    e.mark_read("alice", "team", message.sequence).unwrap();
    e.execute(Some("alice"), Some("team"), "still unread")
        .unwrap();
    e.execute(Some("alice"), None, "/tell bob private").unwrap();
    let dm_sequence = e.history("bob", "@direct:alice").unwrap().messages[0].sequence;
    e.mark_read("bob", "@direct:alice", dm_sequence).unwrap();
    // Exercise orphan cursor cleanup without letting name reuse inherit it.
    e.store_read_position("bob", "room:改名🚀", u64::from(u32::MAX))
        .unwrap();
    e.store
        .read_positions
        .get_mut("bob")
        .unwrap()
        .insert("room:改名🚀".into(), u64::from(u32::MAX));
    let room = e.data.rooms["team"].clone();
    let history = e.history("bob", "team").unwrap().messages;
    let assignments = e.data.policy.assignments.clone();
    let grants = e.data.policy.grants.clone();
    let policy_revision = e.data.policy.revision;
    let unread = e.unreads("bob").unwrap()["team"].count;
    e.execute(Some("alice"), Some("team"), "/rename room team 改名🚀")
        .unwrap();
    assert!(!e.data.rooms.contains_key("team"));
    assert_eq!(e.data.rooms["改名🚀"].id, room.id);
    assert_eq!(e.data.rooms["改名🚀"].owner_id, room.owner_id);
    assert_eq!(e.data.rooms["改名🚀"].members, room.members);
    assert!(e.data.policy.assignments == assignments);
    assert!(e.data.policy.grants == grants);
    assert_eq!(e.data.policy.revision, policy_revision);
    assert!(*e.history("bob", "改名🚀").unwrap().messages == *history);
    assert!(e.history("bob", "team").is_err());
    for user in ["alice", "bob"] {
        assert_eq!(
            e.store.read_positions[user]["room:改名🚀"],
            message.sequence
        );
        assert!(!e.store.read_positions[user].contains_key("room:team"));
    }
    assert_eq!(e.store.read_positions["bob"]["dm:alice:bob"], dm_sequence);
    assert_eq!(e.unreads("bob").unwrap()["改名🚀"].count, unread);
    assert!(
        e.snapshot("bob")
            .unwrap()
            .rooms
            .iter()
            .any(|r| r.name == "改名🚀" && r.id == room.id)
    );
    e.execute(Some("alice"), None, "/new room team").unwrap();
    assert_ne!(e.data.rooms["team"].id, room.id);
    assert!(e.history("bob", "team").is_err());
    e.execute(Some("alice"), None, "/add bob #team").unwrap();
    e.execute(Some("alice"), Some("team"), "replacement")
        .unwrap();
    assert_eq!(e.unreads("bob").unwrap()["team"].count, 1);
    drop(e);
    let mut e = Engine::open(&path).unwrap();
    assert_eq!(e.data.rooms["改名🚀"].id, room.id);
    assert!(*e.history("bob", "改名🚀").unwrap().messages == *history);
    assert_eq!(
        e.store.read_positions["bob"]["room:改名🚀"],
        message.sequence
    );
    assert_eq!(e.unreads("bob").unwrap()["改名🚀"].count, unread);
    assert_eq!(e.unreads("bob").unwrap()["team"].count, 1);
    e.execute(
        Some("alice"),
        Some("改名🚀"),
        "/rename room 改名🚀 renamed_again",
    )
    .unwrap();
}

#[test]
fn existing_owner_grants_are_preserved_and_room_rename_can_be_explicitly_delegated() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    let mut e = Engine::open(&path).unwrap();
    e.provision("alice", "hash".into(), true, false).unwrap();
    e.execute(Some("alice"), None, "/new room team").unwrap();
    let before = e.data.clone();
    // An existing owner grant may omit room rename; inspection cannot add authority.
    for grant in e.data.policy.grants.iter_mut().filter(|grant| grant.owner) {
        grant.permissions.remove("x:room.rename");
        grant.permissions.remove("/rename");
    }
    e.data.policy.revision = e.data.policy.revision.wrapping_add(1);
    e.save_delta(&before).unwrap();
    let policy = e.data.policy.clone();
    drop(e);
    let mut e = Engine::open(&path).unwrap();
    assert!(e.data.policy == policy);
    assert!(
        e.execute(Some("alice"), None, "/rename room team denied")
            .unwrap_err()
            .contains("x:room.rename")
    );
    for permission in ["x:room.rename", "/rename"] {
        e.execute(None, None, &format!("/grant alice #team {permission}"))
            .unwrap();
    }
    e.execute(Some("alice"), Some("team"), "/rename room team renamed")
        .unwrap();
}

#[test]
fn room_rename_storage_failures_roll_back_disk_memory_cursors_and_authority() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    let mut e = Engine::open(&path).unwrap();
    e.provision("alice", "hash".into(), true, false).unwrap();
    e.execute(Some("alice"), None, "/new room team").unwrap();
    e.execute(Some("alice"), Some("team"), "history").unwrap();
    let sequence = e.history("alice", "team").unwrap().messages[0].sequence;
    e.mark_read("alice", "team", sequence).unwrap();
    let before = e.data.clone();
    let positions = e.store.read_positions.clone();
    let revision = e.revision;
    for trigger in [
        "CREATE TRIGGER fail_rename BEFORE UPDATE ON read_positions BEGIN SELECT RAISE(ABORT,'cursor failure'); END;",
        "CREATE TRIGGER fail_rename BEFORE INSERT ON entities WHEN NEW.kind='room' AND NEW.key='new' BEGIN SELECT RAISE(ABORT,'metadata failure'); END;",
        "CREATE TRIGGER fail_rename BEFORE INSERT ON audit_entries BEGIN SELECT RAISE(ABORT,'audit failure'); END;",
    ] {
        e.store.db.lock().unwrap().execute_batch(trigger).unwrap();
        assert!(
            e.execute(Some("alice"), Some("team"), "/rename room team new")
                .unwrap_err()
                .contains("Storage unavailable")
        );
        assert!(e.data == before);
        assert_eq!(e.store.read_positions, positions);
        assert_eq!(e.revision, revision);
        assert!(e.allows(
            Some("alice"),
            &e.room_scope("team").unwrap(),
            Action::RenameRoom
        ));
        let disk = Connection::open(&path).unwrap();
        let raw: String = disk
            .query_row("SELECT json FROM state WHERE id=1", [], |row| row.get(0))
            .unwrap();
        let mut data: Data = serde_json::from_str(&raw).unwrap();
        storage::load_entities(&disk, &mut data).unwrap();
        assert!(data == before);
        let cursor: (String, u64) = disk
            .query_row(
                "SELECT conversation,sequence FROM read_positions WHERE username='alice'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(cursor, ("room:team".into(), sequence));
        let count: u64 = disk
            .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
        drop(disk);
        e.store
            .db
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER fail_rename;")
            .unwrap();
    }
    e.execute(Some("alice"), Some("team"), "/rename room team new")
        .unwrap();
    assert_eq!(e.store.read_positions["alice"]["room:new"], sequence);
}

#[test]
fn command_families_filter_groups_and_remove_assignments_without_amplifying_authority() {
    let mut e = fixture();
    let before = e.data.clone();
    for command in [
        "/ls",
        "/ls room",
        "/ls user",
        "/ls group",
        "/ls @user",
        "/ls @admin",
        "/ls @su",
    ] {
        assert!(e.execute(Some("bob"), None, command).is_ok(), "{command}");
        assert!(e.data == before);
    }
    let admins = e.execute(Some("bob"), None, "/ls @admin").unwrap();
    assert!(admins.contains("alice — admin"));
    assert!(!admins.contains("bob —"));
    for command in [
        "/rooms",
        "/users",
        "/user fresh pass",
        "/delete team",
        "/deleteuser eve",
        "/new legacy",
        "/rm",
        "/ls unknown",
        "/ls room extra",
        "/rm group eve",
        "/rm group eve nonexistent",
    ] {
        assert!(e.execute(None, None, command).is_err(), "{command}");
        assert!(e.data == before);
    }
    assert!(e.execute(Some("bob"), None, "/rm user eve").is_err());
    assert!(
        e.execute(Some("bob"), Some("team"), "/rm group eve user #team")
            .is_err()
    );
    e.execute(None, None, "/grant eve user #team").unwrap();
    e.execute(Some("alice"), Some("team"), "/rm group eve user #team")
        .unwrap();
    assert!(e.history("eve", "team").is_err());
    assert!(e.active("eve"));
    e.execute(None, None, "/grant eve su").unwrap();
    assert!(e.execute(Some("alice"), None, "/rm group eve su").is_err());
    e.execute(None, None, "/rm group eve su").unwrap();
    assert!(!e.is_su(Some("eve")));
}

#[test]
fn group_directory_keeps_disabled_assignments_visible_only_to_su() {
    let mut e = fixture();
    e.execute(None, None, "/grant eve su").unwrap();
    e.execute(None, None, "/disable eve").unwrap();
    assert!(
        e.execute(None, None, "/ls @su")
            .unwrap()
            .contains("eve — su (disabled)")
    );
    assert!(
        !e.execute(Some("bob"), None, "/ls @su")
            .unwrap()
            .contains("eve")
    );
}

#[test]
fn shared_provisioning_syntax_rejects_malformed_account_requests() {
    for input in [
        "/new user",
        "/new user bob",
        "/new user bob pass su",
        "/new user bob pass admin extra",
        "/reset bob",
        "/reset bob pass extra",
    ] {
        assert!(
            crate::commands::provision_command(&input.split_whitespace().collect::<Vec<_>>())
                .is_err(),
            "{input}"
        );
    }
    for (input, admin, reset) in [
        ("/new user bob pass", false, false),
        ("/new user bob pass admin", true, false),
        ("/reset bob pass", false, true),
    ] {
        let parsed =
            crate::commands::provision_command(&input.split_whitespace().collect::<Vec<_>>())
                .unwrap()
                .unwrap();
        assert_eq!(
            (parsed.name, parsed.password, parsed.admin, parsed.reset),
            ("bob", "pass", admin, reset)
        );
    }
}

#[test]
fn edit_any_requires_scoped_read_command_and_action_grants() {
    let mut e = fixture();
    e.execute(Some("alice"), Some("team"), "original").unwrap();
    let original = super::tests::room_messages(&e, "team")[0].clone();
    // Ownership/ordinary admin rights do not include editing other authors.
    assert!(!e.allows(
        Some("alice"),
        &e.room_scope("team").unwrap(),
        Action::EditAny
    ));
    assert!(
        e.execute(Some("alice"), None, "/grant bob #team w:message.edit.any")
            .is_err()
    );
    e.execute(None, None, "/revoke bob user #team").unwrap();
    e.execute(None, None, "/grant bob #team w:message.edit.any")
        .unwrap();
    let command = format!("/edit {} moderated", original.id);
    assert!(e.execute(Some("bob"), Some("team"), &command).is_err());
    e.execute(None, None, "/grant bob #team /edit").unwrap();
    assert!(e.execute(Some("bob"), Some("team"), &command).is_err());
    e.execute(None, None, "/grant bob #team r:message.read")
        .unwrap();
    e.execute(Some("bob"), Some("team"), &command).unwrap();
    let edited = super::tests::room_messages(&e, "team")[0].clone();
    assert_eq!(edited.from, original.from);
    assert_eq!(edited.author_id, original.author_id);
    assert_eq!(edited.sequence, original.sequence);
    assert!(edited.edited);
    // Every gate remains necessary after the first successful edit.
    for permission in ["w:message.edit.any", "/edit", "r:message.read"] {
        e.execute(None, None, &format!("/revoke bob #team {permission}"))
            .unwrap();
        assert!(
            e.execute(
                Some("bob"),
                Some("team"),
                &format!("/edit {} denied", original.id)
            )
            .is_err()
        );
        assert_eq!(super::tests::room_messages(&e, "team")[0].text, "moderated");
        e.execute(None, None, &format!("/grant bob #team {permission}"))
            .unwrap();
    }
    e.execute(Some("mallory"), None, "/new room other").unwrap();
    e.execute(Some("mallory"), Some("other"), "foreign")
        .unwrap();
    let foreign = super::tests::room_messages(&e, "other")[0].id.clone();
    e.execute(None, None, "/grant bob #other r:message.read")
        .unwrap();
    e.execute(None, None, "/grant bob #other /edit").unwrap();
    assert!(
        e.execute(
            Some("bob"),
            Some("other"),
            &format!("/edit {foreign} denied")
        )
        .is_err()
    );
    assert!(
        e.execute(
            Some("bob"),
            Some("team"),
            &format!("/edit {foreign} denied")
        )
        .is_err()
    );
    // Edit-any is outside participant delegation, even when a manager holds it.
    e.execute(None, None, "/grant alice #team w:message.edit.any")
        .unwrap();
    assert!(
        e.execute(Some("alice"), None, "/grant eve #team w:message.edit.any")
            .is_err()
    );
    e.execute(None, None, "/grant eve #team w:message.edit.any")
        .unwrap();
    assert!(
        e.execute(Some("alice"), None, "/grant eve #other w:message.edit.any")
            .is_err()
    );
}

#[test]
fn private_edit_any_preserves_participants_and_cannot_escape_its_scope() {
    let mut e = fixture();
    e.execute(Some("bob"), None, "/tell alice original")
        .unwrap();
    e.execute(Some("bob"), None, "/tell eve other conversation")
        .unwrap();
    let original = super::tests::private_messages(&e, "alice:bob")[0].clone();
    let other = super::tests::private_messages(&e, "bob:eve")[0].id.clone();
    for permission in ["w:message.edit.any", "r:message.read", "/edit"] {
        e.execute(
            None,
            None,
            &format!("/grant eve @private:alice:bob {permission}"),
        )
        .unwrap();
    }
    let view = Some("@private:alice:bob");
    e.execute(
        Some("eve"),
        view,
        &format!("/edit {} @alice @bob @eve", original.id),
    )
    .unwrap();
    let edited = super::tests::private_messages(&e, "alice:bob")[0].clone();
    assert_eq!(edited.from, "bob");
    assert_eq!(edited.to.as_deref(), Some("alice"));
    assert_eq!(edited.author_id, original.author_id);
    assert_eq!(
        edited.mentions,
        BTreeSet::from(["alice".into(), "bob".into()])
    );
    assert!(
        e.execute(Some("eve"), view, &format!("/edit {other} denied"))
            .is_err()
    );
    assert!(
        e.execute(
            Some("eve"),
            Some("@direct:bob"),
            &format!("/edit {} denied", original.id)
        )
        .is_err()
    );
    e.execute(None, None, "/disable eve").unwrap();
    assert!(
        e.execute(
            Some("eve"),
            view,
            &format!("/edit {} disabled", original.id)
        )
        .is_err()
    );
    assert_eq!(
        super::tests::private_messages(&e, "alice:bob")[0].text,
        "@alice @bob @eve"
    );
}

#[test]
fn su_console_and_sudo_edits_use_any_message_authority() {
    let mut e = fixture();
    e.execute(Some("alice"), Some("team"), "original").unwrap();
    let original = super::tests::room_messages(&e, "team")[0].clone();
    e.execute(None, None, &format!("/edit {} console update", original.id))
        .unwrap();
    assert!(
        e.commands_for(None, None)
            .iter()
            .any(|command| command.name == "/edit")
    );
    e.execute(None, None, "/grant eve su").unwrap();
    e.execute(
        Some("eve"),
        None,
        &format!("/edit {} su update", original.id),
    )
    .unwrap();
    for permission in ["/sudo", "x:command.sudo"] {
        e.execute(None, None, &format!("/grant bob @global {permission}"))
            .unwrap();
    }
    e.execute(
        Some("bob"),
        Some("team"),
        &format!("/sudo /edit {} sudo update", original.id),
    )
    .unwrap();
    assert!(!e.allows(Some("bob"), &e.room_scope("team").unwrap(), Action::EditAny));
    assert!(
        e.execute(
            Some("bob"),
            Some("team"),
            &format!("/edit {} denied", original.id)
        )
        .is_err()
    );
    let edited = super::tests::room_messages(&e, "team")[0].clone();
    assert_eq!(edited.text, "sudo update");
    assert_eq!(edited.author_id, original.author_id);
    assert_eq!(edited.from, original.from);
    assert_eq!(edited.time, original.time);
    e.execute(Some("bob"), None, "/tell alice private original")
        .unwrap();
    let private = super::tests::private_messages(&e, "alice:bob")[0]
        .id
        .clone();
    e.execute(None, None, &format!("/edit {private} @alice @bob @eve"))
        .unwrap();
    assert_eq!(
        super::tests::private_messages(&e, "alice:bob")[0].mentions,
        BTreeSet::from(["alice".into(), "bob".into()])
    );
}

#[test]
fn private_edit_any_rolls_back_on_revision_write_failure() {
    let mut e = fixture();
    e.execute(Some("bob"), None, "/tell alice original")
        .unwrap();
    let original = super::tests::private_messages(&e, "alice:bob")[0].clone();
    for permission in ["w:message.edit.any", "r:message.read", "/edit"] {
        e.execute(
            None,
            None,
            &format!("/grant eve @private:alice:bob {permission}"),
        )
        .unwrap();
    }
    let revision = e.data.private["alice:bob"].revision;
    e.store.db.lock().unwrap().execute_batch("CREATE TRIGGER reject_edit_revision BEFORE UPDATE ON entities BEGIN SELECT RAISE(ABORT, 'injected failure'); END;").unwrap();
    assert!(
        e.execute(
            Some("eve"),
            Some("@private:alice:bob"),
            &format!("/edit {} failed", original.id)
        )
        .is_err()
    );
    assert!(super::tests::private_messages(&e, "alice:bob")[0] == original);
    assert_eq!(e.data.private["alice:bob"].revision, revision);
    assert!(e.changed_conversation.is_none());
}
