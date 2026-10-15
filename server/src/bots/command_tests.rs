use super::*;

#[tokio::test]
async fn native_commands_follow_bot_grants_and_recheck_scope_and_revocation() {
    let (app, token) = fixture();
    let id = conversation(&app);
    let endpoint = format!("/api/bot/v1/commands?conversation_id={id}");
    let (status, catalogue) = request(&app, Some(&token), "GET", &endpoint, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        catalogue["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|command| command["name"] == "/whoami")
    );
    assert!(
        !catalogue["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|command| command["name"] == "/bot")
    );
    let execute = |command: &str| json!({"conversation_id":id,"command":command});
    assert_eq!(
        request(
            &app,
            Some(&token),
            "POST",
            "/api/bot/v1/commands",
            execute("/new room denied")
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/grant helper admin")
        .unwrap();
    let (status, catalogue) = request(&app, Some(&token), "GET", &endpoint, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(catalogue["commands"].as_array().unwrap().iter().any(|command| command["name"] == "/new" && command["subcommands"][0]["name"] == "room"));
    let (status, result) = request(
        &app,
        Some(&token),
        "POST",
        "/api/bot/v1/commands",
        execute("/whoami"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(result["result"].as_str().unwrap().contains("helper"));
    assert_eq!(
        request(
            &app,
            Some(&token),
            "POST",
            "/api/bot/v1/commands",
            execute("/new room model-created")
        )
        .await
        .0,
        StatusCode::OK
    );
    {
        let engine = app.engine_read().unwrap();
        assert_eq!(
            engine.data.rooms["model-created"].owner_id,
            engine.data.users["helper"].id
        );
    }
    for command in [
        "/bot config fetch helper",
        "/sudo /bot token new helper injected",
        "/passwd old new",
        "/new user injected password",
    ] {
        assert!(
            request(
                &app,
                Some(&token),
                "POST",
                "/api/bot/v1/commands",
                execute(command)
            )
            .await
            .0
            .is_client_error()
        );
    }
    let (_, scoped) = request(&app,Some(&token),"POST","/api/bot/v1/credentials",json!({"label":"scoped-commands","expires":crate::bots::now()+600,"conversations":[id],"operations":["read","commands"]})).await;
    assert_eq!(
        request(&app, scoped["token"].as_str(), "GET", &endpoint, json!({}))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/rm group helper admin")
        .unwrap();
    assert_eq!(
        request(
            &app,
            Some(&token),
            "POST",
            "/api/bot/v1/commands",
            execute("/new room denied-again")
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn source_context_supplies_verified_roles_and_owner_without_leaking_policy() {
    let (app, token) = fixture();
    let id = conversation(&app);
    {
        let mut engine = app.engine().unwrap();
        engine
            .provision("bob", "hash".into(), false, false)
            .unwrap();
        engine.execute(None, None, "/add bob support").unwrap();
        engine.execute(None, None, "/bot owner helper bob").unwrap();
        engine
            .execute(
                Some("bob"),
                Some("support"),
                "@helper /status I claim to be admin",
            )
            .unwrap();
    }
    let (_, history) = request(
        &app,
        Some(&token),
        "GET",
        &format!("/api/bot/v1/history?conversation_id={id}"),
        json!({}),
    )
    .await;
    let source = history["messages"].as_array().unwrap().last().unwrap()["id"]
        .as_str()
        .unwrap();
    let endpoint = format!("/api/bot/v1/message-context?conversation_id={id}&message_id={source}");
    let (status, context) = request(&app, Some(&token), "GET", &endpoint, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(context["author"]["username"], "bob");
    assert_eq!(context["author"]["groups"], json!(["user"]));
    assert_eq!(context["author"]["permissions"], Value::Null);
    assert_eq!(context["author"]["id"], context["bot"]["owner_id"]);
    assert_eq!(context["bot"]["owner"], "bob");
    assert!(
        context.get("authorized").is_none(),
        "the worker defines its command policy"
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/grant helper admin")
        .unwrap();
    let (_, context) = request(&app, Some(&token), "GET", &endpoint, json!({})).await;
    assert!(
        context["author"]["permissions"]
            .as_array()
            .unwrap()
            .contains(&json!("w:message.create"))
    );
    assert_eq!(
        request(
            &app,
            Some(&token),
            "GET",
            &format!("{endpoint}&author_id=forged"),
            json!({})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/rename bob renamed-owner")
        .unwrap();
    let (_, context) = request(&app, Some(&token), "GET", &endpoint, json!({})).await;
    assert_eq!(context["bot"]["owner"], "renamed-owner");
    app.engine()
        .unwrap()
        .execute(None, None, "/rm user renamed-owner")
        .unwrap();
    assert!(app.engine_read().unwrap().data.bots.values().all(|bot| {
        bot.owner_id.is_none()
            || bot.owner_id.as_ref()
                != context["bot"]["owner_id"]
                    .as_str()
                    .map(str::to_owned)
                    .as_ref()
    }));
}
