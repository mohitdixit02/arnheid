//! The ingestion queue consumer — a continuous background task claiming due
//! rows from the durable `ingestion_jobs` table (see `db::ingestion_jobs`
//! and migration 014). Every capture is a row from the moment it's shared,
//! so a crash mid-wait or mid-fetch never silently drops it.

pub mod chunker;
pub mod fetcher;
pub mod pipeline;
pub mod youtube;

use crate::db;
use crate::state::AppState;
use std::time::Duration;

/// Fallback polling interval. The primary wake-up is the `Notify` fired by
/// `intake::schedule_link`/`schedule_note` on every enqueue; this only
/// matters if that signal was missed (e.g. across a restart) or nothing was
/// due yet the last time the poller checked.
const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Backoff after the claim query itself fails (a DB hiccup), so a flaky
/// connection doesn't spin the loop.
const POLL_ERROR_BACKOFF: Duration = Duration::from_secs(5);

/// Run forever, claiming and processing due jobs one at a time. Each job is
/// fully self-contained; a failure in one never affects the next.
pub async fn run_consumer(state: AppState) {
    tracing::info!("ingestion consumer started");
    println!("[info] ingestion consumer started");
    loop {
        let batch_size = state.config.ingestion_batch_size as i64;
        let claimed = match db::ingestion_jobs::claim_batch(&state.pool, batch_size).await {
            Ok(jobs) => jobs,
            Err(e) => {
                tracing::warn!(error = %e, "claiming ingestion jobs failed");
                eprintln!("[warn] claiming ingestion jobs failed: error={e}");
                tokio::time::sleep(POLL_ERROR_BACKOFF).await;
                continue;
            }
        };

        if claimed.is_empty() {
            tokio::select! {
                _ = tokio::time::sleep(POLL_INTERVAL) => {}
                _ = state.ingestion_notify.notified() => {}
            }
            continue;
        }
        tracing::debug!(count = claimed.len(), "claimed ingestion jobs");
        println!("[debug] claimed ingestion jobs: count={}", claimed.len());

        for job in claimed {
            pipeline::process(&state, job).await;
        }
    }
}
