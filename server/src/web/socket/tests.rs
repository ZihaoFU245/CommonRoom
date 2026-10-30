use super::*;

#[tokio::test]
#[allow(clippy::unwrap_used)]
async fn live_socket_recovers_a_failed_read_without_another_broadcast_and_closes_on_revocation() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::{Message as ClientMessage, client::IntoClientRequest};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.sqlite");
    let app = crate::web::media::tests::app(Engine::open(&path).unwrap());
    let id = {
        let mut engine = app.engine().unwrap();
        engine
            .execute(Some("alice"), None, "/room new recovery")
            .unwrap();
        engine.data.rooms["recovery"].id.clone()
    };
    let database = crate::engine::database::Connection::open(&path).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(super::super::http_server::serve(
        listener,
        super::super::router(app.clone()),
        {
            let app = app.clone();
            async move {
                let _ = stopped.await;
                app.stopping
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                app.shutdown.notify_waiters();
            }
        },
    ));
    let mut request = format!("ws://{address}/ws").into_client_request().unwrap();
    request
        .headers_mut()
        .insert("origin", "http://localhost:5173".parse().unwrap());
    request
        .headers_mut()
        .insert("cookie", "chat_session=alice".parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    let first = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let frame: serde_json::Value = serde_json::from_str(first.to_text().unwrap()).unwrap();
    assert_eq!(frame["kind"], "snapshot");
    {
        let mut engine = app.engine().unwrap();
        engine
            .execute(Some("alice"), Some("recovery"), "after failure")
            .unwrap();
        // Force the next notification to read SQLite rather than a warm page.
        engine
            .history_store()
            .configure_cache(&Default::default())
            .unwrap();
    }
    let original: String = database
        .query_row(
            "SELECT json FROM messages WHERE conversation=?1",
            [&id],
            |row| row.get(0),
        )
        .unwrap();
    // A reversible decode failure exercises the real unavailable path without
    // changing owner-loss or poisoned-lock recovery semantics.
    database
        .execute(
            "UPDATE messages SET json=json_set(json,'$.sequence','invalid') WHERE conversation=?1",
            [&id],
        )
        .unwrap();
    app.changes.send(Change::Conversation(id.clone())).unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let failed =
                app.engine_read().unwrap().history_store().stats().unwrap()["pools"]["operations"]
                    ["history_reads"]["failed"]
                    .as_u64()
                    .unwrap();
            if failed > 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    database
        .execute(
            "UPDATE messages SET json=?1 WHERE conversation=?2",
            [&original, &id],
        )
        .unwrap();
    // No change is broadcast after repair. The timer must recover the frame.
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            match socket.next().await.unwrap().unwrap() {
                ClientMessage::Text(text) => {
                    let frame: serde_json::Value = serde_json::from_str(&text).unwrap();
                    if frame["kind"] == "snapshot" {
                        assert_eq!(frame["rooms"][0]["messages"][0]["text"], "after failure");
                        break;
                    }
                }
                ClientMessage::Ping(bytes) => {
                    socket.send(ClientMessage::Pong(bytes)).await.unwrap()
                }
                message => assert!(!matches!(message, ClientMessage::Close(_))),
            }
        }
    })
    .await
    .unwrap();
    app.engine().unwrap().logout("alice").unwrap();
    app.changes.send(Change::Account("alice".into())).unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if matches!(
                socket.next().await,
                Some(Ok(ClientMessage::Close(_))) | None
            ) {
                break;
            }
        }
    })
    .await
    .unwrap();
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[test]
fn recovery_retains_independent_invalidations_and_retries_without_a_new_change() {
    let mut recovery = Recovery::default();
    recovery.schedule(&[
        Change::Conversation("room".into()),
        Change::BotCommands("room".into()),
        Change::Presence,
    ]);
    for _ in 0..1000 {
        recovery.schedule(&[Change::Conversation("other".into())]);
    }
    assert_eq!(
        recovery.take(),
        Some(vec![
            Change::All,
            Change::BotCommands("room".into()),
            Change::Presence
        ])
    );
    assert_eq!(recovery.take(), None);
    recovery.schedule(&[]);
    assert_eq!(recovery.take(), Some(vec![Change::All]));
    assert!(!retryable(StatusCode::UNAUTHORIZED));
    assert!(!retryable(StatusCode::FORBIDDEN));
    assert!(retryable(StatusCode::CONFLICT));
    assert!(retryable(StatusCode::SERVICE_UNAVAILABLE));
    assert!(retryable(StatusCode::INTERNAL_SERVER_ERROR));
}
