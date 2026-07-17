-- Persistent, self-editing agent memory (MemGPT/Letta-style), per user:
--
-- Core memory: small labelled blocks (persona, human, ...) always shown in
-- the agentic /ask system prompt, edited by the model itself via the
-- core_memory_append / core_memory_replace tools. This is what makes
-- identity persist across sessions, models, and machines — it's a DB row,
-- not in-process state.
--
-- Archival memory: unbounded long-term facts, vector-searched on demand via
-- the archival_memory_insert / archival_memory_search tools. The `embedding
-- vector(N)` column + ANN index are added at startup by
-- db::ensure_vector_schema, same as items/chunks — dimension depends on the
-- embedding model.

CREATE TABLE IF NOT EXISTS agent_memory_blocks (
    user_id    BIGINT NOT NULL,
    label      TEXT NOT NULL,
    value      TEXT NOT NULL DEFAULT '',
    char_limit INT NOT NULL DEFAULT 2000,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (user_id, label)
);

CREATE TABLE IF NOT EXISTS agent_archival_memory (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    BIGINT NOT NULL,
    content    TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS agent_archival_memory_user_idx ON agent_archival_memory (user_id);
