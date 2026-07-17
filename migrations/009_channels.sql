-- Multi-channel identity layer. Arnheid is a context capture protocol with
-- many ingress channels (telegram, whatsapp, later instagram/twitter).
-- Internal ids stay BIGINT everywhere; each channel maps its native id
-- space onto them via (channel, external_id). Telegram rows keep their
-- native ids; other channels draw synthetic ids from a sequence parked
-- far above Telegram's id range.
--
-- The external_id backfill + NOT NULL + index live in 012_channels_backfill.sql:
-- CockroachDB doesn't make a column usable in DML until the ADD COLUMN's
-- schema change has committed, so the backfill has to be a separate migration
-- (sqlx applies each file as its own round-trip; a later file always sees the
-- prior one fully committed, unlike statements batched in the same file).

ALTER TABLE groups ADD COLUMN IF NOT EXISTS channel     TEXT NOT NULL DEFAULT 'telegram';
ALTER TABLE groups ADD COLUMN IF NOT EXISTS external_id TEXT;

ALTER TABLE users ADD COLUMN IF NOT EXISTS channel     TEXT NOT NULL DEFAULT 'telegram';
ALTER TABLE users ADD COLUMN IF NOT EXISTS external_id TEXT;

-- Telegram ids are < ~1e13 today; 9e15 leaves headroom on both sides
-- while staying far under i64::MAX (~9.2e18).
CREATE SEQUENCE IF NOT EXISTS synthetic_id_seq START WITH 9000000000000000;

-- Webhook channels (WhatsApp, later IG/Twitter) redeliver events; first
-- insert wins, replays are dropped.
CREATE TABLE IF NOT EXISTS channel_events (
    channel     TEXT NOT NULL,
    event_id    TEXT NOT NULL,
    received_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (channel, event_id)
);
