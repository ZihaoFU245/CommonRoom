use super::*;

#[tokio::test]
async fn credential_issuance_reclaims_expired_workers_at_capacity() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("chat.sqlite3");
    let (app, worker) = fixture_with_engine(Engine::open(&path).unwrap());
    let issuer = issuer(&app);
    let conversation = conversation(&app);
    let account = app.engine_read().unwrap().data.users["helper"].id.clone();
    for index in 0..6 {
        scoped_worker(
            &app,
            &format!("rotation-{index}"),
            &[&conversation],
            &["read"],
        )
        .await;
    }
    let body = |label: &str| {
        json!({
            "label": label, "expires": crate::bots::now() + 600,
            "conversations": [&conversation], "operations": ["read"],
        })
    };
    let issue = |input| {
        request(
            &app,
            Some(&issuer),
            "POST",
            "/api/bot/v1/credentials",
            input,
        )
    };
    assert_eq!(issue(body("overflow")).await.0, StatusCode::BAD_REQUEST);

    let expired = {
        let mut engine = app.engine().unwrap();
        let registration = engine.data.bots.get_mut(&account).unwrap();
        registration
            .tokens
            .iter_mut()
            .filter(|(_, credential)| credential.label.starts_with("rotation-"))
            .map(|(id, credential)| {
                credential.expires = Some(crate::bots::now());
                id.clone()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(expired.len(), 6);
    assert_eq!(issue(body("test")).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(
        app.engine_read().unwrap().data.bots[&account].tokens.len(),
        8
    );

    let (status, replacement) = issue(body("rotation-0")).await;
    assert_eq!(status, StatusCode::OK, "{replacement}");
    for secret in [worker.as_str(), replacement["token"].as_str().unwrap()] {
        assert_eq!(
            request(&app, Some(secret), "GET", "/api/bot/v1/me", json!({}))
                .await
                .0,
            StatusCode::OK
        );
    }
    drop(app);
    let reopened = Engine::open(&path).unwrap();
    let tokens = &reopened.data.bots[&account].tokens;
    assert_eq!(tokens.len(), 3);
    assert!(expired.iter().all(|id| !tokens.contains_key(id)));
    assert!(tokens.contains_key(replacement["id"].as_str().unwrap()));
}

#[test]
fn issuer_creation_can_reuse_an_expired_worker_label() {
    let (app, _) = fixture();
    let mut engine = app.engine().unwrap();
    let account = engine.data.users["helper"].id.clone();
    let tokens = &mut engine.data.bots.get_mut(&account).unwrap().tokens;
    let (id, worker) = tokens.iter_mut().find(|(_, c)| c.label == "test").unwrap();
    let id = id.clone();
    worker.expires = Some(crate::bots::now());

    engine
        .execute(None, None, "/bot token new helper test")
        .unwrap();
    let tokens = &engine.data.bots[&account].tokens;
    assert!(!tokens.contains_key(&id));
    assert!(tokens.values().any(|credential| {
        credential.label == "test" && credential.kind == models::CredentialKind::Issuer
    }));
}

#[tokio::test]
async fn issuer_only_mints_workers_and_checks_live_account_permissions() {
    let (app, worker) = fixture();
    let issuer = issuer(&app);
    let id = conversation(&app);
    let expires = crate::bots::now() + 600;
    for (method, path, body) in [
        ("GET", "/api/bot/v1/me", json!({})),
        ("GET", "/api/bot/v1/schema", json!({})),
        ("GET", "/api/bot/v1/ws", json!({})),
        ("GET", "/api/bot/v1/commands", json!({})),
        (
            "POST",
            "/api/bot/v1/messages",
            json!({"conversation_id":id,"text":"denied"}),
        ),
        (
            "POST",
            "/api/bot/v1/conversations",
            json!({"kind":"room","name":"denied"}),
        ),
    ] {
        let response = request(&app, Some(&issuer), method, path, body).await;
        assert!(response.0.is_client_error(), "{path}: {response:?}");
    }
    let before = app.engine_read().unwrap().data.bots.clone();
    for body in [
        json!({"label":"denied-upload","expires":expires,"conversations":[id],"operations":["read","send","media_upload"]}),
        json!({"label":"denied-create","expires":expires,"conversations":null,"operations":["create"]}),
        json!({"label":"denied-scope","expires":expires,"conversations":[app.engine_read().unwrap().data.rooms["secret"].id],"operations":["read"]}),
    ] {
        let result = request(&app, Some(&issuer), "POST", "/api/bot/v1/credentials", body).await;
        assert_eq!(result.0, StatusCode::FORBIDDEN, "{result:?}");
    }
    assert!(app.engine_read().unwrap().data.bots == before);
    for body in [
        json!({"label":"missing-expiry","conversations":[id],"operations":["read"]}),
        json!({"label":"missing-scope","expires":expires,"operations":["read"]}),
        json!({"label":"past","expires":crate::bots::now(),"conversations":[id],"operations":["read"]}),
        // The server validates against its own clock after the request is scheduled.
        json!({"label":"too-long","expires":crate::bots::now()+2*86400,"conversations":[id],"operations":["read"]}),
        json!({"label":"empty-ops","expires":expires,"conversations":[id],"operations":[]}),
    ] {
        let label = body["label"].clone();
        let (status, _) =
            request(&app, Some(&issuer), "POST", "/api/bot/v1/credentials", body).await;
        assert!(status.is_client_error(), "{label}: {status}");
    }
    let body = json!({"label":"account-worker","expires":expires,"conversations":null,"operations":["read","send"]});
    assert_eq!(
        request(
            &app,
            Some(&worker),
            "POST",
            "/api/bot/v1/credentials",
            body.clone()
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let issued = request(&app, Some(&issuer), "POST", "/api/bot/v1/credentials", body).await;
    assert_eq!(issued.0, StatusCode::OK, "{issued:?}");
    assert_eq!(issued.1["expires"], expires);
    let token = issued.1["token"].as_str().unwrap();
    assert_eq!(
        request(&app, Some(token), "GET", "/api/bot/v1/me", json!({}))
            .await
            .1["credential"]["kind"],
        "worker"
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/bot rm helper @global")
        .unwrap();
    assert!(app.engine_read().unwrap().data.bots.is_empty());
    for secret in [&issuer, &worker, token] {
        assert_eq!(request(&app, Some(secret), "POST", "/api/bot/v1/credentials", json!({"label":"deleted","expires":expires,"conversations":null,"operations":["read"]})).await.0, StatusCode::UNAUTHORIZED);
    }
}
