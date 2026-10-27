//! PostgreSQL's atomic upsert locks only the affected account/conversation row.
use super::super::{Result, cursor::CursorAdvance, message::number};
use sqlx::PgConnection;

pub(in crate::engine::database) async fn advance(
    db: &mut PgConnection,
    request: &CursorAdvance<'_>,
) -> Result<u64> {
    Ok(sqlx::query("INSERT INTO read_positions VALUES($1,$2,$3) ON CONFLICT(username,conversation) DO UPDATE SET sequence=GREATEST(read_positions.sequence,excluded.sequence)")
        .bind(request.user).bind(request.conversation).bind(number(request.through)?)
        .execute(db).await?.rows_affected())
}
