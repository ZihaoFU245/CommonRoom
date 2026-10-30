use super::*;

#[test]
fn user_lists_and_metadata_default_to_current_scope() {
    let mut e = crate::engine::tests::engine();
    e.provision("outside", "hash".into(), false, false).unwrap();
    let room = e.room_scope("lobby").unwrap();
    let key = Engine::metadata_scope_key(&room).unwrap().to_owned();
    assert_eq!(
        e.execute(Some("bob"), Some("lobby"), "/user ls").unwrap(),
        "alice\nbob\neve"
    );
    assert!(
        e.execute(Some("bob"), Some("lobby"), "/user ls @global")
            .unwrap()
            .contains("outside")
    );
    e.execute(None, Some("lobby"), "/user set @all label local")
        .unwrap();
    assert_eq!(e.data.users["bob"].metadata[&key]["label"], "local");
    assert!(e.data.users["outside"].metadata.is_empty());
    let read = e
        .execute(Some("alice"), Some("lobby"), "/user get bob")
        .unwrap();
    assert!(read.contains("local"));
    e.execute(None, Some("lobby"), "/user set bob label global @global")
        .unwrap();
    assert_eq!(e.data.users["bob"].metadata["@global"]["label"], "global");
    assert!(
        e.execute(None, None, "/user get bob")
            .unwrap()
            .contains("global")
    );
}

#[test]
fn grant_and_revoke_keep_explicit_permission_scopes_and_global_group_defaults() {
    let mut e = crate::engine::tests::engine();
    let before = e.data.clone();
    for command in [
        "/grant bob w:message.react",
        "/revoke bob w:message.react",
        "/grant @all /reply",
        "/revoke @all /reply",
    ] {
        assert!(
            e.execute(None, Some("lobby"), command).is_err(),
            "{command}"
        );
        assert!(e.data == before);
    }
    e.execute(None, Some("lobby"), "/grant eve admin").unwrap();
    assert!(e.is_admin("eve"));
    e.execute(None, Some("lobby"), "/revoke eve admin").unwrap();
    assert!(!e.is_admin("eve"));
    e.execute(
        Some("alice"),
        Some("lobby"),
        "/grant bob #lobby w:message.react",
    )
    .unwrap();
    e.execute(
        Some("alice"),
        Some("lobby"),
        "/revoke bob #lobby w:message.react",
    )
    .unwrap();
}

#[test]
fn unsupported_private_defaults_fail_without_falling_back_to_global() {
    let mut e = crate::engine::tests::engine();
    e.execute(Some("bob"), None, "/tell eve hi").unwrap();
    let before = e.data.clone();
    for text in [
        "/user ls",
        "/user get bob",
        "/user set bob label private",
        "/bot ls",
    ] {
        assert!(
            e.execute(Some("alice"), Some("@private:bob:eve"), text)
                .is_err(),
            "{text}"
        );
        assert!(e.data == before);
    }
    assert!(
        e.execute(None, Some("@private:bob:eve"), "/user ls @global")
            .unwrap()
            .contains("bob")
    );
}

#[test]
fn console_bot_defaults_and_explicit_room_scopes_match_other_commands() {
    let mut e = crate::engine::tests::engine();
    e.execute(None, None, "/bot new worldwide").unwrap();
    assert!(!e.data.rooms["lobby"].members.contains("worldwide"));
    assert!(
        e.execute(None, Some("@command"), "/bot ls")
            .unwrap()
            .contains("worldwide")
    );
    e.execute(None, Some("lobby"), "/bot new local").unwrap();
    assert!(e.data.rooms["lobby"].members.contains("local"));
    assert!(
        !e.execute(None, Some("lobby"), "/bot ls")
            .unwrap()
            .contains("worldwide")
    );
    assert!(
        e.execute(None, None, "/bot ls #lobby")
            .unwrap()
            .contains("local")
    );
    assert!(e.execute(None, None, "/clean 7d #lobby").is_ok());
    assert!(e.execute(None, Some("@command"), "/clean 7d").is_err());
    assert!(e.execute(None, Some("lobby"), "/clean 7d @global").is_ok());
}
