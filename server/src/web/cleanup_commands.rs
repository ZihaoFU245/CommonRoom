//! Async history cleanup with stable authority and an atomic deletion/audit batch.
use super::commands::Input;
use super::*;
use crate::engine::EngineError;
use axum::http::HeaderMap;

pub(super) async fn execute(
    app: &App,
    headers: HeaderMap,
    user: String,
    input: Input,
) -> Result<(String, Option<Change>), EngineError> {
    for _ in 0..3 {
        let (identity, (mut plan, stamp)) =
            super::commands::capture(app, headers.clone(), user.clone(), &input, {
                let view = input.room.clone();
                let user = user.clone();
                move |engine, text| {
                    let plan = engine.prepare_browser_cleanup(&user, view.as_deref(), text)?;
                    let stamp = engine.cleanup_stamp(&plan)?;
                    Ok((plan, stamp))
                }
            })
            .await?;
        let _account = app
            .admission
            .http(identity.account.clone())
            .map_err(EngineError::Unavailable)?;
        // Count without conversation permits. Final revision validation proves
        // this snapshot still describes every target, including empty targets.
        plan.count().await?;
        let ids = plan.ids();
        let ordering = app.mutations.conversations(plan.matched_ids()).await?;
        let reply = plan.report();
        let _audit = if !plan.preview && plan.has_matches() {
            Some(app.mutations.audit.lock().await)
        } else {
            None
        };
        let prepared = app
            .with_conversation(&ordering, {
                let identity = identity.clone();
                let input = input.clone();
                move |engine| -> Result<_, EngineError> {
                    super::commands::phase(engine, &identity, &input, |engine, text| {
                        let current = engine.prepare_browser_cleanup(
                            &identity.user,
                            input.room.as_deref(),
                            text,
                        )?;
                        if current.ids() != ids || engine.cleanup_stamp(&current)? != stamp {
                            return Ok(None);
                        }
                        // Only matched conversations need serialization after this
                        // linearization point. Keep one atomic deletion/audit batch.
                        Ok(Some(engine.prepare_history_cleanup(&identity.user, &plan)?))
                    })
                }
            })
            .await?;
        let Some(pending) = prepared else {
            continue;
        };
        let Some(pending) = pending else {
            return Ok((reply, None));
        };
        pending.save().await?;
        let changes = app.changes.clone();
        return app
            .with_conversation(&ordering, move |engine| -> Result<_, EngineError> {
                if let Some(change) = Change::command(engine.publish_history(pending)?) {
                    let _ = changes.send(change);
                }
                identity.verify(engine)?;
                Ok((reply, None))
            })
            .await;
    }
    Err(EngineError::Conflict(
        "Cleanup targets changed; retry the operation.".into(),
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
    async fn postgres_global_count_does_not_lock_sends_and_retries_changed_empty_targets() {
        let fixture = crate::engine::database::tests::PostgresFixture::new();
        let mut app = crate::web::media::tests::app(Engine::open_postgres(&fixture.url).unwrap());
        app.io_jobs = Arc::new(tokio::sync::Semaphore::new(8));
        {
            let mut engine = app.engine().unwrap();
            engine.execute(None, None, "/grant alice su").unwrap();
            for room in ["one", "two"] {
                engine
                    .execute(Some("alice"), None, &format!("/room new {room}"))
                    .unwrap();
            }
            engine
                .execute(Some("alice"), Some("one"), "original")
                .unwrap();
        }
        let blocker = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
        let hold = blocker.unchecked_transaction().unwrap();
        hold.execute_batch("LOCK TABLE message_media IN ACCESS EXCLUSIVE MODE")
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            "chat_session=alice".parse().unwrap(),
        );
        let job = tokio::spawn({
            let app = app.clone();
            let headers = headers.clone();
            async move {
                super::super::commands::execute_input(
                    &app,
                    &headers,
                    "alice",
                    &Input {
                        id: 1,
                        room: None,
                        text: "/clean -1 @global".into(),
                        markdown: false,
                    },
                )
                .await
            }
        });
        let inspect = crate::engine::database::Connection::postgres(&fixture.url, false).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        loop {
            let waiting: bool = inspect.query_row("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT targets.value,count%')",[],|r|r.get(0)).unwrap();
            if waiting {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "cleanup did not enter count"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            super::super::commands::execute_input(
                &app,
                &headers,
                "alice",
                &Input {
                    id: 2,
                    room: Some("two".into()),
                    text: "during count".into(),
                    markdown: false,
                },
            ),
        )
        .await
        .unwrap()
        .unwrap();
        hold.rollback().unwrap();
        let (report, _) = tokio::time::timeout(std::time::Duration::from_secs(2), job)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(report.contains("Messages deleted: 2"), "{report}");
        assert_eq!(
            inspect
                .query_row::<u64, _, _>("SELECT count(*) FROM messages", [], |r| r.get(0))
                .unwrap(),
            0
        );
        assert_eq!(
            inspect
                .query_row::<u64, _, _>(
                    "SELECT count(*) FROM audit_entries WHERE json LIKE '%/clean%'",
                    [],
                    |r| r.get(0)
                )
                .unwrap(),
            1
        );
    }
}
