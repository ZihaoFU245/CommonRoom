use super::*;
use crate::{engine::session_digest, push::Subscription};
use axum::{extract::State, http::HeaderMap};

pub(super) async fn status(
    State(app): State<App>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_read_origin(&app, &headers)?;
    let identity = auth::authenticated_identity(&app, &headers).await?;
    let digest = session_digest(auth::token(&app.config, &headers).ok_or_else(unauthorized)?);
    app.inspect_engine(move |engine, _| {
        identity.verify(engine)?;
        Ok(Json(json!({
            "public_key": engine.push.as_ref().and_then(|push| push.public_key.as_ref()),
            "installed": engine.push_installed(&digest),
        })))
    })
    .await
}

fn unauthorized() -> ApiError {
    error(StatusCode::UNAUTHORIZED, "Please log in.")
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Update {
    subscription: Option<Subscription>,
}

pub(super) async fn update(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<Update>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_origin(&app, &headers)?;
    let identity = auth::authenticated_identity(&app, &headers).await?;
    let digest = session_digest(auth::token(&app.config, &headers).ok_or_else(unauthorized)?);
    if input
        .subscription
        .as_ref()
        .is_some_and(|subscription| !subscription.valid())
    {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "Invalid or unsupported Web Push subscription.",
        ));
    }
    let permit = app.io_permit()?;
    admission::spawn_io(permit, async move {
        let ordering = app.mutations.metadata().await?;
        let pending = app
            .with_metadata(&ordering, move |engine| -> Result<_, ApiError> {
                identity.verify(engine)?;
                if input.subscription.is_some()
                    && engine
                        .push
                        .as_ref()
                        .and_then(|push| push.public_key.as_ref())
                        .is_none()
                {
                    return Err(error(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "Background notifications are disabled on this server.",
                    ));
                }
                Ok(engine.prepare_push_subscription(&digest, input.subscription, None))
            })
            .await?;
        if let Some(pending) = pending {
            let pending = pending.save().await?;
            app.with_metadata(&ordering, move |engine| engine.publish_sessions(pending))
                .await?;
        }
        Ok(Json(json!({"ok":true})))
    })
    .await?
}

