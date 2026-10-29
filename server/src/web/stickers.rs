//! Browser sticker library. Library files are owner-only; sending one creates
//! (or reuses) an ordinary attachment in the target conversation.
use super::media::{Access, native_context, verify_identity};
use super::*;
use crate::engine::{media::Media, stickers};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Redirect,
};
use serde::Deserialize;

const MISSING: &str = "Sticker not found.";

fn storage(app: &App) -> Result<Arc<crate::media::MediaStorage>, ApiError> {
    app.media.clone().ok_or_else(|| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Sticker uploads are not configured.",
        )
    })
}
fn unavailable() -> ApiError {
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        "Sticker worker unavailable.",
    )
}
fn sticker_json(app: &App, media: &Media) -> serde_json::Value {
    json!({"id":media.id,"path":format!("{}api/stickers/{}/file",app.config.base_url,media.id),"content_type":media.content_type,"size":media.size})
}

/// Runs storage work on a detached task holding one API admission permit.
async fn detached<T, F>(
    app: &App,
    work: impl FnOnce(App) -> F + Send + 'static,
) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: std::future::Future<Output = Result<T, ApiError>> + Send + 'static,
{
    let permit = app
        .io_permit()
        .map_err(|cause| error(StatusCode::SERVICE_UNAVAILABLE, &cause))?;
    let app = app.clone();
    tokio::spawn(async move {
        let _permit = permit;
        work(app).await
    })
    .await
    .map_err(|_| unavailable())?
}

async fn owned(
    app: &App,
    access: &Access,
    id: String,
    completing: bool,
) -> Result<Media, ApiError> {
    let access = access.clone();
    detached(app, move |app| async move {
        let (identity, store) = native_context(&app, access).await?;
        let media = store
            .load(&id)
            .await
            .map_err(|cause| super::error::hidden(cause, MISSING))?
            .0;
        app.inspect_engine(move |engine, _| -> Result<_, ApiError> {
            verify_identity(&identity, engine)?;
            engine
                .authorize_sticker_owner(&identity.user, &media, completing)
                .map_err(|cause| super::error::hidden(cause, MISSING))?;
            Ok(media)
        })
        .await
    })
    .await
}

/// Persists verified bytes; failures release the reservation for cleanup.
async fn finish(
    app: &App,
    access: &Access,
    media: Media,
    result: Result<(), ApiError>,
) -> Result<Media, ApiError> {
    let access = access.clone();
    detached(app, move |app| async move {
        let (identity, store) = native_context(&app, access).await?;
        if let Err(failure) = result {
            store.abandon(&media.id, &identity.account).await?;
            return Err(failure);
        }
        let (current, attached) = store.load(&media.id).await?;
        let ready = current.ready;
        app.inspect_engine({
            let identity = identity.clone();
            move |engine, _| -> Result<_, ApiError> {
                verify_identity(&identity, engine)?;
                if current.conversation == stickers::STICKERS {
                    engine.authorize_sticker_owner(&identity.user, &current, true)?;
                } else {
                    engine.authorize_media_owner(&identity.user, &current, attached)?;
                }
                Ok(())
            }
        })
        .await?;
        if !ready {
            store.ready(&media).await?;
        }
        Ok(media)
    })
    .await
}

async fn copy(
    storage: &crate::media::MediaStorage,
    source: &Media,
    media: &mut Media,
) -> Result<(), ApiError> {
    tokio::time::timeout(
        std::time::Duration::from_secs(100),
        storage.copy_verified(source, media),
    )
    .await
    .map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Sticker copy timed out; retry.",
        )
    })?
    .map_err(super::media::storage_failure)
}

