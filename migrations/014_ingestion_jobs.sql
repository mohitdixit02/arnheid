-- Durable ingestion queue. Every capture (link/note/voice/image) becomes a
-- row here the moment it's shared, instead of living only in an in-memory
-- channel — a process restart mid-wait or mid-fetch no longer silently
-- drops it. The consumer claims due rows, retries failures with backoff
-- (see attempts/run_after), and gives up only after repeated failure.
--
-- This replaces the mpsc channel that used to be the sole record a capture
-- was ever scheduled. It does NOT yet absorb the separate items.fetch_status
-- pending-retry sweep in cron::jobs::health_and_retry (retrying a URL fetch
-- for an item that already exists) — that's a narrower, item-level concern
-- left for a later pass.
--
-- The index lives in 015_ingestion_jobs_index.sql: CockroachDB's schema
-- change for CREATE TABLE hasn't necessarily finalized by the time the next
-- statement runs, so (per 012_channels_backfill.sql's note) anything that
-- depends on the table being fully committed goes in its own migration file.
--
-- `attempts` is BIGINT, not the bare INT/INTEGER alias: CockroachDB's
-- `default_int_size` makes plain INT mean a 64-bit column unless a session
-- overrides it, and sqlx's Postgres decoder rejects reading an int8 wire
-- value into a Rust `i32` outright (found by actually running this migration
-- and the roundtrip test against real CockroachDB, not just compiling).

CREATE TABLE IF NOT EXISTS ingestion_jobs (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    kind           TEXT NOT NULL CHECK (kind IN ('link', 'note', 'voice', 'image')),
    url            TEXT NOT NULL,             -- real URL for 'link'; pseudo-URL otherwise
    group_id       BIGINT NOT NULL REFERENCES groups(id),
    group_name     TEXT,
    shared_by      BIGINT NOT NULL REFERENCES users(id),
    message_id     BIGINT NOT NULL,
    forwarded      BOOL NOT NULL DEFAULT FALSE,
    forward_origin TEXT,
    source_channel TEXT NOT NULL,
    note_title     TEXT,                      -- set for note/voice/image, NULL for link
    note_text      TEXT,                      -- set for note/voice/image, NULL for link
    run_after      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    status         TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'claimed', 'done', 'failed')),
    attempts       BIGINT NOT NULL DEFAULT 0,
    last_error     TEXT,
    claimed_at     TIMESTAMPTZ,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
