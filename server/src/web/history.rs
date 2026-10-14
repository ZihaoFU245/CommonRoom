use super::*;
use super::{
    auth::authenticate,
    security::{check_origin_or_bot, check_read_origin_or_bot},
};
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
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        // Flat typed fields let serde_urlencoded parse numeric query values.
        // Flattening HistoryRange buffers values as strings before deserialization.
        let range = crate::engine::HistoryRange {
            start: input.start,
            end: input.end,
            before: input.before,
            after: input.after,
            around: input.around,
            limit: input.limit,
        };
        range
            .validate()
            .map_err(|e| error(StatusCode::BAD_REQUEST, &e))?;
        let user = authenticate(&app, &headers)?;
        let (id, revision, store) = {
            let engine = app.engine_read_api()?;
            let (_, id, revision) = engine
                .conversation(&user, &input.view)
                .map_err(|e| error(StatusCode::FORBIDDEN, &e))?;
            (id, revision, engine.history_store.clone())
        };
        // WAL readers overlap the writer without holding the engine lock across disk I/O.
        let page = store
            .page_checked(&id, revision, range, &input.view)
            .map_err(|e| {
                let status = match &e {
                    crate::engine::HistoryError::Changed => StatusCode::CONFLICT,
                    crate::engine::HistoryError::InvalidRange(_) => StatusCode::BAD_REQUEST,
                    crate::engine::HistoryError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
                };
                error(status, &e.to_string())
            })?;
        let current = authenticate(&app, &headers)?;
        let engine = app.engine_read_api()?;
        let (_, current_id, current_revision) = engine
            .conversation(&current, &input.view)
            .map_err(|e| error(StatusCode::FORBIDDEN, &e))?;
        if current != user || current_id != id || current_revision != page.revision {
            return Err(error(
                StatusCode::CONFLICT,
                "History changed; retry the current page.",
            ));
        }
        Ok(Json(page))
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
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let user = authenticate(&app, &headers)?;
        let changed = app
            .engine_api()?
            .mark_read(&user, &input.view, input.through)
            .map_err(|e| error(StatusCode::BAD_REQUEST, &e))?;
        if changed {
            let _ = app.changes.send(Change::Read(user));
        }
        Ok(Json(json!({"ok":true})))
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
