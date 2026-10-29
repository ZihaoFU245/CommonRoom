use super::*;

#[tokio::test]
async fn system_catalogue_and_markdown_results_require_su() {
    let (app, token) = fixture();
    let id = conversation(&app);
    let endpoint = format!("/api/bot/v1/commands?conversation_id={id}");
    let execute = |command: &str| json!({"conversation_id":id,"command":command});
    app.engine()
        .unwrap()
        .execute(None, None, "/grant helper admin")
        .unwrap();
    app.engine()
        .unwrap()
        .execute(None, None, "/grant helper @global /system")
        .unwrap();
    let (_, catalogue) = request(&app, Some(&token), "GET", &endpoint, json!({})).await;
    assert!(
        !catalogue["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|command| command["name"] == "/system")
    );
    assert_eq!(
        request(
            &app,
            Some(&token),
            "POST",
            "/api/bot/v1/commands",
            execute("/system metrics")
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/grant helper su")
        .unwrap();
    let (_, catalogue) = request(&app, Some(&token), "GET", &endpoint, json!({})).await;
    let system = catalogue["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|command| command["name"] == "/system")
        .unwrap();
    assert_eq!(system["subcommands"][0]["name"], "configs");
    assert_eq!(system["subcommands"][1]["name"], "metrics");
    for (command, title) in [
        ("/system configs", "# System configuration"),
        ("/system metrics", "# System metrics"),
    ] {
        let (status, result) = keyed_command(
            &app,
            &token,
            if command.ends_with("configs") {
                "system-configs"
            } else {
                "system-metrics"
            },
            execute(command),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{result}");
        assert!(result["result"].as_str().unwrap().starts_with(title));
    }
    app.engine()
        .unwrap()
        .execute(None, None, "/revoke helper su")
        .unwrap();
    for (key, command) in [
        ("system-configs", "/system configs"),
        ("system-metrics", "/system metrics"),
    ] {
        assert_eq!(
            keyed_command(&app, &token, key, execute(command)).await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(
                &app,
                Some(&token),
                "GET",
                &format!("/api/bot/v1/commands/receipts/{key}"),
                json!({})
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }

    for permission in ["/sudo", "x:command.sudo"] {
        app.engine()
            .unwrap()
            .execute(None, None, &format!("/grant helper @global {permission}"))
            .unwrap();
    }
    // Delegated sudo does not silently authorize replay of a plain su command.
    assert_eq!(
        keyed_command(&app, &token, "system-metrics", execute("/system metrics"))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        keyed_command(
            &app,
            &token,
            "sudo-system",
            execute("/sudo /system metrics")
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            Some(&token),
            "GET",
            "/api/bot/v1/commands/receipts/sudo-system",
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/revoke helper @global x:command.sudo")
        .unwrap();
    assert_eq!(
        keyed_command(
            &app,
            &token,
            "sudo-system",
            execute("/sudo /system metrics")
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            Some(&token),
            "GET",
            "/api/bot/v1/commands/receipts/sudo-system",
            json!({})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );

    assert_eq!(
        request(
            &app,
            Some(&token),
            "POST",
            "/api/bot/v1/commands",
            execute("/system metrics")
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

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
            .history_store()
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
            .history_store()
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

#[tokio::test]
async fn native_command_failures_preserve_domain_classes_and_hide_storage_details() {
    let (app, token) = fixture();
    let id = conversation(&app);
    let execute = |command: &str| json!({"conversation_id":id,"command":command});
    let (status, value) = request(
        &app,
        Some(&token),
        "POST",
        "/api/bot/v1/commands",
        execute("/room new denied"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(value["error"]["code"], "forbidden");
    assert_eq!(value["error"]["retryable"], false);
    app.engine()
        .unwrap()
        .execute(None, None, "/grant helper admin")
        .unwrap();
    let (status, value) = request(
        &app,
        Some(&token),
        "POST",
        "/api/bot/v1/commands",
        execute("/room new"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(value["error"]["code"], "invalid_request");
    let (status, value) = request(
        &app,
        Some(&token),
        "POST",
        "/api/bot/v1/commands",
        execute("/room new support"),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(value["error"]["code"], "conflict");
    let store = app.engine_read().unwrap().history_store().clone();
    store.read(|db| {
        db.execute_batch("PRAGMA query_only=OFF; CREATE TRIGGER reject_command_room BEFORE INSERT ON entities WHEN NEW.kind='room' BEGIN SELECT RAISE(ABORT,'private database implementation detail'); END; PRAGMA query_only=ON;")?;
        Ok(())
    }).unwrap();
    let (status, value) = request(
        &app,
        Some(&token),
        "POST",
        "/api/bot/v1/commands",
        execute("/room new failed-write"),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(value["error"]["code"], "unavailable");
    assert_eq!(value["error"]["retryable"], true);
    assert!(
        !value
            .to_string()
            .contains("private database implementation detail")
    );
    assert!(
        !app.engine_read()
            .unwrap()
            .data
            .rooms
            .contains_key("failed-write")
    );
    store
        .read(|db| {
            db.execute_batch(
                "PRAGMA query_only=OFF; DROP TRIGGER reject_command_room; PRAGMA query_only=ON;",
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        request(
            &app,
            Some(&token),
            "POST",
            "/api/bot/v1/commands",
            execute("/room new recovered")
        )
        .await
        .0,
        StatusCode::OK
    );
}

async fn atomic_command_contract(
    engine: Engine,
    db: crate::engine::database::Connection,
    postgres: bool,
) {
    let (app, token) = fixture_with_engine(engine);
    app.engine()
        .unwrap()
        .execute(None, None, "/grant helper su")
        .unwrap();
    let id = conversation(&app);
    let seed = send(
        &app,
        &token,
        "atomic-source",
        json!({"conversation_id":id,"text":"retained source"}),
    )
    .await;
    assert_eq!(seed.0, StatusCode::OK);
    let source = seed.1["message"]["id"].as_str().unwrap();
    db.execute_batch(if postgres {
        "CREATE FUNCTION reject_native_receipt() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'private receipt failure'; END $$; CREATE TRIGGER reject_native_receipt BEFORE INSERT ON bot_command_receipts FOR EACH ROW EXECUTE FUNCTION reject_native_receipt();"
    } else {
        "CREATE TRIGGER reject_native_receipt BEFORE INSERT ON bot_command_receipts BEGIN SELECT RAISE(ABORT,'private receipt failure'); END;"
    }).unwrap();
    for command in [
        "/room new atomic-room".into(),
        "/tell admin new private must roll back".into(),
        format!("/reply {source} new reply must roll back"),
        format!("/edit {source} edited body must roll back"),
        format!("/react {source} 👍"),
        format!("/retract {source}"),
        "/clean -1 #support".into(),
        "/user rename helper renamed-helper".into(),
    ] {
        let before = app.engine_read().unwrap().data.clone();
        let revision = app.engine_read().unwrap().revision;
        let mut notices = app.changes.subscribe();
        let result = keyed_command(
            &app,
            &token,
            "failed-atomic",
            json!({"conversation_id":id,"command":command}),
        )
        .await;
        assert_eq!(
            result.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "{command}: {result:?}"
        );
        assert!(!result.1.to_string().contains("private receipt failure"));
        let engine = app.engine_read().unwrap();
        assert!(engine.data == before, "{command}");
        assert_eq!(engine.revision, revision, "{command}");
        let history = engine.history("admin", "support").unwrap();
        assert_eq!(history.messages.len(), 1, "{command}");
        assert_eq!(history.messages[0].text, "retained source", "{command}");
        assert!(history.messages[0].reactions.is_empty());
        assert!(notices.try_recv().is_err(), "{command}");
        assert_eq!(
            db.query_row("SELECT sequence FROM counters WHERE id=1", [], |row| row
                .get::<_, u64>(0))
                .unwrap(),
            before.next_sequence
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM bot_command_receipts", [], |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
            0
        );
    }
    db.execute_batch(if postgres { "DROP TRIGGER reject_native_receipt ON bot_command_receipts; DROP FUNCTION reject_native_receipt();" } else { "DROP TRIGGER reject_native_receipt;" }).unwrap();
    let input = json!({"conversation_id":id,"command":format!("/reply {source} durable reply")});
    let first = keyed_command(&app, &token, "durable-command", input.clone()).await;
    assert_eq!(first.0, StatusCode::OK, "{first:?}");
    assert_eq!(first.1["replayed"], false);
    let replay = keyed_command(&app, &token, "durable-command", input).await;
    assert_eq!(replay.0, StatusCode::OK);
    assert_eq!(replay.1["replayed"], true);
    let engine = app.engine_read().unwrap();
    assert_eq!(
        engine.history("admin", "support").unwrap().messages.len(),
        2
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM bot_command_receipts", [], |row| row
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        1
    );
}

async fn restart_command_receipt(mut engine: Engine) {
    let account = engine.account_id("helper").unwrap().to_owned();
    let receipts = engine.command_receipts();
    let (digest, stored) = receipts
        .lookup(&account, "durable-command")
        .await
        .unwrap()
        .unwrap();
    let sequence = engine.data.next_sequence;
    let committed = receipts
        .commit(
            &account,
            "durable-command",
            &digest,
            Ok("must not replace stored outcome".into()),
            crate::engine::PendingCommand::None,
            None,
        )
        .await
        .unwrap();
    let (outcome, change) = engine.publish_command(committed).unwrap();
    assert!(outcome.replayed);
    assert_eq!(outcome.result, stored.result);
    assert!(matches!(change, crate::engine::CommandChange::Unchanged));
    assert_eq!(engine.data.next_sequence, sequence);
    assert_eq!(
        engine.history("admin", "support").unwrap().messages.len(),
        2
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_command_receipts_rollback_every_effect_atomically() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("chat.sqlite");
    atomic_command_contract(
        Engine::open(&path).unwrap(),
        crate::engine::database::Connection::open(&path).unwrap(),
        false,
    )
    .await;
    let engine = Engine::open(&path).unwrap();
    assert_eq!(
        engine.history("admin", "support").unwrap().messages.len(),
        2
    );
    assert!(!engine.data.rooms.contains_key("atomic-room"));
    assert!(engine.data.private.is_empty());
    restart_command_receipt(engine).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_native_command_receipts_rollback_every_effect_atomically() {
    let fixture = crate::engine::database::tests::PostgresFixture::new();
    atomic_command_contract(
        Engine::open_postgres(&fixture.url).unwrap(),
        crate::engine::database::Connection::postgres(&fixture.url, false).unwrap(),
        true,
    )
    .await;
    let engine = Engine::open_postgres(&fixture.url).unwrap();
    assert_eq!(
        engine.history("admin", "support").unwrap().messages.len(),
        2
    );
    assert!(!engine.data.rooms.contains_key("atomic-room"));
    assert!(engine.data.private.is_empty());
    restart_command_receipt(engine).await;
}

async fn command_capacity_contract(
    engine: Engine,
    db: crate::engine::database::Connection,
    postgres: bool,
) {
    use crate::engine::PendingCommand;
    let (app, _) = fixture_with_engine(engine);
    let (account, receipts, one, two) = {
        let mut engine = app.engine().unwrap();
        engine.execute(None, None, "/grant helper su").unwrap();
        (
            engine.account_id("helper").unwrap().to_owned(),
            engine.command_receipts(),
            engine
                .prepare_browser_message("helper", Some("support"), "one committed effect", false)
                .unwrap(),
            engine
                .prepare_browser_message("helper", Some("secret"), "two committed effects", false)
                .unwrap(),
        )
    };
    let (one, two) = tokio::join!(
        receipts.commit(
            &account,
            "same-key",
            "same-digest",
            Ok("same result".into()),
            PendingCommand::Message(Box::new(one)),
            None
        ),
        receipts.commit(
            &account,
            "same-key",
            "same-digest",
            Ok("same result".into()),
            PendingCommand::Message(Box::new(two)),
            None
        ),
    );
    let (a, b) = {
        let mut engine = app.engine().unwrap();
        (
            engine.publish_command(one.unwrap()).unwrap(),
            engine.publish_command(two.unwrap()).unwrap(),
        )
    };
    assert_ne!(a.0.replayed, b.0.replayed);
    assert_eq!(
        db.query_row("SELECT count(*) FROM messages", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let created = crate::bots::now();
    db.execute_batch(&format!("DELETE FROM bot_command_receipts; {}", if postgres {
        format!("INSERT INTO bot_command_receipts SELECT '{account}','old-'||n::text,'old',{created},'{{}}' FROM generate_series(1,4095) AS n")
    } else {
        format!("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<4095) INSERT INTO bot_command_receipts SELECT '{account}','old-'||x,'old',{created},'{{}}' FROM n")
    })).unwrap();
    let (one, two) = {
        let engine = app.engine_read().unwrap();
        (
            engine
                .prepare_browser_message("helper", Some("support"), "capacity winner one", false)
                .unwrap(),
            engine
                .prepare_browser_message("helper", Some("secret"), "capacity winner two", false)
                .unwrap(),
        )
    };
    let (one, two) = tokio::join!(
        receipts.commit(
            &account,
            "capacity-one",
            "one",
            Ok("one".into()),
            PendingCommand::Message(Box::new(one)),
            None
        ),
        receipts.commit(
            &account,
            "capacity-two",
            "two",
            Ok("two".into()),
            PendingCommand::Message(Box::new(two)),
            None
        ),
    );
    assert_ne!(one.is_ok(), two.is_ok());
    for result in [one, two] {
        match result {
            Ok(committed) => {
                app.engine().unwrap().publish_command(committed).unwrap();
            }
            Err(error) => assert_eq!(error.code, Code::IdempotencyCapacity),
        }
    }
    assert_eq!(
        db.query_row("SELECT count(*) FROM messages", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM bot_command_receipts", [], |row| row
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        4096
    );
    assert_eq!(
        db.query_row("SELECT sequence FROM counters WHERE id=1", [], |row| row
            .get::<_, u64>(0))
            .unwrap(),
        2
    );
    db.execute_batch("UPDATE bot_command_receipts SET created=0")
        .unwrap();
    let committed = receipts
        .commit(
            &account,
            "capacity-one",
            "fresh",
            Ok("after expiry".into()),
            PendingCommand::None,
            None,
        )
        .await
        .unwrap();
    assert!(
        !app.engine()
            .unwrap()
            .publish_command(committed)
            .unwrap()
            .0
            .replayed
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM bot_command_receipts", [], |row| row
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        1
    );
    let replay = receipts
        .commit(
            &account,
            "capacity-one",
            "fresh",
            Ok("\0".repeat(16000)),
            PendingCommand::None,
            None,
        )
        .await
        .unwrap();
    assert!(
        app.engine()
            .unwrap()
            .publish_command(replay)
            .unwrap()
            .0
            .replayed
    );
    let rejected = receipts
        .commit(
            &account,
            "escaped-too-large",
            "escaped",
            Ok("\0".repeat(16000)),
            PendingCommand::None,
            None,
        )
        .await;
    assert_eq!(rejected.err().unwrap().code, Code::PayloadTooLarge);
    assert!(
        receipts
            .lookup(&account, "escaped-too-large")
            .await
            .unwrap()
            .is_none()
    );
    let pending = app
        .engine_read()
        .unwrap()
        .prepare_browser_message(
            "helper",
            Some("support"),
            "oversized result must not publish",
            false,
        )
        .unwrap();
    let committed = receipts
        .commit(
            &account,
            "oversized-result",
            "large",
            Ok("x".repeat(32769)),
            PendingCommand::Message(Box::new(pending)),
            None,
        )
        .await
        .unwrap();
    let outcome = app.engine().unwrap().publish_command(committed).unwrap().0;
    assert_eq!(outcome.error.unwrap().code, Code::PayloadTooLarge);
    assert_eq!(
        db.query_row("SELECT count(*) FROM messages", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    let bytes = "界".repeat((4 * 1024 * 1024 - 65536) / 3 + 1);
    db.execute(
        if postgres {
            "UPDATE bot_command_receipts SET outcome=$1"
        } else {
            "UPDATE bot_command_receipts SET outcome=?1"
        },
        [bytes.as_str()],
    )
    .unwrap();
    let result = receipts
        .commit(
            &account,
            "byte-capacity",
            "bytes",
            Ok("no effect".into()),
            PendingCommand::None,
            None,
        )
        .await;
    assert_eq!(result.err().unwrap().code, Code::IdempotencyCapacity);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_command_receipts_serialize_replay_and_capacity_before_effects() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("chat.sqlite");
    command_capacity_contract(
        Engine::open(&path).unwrap(),
        crate::engine::database::Connection::open(&path).unwrap(),
        false,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_native_command_receipts_serialize_replay_and_capacity_before_effects() {
    let fixture = crate::engine::database::tests::PostgresFixture::new();
    command_capacity_contract(
        Engine::open_postgres(&fixture.url).unwrap(),
        crate::engine::database::Connection::postgres(&fixture.url, false).unwrap(),
        true,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_cancelled_native_command_commits_effect_and_receipt_before_disable() {
    for metadata in [false, true] {
        let fixture = crate::engine::database::tests::PostgresFixture::new();
        let (app, token) = fixture_with_engine(Engine::open_postgres(&fixture.url).unwrap());
        app.engine()
            .unwrap()
            .execute(None, None, "/grant helper su")
            .unwrap();
        let id = conversation(&app);
        let blocker = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
        let hold = blocker.unchecked_transaction().unwrap();
        hold.execute_batch("LOCK TABLE bot_command_receipts IN SHARE MODE")
            .unwrap();
        let mut notices = app.changes.subscribe();
        let changing = tokio::spawn({
            let app = app.clone();
            async move {
                keyed_command(&app, &token, "cancelled-command", json!({"conversation_id":id,"command":if metadata { "/room new cancelled-room" } else { "/tell admin cancelled direct" }})).await
            }
        });
        let inspect = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let waiting: bool = inspect.query_row("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'DELETE FROM bot_command_receipts WHERE created%')", [], |row| row.get(0)).unwrap();
            if waiting {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "command did not reach receipt persistence"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(app.engine.try_read().is_ok());
        assert!(notices.try_recv().is_err());
        changing.abort();
        assert!(changing.await.unwrap_err().is_cancelled());
        let disable = tokio::task::spawn_blocking({
            let app = app.clone();
            move || app.engine().unwrap().execute(None, None, "/disable helper")
        });
        hold.rollback().unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), disable)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let engine = app.engine_read().unwrap();
        assert!(!engine.active("helper"));
        if metadata {
            assert!(engine.data.rooms.contains_key("cancelled-room"));
        } else {
            assert_eq!(
                engine.history("admin", "@direct:helper").unwrap().messages[0].text,
                "cancelled direct"
            );
        }
        assert_eq!(
            inspect
                .query_row(
                    "SELECT count(*) FROM bot_command_receipts WHERE key='cancelled-command'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        assert!(matches!(notices.try_recv().unwrap(), Change::Metadata(_)));
        assert!(notices.try_recv().is_err());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
async fn postgres_native_commands_write_independently_and_order_cancelled_publication() {
    let fixture = crate::engine::database::tests::PostgresFixture::new();
    let (app, token) = fixture_with_engine(Engine::open_postgres(&fixture.url).unwrap());
    let (other_token, other_id, first, second) = {
        let mut engine = app.engine().unwrap();
        engine.execute(None, None, "/grant helper su").unwrap();
        engine
            .execute(None, None, "/bot new worker #secret")
            .unwrap();
        engine.execute(None, None, "/grant worker su").unwrap();
        let token = engine
            .execute(None, None, "/bot token new worker independent")
            .unwrap()
            .lines()
            .last()
            .unwrap()
            .to_owned();
        engine
            .execute(Some("helper"), Some("support"), "first source")
            .unwrap();
        engine
            .execute(Some("worker"), Some("secret"), "second source")
            .unwrap();
        (
            token,
            engine.data.rooms["secret"].id.clone(),
            engine.history("admin", "support").unwrap().messages[0]
                .id
                .clone(),
            engine.history("worker", "secret").unwrap().messages[0]
                .id
                .clone(),
        )
    };
    let id = conversation(&app);
    let blocker = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
    let hold = blocker.unchecked_transaction().unwrap();
    hold.query_row(
        "SELECT key FROM entities WHERE kind='room' AND key='support' FOR UPDATE",
        [],
        |row| row.get::<_, String>(0),
    )
    .unwrap();
    let changing = tokio::spawn({
        let app = app.clone();
        async move {
            keyed_command(&app, &token, "blocked-edit", json!({"conversation_id":id,"command":format!("/edit {first} committed after cancel")})).await
        }
    });
    let inspect = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let waiting: bool = inspect.query_row("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT revision,%FROM entities%FOR UPDATE')", [], |row| row.get(0)).unwrap();
        if waiting {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "edit did not reach its row lock"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(app.engine.try_read().is_ok());
    let independent = tokio::time::timeout(std::time::Duration::from_secs(1), keyed_command(&app, &other_token, "independent-edit", json!({"conversation_id":other_id,"command":format!("/edit {second} independent committed edit")}))).await.unwrap();
    assert_eq!(independent.0, StatusCode::OK, "{independent:?}");
    assert_eq!(
        app.engine_read()
            .unwrap()
            .history("worker", "secret")
            .unwrap()
            .messages[0]
            .text,
        "independent committed edit"
    );
    changing.abort();
    assert!(changing.await.unwrap_err().is_cancelled());
    let disable = tokio::task::spawn_blocking({
        let app = app.clone();
        move || app.engine().unwrap().execute(None, None, "/disable helper")
    });
    hold.rollback().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), disable)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        app.engine_read()
            .unwrap()
            .history("admin", "support")
            .unwrap()
            .messages[0]
            .text,
        "committed after cancel"
    );
    assert_eq!(
        inspect
            .query_row("SELECT count(*) FROM bot_command_receipts", [], |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        2
    );
}
