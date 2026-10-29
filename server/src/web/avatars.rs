use super::media::Access;
use super::*;
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Redirect,
};
use serde::Deserialize;

fn storage(app: &App) -> Result<Arc<crate::media::MediaStorage>, ApiError> {
    app.media.clone().ok_or_else(|| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Avatar uploads are not configured.",
        )
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetAvatar {
    url: Option<String>,
}
pub(crate) async fn set_as(
    State(app): State<App>,
    access: Access,
    Json(input): Json<SetAvatar>,
) -> Result<Json<serde_json::Value>, ApiError> {
    super::profiles::update(&app, access, super::profiles::Update::Avatar(input.url)).await?;
    Ok(Json(json!({"ok":true})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Upload {
    size: u64,
    upload_id: Option<String>,
}
pub(crate) async fn allocate_as(
    State(app): State<App>,
    access: Access,
    Json(input): Json<Upload>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let permit = app
        .io_permit()
        .map_err(|cause| error(StatusCode::SERVICE_UNAVAILABLE, &cause))?;
    let worker = app.clone();
    let (storage, media) = tokio::spawn(async move {
        let _permit = permit;
        let (identity, _) = super::media::native_context(&worker, access).await?;
        let storage = storage(&worker)?;
        let ordering = worker.mutations.conversations(vec![]).await?;
        let pending = worker
            .with_conversation(&ordering, {
                let identity = identity.clone();
                move |engine| -> Result<_, ApiError> {
                    super::media::verify_identity(&identity, engine)?;
                    Ok(engine.prepare_avatar_allocation(
                        &identity.user,
                        input.size,
                        input
                            .upload_id
                            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                    )?)
                }
            })
            .await?;
        let media = pending.save().await?;
        let media = worker
            .with_conversation(&ordering, move |engine| -> Result<_, ApiError> {
                super::media::verify_identity(&identity, engine)?;
                engine.authorize_avatar_owner(&identity.user, &media)?;
                Ok(media)
            })
            .await?;
        Ok::<_, ApiError>((storage, media))
    })
    .await
    .map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Avatar worker unavailable.",
        )
    })??;
    let url = storage
        .upload_url_with_hints(&media.id, media.size, HeaderMap::new())
        .await
        .map_err(|_| {
            error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Cannot sign avatar upload.",
            )
        })?;
    Ok(Json(
        json!({"media_id":media.id,"upload":{"method":"PUT","url":url,"headers":{"Content-Type":"application/octet-stream"},"expires_in":300}}),
    ))
}
pub(crate) async fn complete_as(
    State(app): State<App>,
    access: Access,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let storage = storage(&app)?;
    owner(&app, &access, id.clone()).await?;
    let object_lock = storage
        .lock_media(&id)
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    let mut media = owner(&app, &access, id).await?;
    if !media.ready {
        tokio::time::timeout(
            std::time::Duration::from_secs(100),
            storage.finalize(&mut media),
        )
        .await
        .map_err(|_| {
            error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Avatar verification timed out.",
            )
        })?
        .map_err(super::media::storage_failure)?;
    }
    let id = media.id.clone();
    super::profiles::update(
        &app,
        access,
        super::profiles::Update::Complete(media, object_lock),
    )
    .await?;
    Ok(Json(
        json!({"ok":true,"media_id":id,"path":crate::engine::avatars::avatar_path(&id)}),
    ))
}
pub(crate) async fn info_as(
    State(app): State<App>,
    access: Access,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let media = readable(&app, &access, id).await?;
    Ok(Json(
        json!({"size":media.size,"content_type":media.content_type,"filename":media.filename}),
    ))
}
pub(crate) async fn download_as(
    State(app): State<App>,
    access: Access,
    Path(id): Path<String>,
) -> Result<Redirect, ApiError> {
    let storage = storage(&app)?;
    let avatar = readable(&app, &access, id).await?;
    let url = storage
        .download_url(&avatar, false)
        .await
        .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "Cannot sign avatar URL."))?;
    readable(&app, &access, avatar.id).await?;
    Ok(Redirect::temporary(&url))
}

