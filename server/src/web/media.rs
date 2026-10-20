use super::*;
use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::Redirect,
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Upload {
    conversation_id: String,
    filename: String,
    size: u64,
    #[serde(default)]
    hints: std::collections::BTreeMap<String, String>,
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

// Each blocking job owns its API permit. No permit survives into S3 I/O or locks.
async fn authenticated<T: Send + 'static>(
    app: &App,
    headers: &HeaderMap,
    work: impl FnOnce(&App, &str) -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    let permit = app
        .io_permit()
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    let worker = app.clone();
    let credentials = headers.clone();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let user = auth::authenticate(&worker, &credentials)?;
        work(&worker, &user)
    })
    .await
    .map_err(|_| unavailable())?
}
fn media_json(app: &App, media: &crate::engine::media::Media) -> Json<serde_json::Value> {
    Json(
        json!({"media_id":media.id,"path":format!("{}api/media/{}/file",app.config.base_url,media.id),"content_type":media.content_type,"size":media.size,"filename":media.filename}),
    )
}
fn storage_failure(failure: crate::media::StorageError) -> ApiError {
    let status = match failure {
        crate::media::StorageError::Invalid(_) => StatusCode::BAD_REQUEST,
        crate::media::StorageError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
    };
    error(status, failure.message())
}

pub(crate) async fn config(
    State(app): State<App>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_read_origin_or_bot(&app, &headers)?;
    authenticated(&app, &headers, |app, _| {
        Ok(Json(
            json!({"enabled":app.media.is_some(),"max_size":app.config.media.max_size}),
        ))
    })
    .await
}

pub(crate) async fn allocate(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<Upload>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_origin(&app, &headers)?;
    let storage = storage(&app)?;
    crate::media::hint_headers(&input.hints).map_err(|e| failed(&e))?;
    let (conversation, uploader, input) = authenticated(&app, &headers, move |app, user| {
        let mut engine = app.engine_api()?;
        let conversation = engine
            .resolve_media_conversation(user, &input.conversation_id)
            .map_err(|e| failed(&e))?;
        engine
            .validate_media_allocation(user, &conversation, &input.filename, input.size)
            .map_err(|e| failed(&e))?;
        let uploader = engine
            .data
            .users
            .get(user)
            .ok_or_else(|| failed("Account unavailable."))?
            .id
            .clone();
        Ok((conversation, uploader, input))
    })
    .await?;
    // Live probe deliberately preserves rejection before reserving offline uploads.
    storage
        .check_reachable()
        .await
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    let media = authenticated(&app, &headers, move |app, user| {
        let engine = app.engine_read_api()?;
        if engine.data.users.get(user).is_none_or(|u| u.id != uploader) {
            return Err(failed("Account changed; retry upload."));
        }
        engine
            .allocate_media_with_hints(
                user,
                &conversation,
                &input.filename,
                input.size,
                input.hints,
            )
            .map_err(|e| failed(&e))
    })
    .await?;
    let hint_headers = crate::media::hint_headers(&media.hints).map_err(|e| failed(&e))?;
    let mut upload_headers = media
        .hints
        .iter()
        .map(|(key, value)| (format!("x-amz-meta-client-{key}"), value.clone()))
        .collect::<std::collections::BTreeMap<_, _>>();
    upload_headers.insert("Content-Type".into(), "application/octet-stream".into());
    let url = storage
        .upload_url_with_hints(&media.id, media.size, hint_headers)
        .await
        .map_err(|cause| {
            tracing::warn!(media_id = %media.id, error = %cause, "Upload signing failed");
            error(StatusCode::SERVICE_UNAVAILABLE, "Cannot sign upload URL.")
        })?;
    Ok(Json(
        json!({"media_id":media.id,"upload":{"method":"PUT","url":url,"headers":upload_headers,"expires_in":300}}),
    ))
}

pub(crate) async fn complete(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_origin(&app, &headers)?;
    let storage = storage(&app)?;
    // Authenticate before taking an object lock; recheck after it is acquired.
    let media_id = id.clone();
    authenticated(&app, &headers, move |app, user| {
        app.engine_read_api()?
            .media_owner(user, &media_id)
            .map_err(|e| failed(&e))
    })
    .await?;
    let _lock = storage
        .lock_media(&id)
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    let mut media = authenticated(&app, &headers, move |app, user| {
        app.engine_read_api()?
            .media_owner(user, &id)
            .map_err(|e| failed(&e))
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
                "Media verification timed out; retry completion.",
            )
        })?
        .map_err(storage_failure)?;
        let verified = media.clone();
        authenticated(&app, &headers, move |app, user| {
            app.engine_read_api()?
                .finish_media(user, &verified)
                .map_err(|e| failed(&e))
        })
        .await?;
    }
    Ok(media_json(&app, &media))
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
            .readable_media(user, &id)
            .map_err(|_| error(StatusCode::NOT_FOUND, "Attachment not found."))?;
        Ok(media_json(app, &media))
    })
    .await
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
    let media = authenticated(&app, &headers, move |app, user| {
        app.engine_read_api()?
            .readable_media(user, &id)
            .map_err(|_| error(StatusCode::NOT_FOUND, "Attachment not found."))
    })
    .await?;
    let url = storage
        .download_url(&media, options.download)
        .await
        .map_err(|cause| {
            tracing::warn!(media_id = %media.id, error = %cause, "Download signing failed");
            error(StatusCode::SERVICE_UNAVAILABLE, "Cannot sign download URL.")
        })?;
    authenticated(&app, &headers, move |app, user| {
        app.engine_read_api()?
            .readable_media(user, &media.id)
            .map_err(|_| error(StatusCode::NOT_FOUND, "Attachment not found."))
    })
    .await?;
    Ok(Redirect::temporary(&url))
}

