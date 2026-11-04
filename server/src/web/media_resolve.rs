//! Browser media reads share admission, native reads and final authorization.
use super::*;
use crate::engine::{EngineError, avatars::AvatarPlan, database::MediaRead};
use axum::{extract::State, http::HeaderMap};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Media,
    Avatars,
}
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Item {
    kind: Kind,
    id: String,
    #[serde(default)]
    link: bool,
    #[serde(default)]
    download: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Input {
    items: Vec<Item>,
}

pub(super) async fn resolve(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<Input>,
) -> Result<Json<serde_json::Value>, ApiError> {
    security::check_origin(&app, &headers)?;
    if input.items.is_empty()
        || input.items.len() > 32
        || input.items.iter().any(|item| !canonical_id(&item.id))
    {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "Resolve requires 1..32 canonical media IDs.",
        ));
    }
    let permit = app.io_permit()?;
    admission::spawn_io(permit, resolve_batch(app, headers, input)).await?
}

async fn resolve_batch(
    app: App,
    headers: HeaderMap,
    input: Input,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Preserve response order while resolving exact duplicates only once.
    let mut unique = Vec::new();
    let order: Vec<_> = input
        .items
        .into_iter()
        .map(|item| {
            if let Some(index) = unique.iter().position(|existing| existing == &item) {
                index
            } else {
                let index = unique.len();
                unique.push(item);
                index
            }
        })
        .collect();
    let input = Input { items: unique };
    let access = media::Access::Browser(headers.clone());
    let (identity, store) = media::native_context(&app, access).await?;
    let ids: Vec<_> = input
        .items
        .iter()
        .map(|item| item.id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    // The metadata fence and sorted asset permits order library changes.
    let _ordering = app
        .mutations
        .conversations(ids.iter().map(|id| format!("@asset:{id}")).collect())
        .await?;
    let scopes = app
        .inspect_engine({
            let identity = identity.clone();
            move |engine, _| -> Result<_, ApiError> {
                media::verify_identity(&identity, engine)?;
                let scopes: Vec<_> = engine
                    .library_read_scopes(&identity.user)
                    .into_iter()
                    .collect();
                Ok(scopes)
            }
        })
        .await?;
    let rows = store.read_batch(&ids, &identity.account, &scopes).await?;
    let initial_rows = rows.clone();
    let items = input.items.clone();
    let (plans, mut stamp) = app
        .inspect_engine({
            let identity = identity.clone();
            move |engine, _| -> Result<_, ApiError> {
                media::verify_identity(&identity, engine)?;
                let rows: BTreeMap<_, _> = rows
                    .into_iter()
                    .map(|row| (row.media.id.clone(), row))
                    .collect();
                let user = &identity.user;
                let mut authorized = BTreeMap::new();
                let plans = items
                    .into_iter()
                    .map(|item| {
                        let row = rows.get(&item.id).ok_or_else(missing_media)?;
                        let plan = authorized
                            .entry((item.kind, item.id.clone()))
                            .or_insert_with(|| item.authorize(engine, user, row))
                            .as_ref()
                            .map(Clone::clone)
                            .map_err(|cause| error(cause.status, &cause.message))?;
                        Ok((row.media.clone(), plan))
                    })
                    .collect::<Vec<Result<_, ApiError>>>();
                let watched: BTreeSet<_> = plans
                    .iter()
                    .filter_map(|plan| plan.as_ref().ok())
                    .filter_map(|(_, avatar)| avatar.as_ref())
                    .flat_map(|avatar| avatar.history_ids().map(str::to_owned))
                    .collect();
                let stamp = (!watched.is_empty())
                    .then(|| engine.delivery_stamp(user, &watched, false))
                    .transpose()?;
                Ok((plans, stamp))
            }
        })
        .await?;
    // One history statement checks all unknown owners in this batch.
    // Share one check for inline/download variants of the same avatar.
    let mut avatars = BTreeMap::new();
    for (item, plan) in input.items.iter().zip(&plans) {
        if let Ok((_, Some(avatar))) = plan {
            avatars.entry(item.id.clone()).or_insert(avatar);
        }
    }
    let avatar_ids: Vec<_> = avatars.keys().cloned().collect();
    let (checks, watched) =
        AvatarPlan::visible_batch(&avatars.into_values().collect::<Vec<_>>()).await?;
    if let Some(stamp) = &mut stamp {
        stamp.restrict_history(&watched);
    }
    let visible: BTreeMap<_, _> = avatar_ids.into_iter().zip(checks).collect();
    let links =
        futures_util::future::join_all(input.items.iter().zip(plans).map(|(item, plan)| {
            let visible = &visible;
            let app = &app;
            async move {
                let (record, avatar) = plan?;
                if avatar.is_some() && !visible.get(&item.id).copied().unwrap_or(false) {
                    return Err(missing_media());
                }
                let now = crate::engine::media::timestamp();
                let readable = record.ready
                    && !record.deleting
                    && !record.deleted
                    && record.expires.is_none_or(|expires| expires > now);
                if item.link && readable {
                    let storage = app.media.as_ref().ok_or_else(|| {
                        error(
                            StatusCode::SERVICE_UNAVAILABLE,
                            "Media storage unavailable.",
                        )
                    })?;
                    let url = storage
                        .download_url(&record, item.download)
                        .await
                        .map_err(|_| {
                            error(StatusCode::SERVICE_UNAVAILABLE, "Cannot sign media URL.")
                        })?;
                    Ok(Some(url))
                } else {
                    Ok(None)
                }
            }
        }))
        .await;
    // Recheck references and mutable columns after signing.
    let rows = if links.iter().any(|link| matches!(link, Ok(Some(_)))) {
        store.read_batch(&ids, &identity.account, &scopes).await?
    } else {
        initial_rows
    };
    app.inspect_engine(move |engine, config| -> Result<_, ApiError> {
        media::verify_identity(&identity, engine)?;
        let user = &identity.user;
        if let Some(stamp) = stamp
            && !engine
                .delivery_stamp(user, &watched, false)?
                .same_media_authority(&stamp)
        {
            return Err(error(
                StatusCode::CONFLICT,
                "Media authority changed; retry.",
            ));
        }
        let now = crate::engine::media::timestamp();
        let unauthorized = StatusCode::UNAUTHORIZED;
        let session_expires = auth::token(config, &headers)
            .and_then(|token| engine.session_expires(token))
            .ok_or_else(|| error(unauthorized, "Session expired."))?;
        if session_expires <= now {
            return Err(error(unauthorized, "Session expired."));
        }
        let rows: BTreeMap<_, _> = rows
            .into_iter()
            .map(|row| (row.media.id.clone(), row))
            .collect();
        let mut authorized = BTreeMap::new();
        let results: Vec<_> = input
            .items
            .into_iter()
            .zip(links)
            .map(|(item, link)| {
                let result = (|| -> Result<_, ApiError> {
                    let link = link?;
                    let row = rows.get(&item.id).ok_or_else(missing_media)?;
                    authorized
                        .entry((item.kind, item.id.clone()))
                        .or_insert_with(|| item.authorize(engine, user, row))
                        .as_ref()
                        .map_err(|cause| error(cause.status, &cause.message))?;
                    Ok(item.value(row, config, now, session_expires, link))
                })();
                result.unwrap_or_else(|cause| {
                    json!({
                        "kind": item.kind,
                        "id": item.id,
                        "status": cause.status.as_u16(),
                        "error": cause.message,
                    })
                })
            })
            .collect();
        let results: Vec<_> = order.into_iter().map(|index| &results[index]).collect();
        Ok(Json(json!({ "items": results })))
    })
    .await
}

fn canonical_id(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value)
}

