# Contributing to Arnheid

Thanks for your interest! Arnheid is a single Rust binary — bot, ingestion worker,
relevance scorer, query engine, and cron all run in-process. This guide gets you
productive quickly.

## Development setup

```bash
# CockroachDB
docker compose up -d

cp .env.example .env        # fill TELEGRAM_BOT_TOKEN + HF_API_KEY (+ a chat provider key)
cargo run                   # migrations run automatically on boot
```

## Before you open a PR

```bash
cargo fmt
cargo clippy --all-targets
cargo test                  # unit tests, no DB required
```

Integration tests need a database and are `#[ignore]`d by default:

```bash
TEST_DATABASE_URL="postgresql://root@localhost:26257/arnheid?sslmode=disable" \
  cargo test --test db_roundtrip -- --ignored
```

## Conventions

- **Migrations are immutable.** Once a migration has shipped, never edit it —
  sqlx checksums applied migrations and will refuse to start. Add a new
  `NNN_*.sql` file instead. Dimensioned vector columns live in
  `db::ensure_vector_schema` (created at boot from `EMBEDDING_DIM`), not in a
  static migration.
- **`.env.example` is a committed template** — no real secrets, and keep comments
  on their own lines (systemd's `EnvironmentFile` doesn't strip trailing `#`).
- **Models are pluggable.** Don't hard-code a provider; chat tiers route through
  `llm::chat::Chat`, embeddings through `llm::embeddings::Embedder` (any
  OpenAI-compatible endpoint).
- **Ingestion must never crash the bot.** Per-item failures are logged and
  swallowed; degrade to a stub rather than erroring out.
- Keep the **`context_window`** populated and the **relevance interrupt** intact —
  they're the product's core (see the README).

## Where things live

| Area | Module |
|------|--------|
| Telegram handling, commands | `src/bot/` |
| Fetching, transcripts, chunking, pipeline | `src/ingestion/` |
| Model clients + per-tier routing | `src/llm/` |
| Database access | `src/db/` |
| Relevance scoring | `src/scorer/` |
| Knowledge graph build | `src/graph/` |
| The `/ask` retrieval pipeline | `src/query/` |
| Background jobs | `src/cron/` |

## Reporting issues

Include the relevant `journalctl -u arnheid` output (redact secrets), what you
expected, and the bot/model configuration (`/ping` output is handy).
