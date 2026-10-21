//! HEAD-only cleanup-constraint migration. No permission is added or removed.
use super::*;
use crate::engine::authorization::{Action, Group, Scope, Subject};

pub(super) fn migrate_cleanup_limits(
    data: &mut Data,
    raw: &serde_json::Value,
) -> Result<(), String> {
    let grants = raw
        .pointer("/policy/grants")
        .and_then(|value| value.as_array());
    let Some(grants) = grants else {
        return Ok(());
    };
    for account in data.users.values_mut() {
        let applies = |subject: &Subject| match subject {
            Subject::Account(id) => id == &account.id,
            Subject::Group(group) => data.policy.assignments.iter().any(|assignment| {
                assignment.account_id == account.id
                    && assignment.group == *group
                    && assignment.scope == Scope::Server
            }),
        };
        let mut rules: BTreeMap<Scope, i64> = BTreeMap::new();
        for grant in grants {
            let subject: Subject = serde_json::from_value(grant["subject"].clone())
                .map_err(|error| error.to_string())?;
            let scope: Scope = serde_json::from_value(grant["scope"].clone())
                .map_err(|error| error.to_string())?;
            let clean = grant["permissions"].as_array().is_some_and(|permissions| {
                permissions
                    .iter()
                    .any(|permission| permission.as_str() == Some(Action::Clean.name()))
            });
            if !clean || !applies(&subject) || matches!(scope, Scope::Account(_)) {
                continue;
            }
            let age = match grant.get("minimum_age") {
                Some(value) => value.as_u64().ok_or("Invalid stored cleanup age.")?,
                None => 0,
            };
            let age = i64::try_from(age)
                .map_err(|_| "Stored cleanup age exceeds the supported range.")?;
            rules
                .entry(scope)
                .and_modify(|minimum| *minimum = (*minimum).min(age))
                .or_insert(age);
        }
        let mut roots = Vec::new();
        for (scope, local_age) in &rules {
            // Keep unrestricted role defaults derived, but preserve explicit
            // constraints where the old role did not itself grant cleanup.
            let admin = data.policy.assignments.iter().any(|assignment| {
                assignment.account_id == account.id
                    && (assignment.group == Group::Su || assignment.group == Group::Admin)
                    && (assignment.scope == Scope::Server || &assignment.scope == scope)
            });
            let owner = matches!(scope, Scope::Room(id) if data.rooms.values().any(|room| &room.id == id && room.owner_id == account.id));
            // Legacy grants were a union: any unrestricted grant wins;
            // otherwise the smallest applicable minimum age wins.
            let age = rules
                .get(&Scope::Server)
                .map_or(*local_age, |global| (*global).min(*local_age));
            let role_unrestricted = data.policy.assignments.iter().any(|assignment| {
                assignment.account_id == account.id
                    && ((assignment.group == Group::Su && assignment.scope == Scope::Server)
                        || (assignment.group == Group::Admin
                            && matches!(scope, Scope::Room(_))
                            && &assignment.scope == scope))
            });
            if role_unrestricted || (age == 0 && (admin || owner)) {
                continue;
            }
            let limit = if age == 0 { -1 } else { age };
            if let Scope::Room(id) = scope {
                account
                    .metadata
                    .entry(id.clone())
                    .or_default()
                    .entry("history.clean_min_age".into())
                    .or_insert_with(|| limit.to_string());
            } else {
                roots.push(limit);
            }
        }
        // Private conversations use the global root. When legacy private
        // constraints differ, retain the strictest one rather than widen access.
        if let Some(limit) = roots.into_iter().max() {
            account
                .metadata
                .entry("@global".into())
                .or_default()
                .entry("history.clean_min_age".into())
                .or_insert_with(|| limit.to_string());
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn grant(subject: Subject, scope: Scope, age: u64) -> serde_json::Value {
        json!({"subject": subject, "scope": scope, "permissions": ["x:history.clean"], "minimum_age": age})
    }

    #[test]
    fn migration_preserves_legacy_union_and_expands_group_limits_without_changing_grants() {
        let e = crate::engine::tests::engine();
        let scope = e.room_scope("lobby").unwrap();
        let id = e.data.rooms["lobby"].id.clone();
        let bob = Subject::Account(e.data.users["bob"].id.clone());
        let raw = json!({"policy": {"grants": [
            grant(Subject::Group(Group::User), scope.clone(), 1209600),
            grant(bob.clone(), scope.clone(), 604800),
        ]}});
        let mut data = e.data.clone();
        migrate_cleanup_limits(&mut data, &raw).unwrap();
        assert_eq!(
            data.users["bob"].metadata[&id]["history.clean_min_age"],
            "604800"
        );
        assert_eq!(
            data.users["eve"].metadata[&id]["history.clean_min_age"],
            "1209600"
        );
        assert!(data.users["alice"].metadata.is_empty());
        assert!(data.policy == e.data.policy);
        let mut data = e.data.clone();
        let unrestricted = json!({"policy": {"grants": [
            grant(bob.clone(), scope, 604800), grant(bob, Scope::Server, 0),
        ]}});
        migrate_cleanup_limits(&mut data, &unrestricted).unwrap();
        assert_eq!(
            data.users["bob"].metadata[&id]["history.clean_min_age"],
            "-1"
        );
        assert_eq!(
            data.users["bob"].metadata["@global"]["history.clean_min_age"],
            "-1"
        );
    }

    #[test]
    fn private_constraints_consolidate_conservatively_and_explicit_metadata_wins() {
        let e = crate::engine::tests::engine();
        let bob = Subject::Account(e.data.users["bob"].id.clone());
        let raw = json!({"policy": {"grants": [
            grant(bob.clone(), Scope::Private("first".into()), 604800),
            grant(bob, Scope::Private("second".into()), 1209600),
        ]}});
        let mut data = e.data.clone();
        migrate_cleanup_limits(&mut data, &raw).unwrap();
        assert_eq!(
            data.users["bob"].metadata["@global"]["history.clean_min_age"],
            "1209600"
        );
        assert_eq!(data.users["bob"].metadata.len(), 1);
        let root = data
            .users
            .get_mut("bob")
            .unwrap()
            .metadata
            .get_mut("@global")
            .unwrap();
        root.insert("history.clean_min_age".into(), "0".into());
        migrate_cleanup_limits(&mut data, &raw).unwrap();
        assert_eq!(
            data.users["bob"].metadata["@global"]["history.clean_min_age"],
            "0"
        );
    }

    #[test]
    fn global_admin_role_does_not_discard_an_existing_private_cleanup_constraint() {
        let e = crate::engine::tests::engine();
        let alice = Subject::Account(e.data.users["alice"].id.clone());
        let raw = json!({"policy": {"grants": [
            grant(alice, Scope::Private("pair".into()), 604800),
        ]}});
        let mut data = e.data.clone();
        migrate_cleanup_limits(&mut data, &raw).unwrap();
        assert_eq!(
            data.users["alice"].metadata["@global"]["history.clean_min_age"],
            "604800"
        );
    }
}
