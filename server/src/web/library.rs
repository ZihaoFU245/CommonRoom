//! Browser library membership/ownership routes. Asset and account constraints are
//! enforced again by the native transaction; no engine guard spans SQL I/O.
use super::*;
use crate::engine::database::LibraryAction;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct List {
    owned: bool,
    recent: bool,
    after: String,
    limit: u64,
}
impl Default for List {
    fn default() -> Self {
        Self {
            owned: false,
            recent: false,
            after: String::new(),
            limit: 50,
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Change {
    action: LibraryAction,
    source_conversation: Option<String>,
    #[serde(default)]
    confirm_last: bool,
}

pub(crate) async fn list(
    State(app): State<App>,
    headers: HeaderMap,
    Query(input): Query<List>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_read_origin(&app, &headers)?;
    let permit = app.io_permit()?;
    admission::spawn_io(permit, async move {
        let (identity, store) =
            media::native_context(&app, media::Access::Browser(headers)).await?;
        let ordering = app.mutations.conversations(vec![]).await?;
        let (quota, scopes) = app
            .with_conversation(&ordering, {
                let identity = identity.clone();
                move |engine| -> Result<_, ApiError> {
                    media::verify_identity(&identity, engine)?;
                    Ok((
                        engine.library_quota(&identity.user),
                        engine.library_read_scopes(&identity.user),
                    ))
                }
            })
            .await?;
        let page = store
            .library_page(&crate::engine::database::LibraryList {
                account: &identity.account,
                owned: input.owned,
                recent: input.recent,
                after: &input.after,
                limit: input.limit,
                quota,
                conversations: &scopes,
            })
            .await?;
        app.with_conversation(&ordering, move |engine| -> Result<_, ApiError> {
            media::verify_identity(&identity, engine)?;
            Ok(Json(json!(page)))
        })
        .await
    })
    .await?
}

pub(crate) async fn change(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<Change>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_origin(&app, &headers)?;
    let permit = app.io_permit()?;
    admission::spawn_io(permit, async move {
        let (identity, store) =
            media::native_context(&app, media::Access::Browser(headers)).await?;
        let ordering = app.mutations.conversation(format!("@asset:{id}")).await?;
        let (quota, scopes) = app
            .with_conversation(&ordering, {
                let identity = identity.clone();
                let source = input.source_conversation.clone();
                move |engine| -> Result<_, ApiError> {
                    media::verify_identity(&identity, engine)?;
                    if let Some(source) = source {
                        engine.authorize_library_source(&identity.user, &source)?;
                    }
                    if matches!(input.action, LibraryAction::Delete)
                        && !engine.is_admin(&identity.user)
                    {
                        return Err(error(
                            StatusCode::FORBIDDEN,
                            "Administrator permission required.",
                        ));
                    }
                    Ok((
                        engine.library_quota(&identity.user),
                        engine.library_read_scopes(&identity.user),
                    ))
                }
            })
            .await?;
        let source = if input.source_conversation.is_none()
            && matches!(input.action, LibraryAction::Save | LibraryAction::Acquire)
        {
            store.library_source(&id, &scopes).await?
        } else {
            input.source_conversation
        };
        let changed = store
            .library_action(&crate::engine::database::LibraryWrite {
                account: &identity.account,
                id: &id,
                source: source.as_deref(),
                action: input.action,
                confirm_last: input.confirm_last,
                quota,
                config: &app.config.media,
            })
            .await?;
        if changed {
            let global = matches!(input.action, LibraryAction::Delete)
                || matches!(input.action, LibraryAction::Release) && input.confirm_last;
            let target = (!global).then(|| identity.account.clone());
            let _ = app.changes.send(super::Change::Media(target));
        }
        app.with_conversation(&ordering, move |engine| -> Result<_, ApiError> {
            media::verify_identity(&identity, engine)?;
            Ok(Json(json!({"ok":true})))
        })
        .await
    })
    .await?
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "library_tests.rs"]
mod tests;
