//! Capture a bounded notification batch once; load and serialize off-lock.
use super::{App, Change, auth};
use crate::engine::{
    EngineError,
    authorization::Action,
    avatars::AvatarPlan,
    snapshot::{ConversationPlan, UnreadPlan},
};
use axum::{extract::ws::Utf8Bytes, http::HeaderMap};
use std::collections::BTreeSet;

enum Plan {
    Ready(Utf8Bytes),
    Avatar(AvatarPlan),
    Conversation(ConversationPlan),
    Read(UnreadPlan),
    Baseline,
}
impl Plan {
    async fn finish(
        self,
        app: &App,
        headers: &HeaderMap,
        bot: bool,
    ) -> Result<Option<Utf8Bytes>, EngineError> {
        let value = match self {
            Self::Ready(frame) => return Ok(Some(frame)),
            Self::Baseline => {
                return super::snapshots::value_async(app, headers, bot)
                    .await
                    .map(Some);
            }
            Self::Avatar(plan) => plan.finish_async().await?,
            Self::Conversation(plan) => Some(plan.finish_async().await?),
            Self::Read(plan) => {
                Some(serde_json::json!({"kind":"read","unread":plan.finish_async().await?}))
            }
        };
        value
            .map(|value| {
                serde_json::to_string(&value)
                    .map(Into::into)
                    .map_err(EngineError::from)
            })
            .transpose()
    }
}
#[cfg(test)]
pub(crate) fn browser(
    app: &App,
    headers: &HeaderMap,
    bot: bool,
    changes: &[Change],
) -> Result<(String, Vec<Utf8Bytes>), EngineError> {
    browser_with_interests(
        app,
        headers,
        bot,
        changes,
        &mut super::transport::Interests::default(),
    )
}
#[cfg(test)]
pub(crate) fn browser_with_interests(
    app: &App,
    headers: &HeaderMap,
    bot: bool,
    changes: &[Change],
    interests: &mut super::transport::Interests,
) -> Result<(String, Vec<Utf8Bytes>), EngineError> {
    crate::engine::database::block_on_storage(browser_with_interests_async(
        app, headers, bot, changes, interests,
    ))?
}
pub(crate) async fn browser_with_interests_async(
    app: &App,
    headers: &HeaderMap,
    bot: bool,
    changes: &[Change],
    interests: &mut super::transport::Interests,
) -> Result<(String, Vec<Utf8Bytes>), EngineError> {
    for _ in 0..3 {
        if app.stopping.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(EngineError::Unavailable("Server stopping.".into()));
        }
        let (user, stamp, ids, presence, plans, refreshed, history) = app
            .inspect_engine({
                let headers = headers.clone();
                let changes = changes.to_vec();
                let interests = interests.clone();
                move |engine, config| -> Result<_, EngineError> {
                    let user = auth::identity(engine, config, &headers)
                        .ok_or_else(|| EngineError::Forbidden("Credential revoked.".into()))?;
                    let account = engine
                        .active_account_id(&user)
                        .ok_or("Account unavailable.")?;
                    let baseline = changes.iter().any(|change| {
                        matches!(change, Change::All)
                            || matches!(change, Change::Metadata(notice) if notice.targets(account))
                            || matches!(change, Change::Account(name) if name == &user)
                            || (bot && matches!(change, Change::Read(name) if name == &user))
                    });
                    let presence = !bot && changes.contains(&Change::Presence);
                    let mut ids = BTreeSet::new();
                    let mut plans = Vec::new();
                    let refreshed = interests
                        .refresh_needed(baseline)
                        .then(|| {
                            engine.snapshot_stamp(&user, bot).map(|stamp| {
                                stamp
                                    .history
                                    .into_iter()
                                    .map(|(id, _)| id)
                                    .collect::<BTreeSet<_>>()
                            })
                        })
                        .transpose()?;
                    if baseline {
                        ids.extend(refreshed.iter().flatten().cloned());
                        plans.push(Plan::Baseline);
                    }
                    for change in &changes {
                        match change {
                            Change::Presence if !bot => plans
                                .push(Plan::Ready(engine.presence_frame(&user).to_string().into())),
                            Change::BotCommands(id) if !bot => {
                                if engine.bot_target(&user, id, Action::Read).is_ok() {
                                    ids.insert(id.clone());
                                    plans.push(Plan::Ready(
                                serde_json::json!({"kind":"bot_commands","conversation_id":id})
                                    .to_string()
                                    .into(),
                            ));
                                }
                            }
                            Change::Metadata(notice) if !bot => {
                                for id in notice.catalogues() {
                                    if engine.bot_target(&user, id, Action::Read).is_ok() {
                                        ids.insert(id.clone());
                                        plans.push(Plan::Ready(
                                    serde_json::json!({"kind":"bot_commands","conversation_id":id})
                                        .to_string()
                                        .into(),
                                ));
                                    }
                                }
                            }
                            _ if baseline => {}
                            Change::Avatar(owner) if !bot => {
                                if let Some(plan) = engine.avatar_plan(&user, owner) {
                                    ids.extend(plan.history_ids().map(str::to_owned));
                                    plans.push(Plan::Avatar(plan));
                                }
                            }
                            Change::Read(name) if name == &user => {
                                let plan = engine.unread_plan(&user)?;
                                ids.extend(plan.ids().map(str::to_owned));
                                plans.push(Plan::Read(plan));
                            }
                            Change::Conversation(id) if bot => {
                                if engine.bot_target(&user, id, Action::Read).is_ok() {
                                    ids.insert(id.clone());
                                    plans.push(Plan::Ready(
                                serde_json::json!({"kind":"conversation","conversation_id":id})
                                    .to_string()
                                    .into(),
                            ));
                                }
                            }
                            Change::Conversation(id) => {
                                if let Some(plan) = engine.conversation_plan_for(
                                    &user,
                                    id,
                                    super::colors::enabled(&headers),
                                )? {
                                    ids.insert(plan.id().into());
                                    plans.push(Plan::Conversation(plan));
                                }
                            }
                            _ => {}
                        }
                    }
                    let stamp = engine.delivery_stamp(&user, &ids, presence)?;
                    Ok((
                        user,
                        stamp,
                        ids,
                        presence,
                        plans,
                        refreshed,
                        engine.history_store().clone(),
                    ))
                }
            })
            .await?;
        // Always revalidate even if a read failed: a deletion/revocation may
        // explain that failure, and retry must capture the new authority.
        let mut frames = Vec::new();
        let mut result = Ok(());
        for plan in plans {
            match plan.finish(app, headers, bot).await {
                Ok(frame) => frames.extend(frame),
                Err(error) => {
                    result = Err(error);
                    break;
                }
            }
        }
        history.ensure_available_async().await?;
        let validated = app
            .inspect_engine({
                let headers = headers.clone();
                let user = user.clone();
                let mut interests = interests.clone();
                move |engine, config| -> Result<_, EngineError> {
                    if auth::identity(engine, config, &headers).as_deref() != Some(&user) {
                        return Err(EngineError::Forbidden("Credential revoked.".into()));
                    }
                    if engine.delivery_stamp(&user, &ids, presence).as_ref().ok() != Some(&stamp) {
                        return Ok(None);
                    }
                    interests.bind(
                        engine
                            .active_account_id(&user)
                            .ok_or("Account unavailable.")?,
                    );
                    if let Some(ids) = refreshed {
                        interests.refresh(ids, engine.metadata_clock());
                    }
                    Ok(Some(interests))
                }
            })
            .await?;
        if let Some(validated) = validated {
            *interests = validated;
            result?;
            return Ok((user, frames));
        }
    }
    Err(EngineError::Conflict(
        "Notifications changed; retry shortly.".into(),
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn history_work(app: &App) -> serde_json::Value {
        let mut stats = app.engine_read().unwrap().history_store().stats().unwrap();
        let pools = stats.as_object_mut().unwrap().remove("pools").unwrap();
        stats.as_object_mut().unwrap().remove("uptime_seconds");
        // Live pool occupancy can change when an asynchronously returned connection
        // becomes idle, even when this request performs no database work.
        stats["operations"] = pools["operations"].clone();
        stats
    }
    #[test]
    fn metadata_targets_new_and_revoked_members_without_history_work_for_outsiders() {
        let app = crate::web::tests::login_test_app();
        let mut headers = Vec::new();
        {
            let mut engine = app.engine_api().unwrap();
            engine.execute(None, None, "/grant alice admin").unwrap();
            for name in ["bob", "eve"] {
                engine.provision(name, "hash".into(), false, false).unwrap();
            }
            engine
                .execute(Some("alice"), None, "/room new hidden")
                .unwrap();
            engine
                .execute(Some("alice"), Some("hidden"), "private room history")
                .unwrap();
            for name in ["alice", "bob", "eve"] {
                let token = format!("metadata-{name}");
                let hash = engine.password_hash(name).unwrap().to_owned();
                engine.login(token.clone(), name, &hash, None).unwrap();
                let mut header = HeaderMap::new();
                header.insert(
                    axum::http::header::COOKIE,
                    format!("chat_session={token}").parse().unwrap(),
                );
                headers.push(header);
            }
        }
        let mut interests: Vec<_> = headers
            .iter()
            .map(|header| {
                let mut interests = super::super::transport::Interests::default();
                browser_with_interests(&app, header, false, &[Change::All], &mut interests)
                    .unwrap();
                interests
            })
            .collect();
        for (command, visible) in [
            ("/room add bob #hidden", true),
            ("/room kick bob #hidden", false),
        ] {
            let notice = {
                let mut engine = app.engine_api().unwrap();
                engine.execute(None, None, command).unwrap();
                engine.metadata_change()
            };
            let change = Change::Metadata(notice);
            assert!(!interests[0].excludes(&change));
            assert!(!interests[1].excludes(&change));
            assert!(interests[2].excludes(&change));
            let before = history_work(&app);
            let (_, outsider) = browser_with_interests(
                &app,
                &headers[2],
                false,
                std::slice::from_ref(&change),
                &mut interests[2],
            )
            .unwrap();
            assert!(outsider.is_empty());
            assert_eq!(before, history_work(&app));
            let (_, frames) =
                browser_with_interests(&app, &headers[1], false, &[change], &mut interests[1])
                    .unwrap();
            assert_eq!(frames.len(), 1);
            let snapshot: serde_json::Value = serde_json::from_str(&frames[0]).unwrap();
            assert_eq!(snapshot["kind"], "snapshot");
            assert_eq!(
                snapshot["rooms"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|room| room["name"] == "hidden"),
                visible
            );
        }
    }
    #[test]
    fn bot_credentials_invalidate_catalogues_without_rebuilding_member_snapshots() {
        let app = crate::web::tests::login_test_app();
        let headers = {
            let mut engine = app.engine_api().unwrap();
            engine.execute(None, None, "/grant alice admin").unwrap();
            engine
                .provision("bob", "hash".into(), false, false)
                .unwrap();
            engine
                .execute(Some("alice"), None, "/room new shared")
                .unwrap();
            engine.execute(None, None, "/room add bob #shared").unwrap();
            engine
                .execute(Some("alice"), Some("shared"), "/bot new helper")
                .unwrap();
            let hash = engine.password_hash("bob").unwrap().to_owned();
            engine
                .login("catalogue-reader".into(), "bob", &hash, None)
                .unwrap();
            let mut headers = HeaderMap::new();
            headers.insert(
                axum::http::header::COOKIE,
                "chat_session=catalogue-reader".parse().unwrap(),
            );
            headers
        };
        let mut interests = super::super::transport::Interests::default();
        browser_with_interests(&app, &headers, false, &[Change::All], &mut interests).unwrap();
        let (notice, id) = {
            let mut engine = app.engine_api().unwrap();
            engine
                .execute(Some("alice"), None, "/bot token new helper catalogue-test")
                .unwrap();
            (
                engine.metadata_change(),
                engine.data.rooms["shared"].id.clone(),
            )
        };
        let before = history_work(&app);
        let (_, frames) = browser_with_interests(
            &app,
            &headers,
            false,
            &[Change::Metadata(notice)],
            &mut interests,
        )
        .unwrap();
        let frames: Vec<serde_json::Value> = frames
            .iter()
            .map(|frame| serde_json::from_str(frame).unwrap())
            .collect();
        assert_eq!(
            frames,
            vec![serde_json::json!({"kind":"bot_commands","conversation_id":id})]
        );
        assert_eq!(before, history_work(&app));
    }
    #[test]
    fn a_reserved_private_conversation_invalidates_negative_hints_before_its_first_message() {
        let app = crate::web::tests::login_test_app();
        {
            let mut engine = app.engine_api().unwrap();
            engine
                .provision("bob", "hash".into(), false, false)
                .unwrap();
            let hash = engine.password_hash("alice").unwrap().to_owned();
            engine
                .login("private-reader".into(), "alice", &hash, None)
                .unwrap();
        }
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            "chat_session=private-reader".parse().unwrap(),
        );
        let mut interests = super::super::transport::Interests::default();
        browser_with_interests(&app, &headers, false, &[Change::All], &mut interests).unwrap();
        let media = app
            .engine_api()
            .unwrap()
            .allocate_media_target(
                "bob",
                "@direct:alice",
                "pending.txt",
                1,
                Default::default(),
                false,
            )
            .unwrap()
            .unwrap();
        assert!(!interests.excludes(&Change::Conversation(media.conversation.clone())));
        app.engine_api()
            .unwrap()
            .execute(
                Some("bob"),
                None,
                "/tell alice first message after reservation",
            )
            .unwrap();
        let (_, frames) = browser_with_interests(
            &app,
            &headers,
            false,
            &[Change::Conversation(media.conversation.clone())],
            &mut interests,
        )
        .unwrap();
        let frame: serde_json::Value = serde_json::from_str(&frames[0]).unwrap();
        assert_eq!(frame["view"], "@direct:bob");
        assert_eq!(
            frame["messages"][0]["text"],
            "first message after reservation"
        );
        assert!(!interests.excludes(&Change::Conversation(media.conversation)));
    }
    #[test]
    fn one_batch_filters_targets_and_baselines_subsume_content_without_losing_discovery() {
        let app = crate::web::tests::login_test_app();
        let (visible, hidden) = {
            let mut engine = app.engine_api().unwrap();
            engine.execute(None, None, "/grant alice admin").unwrap();
            engine
                .execute(Some("alice"), None, "/room new visible")
                .unwrap();
            engine.execute(None, None, "/room new hidden").unwrap();
            engine
                .execute(Some("alice"), Some("visible"), "batch content")
                .unwrap();
            let hash = engine.password_hash("alice").unwrap().to_owned();
            engine
                .login("batch-reader".into(), "alice", &hash, None)
                .unwrap();
            (
                engine.data.rooms["visible"].id.clone(),
                engine.data.rooms["hidden"].id.clone(),
            )
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            "chat_session=batch-reader".parse().unwrap(),
        );
        let before = app.engine_read().unwrap().history_store().stats().unwrap();
        let (_, frames) = browser(
            &app,
            &headers,
            false,
            &[
                Change::Conversation(visible.clone()),
                Change::BotCommands(visible.clone()),
                Change::Conversation(hidden.clone()),
                Change::Read("other".into()),
                Change::Account("other".into()),
                Change::Presence,
            ],
        )
        .unwrap();
        let frames: Vec<serde_json::Value> = frames
            .iter()
            .map(|frame| serde_json::from_str(frame).unwrap())
            .collect();
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0]["kind"], "conversation");
        assert_eq!(frames[0]["messages"][0]["text"], "batch content");
        assert_eq!(frames[1]["kind"], "bot_commands");
        assert_eq!(frames[2]["kind"], "presence");
        let after = app.engine_read().unwrap().history_store().stats().unwrap();
        assert_eq!(
            after["misses"].as_u64().unwrap() - before["misses"].as_u64().unwrap(),
            1
        );
        let (_, frames) = browser(
            &app,
            &headers,
            false,
            &[
                Change::Account("alice".into()),
                Change::Conversation(visible.clone()),
                Change::Read("alice".into()),
                Change::BotCommands(visible),
                Change::Presence,
            ],
        )
        .unwrap();
        let kinds: Vec<_> = frames
            .iter()
            .map(|frame| serde_json::from_str::<serde_json::Value>(frame).unwrap()["kind"].clone())
            .collect();
        assert_eq!(
            kinds,
            vec![
                serde_json::json!("snapshot"),
                serde_json::json!("bot_commands"),
                serde_json::json!("presence")
            ]
        );
        let mut interests = super::super::transport::Interests::default();
        // Even an incorrect/stale positive hint cannot grant access.
        interests.refresh(
            [hidden.clone()].into(),
            app.engine_read().unwrap().metadata_clock(),
        );
        let (_, frames) = browser_with_interests(
            &app,
            &headers,
            false,
            &[Change::Conversation(hidden.clone())],
            &mut interests,
        )
        .unwrap();
        assert!(frames.is_empty());
        browser_with_interests(&app, &headers, false, &[Change::All], &mut interests).unwrap();
        assert!(interests.excludes(&Change::Conversation(hidden)));
        let visible = app.engine_read().unwrap().data.rooms["visible"].id.clone();
        assert!(!interests.excludes(&Change::Conversation(visible.clone())));
        app.engine_api()
            .unwrap()
            .execute(None, None, "/room delete #visible")
            .unwrap();
        let (_, frames) = browser_with_interests(
            &app,
            &headers,
            false,
            &[Change::Conversation(visible.clone())],
            &mut interests,
        )
        .unwrap();
        assert!(frames.is_empty());
        browser_with_interests(&app, &headers, false, &[Change::All], &mut interests).unwrap();
        assert!(interests.excludes(&Change::Conversation(visible)));
        app.engine_api().unwrap().logout("batch-reader").unwrap();
        assert!(matches!(
            browser(&app, &headers, false, &[Change::Presence]),
            Err(EngineError::Forbidden(_))
        ));
    }
}
