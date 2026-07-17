-- Backfill for the channel/external_id columns added in 009_channels.sql.
-- Split into its own migration because CockroachDB won't let DML touch a
-- column added by ALTER TABLE ADD COLUMN until that schema change has
-- committed — separate files guarantee that, a single file doesn't.

UPDATE groups SET external_id = id::text WHERE external_id IS NULL;
ALTER TABLE groups ALTER COLUMN external_id SET NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS groups_channel_external_idx
    ON groups (channel, external_id);

UPDATE users SET external_id = id::text WHERE external_id IS NULL;
ALTER TABLE users ALTER COLUMN external_id SET NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS users_channel_external_idx
    ON users (channel, external_id);
