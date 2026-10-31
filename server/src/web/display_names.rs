use super::*;
use axum::{extract::State, http::HeaderMap};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DisplayNameInput {
    value: Option<String>,
    #[serde(default)]
    room: Option<String>,
}

pub(crate) async fn set_display_name(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<DisplayNameInput>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_origin(&app, &headers)?;
    let permit = app
        .io_permit()
        .map_err(|cause| error(StatusCode::SERVICE_UNAVAILABLE, &cause))?;
    admission::spawn_io(permit, async move {
        let (identity, store) =
            media::native_context(&app, media::Access::Browser(headers)).await?;
        let ordering = app.mutations.metadata().await?;
        let pending = app
            .with_metadata(&ordering, {
                let identity = identity.clone();
                move |engine| -> Result<_, ApiError> {
                    media::verify_identity(&identity, engine)?;
                    Ok(engine.prepare_display_name(&identity.user, input.value, input.room)?)
                }
            })
            .await?;
        if let Some(pending) = pending {
            let pending = pending.save().await?;
            let identity = identity.clone();
            let changes = app.changes.clone();
            app.with_metadata(&ordering, move |engine| -> Result<_, ApiError> {
                engine.publish_metadata(pending)?;
                let _ = changes.send(Change::Metadata(engine.metadata_change()));
                media::verify_identity(&identity, engine)
            })
            .await?;
        } else {
            store.health().await?;
        }
        app.inspect_engine(move |engine, _| -> Result<_, ApiError> {
            media::verify_identity(&identity, engine)?;
            Ok(Json(json!({"ok":true, "sync": engine.preference_sync()})))
        })
        .await
    })
    .await
    .map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Display name worker unavailable.",
        )
    })?
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::web::media::tests::{app, request};
    async fn set(
        app: &App,
        user: &str,
        value: serde_json::Value,
        room: Option<&str>,
    ) -> (StatusCode, serde_json::Value) {
        request(
            app,
            user,
            "POST",
            "/api/displayname",
            json!({"value":value,"room":room}),
        )
        .await
    }
    #[tokio::test]
    async fn display_names_scope_permissions_duplicates_and_unset() {
        let app = app(Engine::open(std::path::Path::new(":memory:")).unwrap());
        app.engine()
            .unwrap()
            .execute(Some("alice"), None, "/room new lobby")
            .unwrap();
        app.engine()
            .unwrap()
            .execute(Some("alice"), None, "/room add bob #lobby")
            .unwrap();
        let room = app.engine_read().unwrap().data.rooms["lobby"].id.clone();
        let mut notices = app.changes.subscribe();
        for user in ["bob", "alice"] {
            let response = set(&app, user, json!("同名 Person"), None).await;
            assert_eq!(response.0, StatusCode::OK, "{}", response.1);
        }
        let Change::Metadata(notice) = notices.try_recv().unwrap() else {
            panic!("missing notice")
        };
        assert!(notice.targets(app.engine_read().unwrap().account_id("alice").unwrap()));
        {
            let mut engine = app.engine().unwrap();
            assert_eq!(
                engine.snapshot("alice").unwrap().display_names["bob"],
                "同名 Person"
            );
            assert!(
                engine
                    .execute(
                        Some("bob"),
                        None,
                        "/user set alice displayname stolen @global"
                    )
                    .is_err()
            );
            engine
                .execute(None, None, "/revoke bob #lobby w:account.displayname.own")
                .unwrap();
        }
        assert_eq!(
            set(&app, "bob", json!("Room Bob"), Some(&room)).await.0,
            StatusCode::FORBIDDEN
        );
        {
            let mut engine = app.engine().unwrap();
            engine
                .execute(
                    Some("alice"),
                    Some("lobby"),
                    "/grant bob #lobby w:account.displayname.own",
                )
                .unwrap();
        }
        assert_eq!(
            set(&app, "bob", json!("Room Bob"), Some(&room)).await.0,
            StatusCode::OK
        );
        {
            let mut engine = app.engine().unwrap();
            let snapshot = engine.snapshot("alice").unwrap();
            assert_eq!(snapshot.display_names["bob"], "同名 Person");
            assert_eq!(
                snapshot
                    .rooms
                    .iter()
                    .find(|room| room.name == "lobby")
                    .unwrap()
                    .display_names["bob"],
                "Room Bob"
            );
            assert!(
                engine
                    .execute(Some("bob"), Some("lobby"), "/user ls")
                    .unwrap()
                    .contains("bob(Room Bob)")
            );
            assert!(
                engine
                    .execute(
                        Some("bob"),
                        Some("lobby"),
                        "/user set alice displayname stolen"
                    )
                    .is_err()
            );
            engine
                .execute(
                    Some("alice"),
                    Some("lobby"),
                    "/user set bob displayname Managed",
                )
                .unwrap();
            engine
                .execute(Some("bob"), Some("lobby"), "/user reset bob displayname")
                .unwrap();
            assert!(
                engine
                    .execute(Some("bob"), Some("lobby"), "/user ls")
                    .unwrap()
                    .contains("bob(同名 Person)")
            );
            for scope in ["@global", "@account:bob"] {
                assert!(
                    engine
                        .execute(
                            None,
                            None,
                            &format!("/grant bob {scope} w:account.displayname.own")
                        )
                        .is_err()
                );
            }
        }
        assert_eq!(set(&app, "bob", json!(""), None).await.0, StatusCode::OK);
        assert!(
            !app.engine_read()
                .unwrap()
                .snapshot("alice")
                .unwrap()
                .display_names
                .contains_key("bob")
        );
        for value in [json!("x\ny"), json!("a".repeat(257))] {
            assert_eq!(
                set(&app, "bob", value, None).await.0,
                StatusCode::BAD_REQUEST
            );
        }
        assert_eq!(
            set(&app, "expired", json!("name"), None).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(
                &app,
                "bob",
                "POST",
                "/api/displayname",
                json!({"value":"bad","user":"alice"})
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    #[tokio::test]
    async fn display_names_restart_failure_rollback_and_identity_preservation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.db");
        let app = app(Engine::open(&path).unwrap());
        assert_eq!(
            set(&app, "bob", json!("Bob Smith"), None).await.0,
            StatusCode::OK
        );
        let before = app.engine_read().unwrap().data.clone();
        let db = crate::engine::database::Connection::open(&path).unwrap();
        db.execute_batch("CREATE TRIGGER reject_displayname BEFORE UPDATE ON entities WHEN OLD.kind='account' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert_eq!(
            set(&app, "bob", json!("Failed"), None).await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert!(app.engine_read().unwrap().data == before);
        db.execute_batch("DROP TRIGGER reject_displayname;")
            .unwrap();
        drop(db);
        app.engine()
            .unwrap()
            .execute(Some("bob"), None, "/user rename robert")
            .unwrap();
        drop(app);
        let engine = Engine::open(&path).unwrap();
        assert_eq!(
            engine.snapshot("alice").unwrap().display_names["robert"],
            "Bob Smith"
        );
        assert!(!engine.data.users.contains_key("Bob Smith"));
    }
}
