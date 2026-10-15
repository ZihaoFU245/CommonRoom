use super::*;
use crate::bots::{
    auth::Context,
    discovery::{Command, Control, Runtime},
};
fn context(app: &App, token: &str) -> Context {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {token}").parse().unwrap(),
    );
    Context::authenticate(app, &headers).unwrap()
}
fn catalogue() -> Vec<Command> {
    vec![Command {
        name: "/model".into(),
        usage: "/model".into(),
        description: "Display model settings".into(),
        subcommands: Vec::new(),
        operator_only: false,
    }]
}
fn publish(app: &App, context: &Context, runtime: &mut Runtime) -> String {
    runtime.control(app, context, Control::Ready).unwrap();
    let request = runtime.next().unwrap();
    let id = request["id"].as_str().unwrap().to_owned();
    let conversation = request["conversation_id"].as_str().unwrap().to_owned();
    runtime
        .control(
            app,
            context,
            Control::Response {
                id,
                commands: catalogue(),
            },
        )
        .unwrap();
    conversation
}
#[test]
fn catalogue_lifecycle_is_session_owned_and_rechecks_credentials_and_members() {
    let (app, token) = fixture();
    let context = context(&app, &token);
    let mut runtime = Runtime::default();
    let conversation = publish(&app, &context, &mut runtime);
    let visible = app
        .bot_runtimes
        .2
        .visible(&app.engine_read().unwrap(), "admin", &conversation)
        .unwrap();
    assert_eq!(visible[0]["username"], "helper");
    assert_eq!(visible[0]["commands"][0]["name"], "/model");
    assert!(
        Runtime::default()
            .control(&app, &context, Control::Ready)
            .is_err()
    );
    runtime.control(&app, &context, Control::Ready).unwrap();
    assert!(
        runtime.next().is_none(),
        "repeated registration does not requery"
    );
    {
        let mut engine = app.engine().unwrap();
        engine
            .provision("reader", "hash".into(), false, false)
            .unwrap();
        engine
            .execute(None, None, "/grant reader support r:message.read")
            .unwrap();
        engine
            .execute(None, None, "/grant reader support w:message.create")
            .unwrap();
        assert!(
            app.bot_runtimes
                .2
                .visible(&engine, "reader", &conversation)
                .unwrap()
                .is_empty(),
            "member-list privacy is preserved"
        );
        let token_id = context.credential.clone();
        engine
            .execute(None, None, &format!("/bot token rm helper {token_id}"))
            .unwrap();
        assert!(
            app.bot_runtimes
                .2
                .visible(&engine, "admin", &conversation)
                .unwrap()
                .is_empty(),
            "revocation hides cached data before socket teardown"
        );
    }
    drop(runtime);
    assert!(
        app.bot_runtimes
            .2
            .visible(&app.engine_read().unwrap(), "admin", &conversation)
            .unwrap()
            .is_empty()
    );
}
#[test]
fn changed_catalogues_cancel_old_requests_and_responses_cannot_inject_targets() {
    let (app, token) = fixture();
    let context = context(&app, &token);
    let mut runtime = Runtime::default();
    let conversation = publish(&app, &context, &mut runtime);
    runtime.control(&app, &context, Control::Changed).unwrap();
    assert!(
        app.bot_runtimes
            .2
            .visible(&app.engine_read().unwrap(), "admin", &conversation)
            .unwrap()
            .is_empty()
    );
    let stale = runtime.next().unwrap()["id"].as_str().unwrap().to_owned();
    runtime.control(&app, &context, Control::Changed).unwrap();
    assert!(
        runtime
            .control(
                &app,
                &context,
                Control::Response {
                    id: stale,
                    commands: catalogue()
                }
            )
            .is_err()
    );
    assert!(
        runtime
            .control(
                &app,
                &context,
                Control::Response {
                    id: "unsolicited".into(),
                    commands: catalogue()
                }
            )
            .is_err()
    );
    assert!(
        serde_json::from_value::<Control>(
            json!({"kind":"commands_response","id":"x","conversation_id":"secret","commands":[]})
        )
        .is_err()
    );
}
#[test]
fn command_catalogues_enforce_names_usage_duplicates_and_byte_limits() {
    use crate::bots::discovery::validate;
    assert!(validate(&catalogue()).is_ok());
    assert!(validate(&[]).is_ok());
    assert!(validate(&[catalogue(), catalogue()].concat()).is_err());
    assert!(validate(&vec![catalogue()[0].clone(); 65]).is_err());
    for command in [
        Command {
            name: "/Bad".into(),
            ..catalogue().remove(0)
        },
        Command {
            usage: "/other".into(),
            ..catalogue().remove(0)
        },
        Command {
            description: "line\nbreak".into(),
            ..catalogue().remove(0)
        },
        Command {
            description: "字".repeat(200),
            ..catalogue().remove(0)
        },
    ] {
        assert!(validate(&[command]).is_err());
    }
}
#[test]
fn subcommands_validate_full_paths_depth_siblings_and_total_nodes() {
    use crate::bots::discovery::validate;
    let command: Command = serde_json::from_value(json!({
        "name":"/history", "usage":"/history [mode]", "description":"Read history",
        "subcommands":[{"name":"before", "usage":"/history before <sequence>", "description":"Older messages"}]
    })).unwrap();
    assert!(validate(std::slice::from_ref(&command)).is_ok());
    let mut invalid = command.clone();
    invalid.subcommands.push(invalid.subcommands[0].clone());
    assert!(validate(&[invalid]).is_err());
    for (name, usage) in [
        ("/before", "/history /before"),
        ("before", "/other before"),
        ("Before", "/history Before"),
    ] {
        let mut invalid = command.clone();
        invalid.subcommands[0].name = name.into();
        invalid.subcommands[0].usage = usage.into();
        assert!(validate(&[invalid]).is_err());
    }
    let mut nested = command.clone();
    let child = &mut nested.subcommands[0];
    child.subcommands = vec![Command {
        name: "page".into(),
        usage: "/history before page".into(),
        description: "Third level".into(),
        subcommands: vec![],
        operator_only: false,
    }];
    assert!(validate(std::slice::from_ref(&nested)).is_ok());
    nested.subcommands[0].subcommands[0].subcommands = vec![Command {
        name: "extra".into(),
        usage: "/history before page extra".into(),
        description: "Fourth level".into(),
        subcommands: vec![],
        operator_only: false,
    }];
    assert!(validate(&[nested]).is_err());
    let mut wide = command;
    wide.subcommands = (0..63)
        .map(|n| Command {
            name: format!("mode{n}"),
            usage: format!("/history mode{n}"),
            description: "Mode".into(),
            subcommands: vec![],
            operator_only: false,
        })
        .collect();
    assert!(validate(std::slice::from_ref(&wide)).is_ok());
    assert!(validate(&[vec![wide], catalogue()].concat()).is_err());
}
#[tokio::test]
async fn scoped_runtime_discovery_does_not_require_configuration_or_operator_grants() {
    let (app, token) = fixture();
    let operator = context(&app, &token);
    let conversation = conversation(&app);
    let worker = app
        .engine()
        .unwrap()
        .issue_bot_credential(
            &operator,
            crate::bots::models::CredentialInput {
                label: "commands".into(),
                expires: crate::bots::now() + 600,
                conversations: [conversation.clone()].into(),
                operations: ["read".into(), "send".into()].into(),
            },
        )
        .unwrap();
    let worker = context(&app, worker["token"].as_str().unwrap());
    let mut runtime = Runtime::default();
    runtime
        .apply(&app, &worker, Some(Control::Ready))
        .await
        .unwrap();
    let request = runtime.next().unwrap();
    assert_eq!(request["conversation_id"], conversation);
    runtime
        .apply(
            &app,
            &worker,
            Some(Control::Response {
                id: request["id"].as_str().unwrap().into(),
                commands: catalogue(),
            }),
        )
        .await
        .unwrap();
    assert_eq!(
        app.bot_runtimes
            .2
            .visible(&app.engine_read().unwrap(), "admin", &conversation)
            .unwrap()
            .len(),
        1
    );
    assert!(
        runtime.next().is_none(),
        "no query leaks outside the credential scope"
    );
}
#[tokio::test]
async fn browser_catalogues_are_session_authenticated_and_conversation_authorized() {
    let (app, token) = fixture();
    let context = context(&app, &token);
    let mut runtime = Runtime::default();
    let conversation = publish(&app, &context, &mut runtime);
    let hash = app.engine_read().unwrap().data.users["admin"].hash.clone();
    app.engine()
        .unwrap()
        .login("catalogue-browser".into(), "admin", &hash, None)
        .unwrap();
    for (cookie, id, expected) in [
        (None, conversation.as_str(), StatusCode::UNAUTHORIZED),
        (
            Some("chat_session=catalogue-browser"),
            conversation.as_str(),
            StatusCode::OK,
        ),
        (
            Some("chat_session=catalogue-browser"),
            "missing",
            StatusCode::FORBIDDEN,
        ),
    ] {
        let mut request = Request::builder().uri(format!("/api/bot-commands?conversation_id={id}"));
        request = request.header("sec-fetch-site", "same-origin");
        if let Some(cookie) = cookie {
            request = request.header(header::COOKIE, cookie);
        }
        let response = web::router(app.clone())
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        if expected == StatusCode::OK {
            let value: serde_json::Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap())
                    .unwrap();
            assert_eq!(value["bots"][0]["commands"][0]["name"], "/model");
        }
    }
}

