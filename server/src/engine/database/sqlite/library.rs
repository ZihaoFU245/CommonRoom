//! Native sqlite library transactions and bounded list reads.
use super::super::{
    Error, Result,
    history::unsigned,
    library::{LibraryAction, LibraryPage, LibraryWrite},
    message::number,
};
use sqlx::{Connection as _, SqliteConnection};

pub(super) async fn apply(db: &mut SqliteConnection, request: &LibraryWrite<'_>) -> Result<u64> {
    let (media, _) = super::media::load(db, request.id)
        .await?
        .ok_or(Error::MissingMedia)?;
    if media.conversation != "@library" {
        return Err(Error::MissingMedia);
    }
    let owned: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM library_owners WHERE account_id=?1 AND media_id=?2)",
    )
    .bind(request.account)
    .bind(request.id)
    .fetch_one(&mut *db)
    .await?;
    let saved: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM library_entries WHERE account_id=?1 AND media_id=?2)",
    )
    .bind(request.account)
    .bind(request.id)
    .fetch_one(&mut *db)
    .await?;
    match request.action {
        LibraryAction::Remove => {
            let mut changed = 0;
            for table in ["library_entries", "library_recents"] {
                let query = format!("DELETE FROM {table} WHERE account_id=?1 AND media_id=?2");
                changed += sqlx::query(&query)
                    .bind(request.account)
                    .bind(request.id)
                    .execute(&mut *db)
                    .await?
                    .rows_affected();
            }
            return Ok(changed);
        }
        LibraryAction::Release => {
            if !owned {
                return Ok(0);
            }
            let others: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM library_owners WHERE media_id=?1 AND account_id!=?2)",
            )
            .bind(request.id)
            .bind(request.account)
            .fetch_one(&mut *db)
            .await?;
            if !others && !media.deleting && !media.deleted && !request.confirm_last {
                return Err(Error::Conflict(
                    "This is the final ownership. Confirm release to make the asset unavailable for everyone.",
                ));
            }
            let mut changed =
                sqlx::query("DELETE FROM library_owners WHERE account_id=?1 AND media_id=?2")
                    .bind(request.account)
                    .bind(request.id)
                    .execute(&mut *db)
                    .await?
                    .rows_affected();
            if !others {
                changed += sqlx::query(
                    "UPDATE media SET deleting=1,unreferenced=?2 WHERE id=?1 AND deleted=0 AND deleting=0",
                )
                .bind(request.id)
                .bind(number(crate::engine::media::timestamp())?)
                .execute(db)
                .await?
                .rows_affected();
            }
            return Ok(changed);
        }
        LibraryAction::Delete => {
            let mut changed = sqlx::query("UPDATE media SET deleting=1,unreferenced=?2 WHERE id=?1 AND deleted=0 AND deleting=0").bind(request.id).bind(number(crate::engine::media::timestamp())?).execute(&mut *db).await?.rows_affected();
            loop {
                let removed = sqlx::query("DELETE FROM library_owners WHERE media_id=?1 AND account_id IN (SELECT account_id FROM library_owners WHERE media_id=?1 ORDER BY account_id LIMIT 100)").bind(request.id).execute(&mut *db).await?.rows_affected();
                changed += removed;
                if removed < 100 {
                    break;
                }
            }
            return Ok(changed);
        }
        LibraryAction::Save | LibraryAction::Acquire => {}
    }
    if !media.ready || media.deleting || media.deleted {
        return Err(Error::MissingMedia);
    }
    let referenced: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM message_media WHERE media_id=?1 AND conversation=?2)",
    )
    .bind(request.id)
    .bind(request.source)
    .fetch_one(&mut *db)
    .await?;
    if !owned && !saved && !referenced {
        return Err(Error::MissingMedia);
    }
    match request.action {
        LibraryAction::Save => {
            if saved {
                return Ok(0);
            }
            let count: i64 = sqlx::query_scalar(
                r#"
            SELECT (SELECT count(*)
            FROM library_entries
            WHERE account_id=?1)+(SELECT count(*)
            FROM media
            WHERE conversation='@library'
            AND uploader=?1
            AND ready=0
            AND deleting=0
            AND deleted=0)
            "#,
            )
            .bind(request.account)
            .fetch_one(&mut *db)
            .await?;
            if unsigned(count)? >= request.config.library_limit {
                return Err(Error::InvalidRequest("Library entry limit reached."));
            }
            Ok(
                sqlx::query("INSERT INTO library_entries(account_id,media_id,position) SELECT ?1,?2,COALESCE(MAX(position),0)+1 FROM library_entries WHERE account_id=?1")
                    .bind(request.account)
                    .bind(request.id)
                    .execute(db)
                    .await?
                    .rows_affected(),
            )
        }
        LibraryAction::Acquire => {
            if owned {
                return Ok(0);
            }
            let count: i64 = sqlx::query_scalar(
                r#"
            SELECT (SELECT count(*)
            FROM library_owners o JOIN media m ON m.id=o.media_id
            WHERE o.account_id=?1
            AND m.deleting=0
            AND m.deleted=0)+(SELECT count(*)
            FROM media
            WHERE conversation='@library'
            AND uploader=?1
            AND ready=0
            AND deleting=0
            AND deleted=0)
            "#,
            )
            .bind(request.account)
            .fetch_one(&mut *db)
            .await?;
            if unsigned(count)? >= request.config.library_owned_limit {
                return Err(Error::InvalidRequest("Owned asset limit reached."));
            }
            if usage(db, request.account).await?.saturating_add(media.size) > request.quota {
                return Err(Error::InvalidRequest("Account media quota reached."));
            }
            Ok(
                sqlx::query("INSERT INTO library_owners(account_id,media_id) VALUES(?1,?2)")
                    .bind(request.account)
                    .bind(request.id)
                    .execute(db)
                    .await?
                    .rows_affected(),
            )
        }
        _ => Err(Error::Conversion("Invalid library operation")),
    }
}

