-- User-centric brain v0: Layer 1 ownership on captures + Layer 2 event stream
-- and global taste profiles (one brain per user, all channels).

-- ── Layer 2: behavioral event stream (feature store spine) ─────────────────
CREATE TABLE IF NOT EXISTS user_events (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     BIGINT NOT NULL REFERENCES users(id),
    event_type  TEXT NOT NULL,
    item_id     UUID REFERENCES items(id),
    signal      FLOAT NOT NULL DEFAULT 0,
    metadata    JSONB NOT NULL DEFAULT '{}',
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS user_events_user_time_idx
    ON user_events (user_id, occurred_at DESC);
CREATE INDEX IF NOT EXISTS user_events_type_idx
    ON user_events (event_type);

-- ── Layer 2: global taste profile (one row per user) ─────────────────────
-- interest_vector column is added at startup via ensure_vector_schema.
CREATE TABLE IF NOT EXISTS user_taste_profiles (
    user_id          BIGINT PRIMARY KEY REFERENCES users(id),
    vector_weight    FLOAT NOT NULL DEFAULT 0.0,
    notify_threshold FLOAT NOT NULL DEFAULT 0.72,
    liked_tags       TEXT[] NOT NULL DEFAULT '{}',
    disliked_tags    TEXT[] NOT NULL DEFAULT '{}',
    capture_count    INT NOT NULL DEFAULT 0,
    query_count      INT NOT NULL DEFAULT 0,
    muted_until      TIMESTAMPTZ,
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ── Layer 1: user-owned captures (extend items) ───────────────────────────
-- Backfill + indexes for these columns live in 013_user_brain_backfill.sql —
-- see the note in 012_channels_backfill.sql for why that has to be a
-- separate migration file on CockroachDB.
ALTER TABLE items ADD COLUMN IF NOT EXISTS owner_user_id BIGINT REFERENCES users(id);
ALTER TABLE items ADD COLUMN IF NOT EXISTS source_channel TEXT;
ALTER TABLE items ADD COLUMN IF NOT EXISTS context_signals JSONB;

-- Passage index scoped to the capture owner for personal-corpus /ask.
ALTER TABLE chunks ADD COLUMN IF NOT EXISTS owner_user_id BIGINT REFERENCES users(id);
