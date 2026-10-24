//! v18 to v19: convert inherited color permissions to direct room capabilities.
use super::*;
use authorization::{Action, Grant, Group, Scope, Subject};

pub(super) fn upgrade_policy(data: &mut Data) {
    let mut grants = Vec::new();
    for room in data.rooms.values() {
        let scope = Scope::Room(room.id.clone());
        for name in &room.members {
            let Some(account) = data.users.get(name) else {
                continue;
            };
            let groups: BTreeSet<_> = data
                .policy
                .assignments
                .iter()
                .filter(|a| a.account_id == account.id && a.scope == Scope::Server)
                .map(|a| a.group)
                .collect();
            let inherited = data.policy.assignments.iter().any(|a| {
                a.account_id == account.id
                    && (a.scope == Scope::Server || a.scope == scope)
                    && matches!(a.group, Group::User | Group::Admin | Group::Su)
            });
            let explicit = data.policy.grants.iter().any(|g| {
                (g.scope == Scope::Server || g.scope == scope)
                    && g.permissions.contains(Action::ColorOwn.name())
                    && match &g.subject {
                        Subject::Account(id) => id == &account.id,
                        Subject::Group(group) => groups.contains(group),
                    }
            });
            if inherited || explicit {
                grants.push(Grant {
                    owner: false,
                    subject: Subject::Account(account.id.clone()),
                    scope: scope.clone(),
                    permissions: [Action::ColorOwn.name().to_owned()].into(),
                });
            }
        }
        // Keep this in the ownership grant so transfers remove it from the old owner.
        if let Some(grant) = data.policy.grants.iter_mut().find(|g| {
            g.owner
                && g.scope == scope
                && matches!(&g.subject, Subject::Account(id) if id == &room.owner_id)
        }) {
            grant.permissions.insert(Action::ColorAny.name().to_owned());
        } else {
            grants.push(Grant {
                owner: true,
                subject: Subject::Account(room.owner_id.clone()),
                scope,
                permissions: [Action::ColorAny.name().to_owned()].into(),
            });
        }
    }
    for grant in &mut data.policy.grants {
        grant.permissions.remove(Action::ColorOwn.name());
    }
    data.policy.grants.retain(|g| !g.permissions.is_empty());
    data.policy.grants.extend(grants);
    data.policy.revision = data.policy.revision.wrapping_add(1);
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn legacy(path: &Path, version: u32) {
        let mut e = Engine::open(path).unwrap();
        for (name, admin) in [("alice", true), ("bob", true), ("eve", false)] {
            e.provision(name, "hash".into(), admin, false).unwrap();
        }
        e.execute(Some("alice"), None, "/room new team").unwrap();
        e.execute(Some("alice"), Some("team"), "/room add bob")
            .unwrap();
        e.execute(Some("alice"), None, "/room new hidden").unwrap();
        e.set_color("bob", None, Some("blue")).unwrap();
        e.execute(Some("alice"), Some("team"), "/user set bob color red")
            .unwrap();
        e.execute(Some("bob"), Some("team"), "retained").unwrap();
        e.mark_read("bob", "team", 1).unwrap();
        // Simulate v18 policy: bundled color rights and an explicit global grant.
        for g in &mut e.data.policy.grants {
            g.permissions.remove(Action::ColorOwn.name());
            g.permissions.remove(Action::ColorAny.name());
        }
        e.data.policy.grants.retain(|g| !g.permissions.is_empty());
        e.data.policy.grants.push(Grant {
            owner: false,
            subject: Subject::Account(e.data.users["bob"].id.clone()),
            scope: Scope::Server,
            permissions: [Action::ColorOwn.name().to_owned()].into(),
        });
        let db = e.store.db.lock().unwrap();
        write_state(&db, &e.data).unwrap();
        if version == 17 {
            db.execute_batch("DROP TRIGGER retention_insert; DROP TRIGGER retention_delete; DROP TRIGGER retention_move; DROP TABLE retention; DROP TABLE auth_events;").unwrap();
        }
        db.pragma_update(None, "user_version", version).unwrap();
    }

    #[test]
    fn upgrade_converts_global_colors_once_without_cross_room_or_root_access() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite");
        legacy(&path, 18);
        let mut e = Engine::open(&path).unwrap();
        let team = e.data.rooms["team"].id.clone();
        let hidden = e.data.rooms["hidden"].id.clone();
        for user in ["alice", "bob", "eve"] {
            for action in [Action::ColorOwn, Action::ColorAny] {
                assert!(!e.allows(Some(user), &Scope::Server, action));
                assert!(!e.allows(
                    Some(user),
                    &Scope::Account(e.data.users[user].id.clone()),
                    action
                ));
            }
        }
        assert!(e.allows(Some("alice"), &Scope::Room(team.clone()), Action::ColorAny));
        assert!(e.allows(Some("bob"), &Scope::Room(team.clone()), Action::ColorOwn));
        assert!(!e.allows(Some("bob"), &Scope::Room(hidden), Action::ColorOwn));
        assert_eq!(e.data.users["bob"].metadata["@global"]["color"], "blue");
        assert_eq!(e.data.users["bob"].metadata[&team]["color"], "red");
        assert_eq!(
            e.history("bob", "team").unwrap().messages[0].text,
            "retained"
        );
        assert_eq!(e.unreads("bob").unwrap()["team"].count, 0);
        e.execute(
            Some("alice"),
            Some("team"),
            "/revoke bob #team w:account.color.own",
        )
        .unwrap();
        drop(e);
        let mut e = Engine::open(&path).unwrap();
        assert!(e.set_color("bob", Some(&team), Some("pink")).is_err());
        e.set_color("bob", None, Some("pink")).unwrap();
        e.execute(Some("alice"), Some("team"), "/room add bob")
            .unwrap();
        e.execute(None, None, "/grant bob admin #team").unwrap();
        assert!(e.set_color("bob", Some(&team), None).is_err());
        let before = e.data.clone();
        drop(e);
        let mut e = Engine::open(&path).unwrap();
        assert!(e.data == before);
        e.execute(Some("bob"), None, "/user rename bobby").unwrap();
        e.execute(Some("alice"), None, "/room rename #team renamed")
            .unwrap();
        assert!(e.set_color("bobby", Some(&team), Some("teal")).is_err());
        e.execute(
            Some("alice"),
            Some("renamed"),
            "/grant bobby #renamed w:account.color.own",
        )
        .unwrap();
        e.set_color("bobby", Some(&team), Some("teal")).unwrap();
    }

    #[test]
    fn failed_color_upgrade_preserves_policy_and_version_including_v17_upgrade() {
        for version in [17, 18] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("chat.sqlite");
            legacy(&path, version);
            let db = Connection::open(&path).unwrap();
            let before: String = db
                .query_row("SELECT json FROM state", [], |r| r.get(0))
                .unwrap();
            db.execute_batch("CREATE TRIGGER reject_color_upgrade BEFORE UPDATE ON state BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
            assert!(Engine::open(&path).is_err());
            assert_eq!(
                db.query_row::<u32, _, _>("PRAGMA user_version", [], |r| r.get(0))
                    .unwrap(),
                version
            );
            assert_eq!(
                db.query_row::<String, _, _>("SELECT json FROM state", [], |r| r.get(0))
                    .unwrap(),
                before
            );
            if version == 17 {
                assert_eq!(db.query_row::<usize,_,_>("SELECT count(*) FROM sqlite_master WHERE name IN ('retention','auth_events')", [], |r|r.get(0)).unwrap(), 0);
            }
            db.execute_batch("DROP TRIGGER reject_color_upgrade;")
                .unwrap();
            drop(db);
            let e = Engine::open(&path).unwrap();
            assert_eq!(
                e.store
                    .db
                    .lock()
                    .unwrap()
                    .query_row::<u32, _, _>("PRAGMA user_version", [], |r| r.get(0))
                    .unwrap(),
                19
            );
        }
    }
}