pub(super) async fn usage(db: &mut SqliteConnection, account: &str) -> Result<u64> {
    let bytes: i64 = sqlx::query_scalar(
        r#"
            SELECT COALESCE((SELECT sum(size)
            FROM media
            WHERE uploader=?1
            AND deleted=0
            AND (conversation!='@library'
            OR (ready=0
            AND deleting=0))),0)+COALESCE((SELECT sum(m.size)
            FROM library_owners o JOIN media m ON m.id=o.media_id
            WHERE o.account_id=?1
            AND m.ready=1
            AND m.deleting=0
            AND m.deleted=0),0)
            "#,
    )
    .bind(account)
    .fetch_one(db)
    .await?;
    unsigned(bytes)
}

pub(in crate::engine::database) async fn page(
    db: &mut SqliteConnection,
    request: &super::super::library::LibraryList<'_>,
    config: &crate::config::MediaConfig,
) -> Result<LibraryPage> {
    let super::super::library::LibraryList {
        account,
        owned,
        recent,
        after,
        limit,
        quota,
        conversations,
    } = *request;
    if !(1..=100).contains(&limit) {
        return Err(Error::InvalidRequest("Library page size must be 1..100."));
    }
    let limit = if recent {
        config.library_recent_limit
    } else {
        limit
    };
    let after = if recent {
        String::new()
    } else if owned {
        after.to_owned()
    } else if after.is_empty() {
        "0".to_owned()
    } else {
        let position = after
            .parse::<i64>()
            .ok()
            .filter(|position| *position >= 0)
            .ok_or(Error::InvalidRequest("Invalid collection cursor."))?;
        position.to_string()
    };
    let mut tx = db.begin().await?;
    let db = &mut *tx;
    let selection = if recent {
        "library_recents"
    } else if owned {
        "library_owners"
    } else {
        "library_entries"
    };
    let order = if recent {
        "selected.used DESC,selected.media_id"
    } else if owned {
        "selected.media_id"
    } else {
        "selected.position"
    };
    let cursor = if recent || owned {
        "selected.media_id"
    } else {
        "CAST(selected.position AS TEXT)"
    };
    let continuation = if recent || owned {
        "selected.media_id>?2"
    } else {
        "selected.position>CAST(?2 AS INTEGER)"
    };
    let sql = format!(
        r#"
            SELECT {cursor} AS cursor, m.json, m.ready, m.expires, m.deleting, m.deleted,
              EXISTS(SELECT 1 FROM library_owners o
                WHERE o.media_id=m.id AND o.account_id=?1) AS owned,
              EXISTS(SELECT 1 FROM library_entries e
                WHERE e.media_id=m.id AND e.account_id=?1) AS saved,
              (SELECT count(*) FROM library_owners o WHERE o.media_id=m.id) AS owners
            FROM {selection} selected JOIN media m ON m.id=selected.media_id
            WHERE selected.account_id=?1 AND {continuation}
            ORDER BY {order}
            LIMIT ?3
            "#
    );
    let rows: Vec<super::super::library::LibraryRow> = sqlx::query_as(&sql)
        .bind(account)
        .bind(after)
        .bind(number(limit + 1)?)
        .fetch_all(&mut *db)
        .await?;
    let mut items = Vec::new();
    let mut cursors = Vec::new();
    for row in rows {
        let cursor = row.cursor.clone();
        let item = row.into_item()?;
        if recent && !access(db, &item.media_id, account, conversations).await? {
            continue;
        }
        cursors.push(cursor);
        items.push(item);
    }
    let page_size =
        usize::try_from(limit).map_err(|_| Error::Conversion("Invalid library page size"))?;
    let more = items.len() > page_size;
    items.truncate(page_size);
    let next_cursor = if !recent && more {
        cursors.get(page_size - 1).cloned()
    } else {
        None
    };
    let usage = usage(db, account).await?;
    let (bytes, objects, oldest) = backlog(db).await?;
    let cleanup_bytes = unsigned(bytes)?;
    let cleanup_objects = unsigned(objects)?;
    let result = LibraryPage {
        items,
        next_cursor,
        usage,
        quota,
        cleanup_bytes,
        cleanup_objects,
        oldest_cleanup: oldest.map(unsigned).transpose()?,
        uploads_paused: cleanup_bytes >= config.library_cleanup_bytes
            || cleanup_objects >= config.library_cleanup_objects,
    };
    tx.commit().await?;
    Ok(result)
}

