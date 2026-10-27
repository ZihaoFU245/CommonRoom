use super::security::{check_origin_or_bot, check_read_origin_or_bot};
use super::*;
use axum::{
    extract::{Query, State},
    http::HeaderMap,
};
use serde::Deserialize;
#[derive(Deserialize)]
pub(crate) struct HistoryQuery {
    view: String,
    start: Option<u64>,
    end: Option<u64>,
    before: Option<u64>,
    after: Option<u64>,
    around: Option<u64>,
    limit: Option<usize>,
}
pub(crate) async fn history(
    State(app): State<App>,
    headers: HeaderMap,
    Query(input): Query<HistoryQuery>,
) -> Result<Json<crate::engine::History>, ApiError> {
    check_read_origin_or_bot(&app, &headers)?;
    let permit = app
        .io_permit()
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    // Detach accepted work from request cancellation so admission remains held
    // until both metadata checks and the native async read have finished.
    tokio::spawn(async move {
        let _permit = permit;
        let range = crate::engine::HistoryRange {
            start: input.start,
            end: input.end,
            before: input.before,
            after: input.after,
            around: input.around,
            limit: input.limit,
        };
        range.validate().map_err(ApiError::from)?;
        let (user, id, revision, store) = app
            .inspect_engine({
                let headers = headers.clone();
                let view = input.view.clone();
                move |engine, config| -> Result<_, ApiError> {
                    let user =
                        super::auth::identity(engine, config, &headers).ok_or_else(|| {
                            error(StatusCode::UNAUTHORIZED, "Invalid or expired credential.")
                        })?;
                    let (_, id, revision) =
                        engine.conversation(&user, &view).map_err(ApiError::from)?;
                    Ok((user, id, revision, engine.history_store().clone()))
                }
            })
            .await?;
        let mut page = store
            .page_checked_async(&id, revision, range, &input.view)
            .await
            .map_err(crate::engine::EngineError::from)
            .map_err(ApiError::from)?;
        store
            .ensure_available_async()
            .await
            .map_err(ApiError::from)?;
        app.inspect_engine(move |engine, config| -> Result<_, ApiError> {
            let current = super::auth::identity(engine, config, &headers)
                .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Invalid or expired credential."))?;
            let (_, current_id, _) = engine
                .conversation(&current, &input.view)
                .map_err(ApiError::from)?;
            if current != user || current_id != id {
                return Err(error(
                    StatusCode::CONFLICT,
                    "History changed; retry the current page.",
                ));
            }
            if super::colors::enabled(&headers) {
                page.name_colors = Some(engine.message_colors(&id, &page.messages));
            }
            Ok(Json(page))
        })
        .await
    })
    .await
    .map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "History worker unavailable.",
        )
    })?
}

#[derive(Deserialize)]
pub(crate) struct Read {
    view: String,
    through: u64,
}
pub(crate) async fn read(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<Read>,
) -> Result<Json<serde_json::Value>, ApiError> {
    check_origin_or_bot(&app, &headers)?;
    let permit = app
        .io_permit()
        .map_err(|e| error(StatusCode::SERVICE_UNAVAILABLE, &e))?;
    tokio::spawn(async move {
        let _permit = permit;
        let id = app
            .inspect_engine({
                let headers = headers.clone();
                let view = input.view.clone();
                move |engine, config| -> Result<_, ApiError> {
                    let user =
                        super::auth::identity(engine, config, &headers).ok_or_else(|| {
                            error(StatusCode::UNAUTHORIZED, "Invalid or expired credential.")
                        })?;
                    engine
                        .conversation(&user, &view)
                        .map(|(_, id, _)| id)
                        .map_err(ApiError::from)
                }
            })
            .await?;
        let ordering = app
            .mutations
            .conversation(id.clone())
            .await
            .map_err(ApiError::from)?;
        let config = app.config.clone();
        let cursor = app
            .with_conversation(&ordering, move |engine| -> Result<_, ApiError> {
                let user = super::auth::identity(engine, &config, &headers).ok_or_else(|| {
                    error(StatusCode::UNAUTHORIZED, "Invalid or expired credential.")
                })?;
                if engine.conversation(&user, &input.view)?.1 != id {
                    return Err(error(
                        StatusCode::CONFLICT,
                        "Conversation changed; retry acknowledgement.",
                    ));
                }
                engine
                    .prepare_read(&user, &input.view, input.through)
                    .map_err(ApiError::from)
            })
            .await?;
        if !cursor.retained().await.map_err(ApiError::from)? {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "Message is no longer in this conversation.",
            ));
        }
        cursor.save().await.map_err(ApiError::from)?;
        let changes = app.changes.clone();
        app.with_conversation(&ordering, move |engine| -> Result<_, ApiError> {
            let user = cursor.user.clone();
            if engine.publish_read(cursor)? {
                let _ = changes.send(Change::Read(user));
            }
            Ok(Json(json!({"ok":true})))
        })
        .await
    })
    .await
    .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "Read worker unavailable."))?
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn history_query_parses_numeric_bounds_without_flattening() {
        let uri = "/api/history?view=room&start=1000&end=1099&limit=50"
            .parse()
            .unwrap();
        let Query(query) = Query::<HistoryQuery>::try_from_uri(&uri).unwrap();
        assert_eq!(query.view, "room");
        assert_eq!(query.start, Some(1000));
        assert_eq!(query.end, Some(1099));
        assert_eq!(query.limit, Some(50));
        for (key, value) in [("before", 1000), ("after", 1099), ("around", 1000)] {
            let uri = format!("/api/history?view=%40direct%3Abob&{key}={value}")
                .parse()
                .unwrap();
            let Query(query) = Query::<HistoryQuery>::try_from_uri(&uri).unwrap();
            assert_eq!(query.view, "@direct:bob");
            assert_eq!(query.before.or(query.after).or(query.around), Some(value));
        }
        for query in [
            "start=abc",
            "after=-1",
            "limit=1.5",
            "before=18446744073709551616",
        ] {
            let uri = format!("/api/history?view=room&{query}").parse().unwrap();
            assert!(Query::<HistoryQuery>::try_from_uri(&uri).is_err());
        }
    }
}