pub(crate) fn start_cleanup(app: App) {
    if app.media.is_none() {
        return;
    }
    tokio::spawn(async move {
        let normal = std::time::Duration::from_secs(app.config.media.cleanup_interval);
        let retry = normal.min(std::time::Duration::from_secs(60));
        let mut delay = std::time::Duration::ZERO;
        loop {
            tokio::time::sleep(delay).await;
            if app.stopping.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            delay = match tokio::time::timeout(std::time::Duration::from_secs(20), cleanup(&app))
                .await
            {
                Ok(Ok(true)) => normal,
                Ok(Ok(false)) => retry,
                Ok(Err(message)) => {
                    tracing::warn!(%message, "Media cleanup deferred");
                    retry
                }
                Err(_) => {
                    tracing::warn!("Media cleanup time budget reached; continuing shortly");
                    retry
                }
            };
        }
    });
}

async fn cleanup_db<T: Send + 'static>(
    app: &App,
    work: impl FnOnce(&App) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let permit = app.io_permit()?;
    let worker = app.clone();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work(&worker)
    })
    .await
    .map_err(|_| "Cleanup worker unavailable.")?
}
async fn delete_media(app: App, media: crate::engine::media::Media) -> Result<(), String> {
    let storage = app.media.as_ref().ok_or("Storage unavailable.")?;
    let _lock = storage.lock_media(&media.id)?;
    let keys =
        std::collections::BTreeSet::from([format!("uploads/{}", media.id), media.final_key()]);
    storage.delete_keys(&keys).await?;
    cleanup_db(&app, move |app| {
        let engine = app.engine_read()?;
        // A PUT admitted before URL expiry can finish after an early cleanup.
        // Keep staging recovery due until the pending grace period has elapsed.
        if media.created.saturating_add(app.config.media.pending_ttl)
            <= crate::engine::media::timestamp()
        {
            engine.media_staging_cleaned(&media.id)?;
        }
        engine.remove_media_record(&media.id)
    })
    .await
}
async fn cleanup(app: &App) -> Result<bool, String> {
    if app.media.is_none() {
        return Ok(true);
    }
    let start = std::time::Instant::now();
    let attempted_before = crate::engine::media::timestamp();
    let mut deleted = 0;
    let mut failed = 0;
    loop {
        let rows = cleanup_db(app, move |app| {
            app.engine_read()?.expired_media_before(attempted_before)
        })
        .await?;
        let count = rows.len();
        let mut jobs = tokio::task::JoinSet::new();
        for media in rows {
            let worker = app.clone();
            jobs.spawn(delete_media(worker, media));
            if jobs.len() == 4 {
                match jobs.join_next().await {
                    Some(Ok(Ok(()))) => deleted += 1,
                    _ => failed += 1,
                }
            }
        }
        while let Some(result) = jobs.join_next().await {
            if matches!(result, Ok(Ok(()))) {
                deleted += 1;
            } else {
                failed += 1;
            }
        }
        if count < app.config.media.cleanup_batch {
            break;
        }
    }
    loop {
        let ids = cleanup_db(app, move |app| {
            app.engine_read()?.media_staging_cleanup(attempted_before)
        })
        .await?;
        let count = ids.len();
        for id in ids {
            let storage = app.media.as_ref().ok_or("Storage unavailable.")?;
            let Ok(_lock) = storage.lock_media(&id) else {
                failed += 1;
                continue;
            };
            if storage.delete(&format!("uploads/{id}")).await.is_err() {
                failed += 1;
                continue;
            }
            cleanup_db(app, move |app| {
                app.engine_read()?.media_staging_cleaned(&id)
            })
            .await?;
        }
        if count < app.config.media.cleanup_batch {
            break;
        }
    }
    tracing::info!(
        deleted,
        failed,
        elapsed_ms = start.elapsed().as_millis(),
        "Media cleanup finished"
    );
    Ok(failed == 0)
}

pub(crate) async fn abandon(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_origin(&app, &headers)?;
    authenticated(&app, &headers, move |app, user| {
        app.engine_read_api()?
            .abandon_media(user, &id)
            .map_err(|e| failed(&e))?;
        Ok(Json(json!({"ok":true})))
    })
    .await
}
