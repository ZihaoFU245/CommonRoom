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
fn invalid(message: &str) -> ApiError {
    error(StatusCode::BAD_REQUEST, message)
}
fn unavailable() -> ApiError {
    error(StatusCode::SERVICE_UNAVAILABLE, "Media worker unavailable.")
}

/// Explicit service caller. Browser handlers cannot opt into machine authentication
/// merely by receiving an Authorization header.
#[derive(Clone)]
pub(crate) enum Access {
    Browser(HeaderMap),
    Bot {
        context: crate::bots::auth::Context,
        operation: &'static str,
    },
}
#[derive(Clone)]
pub(super) struct Identity {
    pub user: String,
    pub account: String,
    access: Access,
    config: Arc<Config>,
}
impl Access {
    fn user(&self, engine: &Engine, config: &Config) -> Result<String, ApiError> {
        match self {
            Self::Browser(access) => auth::identity(engine, config, access)
                .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Invalid or expired session.")),
            Self::Bot { context, operation } => {
                context
                    .permits(engine, operation, crate::bots::auth::Scope::Global)
                    .map_err(|cause| error(cause.status(), &cause.message))?;
                context
                    .user(engine)
                    .map_err(|cause| error(cause.status(), &cause.message))
            }
        }
    }
}
impl Identity {
    pub(super) fn permit_conversation(&self, engine: &Engine, id: &str) -> Result<(), ApiError> {
        if let Access::Bot { context, operation } = &self.access {
            context
                .permits(
                    engine,
                    operation,
                    crate::bots::auth::Scope::Conversation(id),
                )
                .map_err(|cause| error(cause.status(), &cause.message))?;
            if *operation == "media_upload" {
                for op in ["read", "send"] {
                    context
                        .permits(engine, op, crate::bots::auth::Scope::Conversation(id))
                        .map_err(|cause| error(cause.status(), &cause.message))?;
                }
            }
        }
        Ok(())
    }
    pub(super) fn avatar_conversations<'a>(
        &self,
        engine: &'a Engine,
    ) -> Result<Option<&'a std::collections::BTreeSet<String>>, ApiError> {
        match &self.access {
            Access::Browser(_) => Ok(None),
            Access::Bot { context, .. } => Ok(context
                .metadata(engine)
                .map_err(|cause| error(cause.status(), &cause.message))?
                .conversations
                .as_ref()),
        }
    }
}
pub(super) async fn native_context(
    app: &App,
    access: Access,
) -> Result<(Identity, crate::engine::MediaStore), ApiError> {
    if app.stopping.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Server is shutting down.",
        ));
    }
    let config = app.config.clone();
    app.inspect_engine(move |engine, _| {
        let user = access.user(engine, &config)?;
        let account = engine
            .active_account_id(&user)
            .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Account unavailable."))?
            .to_owned();
        Ok((
            Identity {
                user,
                account,
                access,
                config,
            },
            engine.media_store(),
        ))
    })
    .await
}
pub(super) fn verify_identity(identity: &Identity, engine: &Engine) -> Result<(), ApiError> {
    if identity.access.user(engine, &identity.config)? != identity.user
        || engine.active_account_id(&identity.user) != Some(identity.account.as_str())
    {
        return Err(error(StatusCode::UNAUTHORIZED, "Account unavailable."));
    }
    Ok(())
}
async fn native_media<T: Send + 'static>(
    app: &App,
    access: &Access,
    id: String,
    missing: &'static str,
    inspect: impl FnOnce(&Engine, &Identity, crate::engine::media::Media, bool) -> Result<T, ApiError>
    + Send
    + 'static,
) -> Result<T, ApiError> {
    let permit = app
        .io_permit()
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    let app = app.clone();
    let access = access.clone();
    tokio::spawn(async move {
        let _permit = permit;
        let (identity, store) = native_context(&app, access).await?;
        let (media, attached) = store
            .load(&id)
            .await
            .map_err(|cause| super::error::hidden(cause, missing))?;
        app.inspect_engine(move |engine, _| {
            verify_identity(&identity, engine)?;
            identity.permit_conversation(engine, &media.conversation)?;
            inspect(engine, &identity, media, attached)
        })
        .await
    })
    .await
    .map_err(|_| unavailable())?
}
async fn owner(
    app: &App,
    access: &Access,
    id: String,
) -> Result<crate::engine::media::Media, ApiError> {
    native_media(
        app,
        access,
        id,
        "Upload not found.",
        |engine, identity, media, attached| {
            engine.authorize_media_owner(&identity.user, &media, attached)?;
            Ok(media)
        },
    )
    .await
}
enum MediaMutation {
    Ready(
        Box<crate::engine::media::Media>,
        tokio::sync::OwnedMutexGuard<()>,
    ),
    Abandon(String),
}
async fn mutate(app: &App, access: &Access, mutation: MediaMutation) -> Result<(), ApiError> {
    let permit = app
        .io_permit()
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    let app = app.clone();
    let access = access.clone();
    tokio::spawn(async move {
        let _permit = permit;
        let (identity, store) = native_context(&app, access).await?;
        let ordering = app.mutations.conversations(vec![]).await?;
        let id = match &mutation {
            MediaMutation::Ready(media, _) => &media.id,
            MediaMutation::Abandon(id) => id,
        };
        let (current, attached) = store.load(id).await?;
        let mutation = app
            .with_conversation(&ordering, {
                let identity = identity.clone();
                move |engine| -> Result<_, ApiError> {
                    verify_identity(&identity, engine)?;
                    identity.permit_conversation(engine, &current.conversation)?;
                    match &mutation {
                        MediaMutation::Ready(_, _) => {
                            engine.authorize_media_owner(&identity.user, &current, attached)?;
                            if current.ready {
                                return Err(crate::engine::EngineError::Conflict(
                                    "Upload already completed.".into(),
                                )
                                .into());
                            }
                        }
                        MediaMutation::Abandon(_) => {
                            engine.authorize_media_abandon(&identity.user, &current)?
                        }
                    }
                    Ok(mutation)
                }
            })
            .await?;
        // Keep finalization exclusive until accepted database work finishes,
        // even if cancellation drops the outer HTTP future.
        let _object_lock = match mutation {
            MediaMutation::Ready(media, lock) => {
                store.ready(&media).await?;
                Some(lock)
            }
            MediaMutation::Abandon(id) => {
                store.abandon(&id, &identity.account).await?;
                None
            }
        };
        app.with_conversation(&ordering, move |engine| verify_identity(&identity, engine))
            .await
    })
    .await
    .map_err(|_| unavailable())?
}
async fn reserve(
    app: &App,
    identity: Identity,
    input: Upload,
) -> Result<crate::engine::media::Media, ApiError> {
    let permit = app
        .io_permit()
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    let app = app.clone();
    tokio::spawn(async move {
        let _permit = permit;
        let id = app
            .inspect_engine({
                let identity = identity.clone();
                let target = input.conversation_id.clone();
                let filename = input.filename.clone();
                let hints = input.hints.clone();
                move |engine, _| -> Result<_, ApiError> {
                    verify_identity(&identity, engine)?;
                    let id = engine.validate_media_target(
                        &identity.user,
                        &target,
                        &filename,
                        input.size,
                        &hints,
                    )?;
                    identity.permit_conversation(engine, &id)?;
                    Ok(id)
                }
            })
            .await?;
        let ordering = app.mutations.conversation(id.clone()).await?;
        let pending = app
            .with_conversation(&ordering, {
                let identity = identity.clone();
                move |engine| -> Result<_, ApiError> {
                    verify_identity(&identity, engine)?;
                    identity.permit_conversation(engine, &id)?;
                    let pending = engine.prepare_media_allocation(
                        &identity.user,
                        &input.conversation_id,
                        &input.filename,
                        input.size,
                        input.hints,
                    )?;
                    if pending.media.conversation != id {
                        return Err(crate::engine::EngineError::Conflict(
                            "Upload target changed; retry reservation.".into(),
                        )
                        .into());
                    }
                    Ok(pending)
                }
            })
            .await?;
        pending.save().await?;
        app.with_conversation(&ordering, move |engine| -> Result<_, ApiError> {
            let media = engine.publish_media_allocation(pending)?;
            verify_identity(&identity, engine)?;
            identity.permit_conversation(engine, &media.conversation)?;
            Ok(media)
        })
        .await
    })
    .await
    .map_err(|_| unavailable())?
}
pub(super) fn media_json(
    app: &App,
    media: &crate::engine::media::Media,
) -> Json<serde_json::Value> {
    Json(
        json!({"media_id":media.id,"path":format!("{}api/media/{}/file",app.config.base_url,media.id),"content_type":media.content_type,"size":media.size,"filename":media.filename}),
    )
}
pub(super) fn storage_failure(failure: crate::media::StorageError) -> ApiError {
    let status = match failure {
        crate::media::StorageError::Invalid(_) => StatusCode::BAD_REQUEST,
        crate::media::StorageError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
    };
    error(status, failure.message())
}

