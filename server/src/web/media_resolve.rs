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
#[derive(PartialEq, Eq, Deserialize)]
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
    let mut items = Vec::new();
    let order: Vec<_> = input
        .items
        .into_iter()
        .map(|item| {
            if let Some(index) = items.iter().position(|existing| existing == &item) {
                index
            } else {
                let index = items.len();
                items.push(item);
                index
            }
        })
        .collect();
    let access = media::Access::Browser(headers.clone());
    let (identity, store) = media::native_context(&app, access).await?;
    let ids: Vec<_> = items
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
    // Discover only requested media kinds. Discard these authorization facts;
    // the authoritative reread below follows its delivery stamp capture.
    let discovered = store.read_batch(&ids, &identity.account, &[]).await?;
    let (scopes, mut stamp) = app
        .inspect_engine({
            let identity = identity.clone();
            move |engine, _| -> Result<_, ApiError> {
                media::verify_identity(&identity, engine)?;
                let scopes = if discovered
                    .iter()
                    .any(|row| row.media.conversation == "@library")
                {
                    identity.media_read_scopes(engine)?.0
                } else {
                    discovered
                        .iter()
                        .filter_map(|row| {
                            let id = &row.media.conversation;
                            (id != "@avatar"
                                && engine.authorize_library_source(&identity.user, id).is_ok())
                            .then(|| id.clone())
                        })
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect()
                };
                let mut stamp_ids: BTreeSet<_> = scopes.iter().cloned().collect();
                // Unknown avatar authors need historical witnesses. Capture only
                // those plans' candidate revisions, without binding them as library
                // read scopes for every ordinary avatar/media request.
                for row in &discovered {
                    if row.media.conversation == "@avatar"
                        && let Ok(plan) =
                            engine.avatar_read_plan_scoped(&identity.user, &row.media, None)
                    {
                        stamp_ids.extend(plan.history_ids().map(str::to_owned));
                    }
                }
                let stamp = engine.delivery_stamp(&identity.user, &stamp_ids, false)?;
                Ok((scopes, stamp))
            }
        })
        .await?;
    let rows = store.read_batch(&ids, &identity.account, &scopes).await?;
    let (items, rows, mut authorized) = app
        .inspect_engine({
            let identity = identity.clone();
            move |engine, _| -> Result<_, ApiError> {
                media::verify_identity(&identity, engine)?;
                let user = &identity.user;
                let mut authorized = BTreeMap::new();
                for item in &items {
                    authorized
                        .entry((item.kind, item.id.clone()))
                        .or_insert_with(|| {
                            rows.iter()
                                .find(|row| row.media.id == item.id)
                                .ok_or_else(missing_media)
                                .and_then(|row| item.authorize(engine, user, row))
                        });
                }
                Ok((items, rows, authorized))
            }
        })
        .await?;
    // Check each avatar identity once, sharing inline/download variants.
    let avatars: Vec<_> = authorized
        .values()
        .filter_map(|plan| plan.as_ref().ok().and_then(Option::as_ref))
        .collect();
    let (checks, mut watched) = AvatarPlan::visible_batch(&avatars).await?;
    // Initial SQL facts may outlive a concurrent message edit/retraction. Fence
    // attachment references as well as the actual avatar witnesses, without a reread.
    for row in &rows {
        if row.media.conversation == "@library" {
            watched.extend(scopes.iter().cloned());
        } else if row.media.conversation != "@avatar" && scopes.contains(&row.media.conversation) {
            watched.insert(row.media.conversation.clone());
        }
    }
    stamp.restrict_history(&watched);
    let mut checks = checks.into_iter();
    for plan in authorized.values_mut() {
        if matches!(plan, Ok(Some(_))) && checks.next() != Some(true) {
            *plan = Err(missing_media());
        }
    }
    let mut links = Vec::with_capacity(items.len());
    for item in &items {
        let plan = &authorized[&(item.kind, item.id.clone())];
        let link = match plan {
            Err(cause) => Err(error(cause.status, &cause.message)),
            Ok(_) => {
                let record = &rows
                    .iter()
                    .find(|row| row.media.id == item.id)
                    .ok_or_else(missing_media)?
                    .media;
                let now = crate::engine::media::timestamp();
                if item.link && record.available_at(now) {
                    match &app.media {
                        Some(storage) => storage
                            .download_url(record, item.download)
                            .await
                            .map(Some)
                            .map_err(|_| {
                                error(StatusCode::SERVICE_UNAVAILABLE, "Cannot sign media URL.")
                            }),
                        None => Err(error(
                            StatusCode::SERVICE_UNAVAILABLE,
                            "Media storage unavailable.",
                        )),
                    }
                } else {
                    Ok(None)
                }
            }
        };
        links.push(link);
    }
    // Recheck references and mutable columns after signing.
    let rows = if links.iter().any(|link| matches!(link, Ok(Some(_)))) {
        store.read_batch(&ids, &identity.account, &scopes).await?
    } else {
        rows
    };
    app.inspect_engine(move |engine, config| -> Result<_, ApiError> {
        media::verify_identity(&identity, engine)?;
        let user = &identity.user;
        if !engine
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
        let mut authorized = BTreeMap::new();
        let results: Vec<_> = items
            .into_iter()
            .zip(links)
            .map(|(item, link)| {
                let result = (|| -> Result<_, ApiError> {
                    let link = link?;
                    let row = rows
                        .iter()
                        .find(|row| row.media.id == item.id)
                        .ok_or_else(missing_media)?;
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
        let expired = media.expired_at(now);
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
