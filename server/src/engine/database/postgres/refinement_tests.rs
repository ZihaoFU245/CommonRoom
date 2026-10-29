//! Memory budgets and statement-level retention in the current v1 schema.
use super::*;
use crate::engine::{
    Engine, HistoryRange,
    database::{Connection, PageRead, tests::PostgresFixture},
};

#[test]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
fn postgres_history_bounds_driver_payload_before_decoding_reactions() {
    let fixture = PostgresFixture::new();
    let mut engine = Engine::open_postgres(&fixture.url).unwrap();
    engine
        .provision("alice", "hash".into(), true, false)
        .unwrap();
    engine.execute(None, None, "/room new review").unwrap();
    let id = &engine.data.rooms["review"].id;
    let revision = engine.data.rooms["review"].revision;
    let db = Connection::postgres(&fixture.url, false).unwrap();
    db.execute("INSERT INTO messages SELECT $1,n,json_build_object('id',n::text,'from','alice','author_id','a','to',NULL,'text','body','time',1,'sequence',n,'reply',NULL,'mentions','[]'::json)::text,jsonb_build_object('id',n::text,'from','alice','time',1,'mentions','[]'::jsonb) FROM generate_series(1,100) n", [id]).unwrap();
    // The former per-user label encoding fetched >14 MiB for this page.
    db.execute("INSERT INTO reactions SELECT $1,n,lpad(r::text,128,'r'),'user'||u FROM generate_series(1,100) n CROSS JOIN generate_series(1,32) r CROSS JOIN generate_series(1,32) u", [id]).unwrap();
    runtime::run(async {
        for ascending in [false, true] {
            let budget = 64 * 1024;
            let sql = format!("WITH result(revision,payloads,reactions,oldest,newest,anchor) AS ({}) SELECT cardinality(payloads)::bigint,(octet_length(reactions::text)+coalesce((SELECT sum(octet_length(raw)) FROM unnest(payloads) raw),0))::bigint FROM result", history::PAGE_SQL.replace("DIRECTION", if ascending { "ASC" } else { "DESC" }));
            let mut reader = engine.store.db.acquire_reader().await?;
            let super::super::Backend::Postgres(native) = reader.backend.get_mut() else { panic!("PostgreSQL reader required") };
            let (rows, bytes): (i64, i64) = sqlx::query_as(&sql).bind(id).bind(1i64).bind(100i64).bind(100i64).bind(None::<i64>).bind(budget).fetch_one(&mut **native).await?;
            assert!(rows > 1 && rows < 100);
            assert!(bytes <= budget, "driver fetched {bytes} bytes for a {budget} byte budget");
            let range = if ascending { HistoryRange { after: Some(0), ..Default::default() } } else { HistoryRange::default() };
            let request = PageRead { conversation: id, revision, range, limit: 100, max_bytes: usize::try_from(budget).unwrap() };
            let page = reader.read_page(&request).await?;
            assert!(!page.messages.is_empty());
            assert!(page.messages.len() <= usize::try_from(rows).unwrap());
            assert_eq!(page.has_before, !ascending);
            assert_eq!(page.has_after, ascending);
            for message in &page.messages {
                assert_eq!(message.reactions.len(), 32);
                assert!(message.reactions.values().all(|users| users.len() == 32));
            }
            // Even a budget smaller than one message must advance without
            // dropping reaction members or skipping the first eligible row.
            let oversized = reader.read_page(&PageRead { max_bytes: 1, ..request }).await?;
            assert_eq!(oversized.messages.len(), 1);
            assert_eq!(oversized.messages[0].sequence, if ascending { 1 } else { 100 });
            assert!(oversized.messages[0].reactions.values().all(|users| users.len() == 32));
        }
        Ok(())
    }).unwrap();
}

#[test]
#[ignore = "requires CHAT_TEST_POSTGRES_URL; run PostgreSQL tests with --test-threads=1"]
fn postgres_bulk_retention_runs_once_per_statement_and_rolls_back() {
    let fixture = PostgresFixture::new();
    let engine = Engine::open_postgres(&fixture.url).unwrap();
    let db = Connection::postgres(&fixture.url, false).unwrap();
    db.execute_batch("INSERT INTO messages SELECT CASE WHEN n%2=0 THEN 'a' ELSE 'b' END,n,'{}',jsonb_build_object('id',n::text,'from','alice','time',1,'mentions','[]'::jsonb) FROM generate_series(1,16000) n;").unwrap();
    let tx = db.unchecked_transaction().unwrap();
    let mut plan = Vec::new();
    tx.visit_rows(
        "EXPLAIN (ANALYZE) DELETE FROM messages WHERE sequence<=12000",
        |r| {
            plan.push(r.get::<_, String>(0)?);
            Ok(())
        },
    )
    .unwrap();
    let retention = plan
        .iter()
        .find(|line| line.starts_with("Trigger retention_delete:"))
        .unwrap();
    assert!(retention.ends_with("calls=1"), "{retention}");
    eprintln!("{retention}; {}", plan.last().unwrap());
    assert_eq!(
        tx.query_row::<i64, _, _>(
            "SELECT count FROM retention WHERE conversation='a'",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        2000
    );
    assert_eq!(
        tx.query_row::<i64, _, _>(
            "SELECT oldest FROM retention WHERE conversation='b'",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        12001
    );
    drop(tx);
    assert_eq!(
        db.query_row::<i64, _, _>(
            "SELECT count FROM retention WHERE conversation='a'",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        8000
    );
    assert_eq!(
        db.query_row::<i64, _, _>(
            "SELECT oldest FROM retention WHERE conversation='b'",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        1
    );
    db.execute_batch("DELETE FROM messages WHERE conversation='a';")
        .unwrap();
    assert_eq!(
        db.query_row::<i64, _, _>(
            "SELECT count(*) FROM retention WHERE conversation='a'",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        db.query_row::<i64, _, _>(
            "SELECT count FROM retention WHERE conversation='b'",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        8000
    );
    db.execute_batch("DELETE FROM messages;").unwrap();
    assert_eq!(
        db.query_row::<i64, _, _>("SELECT count(*) FROM retention", [], |r| r.get(0))
            .unwrap(),
        0
    );
    drop(engine);
}
