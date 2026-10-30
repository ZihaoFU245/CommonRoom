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
        tokio::task::spawn_blocking(move || {
            value
                .map(|value| {
                    serde_json::to_string(&value)
                        .map(Into::into)
                        .map_err(EngineError::from)
                })
                .transpose()
        })
        .await
        .map_err(EngineError::unavailable)?
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
                    let mut catalogue = None;
                    let mut catalogue_position = 0;
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
                        let missing_catalogue = catalogue.is_none();
                        match change {
                            Change::Presence if !bot => plans
                                .push(Plan::Ready(engine.presence_frame(&user).to_string().into())),
                            Change::BotCommands(id) if !bot => {
                                if catalogue.is_none()
                                    && engine.bot_target(&user, id, Action::Read).is_ok()
                                {
                                    catalogue = Some(id.clone());
                                }
                            }
                            Change::BotCatalogues(changed) if !bot => {
                                if catalogue.is_none() {
                                    catalogue = changed
                                        .iter()
                                        .find(|id| {
                                            engine.bot_target(&user, id, Action::Read).is_ok()
                                        })
                                        .cloned();
                                }
                            }
                            Change::Metadata(notice) if !bot => {
                                if catalogue.is_none() {
                                    catalogue = notice
                                        .catalogues()
                                        .find(|id| {
                                            engine.bot_target(&user, id, Action::Read).is_ok()
                                        })
                                        .cloned();
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
                        if missing_catalogue && catalogue.is_some() {
                            catalogue_position = plans.len();
                        }
                    }
                    // Every browser fetches the complete paginated directory. One
                    // authorized invalidation represents this whole catalogue batch.
                    if let Some(id) = catalogue {
                        ids.insert(id.clone());
                        plans.insert(
                            catalogue_position,
                            Plan::Ready(
                                serde_json::json!({"kind":"bot_commands","conversation_id":id})
                                    .to_string()
                                    .into(),
                            ),
                        );
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
mod tests;
