use super::media::authenticated;
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
pub(crate) async fn set(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<SetAvatar>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_origin(&app, &headers)?;
    authenticated(&app, &headers, move |app, user| {
        let mut engine = app.engine_api()?;
        let account = engine
            .active_account_id(user)
            .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Account unavailable."))?
            .to_owned();
        if !engine.avatar_matches(user, input.url.as_deref()) {
            app.admission.avatar_change(&account)?;
        }
        if engine
            .set_avatar(user, input.url, false)
            .map_err(ApiError::from)?
        {
            let _ = app.changes.send(Change::Avatar(account));
        }
        Ok(Json(json!({"ok":true})))
    })
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Upload {
    size: u64,
    upload_id: Option<String>,
}
pub(crate) async fn allocate(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<Upload>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_origin(&app, &headers)?;
    let (storage, media) = authenticated(&app, &headers, move |app, user| {
        let storage = storage(app)?;
        let media = app
            .engine_api()?
            .allocate_avatar_id(
                user,
                input.size,
                &input
                    .upload_id
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            )
            .map_err(ApiError::from)?;
        Ok((storage, media))
    })
    .await?;
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
pub(crate) async fn complete(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_origin(&app, &headers)?;
    let storage = storage(&app)?;
    let _lock = storage
        .lock_media(&id)
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    let mut media = authenticated(&app, &headers, move |app, user| {
        app.engine_read_api()?
            .avatar_owner(user, &id)
            .map_err(ApiError::from)
    })
    .await?;
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
    authenticated(&app, &headers, move |app, user| {
        let mut engine = app.engine_api()?;
        let account = engine.active_account_id(user).ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Account unavailable."))?.to_owned();
        if !engine.avatar_matches(user, Some(&crate::engine::avatars::avatar_path(&media.id))) { app.admission.avatar_change(&account)?; }
        if engine.finish_avatar(user,&media).map_err(ApiError::from)? {
            let _=app.changes.send(Change::Avatar(account));
        }
        Ok(Json(json!({"ok":true,"media_id":media.id,"path":crate::engine::avatars::avatar_path(&media.id)})))
    })
    .await
}
pub(crate) async fn info(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_read_origin_or_bot(&app, &headers)?;
    authenticated(&app, &headers, move |app, user| {
        let media = app
            .engine_read_api()?
            .readable_avatar(user, &id)
            .map_err(|e| super::error::hidden(e, "Avatar not found."))?;
        Ok(Json(
            json!({"size":media.size,"content_type":media.content_type,"filename":media.filename}),
        ))
    })
    .await
}
pub(crate) async fn download(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Redirect, ApiError> {
    security::check_read_origin_or_bot(&app, &headers)?;
    let storage = storage(&app)?;
    let avatar = authenticated(&app, &headers, move |app, user| {
        app.engine_read_api()?
            .readable_avatar(user, &id)
            .map_err(|e| super::error::hidden(e, "Avatar not found."))
    })
    .await?;
    let url = storage
        .download_url(&avatar, false)
        .await
        .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "Cannot sign avatar URL."))?;
    authenticated(&app, &headers, move |app, user| {
        app.engine_read_api()?
            .readable_avatar(user, &avatar.id)
            .map_err(|e| super::error::hidden(e, "Avatar not found."))
    })
    .await?;
    Ok(Redirect::temporary(&url))
}
