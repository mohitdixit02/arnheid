-- Backfill for the columns added to items/chunks in 010_user_brain.sql.
-- Split into its own migration for the same CockroachDB reason as
-- 012_channels_backfill.sql: DML on a column can't share a migration file
-- with the ALTER TABLE ADD COLUMN that created it.

UPDATE items
SET owner_user_id = shared_by
WHERE owner_user_id IS NULL AND shared_by IS NOT NULL;

UPDATE items
SET source_channel = source
WHERE source_channel IS NULL;

ALTER TABLE items ALTER COLUMN source_channel SET DEFAULT 'telegram';

CREATE INDEX IF NOT EXISTS items_owner_shared_idx
    ON items (owner_user_id, shared_at DESC);

UPDATE chunks c
SET owner_user_id = i.owner_user_id
FROM items i
WHERE c.item_id = i.id AND c.owner_user_id IS NULL AND i.owner_user_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS chunks_owner_idx ON chunks (owner_user_id);