fn missing_media() -> ApiError {
    error(StatusCode::NOT_FOUND, "Media not found.")
}

fn hidden_media(cause: EngineError) -> ApiError {
    super::error::hidden(cause, "Media not found.")
}

impl Item {
    fn authorize(
        &self,
        engine: &Engine,
        user: &str,
        row: &MediaRead,
    ) -> Result<Option<AvatarPlan>, ApiError> {
        match self.kind {
            Kind::Media => {
                engine
                    .authorize_media_read(user, &row.media, row.attached, true)
                    .map_err(hidden_media)?;
                Ok(None)
            }
            Kind::Avatars => engine
                .avatar_read_plan_scoped(user, &row.media, None)
                .map(Some)
                .map_err(hidden_media),
        }
    }
    fn value(
        &self,
        row: &MediaRead,
        config: &Config,
        now: u64,
        session_expires: u64,
        link: Option<String>,
    ) -> serde_json::Value {
        let media = &row.media;
        let expired = media.expires.is_some_and(|expires| expires <= now);
        let unavailable = !media.ready || media.deleting || media.deleted;
        let lease_seconds = config
            .media
            .authorization_lease_seconds
            .min(session_expires.saturating_sub(now))
            .min(
                media
                    .expires
                    .filter(|_| !expired)
                    .map_or(u64::MAX, |expires| expires.saturating_sub(now)),
            );
        let kind = match self.kind {
            Kind::Media => "media",
            Kind::Avatars => "avatars",
        };
        let path = format!("{}api/{kind}/{}/file", config.base_url, media.id);
        let mut value = json!({
            "kind": self.kind,
            "id": self.id,
            "status": 200,
            "lease_seconds": lease_seconds,
            "info": {
                "media_id": media.id,
                "path": path,
                "size": media.size,
                "content_type": media.content_type,
                "filename": media.filename,
                "expires": media.expires,
                "expired": expired,
                "unavailable": unavailable,
            },
        });
        if media.conversation == "@library" {
            value["info"]["library"] = json!(true);
            value["info"]["saved"] = json!(row.saved);
        }
        if let Some(url) = link.filter(|_| !expired && !unavailable) {
            let remaining = media
                .expires
                .map_or(u64::MAX, |expires| expires.saturating_sub(now));
            value["link"] = json!({
                "url": url,
                "expires_in": 60u64.min(remaining),
            });
        }
        value
    }
}
