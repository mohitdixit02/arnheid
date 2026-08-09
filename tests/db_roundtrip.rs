//! Integration test exercising the full DB layer against a real CockroachDB.
//!
//! Ignored by default (needs a database). Run it explicitly:
//!
//!   docker compose up -d
//!   TEST_DATABASE_URL="postgresql://root@localhost:26257/arnheid?sslmode=disable" \
//!     cargo test --test db_roundtrip -- --ignored --nocapture --test-threads=1
//!
//! `--test-threads=1` matters here: each test function calls db::run_migrations
//! independently against the same live database, and two of those racing head
//! to head can hit CockroachDB's async schema-change finalization (a table
//! from one test's migration run looks "still being added" to the other's) —
//! not a bug in the migrations themselves, just two migrators racing the same
//! cluster. Serializing the test binary avoids it.
//!
//! It uses a unique group id per run so it can run repeatedly without cleanup.

use chrono::Utc;
use arnheid::db::{self, items::NewItem};
use arnheid::models::{ContextMessage, ContextPosition, ContextWindow};

const DIM: usize = 8;

fn unit_vec(seed: f32) -> Vec<f32> {
    (0..DIM).map(|i| seed + i as f32 * 0.01).collect()
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL (a live CockroachDB)"]
async fn full_db_roundtrip() {
    let url = std::env::var("TEST_DATABASE_URL")
        .expect("set TEST_DATABASE_URL to a CockroachDB instance");

    let pool = db::init_pool(&url).await.expect("connect");
    db::run_migrations(&pool).await.expect("migrate");
    db::ensure_vector_schema(&pool, DIM)
        .await
        .expect("vector schema");

    // Unique ids per run.
    let now = chrono::Utc::now().timestamp_micros();
    let group_id: i64 = -(1_000_000_000 + (now % 1_000_000));
    let sharer: i64 = 10_000 + (now % 1000);
    let reader: i64 = 20_000 + (now % 1000);

    db::groups::upsert_group(&pool, group_id, Some("test group"))
        .await
        .unwrap();
    db::groups::upsert_user(&pool, sharer, Some("alice"), Some("Alice"))
        .await
        .unwrap();
    db::groups::upsert_user(&pool, reader, Some("bob"), Some("Bob"))
        .await
        .unwrap();

    // Context window (the moat) must survive a round-trip.
    let ctx = ContextWindow {
        messages: vec![ContextMessage {
            user_id: Some(sharer),
            username: Some("alice".into()),
            message_id: 1,
            text: "lol this is exactly what we're building".into(),
            position: ContextPosition::Before,
        }],
        forwarded: false,
        forward_origin: None,
    };

    let emb = unit_vec(0.5);
    let tags = vec!["robotics".to_string(), "funding".to_string()];
    let item_id = db::items::insert(
        &pool,
        NewItem {
            group_id,
            owner_user_id: sharer,
            shared_by: sharer,
            source_channel: "telegram",
            url: "https://example.com/robotics-fund",
            message_id: 42,
            title: Some("Robotics megafund"),
            raw_content: Some("A new fund backs robotics startups."),
            summary: Some("A fund backing robotics."),
            tags: &tags,
            category: Some("venture"),
            context_window: &ctx,
            context_signals: None,
            embedding: Some(&emb),
            fetch_status: "ok",
            content_type: "article",
        },
    )
    .await
    .expect("insert")
    .expect("not deduped");

    // Dedup: same url + group same day → no second row.
    let dup = db::items::is_duplicate(&pool, group_id, "https://example.com/robotics-fund", 7)
        .await
        .unwrap();
    assert!(dup, "expected dedup to flag the URL");

    // Sharer profile update (weight 2.0) then a similarity search retrieves it.
    db::profiles::apply_weighted_update(
        &pool, sharer, group_id, &emb, 2.0, 100.0, 0.72, &tags, true,
    )
    .await
    .unwrap();

    let results = db::items::search(&pool, group_id, &unit_vec(0.5), 10, 0.0)
        .await
        .expect("search");
    assert!(!results.is_empty(), "search returned nothing");
    let top = &results[0];
    assert_eq!(top.id, item_id);
    assert_eq!(top.tags, tags);
    assert_eq!(
        top.context_window.as_ref().unwrap().messages[0].text,
        "lol this is exactly what we're building",
        "context_window must round-trip intact"
    );

    // Notification dedup constraint.
    db::notifications::log(&pool, reader, item_id, 0.9, true)
        .await
        .unwrap();
    assert!(db::notifications::already_notified(&pool, reader, item_id)
        .await
        .unwrap());

    // Graph processing flag flips.
    let unprocessed = db::items::unprocessed(&pool, 50).await.unwrap();
    assert!(unprocessed.iter().any(|i| i.id == item_id));
    db::items::mark_graph_processed(&pool, &[item_id])
        .await
        .unwrap();
    let unprocessed2 = db::items::unprocessed(&pool, 50).await.unwrap();
    assert!(!unprocessed2.iter().any(|i| i.id == item_id));

    // ── Chunk-level RAG: the actual /ask retrieval engine ──────────────────
    // Distinct embeddings per passage so vector search can tell them apart.
    let chunk_rows = vec![
        (
            0,
            "A new robotics megafund backs autonomous warehouse startups.".to_string(),
            unit_vec(0.5),
        ),
        (
            1,
            "The fund's thesis centers on humanoid manipulation research.".to_string(),
            unit_vec(-0.5),
        ),
    ];
    db::chunks::replace_for_item(&pool, item_id, group_id, sharer, &chunk_rows)
        .await
        .expect("replace_for_item");

    // Vector search: nearest passage to the seed embedding should be chunk 0.
    let hits = db::chunks::search(&pool, group_id, &unit_vec(0.5), 10, 0.0)
        .await
        .expect("chunk vector search");
    assert!(!hits.is_empty(), "chunk vector search returned nothing");
    assert_eq!(hits[0].item_id, item_id);
    assert!(hits[0].content.contains("megafund"));

    // Keyword (full-text) search: the CockroachDB-side half of hybrid search
    // (to_tsvector/ts_rank/GIN — no ANN index needed here, exercises the FTS path).
    let kw_hits = db::chunks::keyword_search(&pool, group_id, "humanoid manipulation", 10)
        .await
        .expect("chunk keyword search");
    assert!(
        kw_hits.iter().any(|h| h.content.contains("humanoid")),
        "keyword search should surface the humanoid-manipulation passage"
    );

    // Personal-corpus scoping (owner_user_id) — same passages, owner-scoped path.
    let owner_hits = db::chunks::search_by_owner(&pool, sharer, &unit_vec(0.5), 10, 0.0)
        .await
        .expect("chunk search_by_owner");
    assert!(
        owner_hits.iter().any(|h| h.item_id == item_id),
        "owner-scoped vector search should find the item's chunks"
    );

    // Backfill query: this item now has chunks, so it must drop out of the
    // missing-chunks worklist.
    let missing = db::chunks::items_missing_chunks(&pool, group_id, 50)
        .await
        .expect("items_missing_chunks");
    assert!(!missing.iter().any(|m| m.id == item_id));
}

/// Exercises the durable ingestion queue (migration 014) against a real
/// CockroachDB: not-due-yet vs due claiming, backoff on failure, permanent
/// give-up at max attempts, stale-claim reclaim after a simulated worker
/// crash, queue depth, and purge. This is the queue that replaced the
/// in-memory mpsc channel, so proving its atomicity and backoff actually
/// work against a real database matters more than most — this test is that
/// proof, not just "it compiles."
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL (a live CockroachDB)"]
async fn ingestion_jobs_roundtrip() {
    let url = std::env::var("TEST_DATABASE_URL")
        .expect("set TEST_DATABASE_URL to a CockroachDB instance");

    let pool = db::init_pool(&url).await.expect("connect");
    db::run_migrations(&pool).await.expect("migrate");

    let now = chrono::Utc::now().timestamp_micros();
    let group_id: i64 = -(2_000_000_000 + (now % 1_000_000));
    let user_id: i64 = 30_000 + (now % 1000);

    db::groups::upsert_group(&pool, group_id, Some("ingestion test group"))
        .await
        .unwrap();
    db::groups::upsert_user(&pool, user_id, Some("carol"), Some("Carol"))
        .await
        .unwrap();

    // ── A job not due yet must not be claimed ───────────────────────────────
    let future_run_after = Utc::now() + chrono::Duration::hours(1);
    db::ingestion_jobs::enqueue_link(
        &pool,
        "https://example.com/not-due-yet",
        group_id,
        Some("test"),
        user_id,
        1,
        false,
        None,
        "telegram",
        future_run_after,
    )
    .await
    .expect("enqueue future link");

    // ── A due note job must be claimed, with its fields intact ─────────────
    db::ingestion_jobs::enqueue_note(
        &pool,
        "note",
        &format!("note://{group_id}/2"),
        group_id,
        Some("test"),
        user_id,
        2,
        false,
        "telegram",
        "a note title",
        "a note body",
    )
    .await
    .expect("enqueue note");

    let claimed: Vec<_> = db::ingestion_jobs::claim_batch(&pool, 10)
        .await
        .expect("claim_batch")
        .into_iter()
        .filter(|j| j.group_id == group_id)
        .collect();
    assert_eq!(
        claimed.len(),
        1,
        "only the due note job should be claimed, not the future link"
    );
    let note_job = &claimed[0];
    assert_eq!(note_job.kind, "note");
    assert_eq!(note_job.note_title.as_deref(), Some("a note title"));
    assert_eq!(note_job.note_text.as_deref(), Some("a note body"));
    assert_eq!(note_job.attempts, 1, "claiming increments attempts");

    // Claiming again must not re-claim the same (now 'claimed', non-stale) row.
    let reclaimed = db::ingestion_jobs::claim_batch(&pool, 10)
        .await
        .expect("claim_batch again");
    assert!(
        !reclaimed.iter().any(|j| j.id == note_job.id),
        "an already-claimed, non-stale job must not be claimed twice"
    );

    // ── Failure under max_attempts: backs off instead of disappearing ──────
    db::ingestion_jobs::mark_failed(&pool, note_job.id, note_job.attempts, 5, "boom")
        .await
        .expect("mark_failed");
    let (status, run_after): (String, chrono::DateTime<Utc>) =
        sqlx::query_as("SELECT status, run_after FROM ingestion_jobs WHERE id = $1")
            .bind(note_job.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "pending",
        "under max_attempts, job goes back to pending"
    );
    assert!(
        run_after > Utc::now(),
        "backoff must push run_after into the future"
    );
    let not_yet = db::ingestion_jobs::claim_batch(&pool, 10).await.unwrap();
    assert!(
        !not_yet.iter().any(|j| j.id == note_job.id),
        "a backed-off job must not be claimable before its new run_after"
    );

    // Force it due now and re-claim to observe the incremented attempts count.
    sqlx::query("UPDATE ingestion_jobs SET run_after = NOW() - INTERVAL '1 second' WHERE id = $1")
        .bind(note_job.id)
        .execute(&pool)
        .await
        .unwrap();
    let rejob = db::ingestion_jobs::claim_batch(&pool, 10)
        .await
        .unwrap()
        .into_iter()
        .find(|j| j.id == note_job.id)
        .expect("job must be claimable again after backoff + forced run_after");
    assert_eq!(rejob.attempts, 2);

    // ── Failure at max_attempts: permanently failed, never claimable again ──
    db::ingestion_jobs::mark_failed(
        &pool,
        rejob.id,
        rejob.attempts,
        rejob.attempts,
        "final boom",
    )
    .await
    .expect("mark_failed at cap");
    let (status,): (String,) = sqlx::query_as("SELECT status FROM ingestion_jobs WHERE id = $1")
        .bind(rejob.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "failed");
    let never = db::ingestion_jobs::claim_batch(&pool, 50).await.unwrap();
    assert!(
        !never.iter().any(|j| j.id == rejob.id),
        "a permanently-failed job must never be claimed again"
    );

    // ── Stale-claim reclaim: a job abandoned mid-processing gets picked up ──
    db::ingestion_jobs::enqueue_note(
        &pool,
        "voice",
        &format!("voice://{group_id}/3"),
        group_id,
        Some("test"),
        user_id,
        3,
        false,
        "telegram",
        "voice title",
        "voice body",
    )
    .await
    .expect("enqueue voice");
    let voice_job = db::ingestion_jobs::claim_batch(&pool, 10)
        .await
        .unwrap()
        .into_iter()
        .find(|j| j.group_id == group_id && j.kind == "voice")
        .expect("voice job claimed");
    // Simulate the worker crashing mid-processing: back-date the claim past
    // the stale-claim window instead of waiting 10 real minutes.
    sqlx::query(
        "UPDATE ingestion_jobs SET claimed_at = NOW() - INTERVAL '11 minutes' WHERE id = $1",
    )
    .bind(voice_job.id)
    .execute(&pool)
    .await
    .unwrap();
    let restale = db::ingestion_jobs::claim_batch(&pool, 10)
        .await
        .unwrap()
        .into_iter()
        .find(|j| j.id == voice_job.id)
        .expect("stale-claimed job must be reclaimed");
    assert_eq!(
        restale.attempts, 2,
        "reclaiming a stale job counts as another attempt"
    );

    // ── Done jobs drop out of queue_depth ────────────────────────────────────
    let depth_before = db::ingestion_jobs::queue_depth(&pool).await.unwrap();
    db::ingestion_jobs::mark_done(&pool, restale.id)
        .await
        .unwrap();
    let depth_after = db::ingestion_jobs::queue_depth(&pool).await.unwrap();
    assert_eq!(
        depth_after,
        depth_before - 1,
        "marking a job done must remove exactly one from the queue depth"
    );

    // ── purge_old deletes only genuinely old finished rows ──────────────────
    sqlx::query("UPDATE ingestion_jobs SET created_at = NOW() - INTERVAL '10 days' WHERE id = $1")
        .bind(restale.id)
        .execute(&pool)
        .await
        .unwrap();
    db::ingestion_jobs::purge_old(&pool)
        .await
        .expect("purge_old");
    let gone: Option<(uuid::Uuid,)> = sqlx::query_as("SELECT id FROM ingestion_jobs WHERE id = $1")
        .bind(restale.id)
        .fetch_optional(&pool)
        .await
        .unwrap();
    assert!(
        gone.is_none(),
        "a done job older than 7 days must be purged"
    );

    let still_failed: Option<(uuid::Uuid,)> =
        sqlx::query_as("SELECT id FROM ingestion_jobs WHERE id = $1")
            .bind(rejob.id)
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert!(
        still_failed.is_some(),
        "a recently-failed job must survive purge_old (30-day retention)"
    );

    // Leave no pending row behind for repeated local runs.
    sqlx::query("DELETE FROM ingestion_jobs WHERE group_id = $1")
        .bind(group_id)
        .execute(&pool)
        .await
        .ok();
}
