//! Browser sticker libraries. Entries reference shared sticker objects that are
//! read and sent through the ordinary `/api/media/{id}/file` links.
use super::media::{Access, native_context, verify_identity};
use super::*;
use crate::engine::{media::Media, stickers};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
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
    let mut value = json!({"id":media.id,"path":format!("{}api/media/{}/file",app.config.base_url,media.id),"content_type":media.content_type,"size":media.size});
    if let Some(digest) = &media.content_digest {
        value["digest"] = json!(digest);
    }
    value
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

async fn upload(app: &App, access: &Access, id: String) -> Result<Media, ApiError> {
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
                .authorize_sticker_upload(&identity.user, &media)
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
        let (current, _) = store.load(&media.id).await?;
        let ready = current.ready;
        app.inspect_engine({
            let identity = identity.clone();
            move |engine, _| -> Result<_, ApiError> {
                verify_identity(&identity, engine)?;
                engine.authorize_sticker_upload(&identity.user, &current)?;
                Ok(())
            }
        })
        .await?;
        if let Some(digest) = media.content_digest.as_deref()
            && store
                .sticker_digest_taken(&identity.account, digest)
                .await?
        {
            store.abandon(&media.id, &identity.account).await?;
            return Err(error(
                StatusCode::CONFLICT,
                "This sticker is already in your library.",
            ));
        }
        if !ready {
            if let Err(cause) = store.ready(&media).await {
                store.abandon(&media.id, &identity.account).await?;
                return Err(cause.into());
            }
        }
        Ok(media)
    })
    .await
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
                Ok(engine.prepare_sticker_allocation(&identity.user, input.size)?)
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
    upload(&app, &access, id.clone()).await?;
    let _object_lock = storage
        .lock_media(&id)
        .map_err(|cause| error(StatusCode::SERVICE_UNAVAILABLE, &cause))?;
    let mut media = upload(&app, &access, id).await?;
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
        let result = match stickers::check_type(&mut media) {
            Ok(()) => match storage.content_digest(&media).await {
                Ok(digest) => {
                    media.content_digest = Some(digest);
                    Ok(())
                }
                Err(failure) => Err(super::media::storage_failure(failure)),
            },
            Err(cause) => Err(ApiError::from(cause)),
        };
        media = finish(&app, &access, media, result).await?;
    }
    Ok(Json(sticker_json(&app, &media)))
}

/// Removes the library entry only; sent messages keep showing the sticker.
pub(crate) async fn remove(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_origin(&app, &headers)?;
    let access = Access::Browser(headers);
    detached(&app, move |app| async move {
        let (identity, store) = native_context(&app, access).await?;
        app.inspect_engine({
            let identity = identity.clone();
            move |engine, _| verify_identity(&identity, engine)
        })
        .await?;
        store
            .delete_sticker(&id, &identity.account)
            .await
            .map_err(|cause| super::error::hidden(cause, MISSING))?;
        Ok(Json(json!({"ok":true})))
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Collect {
    media_id: String,
}
/// Adds a sticker the account can see in a conversation to its library.
pub(crate) async fn collect(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<Collect>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_origin(&app, &headers)?;
    storage(&app)?;
    let access = Access::Browser(headers);
    detached(&app, move |app| async move {
        let (identity, store) = native_context(&app, access).await?;
        let (media, _) = store
            .load(&input.media_id)
            .await
            .map_err(|cause| super::error::hidden(cause, MISSING))?;
        let reach = store.sticker_reach(&media.id, &identity.account).await?;
        let media = app
            .inspect_engine({
                let identity = identity.clone();
                move |engine, _| -> Result<_, ApiError> {
                    verify_identity(&identity, engine)?;
                    engine
                        .authorize_sticker_collect(&identity.user, &media, &reach)
                        .map_err(|cause| super::error::hidden(cause, MISSING))?;
                    Ok(media)
                }
            })
            .await?;
        store
            .collect_sticker(&media.id, &identity.account)
            .await
            .map_err(|cause| match cause {
                crate::engine::EngineError::Invalid(_) => cause.into(),
                _ => super::error::hidden(cause, MISSING),
            })?;
        Ok(Json(sticker_json(&app, &media)))
    })
    .await
}