pub(crate) async fn list(
    State(app): State<App>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_read_origin(&app, &headers)?;
    let access = Access::Browser(headers);
    detached(&app, move |app| async move {
        let (identity, store) = native_context(&app, access).await?;
        let items: Vec<_> = store
            .stickers(&identity.account)
            .await?
            .iter()
            .map(|media| sticker_json(&app, media))
            .collect();
        let enabled = app.media.is_some();
        app.inspect_engine(move |engine, config| -> Result<_, ApiError> {
            verify_identity(&identity, engine)?;
            Ok(Json(json!({
                "enabled": enabled,
                "max_size": config.media.sticker_size_limit(),
                "limit": config.media.sticker_limit,
                "stickers": items,
            })))
        })
        .await
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Upload {
    size: u64,
}
pub(crate) async fn allocate(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<Upload>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_origin(&app, &headers)?;
    let storage = storage(&app)?;
    storage
        .check_reachable()
        .await
        .map_err(|cause| error(StatusCode::SERVICE_UNAVAILABLE, &cause))?;
    let access = Access::Browser(headers);
    let media = detached(&app, move |app| async move {
        let (identity, _) = native_context(&app, access).await?;
        let ordering = app.mutations.conversations(vec![]).await?;
        let pending = app
            .with_conversation(&ordering, move |engine| -> Result<_, ApiError> {
                verify_identity(&identity, engine)?;
                Ok(engine.prepare_sticker_allocation(&identity.user, input.size, None)?)
            })
            .await?;
        pending.save().await?;
        Ok(pending.media().clone())
    })
    .await?;
    let url = storage
        .upload_url_with_hints(&media.id, media.size, HeaderMap::new())
        .await
        .map_err(|_| {
            error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Cannot sign sticker upload.",
            )
        })?;
    Ok(Json(
        json!({"media_id":media.id,"upload":{"method":"PUT","url":url,"headers":{"Content-Type":"application/octet-stream"},"expires_in":300}}),
    ))
}

pub(crate) async fn complete(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_origin(&app, &headers)?;
    let storage = storage(&app)?;
    let access = Access::Browser(headers);
    owned(&app, &access, id.clone(), true).await?;
    let _object_lock = storage
        .lock_media(&id)
        .map_err(|cause| error(StatusCode::SERVICE_UNAVAILABLE, &cause))?;
    let mut media = owned(&app, &access, id, true).await?;
    if !media.ready {
        tokio::time::timeout(
            std::time::Duration::from_secs(100),
            storage.finalize(&mut media),
        )
        .await
        .map_err(|_| {
            error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Sticker verification timed out.",
            )
        })?
        .map_err(super::media::storage_failure)?;
        let result = stickers::check_type(&mut media).map_err(ApiError::from);
        media = finish(&app, &access, media, result).await?;
    }
    Ok(Json(sticker_json(&app, &media)))
}

pub(crate) async fn remove(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_origin(&app, &headers)?;
    let access = Access::Browser(headers);
    let media = owned(&app, &access, id, true).await?;
    detached(&app, move |app| async move {
        let (identity, store) = native_context(&app, access).await?;
        store.delete_sticker(&media.id, &identity.account).await?;
        Ok(Json(json!({"ok":true})))
    })
    .await
}

pub(crate) async fn download(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Redirect, ApiError> {
    security::check_read_origin(&app, &headers)?;
    let storage = storage(&app)?;
    let access = Access::Browser(headers);
    let media = owned(&app, &access, id, false).await?;
    let url = storage
        .download_url(&media, false)
        .await
        .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "Cannot sign sticker URL."))?;
    owned(&app, &access, media.id, false).await?;
    Ok(Redirect::temporary(&url))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Target {
    conversation_id: String,
}
pub(crate) async fn send(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<Target>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_origin(&app, &headers)?;
    let storage = storage(&app)?;
    let access = Access::Browser(headers);
    let sticker = owned(&app, &access, id, false).await?;
    let (mut media, reused) = detached(&app, {
        let access = access.clone();
        let sticker = sticker.clone();
        move |app| async move {
            let (identity, store) = native_context(&app, access).await?;
            let check = {
                let identity = identity.clone();
                let sticker = sticker.clone();
                let target = input.conversation_id.clone();
                move |engine: &crate::engine::Engine| -> Result<String, ApiError> {
                    verify_identity(&identity, engine)?;
                    engine.authorize_sticker_owner(&identity.user, &sticker, false)?;
                    Ok(engine.validate_media_target(
                        &identity.user,
                        &target,
                        &sticker.filename,
                        sticker.size,
                        &std::collections::BTreeMap::new(),
                    )?)
                }
            };
            let conversation = app
                .inspect_engine({
                    let check = check.clone();
                    move |engine, _| check(engine)
                })
                .await?;
            if let Some(copy) = store
                .sticker_copy(&conversation, &identity.account, &sticker.id)
                .await?
            {
                app.inspect_engine(move |engine, _| check(engine)).await?;
                return Ok((copy, true));
            }
            let ordering = app.mutations.conversation(conversation.clone()).await?;
            let pending = app
                .with_conversation(&ordering, {
                    let identity = identity.clone();
                    move |engine| -> Result<_, ApiError> {
                        verify_identity(&identity, engine)?;
                        let pending = engine.prepare_sticker_send(
                            &identity.user,
                            &sticker,
                            &input.conversation_id,
                        )?;
                        if pending.media.conversation != conversation {
                            return Err(crate::engine::EngineError::Conflict(
                                "Upload target changed; retry.".into(),
                            )
                            .into());
                        }
                        Ok(pending)
                    }
                })
                .await?;
            pending.save().await?;
            let media = app
                .with_conversation(&ordering, move |engine| -> Result<_, ApiError> {
                    let media = engine.publish_media_allocation(pending)?;
                    verify_identity(&identity, engine)?;
                    Ok(media)
                })
                .await?;
            Ok((media, false))
        }
    })
    .await?;
    if !reused {
        let _object_lock = storage
            .lock_media(&media.id)
            .map_err(|cause| error(StatusCode::SERVICE_UNAVAILABLE, &cause))?;
        let result = copy(&storage, &sticker, &mut media).await;
        media = finish(&app, &access, media, result).await?;
    }
    Ok(super::media::media_json(&app, &media))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Collect {
    media_id: String,
}
pub(crate) async fn collect(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<Collect>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_origin(&app, &headers)?;
    let storage = storage(&app)?;
    let access = Access::Browser(headers);
    let (source, mut media) = detached(&app, {
        let access = access.clone();
        move |app| async move {
            let (identity, store) = native_context(&app, access).await?;
            let (source, attached) = store
                .load(&input.media_id)
                .await
                .map_err(|cause| super::error::hidden(cause, MISSING))?;
            let ordering = app.mutations.conversations(vec![]).await?;
            let pending = app
                .with_conversation(&ordering, {
                    let source = source.clone();
                    move |engine| -> Result<_, ApiError> {
                        verify_identity(&identity, engine)?;
                        engine
                            .authorize_sticker_collect(&identity.user, &source, attached)
                            .map_err(|cause| match cause {
                                crate::engine::EngineError::Invalid(_) => cause.into(),
                                _ => super::error::hidden(cause, MISSING),
                            })?;
                        Ok(engine.prepare_sticker_allocation(
                            &identity.user,
                            source.size,
                            Some(source.id.clone()),
                        )?)
                    }
                })
                .await?;
            pending.save().await?;
            Ok((source, pending.media().clone()))
        }
    })
    .await?;
    let _object_lock = storage
        .lock_media(&media.id)
        .map_err(|cause| error(StatusCode::SERVICE_UNAVAILABLE, &cause))?;
    let result = copy(&storage, &source, &mut media).await;
    let media = finish(&app, &access, media, result).await?;
    Ok(Json(sticker_json(&app, &media)))
}
