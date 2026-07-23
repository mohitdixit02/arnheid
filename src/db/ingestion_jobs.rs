//! The durable ingestion queue: every capture is a row from the moment it's
//! shared, claimed by the consumer, and retried with backoff on failure —
//! see migration 014 for the schema and the durability rationale.

use crate::models::IngestionJob;
use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// A claimed job is considered abandoned (worker crashed mid-processing)
/// once its claim is older than this, and becomes reclaimable.
const STALE_CLAIM_MINUTES: i64 = 10;
/// Backoff base: 1m, 2m, 4m, 8m, ... capped at this.
const MAX_BACKOFF_SECS: i64 = 3600;

#[allow(clippy::too_many_arguments)]
pub async fn enqueue_link(
    pool: &PgPool,
    url: &str,
    group_id: i64,
    group_name: Option<&str>,
    shared_by: i64,
    message_id: i64,
    forwarded: bool,
    forward_origin: Option<&str>,
    source_channel: &str,
    run_after: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO ingestion_jobs
            (kind, url, group_id, group_name, shared_by, message_id,
             forwarded, forward_origin, source_channel, run_after)
        VALUES ('link', $1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(url)
    .bind(group_id)
    .bind(group_name)
    .bind(shared_by)
    .bind(message_id)
    .bind(forwarded)
    .bind(forward_origin)
    .bind(source_channel)
    .bind(run_after)
    .execute(pool)
    .await?;
    Ok(())
}

/// `kind` is one of "note" | "voice" | "image". Runs immediately (`run_after`
/// defaults to now — there's no trailing-context wait for pre-extracted content).
#[allow(clippy::too_many_arguments)]
pub async fn enqueue_note(
    pool: &PgPool,
    kind: &str,
    pseudo_url: &str,
    group_id: i64,
    group_name: Option<&str>,
    shared_by: i64,
    message_id: i64,
    forwarded: bool,
    source_channel: &str,
    note_title: &str,
    note_text: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO ingestion_jobs
            (kind, url, group_id, group_name, shared_by, message_id,
             forwarded, source_channel, note_title, note_text)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        "#,
    )
    .bind(kind)
    .bind(pseudo_url)
    .bind(group_id)
    .bind(group_name)
    .bind(shared_by)
    .bind(message_id)
    .bind(forwarded)
    .bind(source_channel)
    .bind(note_title)
    .bind(note_text)
    .execute(pool)
    .await?;
    Ok(())
}