pub(super) async fn backlog(db: &mut SqliteConnection) -> Result<(i64, i64, Option<i64>)> {
    let result = sqlx::query_as(
        r#"
            SELECT bytes,objects,(SELECT COALESCE(unreferenced,created)
            FROM media
            WHERE conversation='@library'
            AND (deleting=1
            OR deleted=1)
            AND (deleted=0
            OR staging_clean=0)
            ORDER BY COALESCE(unreferenced,created)
            LIMIT 1)
            FROM library_storage
            WHERE id=1
            "#,
    )
    .fetch_one(db)
    .await?;
    Ok(result)
}

pub(super) async fn reserve_checks(
    db: &mut SqliteConnection,
    account: &str,
    config: &crate::config::MediaConfig,
) -> Result<()> {
    let (bytes, objects, _) = backlog(db).await?;
    if unsigned(bytes)? >= config.library_cleanup_bytes
        || unsigned(objects)? >= config.library_cleanup_objects
    {
        return Err(Error::Conflict(
            "Library uploads paused while storage cleanup catches up.",
        ));
    }
    let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM media WHERE conversation='@library' AND uploader=?1 AND ready=0 AND deleting=0 AND deleted=0").bind(account).fetch_one(&mut *db).await?;
    let owned: i64 = sqlx::query_scalar("SELECT count(*) FROM library_owners o JOIN media m ON m.id=o.media_id WHERE o.account_id=?1 AND m.deleting=0 AND m.deleted=0").bind(account).fetch_one(&mut *db).await?;
    let saved: i64 = sqlx::query_scalar("SELECT count(*) FROM library_entries WHERE account_id=?1")
        .bind(account)
        .fetch_one(db)
        .await?;
    if unsigned(owned)?.saturating_add(unsigned(pending)?) >= config.library_owned_limit
        || unsigned(saved)?.saturating_add(unsigned(pending)?) >= config.library_limit
    {
        return Err(Error::InvalidRequest(
            "Library ownership or entry limit reached.",
        ));
    }
    Ok(())
}

