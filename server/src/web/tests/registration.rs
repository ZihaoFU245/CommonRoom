use super::*;
use axum::{body::Body, extract::ConnectInfo, http::Request, response::Response};
use tower::ServiceExt;

async fn register(app: &App, username: &str, invite: &str) -> Response {
    router(app.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/register")
                .extension(ConnectInfo("127.0.0.1:1234".parse::<SocketAddr>().unwrap()))
                .header(header::ORIGIN, "http://localhost:5173")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"username": username, "password": "new-password", "invite": invite})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn command(app: &App, command: &str) -> String {
    crate::console::execute(app, command.into())
        .await
        .unwrap()
        .unwrap()
        .reply
}

async fn invite(app: &App) -> (String, String) {
    let reply = command(app, "/invite new").await;
    let id = reply.lines().nth(1).unwrap().strip_prefix("ID: ").unwrap();
    (id.into(), reply.lines().last().unwrap().into())
}

async fn assert_registration_contract(app: App, reopen: impl FnOnce() -> Engine) {
    assert!(
        app.engine
            .write()
            .unwrap()
            .execute(Some("alice"), None, "/invite new")
            .is_err()
    );
    let (id, code) = invite(&app).await;
    assert_eq!(code.split('-').map(str::len).collect::<Vec<_>>(), [4, 4, 4]);
    let list = command(&app, "/invite ls").await;
    assert!(list.contains(&id));
    assert!(!list.contains(&code));

    assert_eq!(
        register(&app, "alice", &code).await.status(),
        StatusCode::CONFLICT
    );
    let typed = code.replace('-', "").to_ascii_lowercase();
    let (first, second) = tokio::join!(
        register(&app, "carol", &code),
        register(&app, "dave", &typed)
    );
    let (winner, loser, winner_status, loser_status) = if first.status() == StatusCode::CREATED {
        ("carol", "dave", first.status(), second.status())
    } else {
        ("dave", "carol", second.status(), first.status())
    };
    assert_eq!(winner_status, StatusCode::CREATED);
    assert_eq!(loser_status, StatusCode::BAD_REQUEST);
    assert!(!first.headers().contains_key(header::SET_COOKIE));
    assert!(!second.headers().contains_key(header::SET_COOKIE));
    let (revoked_id, revoked) = invite(&app).await;
    command(&app, &format!("/invite revoke {revoked_id}")).await;
    let (_, unused) = invite(&app).await;

    drop(app);
    let engine = reopen();
    assert_eq!(engine.groups(Some(winner)), ["user"]);
    assert!(crate::engine::verify_password(
        "new-password",
        engine.password_hash(winner).unwrap()
    ));
    assert!(!engine.data.users.contains_key(loser));
    for invalid in [&code, &revoked] {
        let (id, digest) = crate::engine::invites::parse(invalid).unwrap();
        assert!(
            engine
                .check_registration_invite(id.as_deref(), &digest)
                .is_err()
        );
    }
    let (id, digest) = crate::engine::invites::parse(&unused).unwrap();
    assert!(
        engine
            .check_registration_invite(id.as_deref(), &digest)
            .is_ok()
    );
}

#[tokio::test]
async fn sqlite_registration_invites_survive_restart_and_redeem_once() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("chat.db");
    let app = auth_test_app(Engine::open(&path).unwrap());
    assert_registration_contract(app, || Engine::open(&path).unwrap()).await;
}

#[tokio::test]
#[ignore = "requires CHAT_TEST_POSTGRES_URL"]
async fn postgres_registration_invites_survive_restart_and_redeem_once() {
    let fixture = crate::engine::database::tests::PostgresFixture::new();
    let app = auth_test_app(Engine::open_postgres(&fixture.url).unwrap());
    assert_registration_contract(app, || Engine::open_postgres(&fixture.url).unwrap()).await;
}