#[test]
fn private_catalogues_are_visible_only_to_authorized_participants() {
    let (app, token) = fixture();
    let context = context(&app, &token);
    let id = {
        let mut engine = app.engine().unwrap();
        engine
            .execute(Some("admin"), None, "/tell helper hello")
            .unwrap();
        for grant in ["r:message.read", "w:message.create", "r:member.list"] {
            engine
                .execute(
                    None,
                    None,
                    &format!("/grant helper @private:admin:helper {grant}"),
                )
                .unwrap();
        }
        engine
            .provision("outsider", "hash".into(), false, false)
            .unwrap();
        engine.data.private.values().next().unwrap().id.clone()
    };
    let mut runtime = Runtime::default();
    runtime.control(&app, &context, Control::Ready).unwrap();
    while let Some(request) = runtime.next() {
        runtime
            .control(
                &app,
                &context,
                Control::Response {
                    id: request["id"].as_str().unwrap().into(),
                    commands: catalogue(),
                },
            )
            .unwrap();
    }
    let engine = app.engine_read().unwrap();
    assert_eq!(
        app.bot_runtimes
            .2
            .visible(&engine, "admin", &id)
            .unwrap()
            .len(),
        1
    );
    assert!(
        app.bot_runtimes
            .2
            .visible(&engine, "outsider", &id)
            .is_err()
    );
}

