//! Database access layer. Thin, hand-written sqlx queries (runtime-checked,
//! so the project builds without a live database at compile time).

pub mod agent_memory;
pub mod channels;
pub mod chunks;
pub mod edges;
pub mod entities;
pub mod events;
pub mod graph;
pub mod groups;
pub mod ingestion_jobs;
pub mod items;
pub mod notifications;
pub mod profiles;
pub mod sessions;
pub mod taste;
pub mod vector;

use anyhow::{Context, Result};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::time::Duration;

/// Build the connection pool.
pub async fn init_pool(database_url: &str) -> Result<PgPool> {
    PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(10))
        .connect(database_url)
        .await
        .context("failed to connect to Postgres")
}

/// Run all migrations in `./migrations`.
pub async fn run_migrations(pool: &PgPool) -> Result<()> {
    // CockroachDB speaks the Postgres wire protocol but has no advisory locks
    // (`pg_advisory_lock`), which sqlx's migrator uses by default to guard
    // against concurrent runs. Locking is sqlx's own recommended workaround
    // for CockroachDB; safe here since Arnheid runs migrations from a single
    // process at startup, not from concurrent instances.
    let mut migrator = sqlx::migrate!("./migrations");
    migrator.set_locking(false);
    migrator.run(pool).await.context("migration failed")
}

/// Create the native `VECTOR` columns + distributed vector index (C-SPANN) at
/// the configured dimension.
///
/// The dimension lives in config (it varies by embedding model: 768 for
/// `bge-base-en-v1.5`, 1024 for `bge-large-en-v1.5`), so the
/// DDL can't be a static migration. `dim` is an internal integer — safe to
/// interpolate. Adding a column is a no-op if it already exists; changing the
/// dimension of an existing column requires a fresh database.
///
/// Requires the cluster setting `feature.vector_index.enabled = true` (set
/// once during cluster provisioning — see scripts/provision_cockroachdb.sh).
pub async fn ensure_vector_schema(pool: &PgPool, dim: usize) -> Result<()> {
    let stmts = [
        format!(
            "ALTER TABLE user_taste_profiles ADD COLUMN IF NOT EXISTS interest_vector vector({dim})"
        ),
        format!("ALTER TABLE items ADD COLUMN IF NOT EXISTS embedding vector({dim})"),
        "CREATE VECTOR INDEX IF NOT EXISTS items_embedding_idx ON items (embedding)".to_string(),
        format!(
            "ALTER TABLE user_profiles ADD COLUMN IF NOT EXISTS interest_vector vector({dim})"
        ),
        format!("ALTER TABLE chunks ADD COLUMN IF NOT EXISTS embedding vector({dim})"),
        "CREATE VECTOR INDEX IF NOT EXISTS chunks_embedding_idx ON chunks (embedding)"
            .to_string(),
        format!(
            "ALTER TABLE agent_archival_memory ADD COLUMN IF NOT EXISTS embedding vector({dim})"
        ),
        "CREATE VECTOR INDEX IF NOT EXISTS agent_archival_embedding_idx \
         ON agent_archival_memory (embedding)"
            .to_string(),
    ];
    for stmt in stmts {
        // `dim` is an internal integer, never user input — safe to execute as
        // raw SQL (we assert past sqlx's dynamic-string injection guard).
        sqlx::raw_sql(sqlx::AssertSqlSafe(stmt.clone()))
            .execute(pool)
            .await
            .with_context(|| format!("vector schema DDL failed: {stmt}"))?;
    }
    Ok(())
}
