use super::*;
use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::Redirect,
};
use serde::Deserialize;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Upload {
    conversation_id: String,
    filename: String,
    size: u64,
}

fn storage(app: &App) -> Result<std::sync::Arc<crate::media::MediaStorage>, ApiError> {
    app.media.clone().ok_or_else(|| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Media uploads are not configured.",
        )
    })
}
fn failed(message: &str) -> ApiError {
    error(StatusCode::BAD_REQUEST, message)
}
fn unavailable() -> ApiError {
    error(StatusCode::SERVICE_UNAVAILABLE, "Media worker unavailable.")
}

pub(crate) async fn config(
    State(app): State<App>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_read_origin_or_bot(&app, &headers)?;
    let permit = app
        .io_permit()
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        auth::authenticate(&app, &headers)?;
        Ok(Json(
            json!({"enabled":app.media.is_some(),"max_size":app.config.media.max_size}),
        ))
    })
    .await
    .map_err(|_| unavailable())?
}

pub(crate) async fn allocate(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<Upload>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_origin(&app, &headers)?;
    let storage = storage(&app)?;
    let _permit = app
        .io_permit()
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    let worker = app.clone();
    let credentials = headers.clone();
    let request = input.clone();
    let (conversation, uploader) = tokio::task::spawn_blocking(move || {
        let user = auth::authenticate(&worker, &credentials)?;
        let mut engine = worker.engine_api()?;
        let conversation = engine
            .resolve_media_conversation(&user, &request.conversation_id)
            .map_err(|e| failed(&e))?;
        engine
            .validate_media_allocation(&user, &conversation, &request.filename, request.size)
            .map_err(|e| failed(&e))?;
        let uploader = engine
            .data
            .users
            .get(&user)
            .ok_or_else(|| failed("Account unavailable."))?
            .id
            .clone();
        Ok::<_, ApiError>((conversation, uploader))
    })
    .await
    .map_err(|_| unavailable())??;
    // No reservation or upload URL is issued when storage cannot be reached.
    storage
        .check_reachable()
        .await
        .map_err(|message| error(StatusCode::SERVICE_UNAVAILABLE, &message))?;
    let media = tokio::task::spawn_blocking(move || {
        let user = auth::authenticate(&app, &headers)?;
        let engine = app.engine_read_api()?;
        if engine
            .data
            .users
            .get(&user)
            .is_none_or(|u| u.id != uploader)
        {
            return Err(failed("Account changed; retry upload."));
        }
        engine
            .allocate_media(&user, &conversation, &input.filename, input.size)
            .map_err(|e| failed(&e))
    })
    .await
    .map_err(|_| unavailable())??;
    let url = storage
        .upload_url(&media.id, media.size)
        .await
        .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "Cannot sign upload URL."))?;
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
    let _permit = app
        .io_permit()
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    let _gate = storage.gate.lock().await;
    let worker = app.clone();
    let credentials = headers.clone();
    let mut media = tokio::task::spawn_blocking(move || {
        let user = auth::authenticate(&worker, &credentials)?;
        worker
            .engine_read_api()?
            .media_owner(&user, &id)
            .map_err(|e| failed(&e))
    })
    .await
    .map_err(|_| unavailable())??;
    if !media.ready {
        storage.finalize(&mut media).await.map_err(|e| failed(&e))?;
        let worker = app.clone();
        let verified = media.clone();
        tokio::task::spawn_blocking(move || {
            let user = auth::authenticate(&worker, &headers)?;
            worker
                .engine_read_api()?
                .finish_media(&user, &verified)
                .map_err(|e| failed(&e))
        })
        .await
        .map_err(|_| unavailable())??;
    }
    Ok(Json(
        json!({"media_id":media.id,"path":format!("{}api/media/{}/file",app.config.base_url,media.id),"content_type":media.content_type,"size":media.size,"filename":media.filename}),
    ))
}

