//! SQLite's cursor upsert is serialized by its single writer pool.
use super::super::{Result, cursor::CursorAdvance, message::number};
use sqlx::SqliteConnection;

pub(in crate::engine::database) async fn advance(
    db: &mut SqliteConnection,
    request: &CursorAdvance<'_>,
) -> Result<u64> {
    Ok(sqlx::query("INSERT INTO read_positions VALUES(?1,?2,?3) ON CONFLICT(username,conversation) DO UPDATE SET sequence=MAX(sequence,excluded.sequence)")
        .bind(request.user).bind(request.conversation).bind(number(request.through)?)
        .execute(db).await?.rows_affected())
}
