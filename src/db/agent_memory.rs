//! Persistent, self-editing agent memory (MemGPT/Letta-style):
//!
//! - Core memory: small labelled blocks (persona, human, ...), always shown
//!   in the agentic system prompt, edited by the model itself via
//!   core_memory_append/core_memory_replace. Lives in the DB, not process
//!   memory — that's what makes identity survive a restart or a model swap.
//! - Archival memory: unbounded long-term facts, vector-searched on demand.
//!   Same embedder/vector-search pattern as chunks/items (see `db::vector`).

use super::vector;
use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

pub const DEFAULT_CHAR_LIMIT: i32 = 2000;

const DEFAULT_PERSONA: &str = "I am Arnheid, a sharp, warm second-brain assistant. I help this \
user capture, remember, and act on what matters to them. I am direct and concise, and I say \
plainly when I don't know something rather than guessing.";

const DEFAULT_HUMAN: &str = "(nothing learned about this user yet — fill this in as I learn \
their name, role, preferences, and ongoing projects.)";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MemoryBlock {
    pub label: String,
    pub value: String,
    pub char_limit: i32,
}

/// One archival hit: the content plus how well it matched.
#[derive(Debug, Clone)]
pub struct ArchivalHit {
    pub id: Uuid,
    pub content: String,
    pub created_at: DateTime<Utc>,
    pub similarity: f32,
}

/// Result of an edit tool call: either it applied and here's the new value,
/// or it was rejected (over budget, no match) and here's why — a rejection
/// is not a failure, it's information the model reads and reacts to.
pub enum EditOutcome {
    Applied(String),
    Rejected(String),
}

/// All of this user's core memory blocks, seeding the two defaults
/// (persona, human) on first access so there's always an identity to show
/// the model — a fresh user is never missing one.
pub async fn get_or_seed_blocks(pool: &PgPool, user_id: i64) -> Result<Vec<MemoryBlock>> {
    sqlx::query(
        "INSERT INTO agent_memory_blocks (user_id, label, value, char_limit) \
         VALUES ($1, 'persona', $2, $4), ($1, 'human', $3, $4) \
         ON CONFLICT (user_id, label) DO NOTHING",
    )
    .bind(user_id)
    .bind(DEFAULT_PERSONA)
    .bind(DEFAULT_HUMAN)
    .bind(DEFAULT_CHAR_LIMIT)
    .execute(pool)
    .await?;

    let blocks = sqlx::query_as::<_, MemoryBlock>(
        "SELECT label, value, char_limit FROM agent_memory_blocks \
         WHERE user_id = $1 ORDER BY label",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(blocks)
}

/// Append text to a block, enforcing its char budget. A label that doesn't
/// exist yet is created on the fly with the default limit — the model isn't
/// restricted to `persona`/`human`, it can start a new block just by naming
/// it. Overflowing the budget is a `Rejected`, not an `Err`: the model reads
/// the message and edits the block down with `replace_block` instead, which
/// is the self-management loop this whole thing exists for.
pub async fn append_block(
    pool: &PgPool,
    user_id: i64,
    label: &str,
    text: &str,
) -> Result<EditOutcome> {
    let existing = current_or_default(pool, user_id, label).await?;
    let separator = if existing.value.is_empty() { "" } else { "\n" };
    let new_value = format!("{}{separator}{}", existing.value, text.trim());
    if new_value.chars().count() > existing.char_limit as usize {
        return Ok(EditOutcome::Rejected(format!(
            "appending would make '{label}' {} chars, over its {}-char limit (currently {}). \
             Use core_memory_replace to edit it down first, or append a shorter note.",
            new_value.chars().count(),
            existing.char_limit,
            existing.value.chars().count(),
        )));
    }
    write_block(pool, user_id, label, &new_value, existing.char_limit).await?;
    Ok(EditOutcome::Applied(new_value))
}

/// Replace the first occurrence of `old` with `new` in a block.
pub async fn replace_block(
    pool: &PgPool,
    user_id: i64,
    label: &str,
    old: &str,
    new: &str,
) -> Result<EditOutcome> {
    let existing = current_or_default(pool, user_id, label).await?;
    let Some(pos) = existing.value.find(old) else {
        return Ok(EditOutcome::Rejected(format!(
            "'{old}' was not found in block '{label}' — check the current value and retry with \
             an exact substring."
        )));
    };
    let mut new_value = existing.value.clone();
    new_value.replace_range(pos..pos + old.len(), new);
    if new_value.chars().count() > existing.char_limit as usize {
        return Ok(EditOutcome::Rejected(format!(
            "that replacement would make '{label}' {} chars, over its {}-char limit.",
            new_value.chars().count(),
            existing.char_limit,
        )));
    }
    write_block(pool, user_id, label, &new_value, existing.char_limit).await?;
    Ok(EditOutcome::Applied(new_value))
}

async fn current_or_default(pool: &PgPool, user_id: i64, label: &str) -> Result<MemoryBlock> {
    if let Some(b) = sqlx::query_as::<_, MemoryBlock>(
        "SELECT label, value, char_limit FROM agent_memory_blocks \
         WHERE user_id = $1 AND label = $2",
    )
    .bind(user_id)
    .bind(label)
    .fetch_optional(pool)
    .await?
    {
        return Ok(b);
    }
    Ok(MemoryBlock {
        label: label.to_string(),
        value: String::new(),
        char_limit: DEFAULT_CHAR_LIMIT,
    })
}

async fn write_block(
    pool: &PgPool,
    user_id: i64,
    label: &str,
    value: &str,
    char_limit: i32,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO agent_memory_blocks (user_id, label, value, char_limit, updated_at) \
         VALUES ($1, $2, $3, $4, NOW()) \
         ON CONFLICT (user_id, label) DO UPDATE SET value = $3, updated_at = NOW()",
    )
    .bind(user_id)
    .bind(label)
    .bind(value)
    .bind(char_limit)
    .execute(pool)
    .await?;
    Ok(())
}

/// Store a new long-term fact.
pub async fn insert_archival(
    pool: &PgPool,
    user_id: i64,
    content: &str,
    embedding: &[f32],
) -> Result<Uuid> {
    let qvec = vector::encode(embedding);
    let (id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO agent_archival_memory (user_id, content, embedding) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(user_id)
    .bind(content)
    .bind(qvec)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// Vector search over this user's archival facts.
pub async fn search_archival(
    pool: &PgPool,
    user_id: i64,
    query: &[f32],
    limit: i64,
) -> Result<Vec<ArchivalHit>> {
    let qvec = vector::encode(query);
    let rows: Vec<(Uuid, String, DateTime<Utc>, f64)> = sqlx::query_as(
        "SELECT id, content, created_at, 1 - (embedding <=> $2) AS similarity \
         FROM agent_archival_memory \
         WHERE user_id = $1 AND embedding IS NOT NULL \
         ORDER BY embedding <=> $2 ASC LIMIT $3",
    )
    .bind(user_id)
    .bind(qvec)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, content, created_at, similarity)| ArchivalHit {
            id,
            content,
            created_at,
            similarity: similarity as f32,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_persona_and_human_fit_the_default_limit() {
        assert!(DEFAULT_PERSONA.chars().count() < DEFAULT_CHAR_LIMIT as usize);
        assert!(DEFAULT_HUMAN.chars().count() < DEFAULT_CHAR_LIMIT as usize);
    }
}