/// Read metadata only on demand; never probes storage or scans the file again.
pub(crate) async fn info(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_read_origin_or_bot(&app, &headers)?;
    let permit = app
        .io_permit()
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let user = auth::authenticate(&app, &headers)?;
        let media = app.engine_read_api()?.readable_media(&user, &id)
            .map_err(|_| error(StatusCode::NOT_FOUND, "Attachment not found."))?;
        Ok(Json(json!({"media_id":media.id,"path":format!("{}api/media/{}/file",app.config.base_url,media.id),"content_type":media.content_type,"size":media.size,"filename":media.filename})))
    }).await.map_err(|_| unavailable())?
}

#[derive(Default, Deserialize)]
pub(crate) struct Download {
    #[serde(default)]
    download: bool,
}
pub(crate) async fn download(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(options): Query<Download>,
) -> Result<Redirect, ApiError> {
    security::check_read_origin_or_bot(&app, &headers)?;
    let storage = storage(&app)?;
    let _permit = app
        .io_permit()
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    let worker = app.clone();
    let credentials = headers.clone();
    let media = tokio::task::spawn_blocking(move || {
        let user = auth::authenticate(&worker, &credentials)?;
        worker
            .engine_read_api()?
            .readable_media(&user, &id)
            .map_err(|_| error(StatusCode::NOT_FOUND, "Attachment not found."))
    })
    .await
    .map_err(|_| unavailable())??;
    let url = storage
        .download_url(&media, options.download)
        .await
        .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "Cannot sign download URL."))?;
    tokio::task::spawn_blocking(move || {
        let user = auth::authenticate(&app, &headers)?;
        app.engine_read_api()?
            .readable_media(&user, &media.id)
            .map_err(|_| error(StatusCode::NOT_FOUND, "Attachment not found."))
    })
    .await
    .map_err(|_| unavailable())??;
    Ok(Redirect::temporary(&url))
}

pub(crate) fn start_cleanup(app: App) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(
            app.config.media.cleanup_interval,
        ));
        loop {
            interval.tick().await;
            if app.stopping.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            if let Err(message) = cleanup(&app).await {
                tracing::warn!(%message, "Media cleanup deferred");
            }
        }
    });
}

async fn cleanup(app: &App) -> Result<(), String> {
    let Some(storage) = &app.media else {
        return Ok(());
    };
    let _gate = storage.gate.lock().await;
    let worker = app.clone();
    let expired = tokio::task::spawn_blocking(move || worker.engine_read()?.expired_media())
        .await
        .map_err(|_| "Cleanup worker unavailable.")??;
    for media in expired {
        let deletion = async {
            storage.delete(&format!("uploads/{}", media.id)).await?;
            storage.delete(&media.final_key()).await?;
            // A v13 crash could leave a final copy before its legacy key was saved.
            storage.delete(&format!("media/{}", media.id)).await?;
            if !media.object_key.is_empty() && media.object_key != media.final_key() {
                storage.delete(&media.object_key).await?;
            }
            Ok::<_, String>(())
        }
        .await;
        if deletion.is_err() {
            tracing::warn!("Media deletion deferred");
            continue;
        }
        let worker = app.clone();
        tokio::task::spawn_blocking(move || worker.engine_read()?.remove_media_record(&media.id))
            .await
            .map_err(|_| "Cleanup worker unavailable.")??;
    }
    let worker = app.clone();
    let ids = tokio::task::spawn_blocking(move || worker.engine_read()?.media_staging_cleanup())
        .await
        .map_err(|_| "Cleanup worker unavailable.")??;
    for id in ids {
        if storage.delete(&format!("uploads/{id}")).await.is_err() {
            tracing::warn!("Staging deletion deferred");
            continue;
        }
        let worker = app.clone();
        tokio::task::spawn_blocking(move || worker.engine_read()?.media_staging_cleaned(&id))
            .await
            .map_err(|_| "Cleanup worker unavailable.")??;
    }
    Ok(())
}

pub(crate) async fn abandon(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_origin(&app, &headers)?;
    let permit = app
        .io_permit()
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let user = auth::authenticate(&app, &headers)?;
        app.engine_read_api()?
            .abandon_media(&user, &id)
            .map_err(|e| failed(&e))?;
        Ok(Json(json!({"ok":true})))
    })
    .await
    .map_err(|_| unavailable())?
}