pub(crate) async fn clear_gone(
    app: &App,
    digest: String,
    subscription: Subscription,
) -> Result<(), crate::engine::EngineError> {
    let io = app.io_permit()?;
    let app = app.clone();
    admission::spawn_io(io, async move {
        let ordering = app.mutations.metadata().await?;
        let pending = app
            .with_metadata(
                &ordering,
                move |engine| -> Result<_, crate::engine::EngineError> {
                    Ok(engine.prepare_push_subscription(&digest, None, Some(&subscription)))
                },
            )
            .await?;
        if let Some(pending) = pending {
            let pending = pending.save().await?;
            app.with_metadata(&ordering, move |engine| engine.publish_sessions(pending))
                .await?;
        }
        Ok(())
    })
    .await
    .map_err(crate::engine::EngineError::unavailable)?
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::web::media::tests::{app, request};

    const PATH: &str = "/api/notifications/push";

    fn subscription(endpoint: &str) -> serde_json::Value {
        json!({
            "endpoint":endpoint,
            "keys":{
                "p256dh":"BLn9b-VR0ca83knDNZ32dCHGyjJp-1riX9ZTN40MqV8K_LpQmLqxC_DoHvqvFXO_nGdAB4W9dogZb_sM-uV4JbY",
                "auth":"_ordMnz7uTCmrpBTeUV4Bw"
            }
        })
    }

    #[tokio::test]
    async fn web_push_subscription_is_session_scoped_validated_and_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.sqlite");
        let app = app(Engine::open(&path).unwrap());
        assert_eq!(
            request(&app, "bob", "GET", PATH, json!(null)).await.1,
            json!({"public_key":null,"installed":false})
        );
        let (runtime, _) = crate::push::Runtime::new(Some("test-key".into()));
        app.engine().unwrap().push = Some(runtime);
        let valid = subscription("https://fcm.googleapis.com/fcm/send/test");
        for endpoint in [
            "http://fcm.googleapis.com/token",
            "https://fcm.googleapis.com.example.com/token",
            "https://127.0.0.1/token",
            "https://bob@fcm.googleapis.com/token",
            "https://jmt17.google.com/other",
            "https://jmt17.google.com/fcm/send/",
            "https://jmt17.google.com.evil.example/fcm/send/test",
        ] {
            assert_eq!(
                request(
                    &app,
                    "bob",
                    "POST",
                    PATH,
                    json!({"subscription":subscription(endpoint)})
                )
                .await
                .0,
                StatusCode::BAD_REQUEST
            );
        }
        assert_eq!(
            request(&app, "unknown", "POST", PATH, json!({"subscription":valid}))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(
                &app,
                "bob",
                "POST",
                PATH,
                json!({"subscription":subscription("https://jmt17.google.com/fcm/send/test")})
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(
            request(&app, "bob", "POST", PATH, json!({"subscription":valid}))
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(
            request(&app, "alice", "GET", PATH, json!(null)).await.1["installed"],
            false
        );
        assert_eq!(
            request(&app, "bob", "GET", PATH, json!(null)).await.1,
            json!({"public_key":"test-key","installed":true})
        );
        drop(app);

        let mut reopened = Engine::open(&path).unwrap();
        assert!(reopened.push_installed(&session_digest("bob")));
        reopened.logout("bob").unwrap();
        assert!(!reopened.push_installed(&session_digest("bob")));
    }

    #[tokio::test]
    async fn web_push_delivery_obeys_conversation_policies_and_live_session() {
        let app = app(Engine::open(std::path::Path::new(":memory:")).unwrap());
        let (runtime, mut events) = crate::push::Runtime::new(Some("test-key".into()));
        {
            let mut engine = app.engine().unwrap();
            engine.push = Some(runtime.clone());
            engine
                .execute(Some("alice"), None, "/room new lobby")
                .unwrap();
            engine
                .execute(Some("alice"), None, "/room add bob #lobby")
                .unwrap();
            engine
                .execute(None, Some("lobby"), "/bot new helper")
                .unwrap();
        }
        let digest = session_digest("bob");
        let valid = subscription("https://web.push.apple.com/test");
        assert_eq!(
            request(&app, "bob", "POST", PATH, json!({"subscription":valid}))
                .await
                .0,
            StatusCode::OK
        );
        let send_room = |body: &str| {
            app.engine()
                .unwrap()
                .execute(Some("alice"), Some("lobby"), body)
                .unwrap();
        };
        let eligible = |event: &crate::push::Event| {
            app.engine_read()
                .unwrap()
                .push_delivery(&digest, event)
                .map(|(_, view)| view)
        };
        {
            let mut engine = app.engine().unwrap();
            engine
                .execute(Some("bob"), Some("lobby"), "original without a mention")
                .unwrap();
        }
        let original = events.try_recv().unwrap();
        send_room(&format!("/reply {} plain reply", original.id));
        let reply = events.try_recv().unwrap();
        assert_eq!(eligible(&reply).as_deref(), Some("lobby"));
        send_room("@all room-wide alert");
        let room_wide = events.try_recv().unwrap();
        assert_eq!(eligible(&room_wide).as_deref(), Some("lobby"));
        send_room("ordinary");
        let event = events.try_recv().unwrap();
        assert_eq!(eligible(&event), None);
        let key = format!("notification.{}", event.conversation);
        let set = |mode| {
            request(
                &app,
                "bob",
                "POST",
                "/api/preferences",
                json!({"values":{(key.clone()):mode}}),
            )
        };
        assert_eq!(set(json!("@all")).await.0, StatusCode::OK);
        assert_eq!(eligible(&room_wide).as_deref(), Some("lobby"));
        assert_eq!(eligible(&event), None);
        assert_eq!(set(json!("mentions_all")).await.0, StatusCode::OK);
        assert_eq!(eligible(&room_wide).as_deref(), Some("lobby"));
        assert_eq!(
            request(
                &app,
                "bob",
                "POST",
                "/api/preferences",
                json!({"values":{(key.clone()):null}})
            )
            .await
            .0,
            StatusCode::OK
        );
        let global = |rule| {
            request(
                &app,
                "bob",
                "POST",
                "/api/preferences",
                json!({"values":{"notification.global":rule}}),
            )
        };
        assert_eq!(
            global(json!("user_is:alice contains_keyword:ordinary"))
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(eligible(&event).as_deref(), Some("lobby"));
        assert_eq!(
            global(json!("none; mentions_me")).await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(eligible(&event).as_deref(), Some("lobby"));
        assert_eq!(global(json!("none")).await.0, StatusCode::OK);
        assert_eq!(eligible(&event), None);
        assert_eq!(set(json!("all")).await.0, StatusCode::OK);
        assert_eq!(eligible(&event).as_deref(), Some("lobby"));
        {
            let _socket = runtime.open_socket(digest.clone()).unwrap();
            assert_eq!(eligible(&event), None);
        }
        assert_eq!(global(json!(null)).await.0, StatusCode::OK);
        assert_eq!(set(json!("mentions")).await.0, StatusCode::OK);
        send_room("hello @bob and @helper");
        let event = events.try_recv().unwrap();
        assert_eq!(eligible(&event).as_deref(), Some("lobby"));
        assert_eq!(set(serde_json::Value::Null).await.0, StatusCode::OK);
        assert_eq!(eligible(&event), None);
        assert!(event.bot_mention);
        assert_eq!(
            request(
                &app,
                "bob",
                "POST",
                "/api/preferences",
                json!({"values":{"notification_exclude_bot_mentions":"false"}}),
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(eligible(&event).as_deref(), Some("lobby"));
        send_room("hello @bob");
        let event = events.try_recv().unwrap();
        assert_eq!(eligible(&event).as_deref(), Some("lobby"));
        assert_eq!(set(json!("off")).await.0, StatusCode::OK);
        assert_eq!(eligible(&event), None);

        app.engine()
            .unwrap()
            .execute(Some("alice"), None, "/tell bob hello")
            .unwrap();
        let dm = events.try_recv().unwrap();
        assert_eq!(eligible(&dm).as_deref(), Some("@direct:alice"));
        assert_eq!(
            request(
                &app,
                "bob",
                "POST",
                "/api/preferences",
                json!({"values":{"notification_default_mentions_me":"false"}})
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(set(json!(null)).await.0, StatusCode::OK);
        assert_eq!(eligible(&event), None);
        assert_eq!(eligible(&dm).as_deref(), Some("@direct:alice"));
        let room_mentions = format!("notification_default_mentions_me.{}", event.conversation);
        let direct = format!("notification_default_direct.{}", dm.conversation);
        let preferences = |values| {
            request(
                &app,
                "bob",
                "POST",
                "/api/preferences",
                json!({"values": values}),
            )
        };
        assert_eq!(
            preferences(json!({(room_mentions.clone()): "true", (direct.clone()): "false"}))
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(eligible(&event).as_deref(), Some("lobby"));
        assert_eq!(eligible(&dm), None);
        assert_eq!(set(json!("none")).await.0, StatusCode::OK);
        assert_eq!(eligible(&event), None);
        assert_eq!(set(json!(null)).await.0, StatusCode::OK);
        assert_eq!(
            preferences(json!({(room_mentions): null, (direct): null}))
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(eligible(&event), None);
        assert_eq!(eligible(&dm).as_deref(), Some("@direct:alice"));
        for (muted, expected) in [("true", None), ("false", Some("@direct:alice"))] {
            assert_eq!(
                request(
                    &app,
                    "bob",
                    "POST",
                    "/api/preferences",
                    json!({"values":{"notifications_muted":muted}})
                )
                .await
                .0,
                StatusCode::OK
            );
            assert!(app.engine_read().unwrap().push_installed(&digest));
            assert_eq!(eligible(&dm).as_deref(), expected);
            assert_eq!(
                app.engine_read()
                    .unwrap()
                    .notification_view("bob", &dm)
                    .as_deref(),
                expected
            );
        }
        let dm_key = format!("notification.{}", dm.conversation);
        for (mode, expected) in [(json!("off"), None), (json!(null), Some("@direct:alice"))] {
            assert_eq!(
                request(
                    &app,
                    "bob",
                    "POST",
                    "/api/preferences",
                    json!({"values":{(dm_key.clone()):mode}}),
                )
                .await
                .0,
                StatusCode::OK
            );
            assert_eq!(eligible(&dm).as_deref(), expected);
        }
        assert_eq!(
            request(&app, "bob", "POST", PATH, json!({"subscription":null}))
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(eligible(&dm), None);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
    async fn postgres_web_push_subscription_survives_restart_and_expires_with_its_session() {
        let fixture = crate::engine::database::tests::PostgresFixture::new();
        let app = app(Engine::open_postgres(&fixture.url).unwrap());
        let (runtime, _) = crate::push::Runtime::new(Some("test-key".into()));
        app.engine().unwrap().push = Some(runtime);
        let valid = subscription("https://fcm.googleapis.com/fcm/send/test");
        assert_eq!(
            request(&app, "bob", "POST", PATH, json!({"subscription":valid}))
                .await
                .0,
            StatusCode::OK
        );
        drop(app);
        let mut reopened = Engine::open_postgres(&fixture.url).unwrap();
        assert!(reopened.push_installed(&session_digest("bob")));
        reopened.logout("bob").unwrap();
        assert!(!reopened.push_installed(&session_digest("bob")));
    }

    #[tokio::test]
    async fn bot_reply_defaults_to_the_asker_and_custom_all_matches_every_eligible_message() {
        let app = app(Engine::open(std::path::Path::new(":memory:")).unwrap());
        let (runtime, mut events) = crate::push::Runtime::new(Some("test-key".into()));
        let room = {
            let mut engine = app.engine().unwrap();
            engine.push = Some(runtime);
            engine
                .execute(Some("alice"), None, "/room new lobby")
                .unwrap();
            engine
                .execute(Some("alice"), None, "/room add bob #lobby")
                .unwrap();
            engine
                .execute(None, Some("lobby"), "/bot new helper")
                .unwrap();
            engine.data.rooms["lobby"].id.clone()
        };
        for user in ["alice", "bob", "carol"] {
            assert_eq!(
                request(
                    &app,
                    user,
                    "POST",
                    PATH,
                    json!({"subscription": subscription(&format!("https://web.push.apple.com/{user}"))})
                )
                .await
                .0,
                StatusCode::OK
            );
        }
        app.engine()
            .unwrap()
            .execute(Some("bob"), Some("lobby"), "@helper can you help, @alice?")
            .unwrap();
        let question = events.try_recv().unwrap();
        let delivered = |user: &str, event: &crate::push::Event| {
            app.engine_read()
                .unwrap()
                .push_delivery(&session_digest(user), event)
                .is_some()
        };
        assert!(!delivered("alice", &question));
        assert!(!delivered("bob", &question));

        let answer = |reply_to| crate::bots::models::MessageInput {
            conversation_id: room.clone(),
            text: "Answer for @bob, cc @alice".into(),
            reply_to,
            markdown: false,
        };
        app.engine()
            .unwrap()
            .send_bot_message("helper", &answer(None), None)
            .unwrap();
        assert!(!delivered("bob", &events.try_recv().unwrap()));
        app.engine()
            .unwrap()
            .send_bot_message("helper", &answer(Some(question.id.clone())), None)
            .unwrap();
        let reply = events.try_recv().unwrap();
        assert!(delivered("bob", &reply));
        assert!(!delivered("alice", &reply));
        assert_eq!(
            request(
                &app,
                "bob",
                "POST",
                "/api/preferences",
                json!({"values":{"notification_default_replies":"false"}})
            )
            .await
            .0,
            StatusCode::OK
        );
        assert!(!delivered("bob", &reply));
        assert_eq!(
            request(
                &app,
                "alice",
                "POST",
                "/api/preferences",
                json!({"values":{"notification.global":"all"}})
            )
            .await
            .0,
            StatusCode::OK
        );
        assert!(delivered("alice", &question));
        assert!(delivered("alice", &reply));
        assert_eq!(
            request(
                &app,
                "carol",
                "POST",
                "/api/preferences",
                json!({"values":{"notification.global":"all"}})
            )
            .await
            .0,
            StatusCode::OK
        );
        assert!(!delivered("carol", &reply));
        assert_eq!(
            request(
                &app,
                "alice",
                "POST",
                "/api/preferences",
                json!({"values":{"notifications_muted":"true"}})
            )
            .await
            .0,
            StatusCode::OK
        );
        assert!(!delivered("alice", &reply));
    }
}
