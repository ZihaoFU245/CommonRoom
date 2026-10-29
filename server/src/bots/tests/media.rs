use super::*;

#[tokio::test]
async fn bearer_authentication_is_confined_to_bot_routes_including_base_paths() {
    let (mut app, token) = fixture();
    app.engine()
        .unwrap()
        .login("browser".into(), "admin", "hash", None)
        .unwrap();
    for base in ["/", "/chat/"] {
        Arc::make_mut(&mut app.config).base_url = base.into();
        for (method, path) in [
            ("GET", "api/me"),
            ("GET", "api/session"),
            ("GET", "api/history?view=support"),
            ("POST", "api/read"),
            ("POST", "api/login"),
            ("POST", "api/logout"),
            ("POST", "api/color"),
            ("POST", "api/avatars"),
            ("POST", "api/avatars/uploads"),
            ("POST", "api/avatars/uploads/missing/complete"),
            ("GET", "api/avatars/missing/info"),
            ("GET", "api/avatars/missing/file"),
            ("GET", "api/media/config"),
            ("POST", "api/media/uploads"),
            ("POST", "api/media/uploads/missing/complete"),
            ("POST", "api/media/uploads/missing/abandon"),
            ("GET", "api/media/missing/info"),
            ("GET", "api/media/missing/file"),
            ("GET", "api/bot-commands?conversation_id=missing"),
            ("GET", "ws"),
            ("GET", "api/health"),
            ("GET", "future-browser-feature"),
        ] {
            // Even an allowed Origin and a valid session must not turn bearer
            // credentials into browser authentication or fall back to the cookie.
            let response = web::router(app.clone())
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(format!("{base}{path}"))
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .header(header::COOKIE, "chat_session=browser")
                        .header(header::ORIGIN, "http://localhost:5173")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from("{}"))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {base}{path}"
            );
        }
        assert_eq!(
            request(
                &app,
                Some(&token),
                "GET",
                &format!("{base}api/bot/v1/me"),
                json!({})
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
                &format!("{base}api/bot/v1/media/config"),
                json!({})
            )
            .await
            .0,
            StatusCode::OK
        );
        let response = web::router(app.clone())
            .oneshot(
                Request::builder()
                    .uri(format!("{base}api/me"))
                    .header(header::COOKIE, "chat_session=browser")
                    .header(header::ORIGIN, "http://localhost:5173")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}

#[tokio::test]
async fn media_profile_routes_require_machine_credentials_and_explicit_operations() {
    let (app, _) = fixture();
    app.engine()
        .unwrap()
        .login("browser-profile".into(), "admin", "hash", None)
        .unwrap();
    let limited = scoped_worker(&app, "limited-media", &[&conversation(&app)], &["activity"]).await;
    let issuer = issuer(&app);
    for (method, path, body) in [
        ("GET", "/media/config", json!({})),
        (
            "POST",
            "/media/uploads",
            json!({"conversation_id":conversation(&app),"filename":"a.png","size":8}),
        ),
        ("POST", "/media/uploads/missing/complete", json!({})),
        ("POST", "/media/uploads/missing/abandon", json!({})),
        ("GET", "/media/missing/info", json!({})),
        ("GET", "/media/missing/file", json!({})),
        ("POST", "/avatars", json!({"url":null})),
        ("POST", "/avatars/uploads", json!({"size":8})),
        ("POST", "/avatars/uploads/missing/complete", json!({})),
        ("GET", "/avatars/missing/info", json!({})),
        ("GET", "/avatars/missing/file", json!({})),
        ("POST", "/color", json!({"color":"blue"})),
    ] {
        let path = format!("/api/bot/v1{path}");
        let cookie_only = web::router(app.clone())
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(&path)
                    .header(header::COOKIE, "chat_session=browser-profile")
                    .header(header::ORIGIN, "http://localhost:5173")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cookie_only.status(), StatusCode::UNAUTHORIZED, "{path}");
        for (secret, expected) in [
            (None, StatusCode::UNAUTHORIZED),
            (Some(limited.as_str()), StatusCode::FORBIDDEN),
            (Some(issuer.as_str()), StatusCode::FORBIDDEN),
        ] {
            let (status, error) = request(&app, secret, method, &path, body.clone()).await;
            assert_eq!(status, expected, "{path}: {error}");
            assert_eq!(
                error["error"]["code"],
                if secret.is_some() {
                    "forbidden"
                } else {
                    "unauthorized"
                }
            );
        }
    }
}