async fn owner(
    app: &App,
    access: &Access,
    id: String,
) -> Result<crate::engine::media::Media, ApiError> {
    let permit = app
        .io_permit()
        .map_err(|cause| error(StatusCode::SERVICE_UNAVAILABLE, &cause))?;
    let app = app.clone();
    let access = access.clone();
    tokio::spawn(async move {
        let _permit = permit;
        let (identity, store) = super::media::native_context(&app, access).await?;
        let media = store.load(&id).await?.0;
        app.inspect_engine(move |engine, _| -> Result<_, ApiError> {
            super::media::verify_identity(&identity, engine)?;
            engine.authorize_avatar_owner(&identity.user, &media)?;
            Ok(media)
        })
        .await
    })
    .await
    .map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Avatar worker unavailable.",
        )
    })?
}
async fn readable(
    app: &App,
    access: &Access,
    id: String,
) -> Result<crate::engine::media::Media, ApiError> {
    let permit = app
        .io_permit()
        .map_err(|cause| error(StatusCode::SERVICE_UNAVAILABLE, &cause))?;
    let app = app.clone();
    let access = access.clone();
    tokio::spawn(async move {
        let _permit = permit;
        for _ in 0..2 {
            let (identity, store) = super::media::native_context(&app, access.clone()).await?;
            let media = store
                .load(&id)
                .await
                .map_err(|cause| super::error::hidden(cause, "Avatar not found."))?
                .0;
            let (media, visibility, ids, stamp) = app
                .inspect_engine({
                    let identity = identity.clone();
                    move |engine, _| -> Result<_, ApiError> {
                        super::media::verify_identity(&identity, engine)?;
                        let visibility = engine
                            .avatar_read_plan_scoped(
                                &identity.user,
                                &media,
                                identity.avatar_conversations(engine)?,
                            )
                            .map_err(|cause| super::error::hidden(cause, "Avatar not found."))?;
                        let ids = visibility.history_ids().map(str::to_owned).collect();
                        let stamp = engine.delivery_stamp(&identity.user, &ids, false)?;
                        Ok((media, visibility, ids, stamp))
                    }
                })
                .await?;
            let visible = visibility.finish_async().await?.is_some();
            store.health().await?;
            let result = app
                .inspect_engine(move |engine, _| -> Result<_, ApiError> {
                    super::media::verify_identity(&identity, engine)?;
                    if engine
                        .delivery_stamp(&identity.user, &ids, false)
                        .ok()
                        .as_ref()
                        != Some(&stamp)
                    {
                        return Ok(None);
                    }
                    engine
                        .avatar_read_plan_scoped(
                            &identity.user,
                            &media,
                            identity.avatar_conversations(engine)?,
                        )
                        .map_err(|cause| super::error::hidden(cause, "Avatar not found."))?;
                    if !visible {
                        return Err(error(StatusCode::NOT_FOUND, "Avatar not found."));
                    }
                    Ok(Some(media))
                })
                .await?;
            if let Some(media) = result {
                return Ok(media);
            }
        }
        Err(error(
            StatusCode::CONFLICT,
            "Avatar visibility changed; retry shortly.",
        ))
    })
    .await
    .map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Avatar worker unavailable.",
        )
    })?
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "avatar_tests.rs"]
mod tests;

pub(crate) async fn set(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<SetAvatar>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_origin(&app, &headers)?;
    set_as(
        State(app),
        super::media::Access::Browser(headers),
        Json(input),
    )
    .await
}

pub(crate) async fn allocate(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<Upload>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_origin(&app, &headers)?;
    allocate_as(
        State(app),
        super::media::Access::Browser(headers),
        Json(input),
    )
    .await
}

pub(crate) async fn complete(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_origin(&app, &headers)?;
    complete_as(State(app), super::media::Access::Browser(headers), Path(id)).await
}

pub(crate) async fn info(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_read_origin(&app, &headers)?;
    info_as(State(app), super::media::Access::Browser(headers), Path(id)).await
}

pub(crate) async fn download(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Redirect, ApiError> {
    security::check_read_origin(&app, &headers)?;
    download_as(State(app), super::media::Access::Browser(headers), Path(id)).await
}
