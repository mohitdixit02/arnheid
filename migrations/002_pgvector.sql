-- No-op on CockroachDB: VECTOR is a built-in type, not an extension (CRDB has
-- no CREATE EXTENSION at all). Kept as a migration file so the numbering and
-- checksum history stay stable; the dimensioned `embedding` / `interest_vector`
-- columns and the vector index are created at startup by
-- `db::ensure_vector_schema`, because the dimension depends on the configured
-- embedding model (768 for nomic-embed-text, 1024 for mxbai-embed-large, 1536
-- for OpenAI).
SELECT 1;