async fn redirect(app: &App, token: &str, path: &str) -> (StatusCode, String) {
    let response = web::router(app.clone())
        .oneshot(
            Request::builder()
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["bot-api-version"], "1");
    let status = response.status();
    let location = response
        .headers()
        .get(header::LOCATION)
        .map(|value| value.to_str().unwrap().to_owned())
        .unwrap_or_default();
    (status, location)
}

#[tokio::test]
async fn bot_media_lifecycle_preserves_grants_ownership_and_download_authorization() {
    let (mut app, token) = fixture();
    let (storage, server) = web::media::tests::storage_server().await;
    app.media = Some(storage);
    let id = conversation(&app);
    let body = json!({"conversation_id":id,"filename":"photo.png","size":8});
    let (status, error) = request(
        &app,
        Some(&token),
        "POST",
        "/api/bot/v1/media/uploads",
        body.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{error}");
    assert_eq!(error["error"]["code"], "forbidden");
    app.engine()
        .unwrap()
        .execute(None, None, "/grant helper #support w:media.upload")
        .unwrap();
    let (status, allocation) = request(
        &app,
        Some(&token),
        "POST",
        "/api/bot/v1/media/uploads",
        body.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{allocation}");
    assert_eq!(allocation["upload"]["method"], "PUT");
    let media = allocation["media_id"].as_str().unwrap();
    let other = {
        let mut engine = app.engine().unwrap();
        engine
            .execute(None, Some("support"), "/bot new other")
            .unwrap();
        test_credentials(&mut engine, "other", "owner-check").1
    };
    for action in ["complete", "abandon"] {
        let (status, error) = request(
            &app,
            Some(&other),
            "POST",
            &format!("/api/bot/v1/media/uploads/{media}/{action}"),
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{error}");
    }
    let complete = format!("/api/bot/v1/media/uploads/{media}/complete");
    let (status, completed) = request(&app, Some(&token), "POST", &complete, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{completed}");
    let file = format!("/api/bot/v1/media/{media}/file");
    assert_eq!(redirect(&app, &token, &file).await.0, StatusCode::NOT_FOUND);
    let (status, sent) = send(
        &app,
        &token,
        "media-attachment",
        json!({
            "conversation_id":id,"text":format!("![photo]({})",completed["path"].as_str().unwrap())
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{sent}");
    let info = format!("/api/bot/v1/media/{media}/info");
    let (status, metadata) = request(&app, Some(&token), "GET", &info, json!({})).await;
    assert_eq!(status, StatusCode::OK, "{metadata}");
    assert_eq!(metadata["content_type"], "image/png");
    let (status, location) = redirect(&app, &token, &file).await;
    assert_eq!(status, StatusCode::TEMPORARY_REDIRECT);
    assert!(location.contains("X-Amz-Signature="));
    assert!(location.contains("X-Amz-Expires="));
    assert_eq!(
        request(
            &app,
            Some(&token),
            "GET",
            &format!("{info}?unexpected=true"),
            json!({})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    // Unattached reservations can be abandoned through the machine API.
    let (_, pending) = request(
        &app,
        Some(&token),
        "POST",
        "/api/bot/v1/media/uploads",
        body,
    )
    .await;
    assert_eq!(
        request(
            &app,
            Some(&token),
            "POST",
            &format!(
                "/api/bot/v1/media/uploads/{}/abandon",
                pending["media_id"].as_str().unwrap()
            ),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/room kick helper #support")
        .unwrap();
    assert_eq!(redirect(&app, &token, &file).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        request(&app, Some(&token), "GET", &info, json!({})).await.0,
        StatusCode::NOT_FOUND
    );
    server.abort();
}

#[tokio::test]
async fn bot_avatar_and_color_routes_preserve_self_service_and_revocation() {
    let (mut app, token) = fixture();
    let (storage, server) = web::media::tests::storage_server().await;
    app.media = Some(storage);
    app.engine()
        .unwrap()
        .set_avatar_hosts(["example.com".into()].into());
    let (status, value) = request(
        &app,
        Some(&token),
        "POST",
        "/api/bot/v1/avatars",
        json!({"url":"https://example.com/avatar.png"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(
        request(
            &app,
            Some(&token),
            "POST",
            "/api/bot/v1/avatars",
            json!({"url":"https://denied.example/avatar.png"})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &app,
            Some(&token),
            "POST",
            "/api/bot/v1/color",
            json!({"color":"blue"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        app.engine_read().unwrap().data.users["helper"].metadata["@global"]["color"],
        "blue"
    );
    let (_, allocated) = request(
        &app,
        Some(&token),
        "POST",
        "/api/bot/v1/avatars/uploads",
        json!({"size":8}),
    )
    .await;
    let media = allocated["media_id"].as_str().unwrap();
    let (status, completed) = request(
        &app,
        Some(&token),
        "POST",
        &format!("/api/bot/v1/avatars/uploads/{media}/complete"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{completed}");
    let info = format!("/api/bot/v1/avatars/{media}/info");
    let file = format!("/api/bot/v1/avatars/{media}/file");
    assert_eq!(
        request(&app, Some(&token), "GET", &info, json!({})).await.0,
        StatusCode::OK
    );
    assert_eq!(
        redirect(&app, &token, &file).await.0,
        StatusCode::TEMPORARY_REDIRECT
    );
    let context = context(&app, &token);
    app.engine()
        .unwrap()
        .execute(
            None,
            None,
            &format!("/bot token rm helper {}", context.credential),
        )
        .unwrap();
    assert_eq!(
        request(&app, Some(&token), "GET", &info, json!({})).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            Some(&token),
            "POST",
            "/api/bot/v1/color",
            json!({"color":"red"})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    server.abort();
}

async fn upload(app: &App, token: &str, room: &str) -> String {
    let (status, result) = request(
        app,
        Some(token),
        "POST",
        "/api/bot/v1/media/uploads",
        json!({"conversation_id":room,"filename":"image.png","size":8}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    result["media_id"].as_str().unwrap().to_owned()
}

async fn attach(app: &App, token: &str, room: &str, media: &str) {
    let (status, result) = request(
        app,
        Some(token),
        "POST",
        &format!("/api/bot/v1/media/uploads/{media}/complete"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let (status, result) = send(
        app,
        token,
        media,
        json!({"conversation_id":room,
        "text":format!("![image](/api/media/{media}/file)")}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
}

#[tokio::test]
async fn worker_media_access_intersects_operations_conversations_and_account_grants() {
    let (mut app, account) = fixture();
    let (storage, server) = web::media::tests::storage_server().await;
    app.media = Some(storage);
    let a = conversation(&app);
    let b = {
        let mut engine = app.engine().unwrap();
        engine
            .execute(None, None, "/room add helper #secret")
            .unwrap();
        engine
            .execute(None, None, "/grant helper user #secret")
            .unwrap();
        engine
            .execute(None, None, "/grant helper #secret w:media.upload")
            .unwrap();
        engine.data.rooms["secret"].id.clone()
    };
    app.engine()
        .unwrap()
        .execute(None, None, "/grant helper #support w:media.upload")
        .unwrap();
    let scoped = scoped_worker(&app, "upload-a", &[&a], &["read", "send", "media_upload"]).await;
    let read_only = scoped_worker(&app, "read-a", &[&a], &["read"]).await;
    let (status, _) = request(&app, Some(&issuer(&app)), "POST", "/api/bot/v1/credentials",
        json!({"label":"upload-no-send","expires":crate::bots::now()+600,"conversations":[a],"operations":["read","media_upload"]})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let empty = scoped_worker(&app, "empty", &[], &["read", "send", "media_upload"]).await;
    app.engine()
        .unwrap()
        .execute(None, None, "/revoke helper #support w:media.upload")
        .unwrap();
    let payload = json!({"conversation_id":a,"filename":"image.png","size":8});
    // Token operations cannot grant the account an upload permission it lacks.
    assert_eq!(
        request(
            &app,
            Some(&scoped),
            "POST",
            "/api/bot/v1/media/uploads",
            payload.clone()
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/grant helper #support w:media.upload")
        .unwrap();
    for token in [&read_only, &empty] {
        assert_eq!(
            request(
                &app,
                Some(token),
                "POST",
                "/api/bot/v1/media/uploads",
                payload.clone()
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        request(
            &app,
            Some(&scoped),
            "POST",
            "/api/bot/v1/media/uploads",
            json!({"conversation_id":b,"filename":"image.png","size":8})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            Some(&read_only),
            "GET",
            "/api/bot/v1/media/config",
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    let media_a = upload(&app, &scoped, &a).await;
    attach(&app, &scoped, &a, &media_a).await;
    let file_a = format!("/api/bot/v1/media/{media_a}/file");
    assert_eq!(
        redirect(&app, &read_only, &file_a).await.0,
        StatusCode::TEMPORARY_REDIRECT
    );
    assert_eq!(
        request(
            &app,
            Some(&read_only),
            "GET",
            &format!("/api/bot/v1/media/{media_a}/info"),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        redirect(&app, &empty, &file_a).await.0,
        StatusCode::FORBIDDEN
    );
    // Even an upload owned by the same account is inaccessible outside the token scope.
    let pending_b = upload(&app, &account, &b).await;
    for action in ["complete", "abandon"] {
        assert_eq!(
            request(
                &app,
                Some(&scoped),
                "POST",
                &format!("/api/bot/v1/media/uploads/{pending_b}/{action}"),
                json!({})
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    attach(&app, &account, &b, &pending_b).await;
    for action in ["info", "file"] {
        assert_eq!(
            request(
                &app,
                Some(&read_only),
                "GET",
                &format!("/api/bot/v1/media/{pending_b}/{action}"),
                json!({})
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
    }
    let abandoned = upload(&app, &scoped, &a).await;
    assert_eq!(
        request(
            &app,
            Some(&scoped),
            "POST",
            &format!("/api/bot/v1/media/uploads/{abandoned}/abandon"),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/revoke helper #support w:media.upload")
        .unwrap();
    assert_eq!(
        request(
            &app,
            Some(&scoped),
            "POST",
            "/api/bot/v1/media/uploads",
            payload
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/room kick helper #support")
        .unwrap();
    assert_eq!(
        redirect(&app, &read_only, &file_a).await.0,
        StatusCode::NOT_FOUND
    );
    server.abort();
}

#[tokio::test]
async fn worker_profile_is_explicit_and_room_colors_remain_scoped() {
    let (mut app, _) = fixture();
    let (storage, server) = web::media::tests::storage_server().await;
    app.media = Some(storage);
    let a = conversation(&app);
    let b = {
        let mut engine = app.engine().unwrap();
        engine
            .execute(None, None, "/room add helper #secret")
            .unwrap();
        engine
            .execute(None, None, "/grant helper user #secret")
            .unwrap();
        engine.set_avatar_hosts(["example.com".into()].into());
        engine.data.rooms["secret"].id.clone()
    };
    let profile = scoped_worker(&app, "profile-a", &[&a], &["profile", "read"]).await;
    let reader = scoped_worker(&app, "profile-reader", &[&a], &["read"]).await;
    assert_eq!(
        request(
            &app,
            Some(&profile),
            "POST",
            "/api/bot/v1/color",
            json!({"color":"blue","room":a})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/grant helper #support w:account.color.own")
        .unwrap();
    assert_eq!(
        request(
            &app,
            Some(&reader),
            "POST",
            "/api/bot/v1/avatars",
            json!({"url":null})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            Some(&profile),
            "POST",
            "/api/bot/v1/avatars",
            json!({"url":"https://example.com/avatar.png"})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            Some(&profile),
            "POST",
            "/api/bot/v1/color",
            json!({"color":"blue","room":a})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            Some(&profile),
            "POST",
            "/api/bot/v1/color",
            json!({"color":"red","room":b})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            Some(&profile),
            "POST",
            "/api/bot/v1/color",
            json!({"color":"blue"})
        )
        .await
        .0,
        StatusCode::OK
    );
    let (status, allocated) = request(
        &app,
        Some(&profile),
        "POST",
        "/api/bot/v1/avatars/uploads",
        json!({"size":8}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{allocated}");
    let id = allocated["media_id"].as_str().unwrap();
    assert_eq!(
        request(
            &app,
            Some(&profile),
            "POST",
            &format!("/api/bot/v1/avatars/uploads/{id}/complete"),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        redirect(&app, &reader, &format!("/api/bot/v1/avatars/{id}/file"))
            .await
            .0,
        StatusCode::TEMPORARY_REDIRECT
    );
    // Expiry is rechecked on every service call, not just issuance or connection.
    let context = context(&app, &profile);
    app.engine()
        .unwrap()
        .data
        .bots
        .get_mut(&context.account)
        .unwrap()
        .tokens
        .get_mut(&context.credential)
        .unwrap()
        .expires = Some(crate::bots::now());
    assert_eq!(
        request(
            &app,
            Some(&profile),
            "POST",
            "/api/bot/v1/color",
            json!({"color":null})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    server.abort();
}

#[tokio::test]
async fn worker_avatar_reads_do_not_inherit_account_directory_or_other_conversations() {
    let (mut app, account) = fixture();
    let (storage, server) = web::media::tests::storage_server().await;
    app.media = Some(storage);
    let a = conversation(&app);
    {
        let mut engine = app.engine().unwrap();
        engine
            .execute(None, None, "/room add helper #secret")
            .unwrap();
        engine
            .execute(None, None, "/grant helper user #secret")
            .unwrap();
        engine
            .execute(None, None, "/bot new outsider #secret")
            .unwrap();
    }
    let outsider = test_credentials(&mut app.engine().unwrap(), "outsider", "avatar").1;
    let reader = scoped_worker(&app, "scoped-avatar", &[&a], &["read"]).await;
    let (status, allocated) = request(
        &app,
        Some(&outsider),
        "POST",
        "/api/bot/v1/avatars/uploads",
        json!({"size":8}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{allocated}");
    let id = allocated["media_id"].as_str().unwrap();
    assert_eq!(
        request(
            &app,
            Some(&outsider),
            "POST",
            &format!("/api/bot/v1/avatars/uploads/{id}/complete"),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    let file = format!("/api/bot/v1/avatars/{id}/file");
    assert_eq!(
        redirect(&app, &account, &file).await.0,
        StatusCode::TEMPORARY_REDIRECT
    );
    assert_eq!(
        redirect(&app, &reader, &file).await.0,
        StatusCode::NOT_FOUND
    );
    // A global account-directory grant must not escape the worker's room scope.
    app.engine()
        .unwrap()
        .execute(None, None, "/grant helper @global r:account.list")
        .unwrap();
    assert_eq!(
        redirect(&app, &reader, &file).await.0,
        StatusCode::NOT_FOUND
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/room add outsider #support")
        .unwrap();
    assert_eq!(
        redirect(&app, &reader, &file).await.0,
        StatusCode::TEMPORARY_REDIRECT
    );
    app.engine()
        .unwrap()
        .execute(None, None, "/room kick outsider #support")
        .unwrap();
    assert_eq!(
        redirect(&app, &reader, &file).await.0,
        StatusCode::NOT_FOUND
    );
    server.abort();
}

#[tokio::test]
async fn worker_media_and_profile_operations_survive_restart_without_broadening_legacy_tokens() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("chat.sqlite");
    let (app, account) = fixture_with_engine(Engine::open(&path).unwrap());
    let id = conversation(&app);
    app.engine()
        .unwrap()
        .execute(None, None, "/grant helper #support w:media.upload")
        .unwrap();
    let new = scoped_worker(
        &app,
        "new-operations",
        &[&id],
        &["read", "send", "media_upload", "profile"],
    )
    .await;
    let legacy = scoped_worker(&app, "legacy-operations", &[&id], &["read", "send"]).await;
    drop(app);
    let engine = Engine::open(&path).unwrap();
    for (token, allowed) in [(&new, true), (&legacy, false), (&account, true)] {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        let context = crate::bots::auth::Context::from_headers(&engine, &headers).unwrap();
        for operation in ["media_upload", "profile"] {
            assert_eq!(
                context
                    .permits(
                        &engine,
                        operation,
                        crate::bots::auth::Scope::Conversation(&id)
                    )
                    .is_ok(),
                allowed
            );
        }
    }
}
