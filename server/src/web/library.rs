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
    sections: bool,
    owned: bool,
    recent: bool,
    after: String,
    limit: u64,
}
impl Default for List {
    fn default() -> Self {
        Self {
            sections: false,
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
    before_id: Option<String>,
    after_id: Option<String>,
}

pub(crate) async fn list(
    State(app): State<App>,
    headers: HeaderMap,
    Query(input): Query<List>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_read_origin(&app, &headers)?;
    if input.sections && (input.owned || input.recent || !input.after.is_empty()) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "Section batches require the first collection page.",
        ));
    }
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
        let request = crate::engine::database::LibraryList {
            account: &identity.account,
            owned: input.owned,
            recent: input.recent,
            after: &input.after,
            limit: input.limit,
            quota,
            conversations: &scopes,
        };
        let (library, recent) = store.library_pages(&request, input.sections).await?;
        let page = if let Some(recent) = recent {
            json!({"library": library, "recent": recent})
        } else {
            json!(library)
        };
        app.with_conversation(&ordering, move |engine| -> Result<_, ApiError> {
            media::verify_identity(&identity, engine)?;
            Ok(Json(page))
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
    if (input.before_id.is_some() && input.after_id.is_some())
        || (!matches!(input.action, LibraryAction::Move)
            && (input.before_id.is_some() || input.after_id.is_some()))
        || input
            .before_id
            .iter()
            .chain(input.after_id.iter())
            .any(|id| uuid::Uuid::parse_str(id).is_err())
    {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "Invalid library move anchors.",
        ));
    }

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
                before: input.before_id.as_deref(),
                after: input.after_id.as_deref(),
                action: input.action,
                confirm_last: input.confirm_last,
                quota,
                config: &app.config.media,
            })
            .await?;
        if changed {
            let global = matches!(
                input.action,
                LibraryAction::Delete | LibraryAction::Acquire | LibraryAction::Save
            ) || matches!(
                input.action,
                LibraryAction::Release | LibraryAction::ReleaseAndRemove
            ) && input.confirm_last;
            let target = (!global).then(|| identity.account.clone());
            let asset =
                (!global && !matches!(input.action, LibraryAction::Move)).then(|| id.clone());
            let _ = app.changes.send(super::Change::Media(target, asset));
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
