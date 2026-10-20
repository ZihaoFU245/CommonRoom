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
    assert!(catalogue["commands"].as_array().unwrap().iter().any(|command| command["name"] == "/room" && command["subcommands"][1]["name"] == "new"));
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
        engine.execute(None, None, "/add bob #support").unwrap();
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

async fn keyed_command(app: &App, token: &str, key: &str, value: Value) -> (StatusCode, Value) {
    let response = web::router(app.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/bot/v1/commands")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header("idempotency-key", key)
                .body(Body::from(value.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}
#[tokio::test]
async fn command_receipts_resolve_lost_responses_conflicts_failures_and_access_changes() {
    let (app, token) = fixture();
    let id = conversation(&app);
    app.engine()
        .unwrap()
        .execute(None, None, "/grant helper admin")
        .unwrap();
    let input = json!({"conversation_id":id,"command":"/new room once"});
    let (first, second) = tokio::join!(
        keyed_command(&app, &token, "once", input.clone()),
        keyed_command(&app, &token, "once", input.clone())
    );
    assert_eq!(first.0, StatusCode::OK);
    assert_eq!(second.0, StatusCode::OK);
    assert_ne!(first.1["replayed"], second.1["replayed"]);
    assert_eq!(first.1["result"], second.1["result"]);
    assert_eq!(
        app.engine_read()
            .unwrap()
            .data
            .rooms
            .keys()
            .filter(|name| name.as_str() == "once")
            .count(),
        1
    );
    let receipt = request(
        &app,
        Some(&token),
        "GET",
        "/api/bot/v1/commands/receipts/once",
        json!({}),
    )
    .await;
    assert_eq!(receipt.1["state"], "succeeded");
    assert_eq!(
        keyed_command(
            &app,
            &token,
            "once",
            json!({"conversation_id":id,"command":"/new room different"})
        )
        .await
        .1["error"]["code"],
        "idempotency_conflict"
    );
    // Removing the source conversation cannot turn a replay into a new effect.
    app.engine()
        .unwrap()
        .execute(None, None, "/rm room support")
        .unwrap();
    assert_eq!(
        keyed_command(&app, &token, "once", input).await.0,
        StatusCode::OK
    );
    assert_eq!(
        keyed_command(
            &app,
            &token,
            "failed",
            json!({"conversation_id":id,"command":"/new room denied"})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(
            &app,
            Some(&token),
            "GET",
            "/api/bot/v1/commands/receipts/failed",
            json!({})
        )
        .await
        .1["state"],
        "failed"
    );
    assert_eq!(
        request(
            &app,
            Some(&token),
            "GET",
            "/api/bot/v1/commands/receipts/never",
            json!({})
        )
        .await
        .1["state"],
        "not_found"
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/disable helper")
        .unwrap();
    assert_eq!(
        request(
            &app,
            Some(&token),
            "GET",
            "/api/bot/v1/commands/receipts/once",
            json!({})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}
#[tokio::test]
async fn history_loads_a_referenced_message_page_without_cross_conversation_disclosure() {
    let (app, token) = fixture();
    let id = conversation(&app);
    let source;
    let foreign_source;
    {
        let mut engine = app.engine().unwrap();
        for index in 0..12 {
            engine
                .execute(Some("admin"), Some("support"), &format!("retained {index}"))
                .unwrap();
        }
        source = engine
            .history_store
            .page(
                &id,
                engine.data.rooms["support"].revision,
                crate::engine::HistoryRange {
                    limit: Some(100),
                    ..Default::default()
                },
                "",
            )
            .unwrap()
            .messages[0]
            .id
            .clone();
        engine.execute(None, None, "/new room foreign").unwrap();
        engine
            .execute(None, Some("foreign"), "private to foreign")
            .unwrap();
        foreign_source = engine
            .history_store
            .page(
                &engine.data.rooms["foreign"].id,
                engine.data.rooms["foreign"].revision,
                crate::engine::HistoryRange::default(),
                "",
            )
            .unwrap()
            .messages[0]
            .id
            .clone();
    }
    let (_, page) = request(
        &app,
        Some(&token),
        "GET",
        &format!("/api/bot/v1/history?conversation_id={id}&message_id={source}&limit=3"),
        json!({}),
    )
    .await;
    assert!(
        page["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["id"] == source)
    );
    assert_eq!(page["has_after"], true);
    assert_eq!(
        request(
            &app,
            Some(&token),
            "GET",
            &format!("/api/bot/v1/history?conversation_id={id}&message_id={foreign_source}"),
            json!({})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(
            &app,
            Some(&token),
            "GET",
            &format!("/api/bot/v1/history?conversation_id={id}&message_id={source}&after=1"),
            json!({})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}