#[test]
fn operator_catalogues_follow_current_bot_owner_and_admin_membership() {
    let (app, token) = fixture();
    let context = context(&app, &token);
    {
        let mut engine = app.engine().unwrap();
        for name in ["bob", "carol"] {
            engine.provision(name, "hash".into(), false, false).unwrap();
            engine
                .execute(None, None, &format!("/add {name} support"))
                .unwrap();
        }
        engine.execute(None, None, "/bot owner helper bob").unwrap();
    }
    let mut runtime = Runtime::default();
    runtime.control(&app, &context, Control::Ready).unwrap();
    let request = runtime.next().unwrap();
    let id = request["conversation_id"].as_str().unwrap();
    let mut commands = catalogue();
    commands[0].operator_only = true;
    runtime
        .control(
            &app,
            &context,
            Control::Response {
                id: request["id"].as_str().unwrap().into(),
                commands,
            },
        )
        .unwrap();
    {
        let engine = app.engine_read().unwrap();
        assert_eq!(
            app.bot_runtimes
                .2
                .visible(&engine, "admin", id)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            app.bot_runtimes
                .2
                .visible(&engine, "bob", id)
                .unwrap()
                .len(),
            1
        );
        assert!(
            app.bot_runtimes
                .2
                .visible(&engine, "carol", id)
                .unwrap()
                .is_empty()
        );
    }
    app.engine()
        .unwrap()
        .execute(None, None, "/bot owner helper carol")
        .unwrap();
    let engine = app.engine_read().unwrap();
    assert!(
        app.bot_runtimes
            .2
            .visible(&engine, "bob", id)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        app.bot_runtimes
            .2
            .visible(&engine, "carol", id)
            .unwrap()
            .len(),
        1
    );
}