pub(super) async fn remove_account(db: &mut SqliteConnection, account: &str) -> Result<u64> {
    let mut changed = 0;
    loop {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT media_id FROM library_owners WHERE account_id=?1 ORDER BY media_id LIMIT 100",
        )
        .bind(account)
        .fetch_all(&mut *db)
        .await?;
        if ids.is_empty() {
            break;
        }
        for id in ids {
            changed +=
                sqlx::query("DELETE FROM library_owners WHERE account_id=?1 AND media_id=?2")
                    .bind(account)
                    .bind(&id)
                    .execute(&mut *db)
                    .await?
                    .rows_affected();
            changed += sqlx::query("UPDATE media SET deleting=1,unreferenced=?2 WHERE id=?1 AND deleted=0 AND deleting=0 AND NOT EXISTS(SELECT 1 FROM library_owners WHERE media_id=?1)").bind(&id).bind(number(crate::engine::media::timestamp())?).execute(&mut *db).await?.rows_affected();
        }
    }
    for table in ["library_entries", "library_recents"] {
        loop {
            let query = format!(
                "DELETE FROM {table} WHERE account_id=?1 AND media_id IN (SELECT media_id FROM {table} WHERE account_id=?1 ORDER BY media_id LIMIT 100)"
            );
            let removed = sqlx::query(&query)
                .bind(account)
                .execute(&mut *db)
                .await?
                .rows_affected();
            changed += removed;
            if removed < 100 {
                break;
            }
        }
    }
    changed += sqlx::query("UPDATE media SET deleting=1,unreferenced=?2 WHERE uploader=?1 AND conversation='@library' AND ready=0 AND deleted=0 AND deleting=0").bind(account).bind(number(crate::engine::media::timestamp())?).execute(db).await?.rows_affected();
    Ok(changed)
}

pub(in crate::engine::database) async fn access(
    db: &mut SqliteConnection,
    id: &str,
    account: &str,
    conversations: &[String],
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        r#"
            SELECT EXISTS(SELECT 1
            FROM library_owners
            WHERE media_id=?1
            AND account_id=?2)
            OR EXISTS(SELECT 1
            FROM library_entries
            WHERE media_id=?1
            AND account_id=?2)
            OR EXISTS(SELECT 1
            FROM message_media
            WHERE media_id=?1
            AND conversation IN (SELECT value
            FROM json_each(?3)))
            "#,
    )
    .bind(id)
    .bind(account)
    .bind(
        serde_json::to_string(conversations)
            .map_err(|_| Error::Conversion("Invalid library scopes"))?,
    )
    .fetch_one(db)
    .await?)
}

pub(in crate::engine::database) async fn source(
    db: &mut SqliteConnection,
    id: &str,
    conversations: &[String],
) -> Result<Option<String>> {
    Ok(sqlx::query_scalar("SELECT conversation FROM message_media WHERE media_id=?1 AND conversation IN (SELECT value FROM json_each(?2)) LIMIT 1").bind(id).bind(serde_json::to_string(conversations).map_err(|_| Error::Conversion("Invalid library scopes"))?).fetch_optional(db).await?)
}

/// Account-local collection membership, independent of message read access.
pub(in crate::engine::database) async fn saved(
    db: &mut SqliteConnection,
    id: &str,
    account: &str,
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM library_entries WHERE media_id=?1 AND account_id=?2)",
    )
    .bind(id)
    .bind(account)
    .fetch_one(db)
    .await?)
}

/// Record a use in commit order, including multiple sends within one second.
/// The caller holds the SQLite writer or PostgreSQL per-account recent lock.
pub(super) async fn record_recent(
    db: &mut SqliteConnection,
    account: &str,
    id: &str,
) -> Result<()> {
    let latest: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(used),0) FROM library_recents WHERE account_id=?1")
            .bind(account)
            .fetch_one(&mut *db)
            .await?;
    let next = latest
        .checked_add(1)
        .ok_or(Error::Conversion("Library recent order exhausted"))?;
    sqlx::query("INSERT INTO library_recents(account_id,media_id,used) VALUES(?1,?2,?3) ON CONFLICT(account_id,media_id) DO UPDATE SET used=excluded.used")
        .bind(account).bind(id).bind(next).execute(db).await?;
    Ok(())
}
