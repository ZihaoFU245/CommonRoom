//! SQLite profile transaction. The dispatcher owns its single writer lease.
use super::super::{Error, ProfileWrite, Result, media::MediaWrite, metadata::WriteResult};
use sqlx::SqliteConnection;

pub(in crate::engine::database) async fn apply(
    db: &mut SqliteConnection,
    request: &ProfileWrite<'_>,
) -> Result<WriteResult> {
    let id: Option<String> = sqlx::query_scalar(
        "SELECT json_extract(json,'$.id') FROM entities WHERE kind='account' AND key=?1",
    )
    .bind(request.user)
    .fetch_optional(&mut *db)
    .await?;
    if id.as_deref() != Some(request.account) {
        return Err(Error::RevisionConflict);
    }
    let mut changed = 0;
    if let Some(media) = request.avatar {
        changed += super::media::apply(
            db,
            &MediaWrite::Ready {
                media,
                at: request.at,
            },
        )
        .await?
        .changed;
    }
    if let Some(json) = request.json {
        changed += super::messages::put_entity(db, "account", request.user, json).await?;
    }
    Ok(WriteResult {
        changed,
        ..Default::default()
    })
}