/// Atomically claim up to `limit` due jobs: new pending work whose
/// `run_after` has passed, plus anything a crashed worker abandoned
/// mid-processing. A single `UPDATE ... WHERE id IN (SELECT ...)` is one
/// statement, so CockroachDB's per-statement atomicity is what prevents two
/// concurrent claimers from taking the same row — no explicit row locking
/// needed. Claiming increments `attempts`, so a job that keeps crashing the
/// worker still eventually stops being reclaimed once `mark_failed` sees it
/// past `max_attempts`.
pub async fn claim_batch(pool: &PgPool, limit: i64) -> Result<Vec<IngestionJob>> {
    let rows: Vec<JobRow> = sqlx::query_as(
        r#"
        UPDATE ingestion_jobs
        SET status = 'claimed', claimed_at = NOW(), attempts = attempts + 1
        WHERE id IN (
            SELECT id FROM ingestion_jobs
            WHERE (status = 'pending' AND run_after <= NOW())
               OR (status = 'claimed' AND claimed_at < NOW() - ($2 || ' minutes')::interval)
            ORDER BY run_after ASC
            LIMIT $1
        )
        RETURNING id, kind, url, group_id, group_name, shared_by, message_id,
                  forwarded, forward_origin, source_channel, note_title, note_text, attempts
        "#,
    )
    .bind(limit)
    .bind(STALE_CLAIM_MINUTES.to_string())
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn mark_done(pool: &PgPool, id: Uuid) -> Result<()> {
    sqlx::query("UPDATE ingestion_jobs SET status = 'done' WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Mark a claimed job failed. `attempts` is the count already incremented by
/// `claim_batch` for this try. Under `max_attempts` it goes back to
/// 'pending' with an exponential backoff on `run_after`; at or past it, it's
/// parked as permanently 'failed'.
pub async fn mark_failed(
    pool: &PgPool,
    id: Uuid,
    attempts: i64,
    max_attempts: i64,
    error: &str,
) -> Result<()> {
    if attempts >= max_attempts {
        sqlx::query("UPDATE ingestion_jobs SET status = 'failed', last_error = $2 WHERE id = $1")
            .bind(id)
            .bind(error)
            .execute(pool)
            .await?;
        tracing::warn!(job_id = %id, attempts, "ingestion job permanently failed");
        eprintln!("[warn] ingestion job permanently failed: job_id={id} attempts={attempts}");
        return Ok(());
    }

    let backoff_secs = backoff_for_attempt(attempts);
    sqlx::query(
        r#"
        UPDATE ingestion_jobs
        SET status = 'pending', last_error = $2, run_after = NOW() + ($3 || ' seconds')::interval
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(error)
    .bind(backoff_secs.to_string())
    .execute(pool)
    .await?;
    tracing::debug!(job_id = %id, attempts, backoff_secs, "ingestion job requeued with backoff");
    println!("[debug] ingestion job requeued with backoff: job_id={id} attempts={attempts} backoff_secs={backoff_secs}");
    Ok(())
}

fn backoff_for_attempt(attempts: i64) -> i64 {
    // Capped well below where `60 << shift` would overflow i64 — the `.min`
    // below makes anything past ~6 shifts identical anyway (60 * 2^6 > the cap).
    let shift = attempts.saturating_sub(1).clamp(0, 32) as u32;
    (60i64 << shift).min(MAX_BACKOFF_SECS)
}

/// Jobs still needing attention — a real, restart-safe queue depth.
pub async fn queue_depth(pool: &PgPool) -> Result<i64> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM ingestion_jobs WHERE status IN ('pending', 'claimed')",
    )
    .fetch_one(pool)
    .await?;
    Ok(n)
}

/// Delete old finished jobs so the table doesn't grow unbounded. Done jobs
/// after a week; permanently-failed ones after a month, kept longer so
/// there's time to notice and investigate.
pub async fn purge_old(pool: &PgPool) -> Result<u64> {
    let done = sqlx::query(
        "DELETE FROM ingestion_jobs WHERE status = 'done' AND created_at < NOW() - INTERVAL '7 days'",
    )
    .execute(pool)
    .await?
    .rows_affected();
    let failed = sqlx::query(
        "DELETE FROM ingestion_jobs WHERE status = 'failed' AND created_at < NOW() - INTERVAL '30 days'",
    )
    .execute(pool)
    .await?
    .rows_affected();
    Ok(done + failed)
}

#[derive(sqlx::FromRow)]
struct JobRow {
    id: Uuid,
    kind: String,
    url: String,
    group_id: i64,
    group_name: Option<String>,
    shared_by: i64,
    message_id: i64,
    forwarded: bool,
    forward_origin: Option<String>,
    source_channel: String,
    note_title: Option<String>,
    note_text: Option<String>,
    attempts: i64,
}

impl From<JobRow> for IngestionJob {
    fn from(r: JobRow) -> Self {
        IngestionJob {
            id: r.id,
            kind: r.kind,
            url: r.url,
            group_id: r.group_id,
            group_name: r.group_name,
            shared_by: r.shared_by,
            message_id: r.message_id,
            forwarded: r.forwarded,
            forward_origin: r.forward_origin,
            source_channel: r.source_channel,
            note_title: r.note_title,
            note_text: r.note_text,
            attempts: r.attempts,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_exponentially_then_caps() {
        assert_eq!(backoff_for_attempt(1), 60);
        assert_eq!(backoff_for_attempt(2), 120);
        assert_eq!(backoff_for_attempt(3), 240);
        assert_eq!(backoff_for_attempt(4), 480);
        // 60 * 2^9 = 30720 > cap, so by attempt 10 it's pinned at the cap.
        assert_eq!(backoff_for_attempt(10), MAX_BACKOFF_SECS);
        assert_eq!(backoff_for_attempt(100), MAX_BACKOFF_SECS);
    }

    #[test]
    fn backoff_handles_zero_and_negative_gracefully() {
        // attempts should never be < 1 in practice (claim_batch increments
        // before mark_failed sees it), but this must not panic or overflow.
        assert_eq!(backoff_for_attempt(0), 60);
        assert_eq!(backoff_for_attempt(-5), 60);
    }
}
