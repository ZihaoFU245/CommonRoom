//! Explicit machine adapters for the shared media/profile services.
//! Sharing services does not expose browser routes to bearer credentials.
use super::{auth::Context, error::Error};
use crate::web::{App, media::Access};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
    response::Redirect,
};
use serde_json::Value;

async fn access(app: &App, headers: &HeaderMap, operation: &'static str) -> Result<Access, Error> {
    let context = Context::authenticate_async(app, headers).await?;
    app.inspect_engine({
        let context = context.clone();
        move |engine, _| context.permits(engine, operation, super::auth::Scope::Global)
    })
    .await?;
    Ok(Access::Bot { context, operation })
}

pub(super) async fn media_config(
    State(app): State<App>,
    headers: HeaderMap,
) -> Result<Json<Value>, Error> {
    let access = access(&app, &headers, "read").await?;
    crate::web::media::config_as(State(app), access)
        .await
        .map_err(Error::from)
}

pub(super) async fn media_allocate(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<crate::web::media::Upload>,
) -> Result<Json<Value>, Error> {
    let access = access(&app, &headers, "media_upload").await?;
    crate::web::media::allocate_as(State(app), access, Json(input))
        .await
        .map_err(Error::from)
}

pub(super) async fn media_complete(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, Error> {
    let access = access(&app, &headers, "media_upload").await?;
    crate::web::media::complete_as(State(app), access, Path(id))
        .await
        .map_err(Error::from)
}

pub(super) async fn media_abandon(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, Error> {
    let access = access(&app, &headers, "media_upload").await?;
    crate::web::media::abandon_as(State(app), access, Path(id))
        .await
        .map_err(Error::from)
}

pub(super) async fn media_info(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(options): Query<crate::web::media::MediaInfoQuery>,
) -> Result<Json<Value>, Error> {
    let access = access(&app, &headers, "read").await?;
    crate::web::media::info_as(State(app), access, Path(id), Query(options))
        .await
        .map_err(Error::from)
}

pub(super) async fn media_download(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(options): Query<crate::web::media::Download>,
) -> Result<Redirect, Error> {
    let access = access(&app, &headers, "read").await?;
    crate::web::media::download_as(State(app), access, Path(id), Query(options))
        .await
        .map_err(Error::from)
}

pub(super) async fn avatar_set(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<crate::web::avatars::SetAvatar>,
) -> Result<Json<Value>, Error> {
    let access = access(&app, &headers, "profile").await?;
    crate::web::avatars::set_as(State(app), access, Json(input))
        .await
        .map_err(Error::from)
}

pub(super) async fn avatar_allocate(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<crate::web::avatars::Upload>,
) -> Result<Json<Value>, Error> {
    let access = access(&app, &headers, "profile").await?;
    crate::web::avatars::allocate_as(State(app), access, Json(input))
        .await
        .map_err(Error::from)
}

pub(super) async fn avatar_complete(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, Error> {
    let access = access(&app, &headers, "profile").await?;
    crate::web::avatars::complete_as(State(app), access, Path(id))
        .await
        .map_err(Error::from)
}

pub(super) async fn avatar_info(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, Error> {
    let access = access(&app, &headers, "read").await?;
    crate::web::avatars::info_as(State(app), access, Path(id))
        .await
        .map_err(Error::from)
}

pub(super) async fn avatar_download(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Redirect, Error> {
    let access = access(&app, &headers, "read").await?;
    crate::web::avatars::download_as(State(app), access, Path(id))
        .await
        .map_err(Error::from)
}

pub(super) async fn color_set(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<crate::web::colors::SetColor>,
) -> Result<Json<Value>, Error> {
    let access = access(&app, &headers, "profile").await?;
    crate::web::colors::set_as(State(app), access, Json(input))
        .await
        .map_err(Error::from)
}

pub(super) async fn library_list(
    State(app): State<App>,
    headers: HeaderMap,
    input: Query<crate::web::library::List>,
) -> Result<Json<Value>, Error> {
    let access = access(&app, &headers, "library").await?;
    Ok(crate::web::library::list_as(State(app), access, input).await?)
}

pub(super) async fn library_change(
    State(app): State<App>,
    headers: HeaderMap,
    id: Path<String>,
    input: Json<crate::web::library::Change>,
) -> Result<Json<Value>, Error> {
    let access = access(&app, &headers, "library").await?;
    Ok(crate::web::library::change_as(State(app), access, id, input).await?)
}

pub(super) async fn media_resolve(
    State(app): State<App>,
    headers: HeaderMap,
    input: Json<crate::web::media_resolve::Input>,
) -> Result<Json<Value>, Error> {
    let access = access(&app, &headers, "read").await?;
    Ok(crate::web::media_resolve::resolve_as(State(app), access, input).await?)
}

pub(super) async fn media_link(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(options): Query<crate::web::media::Download>,
) -> Result<Json<Value>, Error> {
    let access = access(&app, &headers, "read").await?;
    let url = crate::web::media::download_url_as(&app, access, id, options).await?;
    Ok(Json(serde_json::json!({"url":url})))
}

pub(super) async fn avatar_link(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, Error> {
    let access = access(&app, &headers, "read").await?;
    let url = crate::web::avatars::download_url_as(&app, access, id).await?;
    Ok(Json(serde_json::json!({"url":url})))
}

pub(super) async fn bot_commands(
    State(app): State<App>,
    headers: HeaderMap,
    input: Query<crate::web::bot_commands::QueryInput>,
) -> Result<Json<Value>, Error> {
    let access = access(&app, &headers, "read").await?;
    Ok(crate::web::bot_commands::get_as(State(app), access, input).await?)
}