pub(crate) async fn config_as(
    State(app): State<App>,
    access: Access,
) -> Result<Json<serde_json::Value>, ApiError> {
    let permit = app
        .io_permit()
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    tokio::spawn(async move {
        let _permit = permit;
        let (identity, store) = native_context(&app, access).await?;
        store.health().await?;
        let enabled = app.media.is_some();
        app.inspect_engine(move |engine, config| {
            verify_identity(&identity, engine)?;
            Ok(Json(
                json!({"enabled":enabled,"max_size":config.media.max_size}),
            ))
        })
        .await
    })
    .await
    .map_err(|_| unavailable())?
}

pub(crate) async fn allocate_as(
    State(app): State<App>,
    access: Access,
    Json(input): Json<Upload>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let permit = app
        .io_permit()
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    let worker = app.clone();
    let (storage, identity, input) = tokio::spawn(async move {
        let _permit = permit;
        let (identity, store) = native_context(&worker, access).await?;
        let storage = storage(&worker)?;
        let input = worker
            .inspect_engine({
                let identity = identity.clone();
                move |engine, _| -> Result<_, ApiError> {
                    verify_identity(&identity, engine)?;
                    let id = engine.validate_media_target(
                        &identity.user,
                        &input.conversation_id,
                        &input.filename,
                        input.size,
                        &input.hints,
                    )?;
                    identity.permit_conversation(engine, &id)?;
                    Ok(input)
                }
            })
            .await?;
        store.health().await?;
        Ok::<_, ApiError>((storage, identity, input))
    })
    .await
    .map_err(|_| unavailable())??;
    // Live probe deliberately preserves rejection before reserving offline uploads.
    storage
        .check_reachable()
        .await
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    let media = reserve(&app, identity, input).await?;
    let hint_headers = crate::media::hint_headers(&media.hints).map_err(|e| invalid(&e))?;
    let mut upload_headers = hint_headers
        .iter()
        .map(|(key, value)| {
            value
                .to_str()
                .map(|value| (key.as_str().to_owned(), value.to_owned()))
                .map_err(|_| invalid("Invalid hint value."))
        })
        .collect::<Result<std::collections::BTreeMap<_, _>, _>>()?;
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

pub(crate) async fn complete_as(
    State(app): State<App>,
    access: Access,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let storage = storage(&app)?;
    // Authenticate before taking an object lock; recheck after it is acquired.
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
                "Media verification timed out; retry completion.",
            )
        })?
        .map_err(storage_failure)?;
        mutate(
            &app,
            &access,
            MediaMutation::Ready(Box::new(media.clone()), object_lock),
        )
        .await?;
    }
    Ok(media_json(&app, &media))
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MediaInfoQuery {
    #[serde(default)]
    include_expired: bool,
}
pub(crate) async fn info_as(
    State(app): State<App>,
    access: Access,
    Path(id): Path<String>,
    Query(options): Query<MediaInfoQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let media = native_media(
        &app,
        &access,
        id,
        "Attachment not found.",
        move |engine, identity, media, attached| {
            engine
                .authorize_media_read(&identity.user, &media, attached, options.include_expired)
                .map_err(|cause| super::error::hidden(cause, "Attachment not found."))?;
            Ok(media)
        },
    )
    .await?;
    let Json(mut value) = media_json(&app, &media);
    value["expires"] = json!(media.expires);
    value["expired"] = json!(
        media
            .expires
            .is_some_and(|time| time <= crate::engine::media::timestamp())
    );
    Ok(Json(value))
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Download {
    #[serde(default)]
    download: bool,
}
pub(crate) async fn download_as(
    State(app): State<App>,
    access: Access,
    Path(id): Path<String>,
    Query(options): Query<Download>,
) -> Result<Redirect, ApiError> {
    let storage = storage(&app)?;
    let ticket = native_media(
        &app,
        &access,
        id,
        "Attachment not found.",
        |engine, identity, media, attached| {
            engine
                .prepare_media_download_record(&identity.user, media, attached)
                .map_err(|cause| super::error::hidden(cause, "Attachment not found."))
        },
    )
    .await?;
    let url = storage
        .download_url(&ticket.media, options.download)
        .await
        .map_err(|cause| {
            tracing::warn!(media_id = %ticket.media.id, error = %cause, "Download signing failed");
            error(StatusCode::SERVICE_UNAVAILABLE, "Cannot sign download URL.")
        })?;
    native_media(
        &app,
        &access,
        ticket.media.id.clone(),
        "Attachment not found.",
        move |engine, identity, current, attached| {
            engine
                .validate_media_download_record(&identity.user, &ticket, &current, attached)
                .map_err(|cause| super::error::hidden(cause, "Attachment not found."))
        },
    )
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
            if app.stopping.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            tokio::select! {
                () = app.shutdown.notified() => break,
                () = tokio::time::sleep(delay) => {},
            }
            if app.stopping.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            let result = tokio::select! {
                () = app.shutdown.notified() => break,
                result = tokio::time::timeout(std::time::Duration::from_secs(20), cleanup(&app)) => result,
            };
            delay = match result {
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

async fn cleanup_db<T, F>(
    app: &App,
    work: impl FnOnce(crate::engine::MediaStore) -> F + Send + 'static,
) -> Result<T, String>
where
    T: Send + 'static,
    F: std::future::Future<Output = Result<T, crate::engine::EngineError>> + Send + 'static,
{
    let permit = app.io_permit()?;
    let app = app.clone();
    tokio::spawn(async move {
        let _permit = permit;
        let store = app
            .inspect_engine(|engine, _| Ok::<_, crate::engine::EngineError>(engine.media_store()))
            .await
            .map_err(String::from)?;
        work(store).await.map_err(String::from)
    })
    .await
    .map_err(|_| "Cleanup worker unavailable.")?
}
async fn delete_media(
    app: App,
    media: crate::engine::media::Media,
) -> Result<
    (
        crate::engine::media::Media,
        tokio::sync::OwnedMutexGuard<()>,
    ),
    String,
> {
    let storage = app.media.as_ref().ok_or("Storage unavailable.")?;
    let lock = storage.lock_media(&media.id)?;
    let keys =
        std::collections::BTreeSet::from([format!("uploads/{}", media.id), media.final_key()]);
    storage.delete_keys(&keys).await?;
    Ok((media, lock))
}

async fn delete_staging(
    app: App,
    id: String,
) -> Result<(String, tokio::sync::OwnedMutexGuard<()>), String> {
    let storage = app.media.as_ref().ok_or("Storage unavailable.")?;
    let lock = storage.lock_media(&id)?;
    storage.delete(&format!("uploads/{id}")).await?;
    Ok((id, lock))
}

pub(super) async fn cleanup(app: &App) -> Result<bool, String> {
    if app.media.is_none() {
        return Ok(true);
    }
    let start = std::time::Instant::now();
    let attempted_before = crate::engine::media::timestamp();
    let mut deleted = 0;
    let mut failed = 0;
    loop {
        let rows = cleanup_db(app, move |store| async move {
            store.expired(attempted_before).await
        })
        .await?;
        let count = rows.len();
        let mut jobs = tokio::task::JoinSet::new();
        let mut completed = Vec::new();
        for media in rows {
            let worker = app.clone();
            jobs.spawn(delete_media(worker, media));
            if jobs.len() == 4 {
                match jobs.join_next().await {
                    Some(Ok(Ok(record))) => completed.push(record),
                    _ => failed += 1,
                }
            }
        }
        while let Some(result) = jobs.join_next().await {
            match result {
                Ok(Ok(record)) => completed.push(record),
                _ => failed += 1,
            }
        }
        if !completed.is_empty() {
            deleted += completed.len();
            cleanup_db(app, move |store| async move {
                // Retain every object lock through the detached acknowledgement.
                let records: Vec<_> = completed.iter().map(|(media, _)| media.clone()).collect();
                store.cleanup_done(&records, &[]).await?;
                drop(completed);
                Ok(())
            })
            .await?;
        }
        if count < app.config.media.cleanup_batch {
            break;
        }
    }
    loop {
        let ids = cleanup_db(app, move |store| async move {
            store.staging(attempted_before).await
        })
        .await?;
        let count = ids.len();
        let mut completed = Vec::new();
        let mut jobs = tokio::task::JoinSet::new();
        for id in ids {
            jobs.spawn(delete_staging(app.clone(), id));
            if jobs.len() == 4 {
                match jobs.join_next().await {
                    Some(Ok(Ok(record))) => completed.push(record),
                    _ => failed += 1,
                }
            }
        }
        while let Some(result) = jobs.join_next().await {
            match result {
                Ok(Ok(record)) => completed.push(record),
                _ => failed += 1,
            }
        }
        if !completed.is_empty() {
            cleanup_db(app, move |store| async move {
                let ids: Vec<_> = completed.iter().map(|(id, _)| id.clone()).collect();
                store.cleanup_done(&[], &ids).await?;
                drop(completed);
                Ok(())
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

pub(crate) async fn abandon_as(
    State(app): State<App>,
    access: Access,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    mutate(&app, &access, MediaMutation::Abandon(id)).await?;
    Ok(Json(json!({"ok":true})))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "media_tests.rs"]
pub(crate) mod tests;

pub(crate) async fn config(
    State(app): State<App>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_read_origin(&app, &headers)?;
    config_as(State(app), super::media::Access::Browser(headers)).await
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

pub(crate) async fn abandon(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_origin(&app, &headers)?;
    abandon_as(State(app), super::media::Access::Browser(headers), Path(id)).await
}

pub(crate) async fn info(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(options): Query<MediaInfoQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_read_origin(&app, &headers)?;
    info_as(
        State(app),
        super::media::Access::Browser(headers),
        Path(id),
        Query(options),
    )
    .await
}

pub(crate) async fn download(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(options): Query<Download>,
) -> Result<Redirect, ApiError> {
    security::check_read_origin(&app, &headers)?;
    download_as(
        State(app),
        super::media::Access::Browser(headers),
        Path(id),
        Query(options),
    )
    .await
}
