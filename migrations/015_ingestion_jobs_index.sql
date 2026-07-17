-- Split from 014_ingestion_jobs.sql: see that file's header for why.
-- Serves the claim query's `status = 'pending' AND run_after <= NOW()` scan;
-- the much rarer stale-claim-reclaim branch does a full scan of the (small,
-- mostly-empty) 'claimed' slice, which doesn't need its own index at this scale.
CREATE INDEX IF NOT EXISTS ingestion_jobs_poll_idx ON ingestion_jobs (status, run_after);
