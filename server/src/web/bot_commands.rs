use super::*;
use crate::engine::EngineError;
use axum::{
    extract::{Query, State},
    http::HeaderMap,
};
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct QueryInput {
    conversation_id: Option<String>,
    after: Option<String>,
}
const DIRECTORY_PAGE_CONVERSATIONS: usize = 32;
const DIRECTORY_PAGE_BYTES: usize = 512 * 1024;
pub(crate) async fn get(
    State(app): State<App>,
    headers: HeaderMap,
    Query(input): Query<QueryInput>,
) -> Result<Json<serde_json::Value>, ApiError> {
    super::security::check_read_origin(&app, &headers)?;
    if super::auth::bearer(&headers).is_some() {
        return Err(error(StatusCode::UNAUTHORIZED, "Browser session required."));
    }
    if input
        .after
        .as_ref()
        .is_some_and(|id| !crate::bots::models::valid_id(id))
        || (input.after.is_some() && input.conversation_id.is_some())
    {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "Invalid command directory cursor.",
        ));
    }
    let permit = app
        .io_permit()
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    tokio::spawn(async move {
        let _permit = permit;
        let identity = super::auth::authenticated_identity(&app, &headers).await?;
        let state = app.clone();
        app.inspect_engine(move |engine, _| {
            identity.verify(engine)?;
            let directory = |id: &str| {
                state
                    .bot_runtimes
                    .commands
                    .visible(engine, &identity.user, id)
                    .map(|bots| json!({"conversation_id":id,"bots":bots}))
            };
            if let Some(id) = input.conversation_id {
                return directory(&id).map(Json).map_err(ApiError::from);
            }
            let mut conversations: Vec<serde_json::Value> = Vec::new();
            let mut ids: Vec<_> = engine.bot_index.conversations.keys().collect();
            ids.sort();
            let mut bytes = 0;
            let mut next = None;
            for id in ids
                .into_iter()
                .filter(|id| input.after.as_ref().is_none_or(|after| *id > after))
            {
                match directory(id) {
                    Ok(value) => {
                        let size = serde_json::to_vec(&value)
                            .map_err(|_| {
                                error(
                                    StatusCode::SERVICE_UNAVAILABLE,
                                    "Command metadata unavailable.",
                                )
                            })?
                            .len();
                        if !conversations.is_empty() && bytes + size > DIRECTORY_PAGE_BYTES {
                            next = conversations
                                .last()
                                .and_then(|entry| entry["conversation_id"].as_str())
                                .map(str::to_owned);
                            break;
                        }
                        bytes += size;
                        conversations.push(value);
                        // Include one large conversation to guarantee forward progress.
                        // Individual catalogues and the publisher count remain bounded.
                        if conversations.len() >= DIRECTORY_PAGE_CONVERSATIONS
                            || bytes >= DIRECTORY_PAGE_BYTES
                        {
                            next = Some(id.clone());
                            break;
                        }
                    }
                    Err(EngineError::Forbidden(_) | EngineError::NotFound(_)) => {}
                    Err(error) => return Err(ApiError::from(error)),
                }
            }
            Ok(Json(json!({"conversations":conversations,"next":next})))
        })
        .await
    })
    .await
    .map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Command metadata worker unavailable.",
        )
    })?
}
