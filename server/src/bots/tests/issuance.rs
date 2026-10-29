use super::*;

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
        json!({"label":"too-long","expires":crate::bots::now()+86401,"conversations":[id],"operations":["read"]}),
        json!({"label":"empty-ops","expires":expires,"conversations":[id],"operations":[]}),
    ] {
        let result = request(&app, Some(&issuer), "POST", "/api/bot/v1/credentials", body).await;
        assert!(result.0.is_client_error(), "{result:?}");
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
