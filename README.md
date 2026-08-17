<div align="center">

<img src="assets/main.jpg" alt="Arnheid" width="200" />

# ARNHEID

### The Autonomous Memory Engine for Team Communication

**A context capture protocol — forward anything, remember everything, ask anything.**

<sub>Powered by</sub>

**🪳 CockroachDB Cloud** &nbsp;·&nbsp; **☁️ AWS**

<br>

[![License: MIT](https://img.shields.io/badge/License-MIT-000000.svg?style=flat-square)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.82%2B-CE422B.svg?style=flat-square&logo=rust&logoColor=white)](Cargo.toml)
[![CockroachDB](https://img.shields.io/badge/CockroachDB-Serverless-6933FF.svg?style=flat-square)](https://cockroachlabs.cloud)
[![AWS](https://img.shields.io/badge/AWS-EC2-FF9900.svg?style=flat-square&logo=amazonec2&logoColor=white)](#prerequisites--installation)
[![React](https://img.shields.io/badge/React-19-61DAFB.svg?style=flat-square&logo=react&logoColor=black)](web/)

[**Try the bot →**](https://t.me/arnheidgenbot) &nbsp;·&nbsp; [**Demo video →**](#demo--video) &nbsp;·&nbsp; [**Architecture →**](#architecture)

</div>

---

Teams lose their best thinking to the scroll. A link shared on Tuesday, a decision made in a voice
note, a spec dropped in a thread — all gone by Friday.

**Arnheid** is an always-on AI agent that lives inside your Telegram and WhatsApp chats. It silently
captures every link, note, voice memo, and photo shared around it, ingests the *full content*
(articles, YouTube transcripts, transcribed audio, described images), embeds it into a distributed
vector space, and builds a knowledge graph over it. Ask it a question and it reasons over your own
material like a researcher — with citations. Share something relevant to a teammate's interests and
it DMs them the exact excerpt, unprompted.

**CockroachDB Cloud is the memory layer.** Relational state, vector embeddings, the knowledge graph,
the durable job queue, and the agent's own self-editing memory all live in one serializable,
distributed database — no separate vector store, no consistency gaps. The whole thing ships as one
Rust binary on **AWS EC2**.

---

## Table of Contents

- [Key Features](#key-features)
- [Architecture](#architecture)
- [CockroachDB Suites Used](#cockroachdb-suites-used)
- [Prerequisites & Installation](#prerequisites--installation)
- [Environment Guide](#environment-guide)
- [Demo & Video](#demo--video)
- [Upcoming Features](#upcoming-features)
- [Credits](#credits)
- [License](#license)
- [Contact & Thanks](#contact--thanks)

---

## Key Features

### Capture — zero friction, zero commands

| | Feature | What it does |
|---|---|---|
| 📥 | **Passive link ingestion** | Any URL shared in a chat is fetched, cleaned, summarized, chunked, and indexed in the background. No command, no @mention, no folders. |
| 💬 | **Text ingestion & memory** | Messages, summaries, and stray ideas are buffered into a rolling 48-hour `messages_buffer`, preserving short-term chat context around every capture. |
| 🔗 | **Webpage scraping** | Readability-style extraction pulls the readable body out of articles, docs, and repos; `yt-dlp` pulls full YouTube transcripts. |
| 🎙️ | **Voice & vision** | WhatsApp voice notes are transcribed via Whisper; photos are described by Claude vision. Both become first-class, searchable knowledge. |
| 🧵 | **The context moat** | Every item stores the 3–5 messages surrounding the share — capturing *why* it was shared, not just *what*. No bookmarking tool has this. |
| 🔁 | **Durable ingestion queue** | Every capture is a CockroachDB row the moment it lands. A restart never loses one; failures retry with exponential backoff before alerting an admin. |

### Recall — retrieve broadly, reason narrowly

| | Feature | What it does |
|---|---|---|
| 🧠 | **Chunk-level RAG** | Content is split into overlapping passages and embedded individually, so `/ask` reads the *whole* document — not a summary of it. |
| 🔎 | **Hybrid retrieval** | CockroachDB native vector search **+** GIN full-text search, fused with Reciprocal Rank Fusion. Embeddings blur acronyms and proper nouns; keyword search catches them. |
| 🕸️ | **Knowledge graph construction** | Entities (people, companies, technologies, topics) and typed edges are extracted across items and stored in CockroachDB, then walked one hop to expand retrieval along connections vector similarity missed. |
| 🧭 | **Multi-query / HyDE + self-correction** | The question is expanded into alternate probes plus a hypothetical answer; if synthesis reports it can't answer, it emits follow-up queries and retrieves again before giving up. |
| 📎 | **Cited synthesis** | Answers ground every claim in a numbered source and list only the sources actually used. |

### Act — a real agent, not a search box

| | Feature | What it does |
|---|---|---|
| 🤖 | **Smart agentic loop** | A bounded 5-turn reasoning loop (max 3 tool calls/turn) where the model decides each turn whether it can answer outright or needs a tool. Small talk never triggers a search. |
| 🧰 | **Universal MCP tool layer** | Built-ins (`search_brain`, `graph_lookup`, `web_search`, `fetch_url`, `search_chat_history`) and every third-party MCP server are described and dispatched through **one** namespaced surface — the loop never special-cases a backend. |
| 📧 | **GSuite integrations** | In-process Gmail / Calendar / Drive tools (`gsuite_gmail_search`, `gsuite_gmail_send`, `gsuite_calendar_create_event`, `gsuite_drive_search`, …) — no sidecar to deploy, and the Google refresh token never leaves the bot. |
| 🌐 | **Web search interceptor** | A shell-command search backend (`ddgr`) augments answers with live results when the brain alone can't cover the question. |
| 🧬 | **Self-editing agent memory** | MemGPT-style core memory blocks + unbounded archival memory, both stored in CockroachDB and edited by the model itself. Identity persists across sessions, models, and machines — because it's a row, not process state. |
| 🎯 | **The relevance interrupt** | Per-user interest vectors (with time decay) score every new capture. Cross a personal threshold and that user gets a DM with the exact relevant excerpt — the moment the product feels like magic. |

### Operate — production posture

| | Feature | What it does |
|---|---|---|
| 🦀 | **One Rust binary** | Bot dispatcher, webhook server, ingestion worker, scorer, agent loop, and cron all in-process on one Tokio runtime. |
| 🪙 | **Cheap & swappable models** | Five model tiers, each routable independently to Anthropic, Ollama, DeepSeek, Groq, or any OpenAI-compatible endpoint. |
| 📈 | **Prometheus `/metrics`** | Ask outcomes and latency, ingestion throughput and queue depth, send failures, relevance DMs, agentic-fallback rate. |
| 🖥️ | **Live React dashboard** | Polls `/api/dashboard` every 5s for cluster status, backup freshness, and live memory counts. |
| 🛡️ | **Untrusted-input hardening** | Remote MCP tool descriptions and results are control-char stripped, truncated, and namespaced at the boundary; no server can shadow a built-in tool name. |

---

## Architecture

### 1. System topology — where CockroachDB and AWS sit

```mermaid
flowchart TB
    subgraph CLIENTS["💬 Messaging Clients"]
        TG["Telegram<br/>DMs + Groups"]
        WA["WhatsApp<br/>Cloud API"]
    end

    subgraph AWS["☁️ AWS EC2 · Ubuntu 24.04 · Docker + systemd"]
        direction TB
        HTTP["axum HTTP server<br/>/webhook/whatsapp · /health<br/>/metrics · /api/dashboard"]
        DISP["teloxide dispatcher<br/>long-polling"]
        ROUTER["LLM intent router<br/>capture vs. question"]
        INTAKE["intake.rs<br/>channel-agnostic capture"]
        WORKER["ingestion worker<br/>fetch → extract → chunk → embed"]
        AGENT["agentic /ask loop<br/>5 turns · 3 calls/turn"]
        TOOLBOX["MCP toolbox<br/>built-ins + integrations"]
        CRON["cron scheduler<br/>graph · cleanup · health · cluster monitor"]
        SCORER["relevance scorer<br/>interest vectors"]
    end

    subgraph CRDB["🪳 CockroachDB Cloud Serverless — THE MEMORY LAYER"]
        direction LR
        REL["Relational state<br/>groups · users · channel_events"]
        VEC["Vector space<br/>items.embedding · chunks.embedding<br/>C-SPANN VECTOR INDEX"]
        GRAPH["Knowledge graph<br/>entities · edges"]
        QUEUE["Durable queue<br/>ingestion_jobs"]
        BUF["Short-term buffer<br/>messages_buffer · chat_sessions"]
        AMEM["Agent memory<br/>core blocks · archival facts"]
    end

    subgraph EXT["🔌 External Services"]
        HF["Hugging Face<br/>embeddings"]
        LLM["Anthropic / Groq / DeepSeek / Ollama<br/>chat tiers"]
        GS["Google APIs<br/>Gmail · Calendar · Drive"]
        DDG["ddgr<br/>web search"]
    end

    TG --> DISP
    WA --> HTTP
    HTTP --> ROUTER
    DISP --> ROUTER
    ROUTER -->|capture| INTAKE
    ROUTER -->|question| AGENT

    INTAKE -->|enqueue job| QUEUE
    QUEUE --> WORKER
    WORKER --> HF
    WORKER --> LLM
    WORKER -->|write| REL
    WORKER -->|write| VEC
    WORKER --> SCORER
    SCORER -->|read interest vectors| REL
    SCORER -->|relevance DM| TG

    AGENT --> TOOLBOX
    TOOLBOX -->|search_brain| VEC
    TOOLBOX -->|graph_lookup| GRAPH
    TOOLBOX -->|search_chat_history| BUF
    TOOLBOX -->|core/archival memory| AMEM
    TOOLBOX -->|web_search| DDG
    TOOLBOX -->|gsuite_*| GS
    AGENT --> LLM
    AGENT -->|cited answer| TG
    AGENT -->|cited answer| WA

    CRON --> GRAPH
    CRON -->|ccloud CLI| CRDB
    CRON -->|alert email| GS
    HTTP -->|live stats| CRDB

    style CRDB fill:#1a0b33,stroke:#6933FF,stroke-width:3px,color:#fff
    style AWS fill:#2b1a00,stroke:#FF9900,stroke-width:3px,color:#fff
    style CLIENTS fill:#0b2233,stroke:#2AABEE,color:#fff
    style EXT fill:#141414,stroke:#666,color:#fff
```

> **Read this diagram as two boxes.** AWS EC2 is where *compute* lives — one stateless Rust process.
> CockroachDB Cloud is where *everything the agent knows* lives. Kill the EC2 box and restart it: the
> queue resumes, the memory is intact, the agent still knows who you are. That separation is the whole
> design.

### 2. Write path — capture to memory (durable ingestion)

```mermaid
flowchart LR
    A["Link · note · voice · photo"] --> B["intake.rs<br/>URL extraction +<br/>context window"]
    B --> C["ingestion_jobs<br/>status: pending"]

    C --> D["worker claims job<br/>status: processing"]

    subgraph PIPE["Ingestion pipeline"]
        direction TB
        D --> E["Tier 1 · fetch<br/>article | YouTube | STT | vision"]
        E --> F["Tier 2 · LLM<br/>summarize + tag + classify"]
        F --> G["chunk into<br/>overlapping passages"]
        G --> H["Tier 3 · embed<br/>each passage"]
    end

    H --> I["items + chunks<br/>+ VECTOR INDEX"]
    I --> J["relevance scorer<br/>cosine vs. interest vectors"]
    J -->|above threshold| K["personal DM<br/>with exact excerpt"]

    I --> L["Tier 4 · cron 6h<br/>entity + edge extraction"]
    L --> M["entities + edges"]

    D -.->|failure| N{"attempts<br/>&lt; max?"}
    N -.->|yes| C
    N -.->|no| O["status: exhausted<br/>+ admin alert"]

    style C fill:#1a0b33,stroke:#6933FF,color:#fff
    style I fill:#1a0b33,stroke:#6933FF,color:#fff
    style M fill:#1a0b33,stroke:#6933FF,color:#fff
```

### 3. Read path — the agentic `/ask` loop

```mermaid
flowchart TB
    Q["User question"] --> ROUTE{"Needs the<br/>user's material?"}
    ROUTE -->|"no — small talk,<br/>'what can you do'"| ANS["Answer on turn 1<br/>zero tool calls"]
    ROUTE -->|yes| LOOP

    subgraph LOOP["Bounded agent loop · max 5 turns"]
        direction TB
        THINK["Model emits JSON:<br/>{reasoning, message,<br/>tool_calls, final_response}"]
        THINK --> PICK{"Pick tools<br/>max 3 per turn"}
        PICK --> T1["search_brain<br/>→ hybrid vector + FTS + RRF"]
        PICK --> T2["graph_lookup<br/>→ 1-hop entity walk"]
        PICK --> T3["search_chat_history<br/>→ messages_buffer"]
        PICK --> T4["archival_memory_search<br/>→ long-term facts"]
        PICK --> T5["web_search / fetch_url"]
        PICK --> T6["gsuite_* · any MCP server"]
        T1 --> FEED["Append results<br/>to history"]
        T2 --> FEED
        T3 --> FEED
        T4 --> FEED
        T5 --> FEED
        T6 --> FEED
        FEED --> BUDGET{"final_response<br/>or budget spent?"}
        BUDGET -->|no| THINK
    end

    BUDGET -->|yes| ANS
    LOOP -.->|"invalid JSON ×2<br/>or loop error"| FIXED["Fallback: fixed pipeline<br/>expand → hybrid → graph<br/>→ synthesize → self-correct"]
    FIXED --> ANS
    ANS --> OUT["Cited reply on the<br/>same channel"]

    style T1 fill:#1a0b33,stroke:#6933FF,color:#fff
    style T2 fill:#1a0b33,stroke:#6933FF,color:#fff
    style T3 fill:#1a0b33,stroke:#6933FF,color:#fff
    style T4 fill:#1a0b33,stroke:#6933FF,color:#fff
```

<details>
<summary><b>Data model — what each table holds</b></summary>

| Table | Purpose |
|---|---|
| `groups`, `users` | Spaces and people on any channel; `(channel, external_id)` maps native ids onto internal BIGINTs |
| `channel_events` | Webhook idempotency — Meta redeliveries dropped on conflict |
| `items` | One row per capture: url, title, summary, tags, `content_type`, `context_window` (JSONB), item `embedding` |
| `chunks` | Passage-level text + `embedding` (the `/ask` retrieval unit) + a GIN full-text index |
| `entities`, `edges` | The knowledge graph |
| `messages_buffer` | Rolling 48h buffer used to reconstruct context windows |
| `chat_sessions` | Per-chat conversational continuity |
| `user_taste_profiles` / `user_profiles` | Interest vector, relevance threshold, mute state, liked/disliked tags |
| `ingestion_jobs` | The durable queue: payload, status, attempts |
| `notifications_log` | Dedup + calibration log for relevance DMs |
| `agent_memory_blocks`, `agent_archival_memory` | Self-editing core memory + vector-searched long-term facts |

Migrations `001`–`016` are embedded in the binary and applied at boot. Dimensioned vector columns and
their indexes are created at startup from `EMBEDDING_DIM` (`db::ensure_vector_schema`).

</details>

<details>
<summary><b>Model tiers — cheapest model that can do the job</b></summary>

| Tier | Task | Routed by | Default |
|---|---|---|---|
| 1 | URL detect, fetch, extraction, chunking, dedup | — | no model |
| 2 | Summarize, tag, classify, relevance excerpt, query expansion | `TIER2_PROVIDER` | Claude Haiku 4.5 |
| 3 | Embed passages, queries, interest vectors | `EMBEDDING_MODEL` | `BAAI/bge-large-en-v1.5` (1024-d, hosted on HF) |
| 4 | Entity + edge extraction (graph) | `GRAPH_PROVIDER` | Claude Haiku 4.5 |
| 5 | Agentic loop + answer synthesis | `RAG_PROVIDER` | Claude Sonnet 4.6 |
| R | Message intent routing (capture vs. question) | `ROUTER_PROVIDER` | Claude Haiku 4.5 |

Every chat tier accepts `anthropic` or `ollama` (which means *any* OpenAI-compatible endpoint —
Ollama, DeepSeek, Groq, vLLM).

</details>

---

## CockroachDB Suites Used

Three distinct CockroachDB surfaces, each doing a job nothing else in the stack does.

```mermaid
flowchart LR
    subgraph S1["1 · Vector Index"]
        direction TB
        A1["CREATE VECTOR INDEX<br/>C-SPANN, native VECTOR(1024)"]
        A2["items.embedding<br/>chunks.embedding<br/>agent_archival_memory.embedding<br/>interest_vector"]
        A3["cosine &lt;=&gt; queries<br/>fused with GIN full-text via RRF"]
        A1 --> A2 --> A3
    end

    subgraph S2["2 · Cloud Managed MCP"]
        direction TB
        B1["https://cockroachlabs.cloud/mcp<br/>bearer + mcp-cluster-id header"]
        B2["Tools registered as crdb_*<br/>alongside gsuite_* and built-ins"]
        B3["Agent lists, inspects, and<br/>queries tables mid-conversation"]
        B1 --> B2 --> B3
    end

    subgraph S3["3 · ccloud CLI"]
        direction TB
        C1["Hourly cron job<br/>COCKROACH_CLOUD_MONITOR_CRON"]
        C2["ccloud cluster info<br/>ccloud cluster backup list"]
        C3["State != RUNNING or backup &gt; 24h<br/>→ Gmail alert via MCP"]
        C1 --> C2 --> C3
    end

    S1 -.->|"retrieval"| CORE(("Arnheid<br/>Agent"))
    S2 -.->|"introspection"| CORE
    S3 -.->|"assurance"| CORE

    style S1 fill:#1a0b33,stroke:#6933FF,stroke-width:2px,color:#fff
    style S2 fill:#1a0b33,stroke:#6933FF,stroke-width:2px,color:#fff
    style S3 fill:#1a0b33,stroke:#6933FF,stroke-width:2px,color:#fff
    style CORE fill:#2b1a00,stroke:#FF9900,stroke-width:2px,color:#fff
```

| # | Suite | How we integrated it | Where it lives in the code |
|---|---|---|---|
| **1** | **Distributed Vector Index** | Text embeddings are stored in native `VECTOR(1024)` columns beside the relational rows they describe. `CREATE VECTOR INDEX` (C-SPANN) backs low-latency ANN search; every retrieval runs cosine-distance (`<=>`) queries fused with GIN full-text results via Reciprocal Rank Fusion. Vectors cross the wire as bracketed text literals to sidestep client-side binary-codec quirks. | `src/db/vector.rs`, `src/db/mod.rs` (`ensure_vector_schema`), `src/db/chunks.rs`, `src/db/agent_memory.rs`, `migrations/007`, `008`, `016` |
| **2** | **Cloud Managed MCP Server** | A secure Streamable-HTTP MCP client connects to `cockroachlabs.cloud/mcp`, passing the API key as bearer auth and the cluster id via an `mcp-cluster-id` header. Its tools are namespaced (`crdb_*`) and merged into the same registry as the built-ins, so the agent can list, inspect, and query database tables dynamically mid-conversation. | `src/mcp/mod.rs` (`connect_cockroach_cloud`), `src/mcp/client.rs`, `src/config.rs` (`CockroachCloudConfig`) |
| **3** | **ccloud CLI** | An hourly background cron job shells out to the `ccloud` CLI to verify the cluster reports `RUNNING` and that the newest backup is under 24h old. Results feed the live web dashboard; anomalies fire a Gmail alert through the GSuite MCP tool. Runs once immediately at boot, too. | `src/cron/jobs.rs` (`cockroach_cloud_monitor`, `run_ccloud`), `src/cron/mod.rs`, `Dockerfile` (CLI install), `scripts/provision_cockroachdb.sh` |

**Why CockroachDB Cloud for all three:**

- **Unified multi-model memory** — the relational knowledge graph and the high-dimensional semantic
  vectors live in *one* database. No syncing a separate vector store, no consistency gap between the
  row and its embedding.
- **Transactional consistency** — serializable ACID transactions across chat buffers, graph edges, and
  the job queue mean the agent never retrieves a corrupted or duplicated memory.
- **Elastic serverless scaling** — the memory layer absorbs bursty webhook ingestion without a capacity
  decision, and stays available while the EC2 host is rebuilt underneath it.

---

## Prerequisites & Installation

### Prerequisites

| Requirement | Why | Install |
|---|---|---|
| **Rust 1.82+** | Builds the single binary | [rustup.rs](https://rustup.rs) |
| **CockroachDB** | The memory layer | Docker (local) or CockroachDB Cloud Serverless |
| **Telegram bot token** | Primary channel | [@BotFather](https://t.me/BotFather) → `/newbot` |
| **Hugging Face token** | Tier-3 embeddings — *required* | [HF tokens](https://huggingface.co/settings/tokens), **"Inference Providers"** scope |
| **A chat provider key** | Tiers 2/4/5/router | Anthropic, or any OpenAI-compatible endpoint (DeepSeek / Groq / local Ollama) |
| **`yt-dlp`** | YouTube transcripts | [yt-dlp releases](https://github.com/yt-dlp/yt-dlp/releases) |
| **`ccloud` CLI** | Cluster provisioning + monitoring | `brew install cockroachdb/tap/ccloud` |
| **`ddgr`** | Web search backend (optional) | `pip install ddgr` |
| **Node.js 20+** | Web dashboard | [nodejs.org](https://nodejs.org) |

---

### A. Backend — local development

**1 · Start CockroachDB**

```bash
docker compose up -d       # single-node CockroachDB on :26257
```

Or provision CockroachDB Cloud Serverless (required for the AWS deployment):

```bash
./scripts/provision_cockroachdb.sh    # walks the ccloud CLI through cluster creation
```

Then, once on the new cluster:

```bash
cockroach sql --url "$DATABASE_URL" -e "
  SET CLUSTER SETTING feature.vector_index.enabled = true;
  CREATE DATABASE IF NOT EXISTS arnheid;
"
```

**2 · Configure**

```bash
cp .env.example .env       # see the Environment Guide below for the full template
```

At minimum set `TELEGRAM_BOT_TOKEN`, `DATABASE_URL`, `HF_API_KEY`, and one chat provider key.

**3 · Run**

```bash
cargo run                  # migrations + dimensioned vector schema applied automatically at boot
```

You should see `[info] database ready`, `[info] telegram connected`, `[info] Arnheid is live`.

**4 · Test**

```bash
cargo fmt
cargo clippy --all-targets
cargo test                 # unit tests, no DB needed

# integration test against a real DB:
TEST_DATABASE_URL="postgresql://root@localhost:26257/arnheid?sslmode=disable" \
  cargo test --test db_roundtrip -- --ignored --nocapture --test-threads=1
```

---

### B. Backend — AWS EC2 deployment

**Option 1 — Docker (recommended)**

```bash
# on the EC2 box (Ubuntu 24.04)
git clone https://github.com/mohitdixit02/arnheid.git && cd arnheid
cp .env.example .env && nano .env      # fill in tokens + the CockroachDB Cloud DATABASE_URL
docker compose up -d --build           # image bundles ddgr + the ccloud CLI
docker compose logs -f
```

The container exposes port `8080` internally (mapped to `8081` on the host) serving `/health`,
`/metrics`, `/api/dashboard`, and the WhatsApp webhook. Put Caddy or an ALB in front for TLS.

**Option 2 — bare metal + systemd**

```bash
# from your machine:
rsync -av --exclude target --exclude .env --exclude .git ./ user@EC2_IP:arnheid/

ssh user@EC2_IP
cd ~/arnheid
DATABASE_URL='postgresql://...cockroachlabs.cloud:26257/arnheid?sslmode=verify-full' \
  sudo -E bash install.sh              # installs Rust, Ollama, yt-dlp, builds, registers systemd unit

nano /opt/arnheid/.env                 # add TELEGRAM_BOT_TOKEN, HF_API_KEY, chat provider key
sudo systemctl start arnheid
journalctl -u arnheid -f
```

`install.sh` is idempotent and configurable via env (`HF_API_KEY`, `EMBED_MODEL`, `CHAT_MODEL`,
`INSTALL_CHAT_MODEL`, …). `DATABASE_URL` is required on first install; re-runs reuse the one already
in `.env`.

> **Hardware note:** nothing heavy runs on the box. Embeddings are hosted on Hugging Face and the chat
> tiers should point at Anthropic or a cheap hosted OSS endpoint. A `t3.small`/CX22-class instance is
> plenty; more RAM does not make CPU generation fast.

---

### C. Web service — the live dashboard

A React 19 + Vite + Tailwind 4 single-page dashboard that polls the backend's `/api/dashboard` every
5 seconds for cluster status, backup freshness, and live memory counts (items, entities, edges,
vector chunks). It falls back to representative values when the API is unreachable.

```bash
cd web
npm install
npm run dev        # http://localhost:5173 — proxies stats from http://localhost:8080
```

```bash
npm run lint       # oxlint
npm run build      # static bundle → web/dist
npm run preview    # serve the production build locally
```

To deploy, serve `web/dist` from any static host (S3 + CloudFront, Netlify, Vercel) and make sure it
can reach the backend's `/api/dashboard` — same origin needs no extra config, cross-origin needs a
reverse proxy in front of both.

Content shown on the page is driven by JSON, not code — edit
`web/src/data/{pitch,features,commands,roadmap}.json` and it re-renders.

---

### D. Telegram setup & commands guide

**Setup**

1. **Create the bot** — [@BotFather](https://t.me/BotFather) → `/newbot` → paste the token into `TELEGRAM_BOT_TOKEN`.
2. **Disable privacy mode** — BotFather → Bot Settings → Group Privacy → **Turn off**. Without this,
   Telegram only delivers commands and mentions, and Arnheid can't watch ambient links.
3. **Add the bot to your group** — plain member is enough once privacy mode is off.
4. **Every user DMs the bot `/start` once** — bots can't initiate DMs, so relevance notifications only
   reach people who have opened a chat with it.
5. Deep links (`t.me/c/…`) resolve only for **supergroups**.

**Commands**

| Command | What it does |
|---|---|
| `/ask <question>` | Query your personal brain across all channels, automatically augmented with live web results. |
| `/ask --here <question>` | Restrict the search to captures from **this** chat (+ web). |
| `/ask --web-only <question>` | Live web search only — bypass saved context entirely. |
| `/stats` | Captures across all channels, count in this chat, date range, top tags, taste signals. |
| `/taste` | Your liked/disliked tags, notify threshold, and signal counts. |
| `/threshold 0.65` | Tune relevance-DM sensitivity, `0.0`–`1.0`. Lower = more pings. Default `0.72`. |
| `/mute` / `/unmute` | Pause relevance DMs for 24h / resume them. |
| `/ping` | Live health check — DB, chat model, and embedding canary calls plus capability status. |
| `/buildgraph` | Build the knowledge graph now instead of waiting for the 6h cron. |
| `/reindex` | Backfill passage chunks for items ingested before chunk-level RAG. |
| `/help` | Command reference. |
| `/start` | Onboarding message (different in DM vs. group). |

**Beyond commands**

| You do | Arnheid does |
|---|---|
| Share a link in a group | Passively fetches, summarizes, chunks, embeds, and graphs it — no @mention needed |
| DM a link, note, voice memo, or photo | Full ingestion into **your** personal brain |
| `@arnheidgenbot <question>` in a group | Answers from that group's memory |
| `@arnheidgenbot <note or media>` in a group | Captures it (groups need the mention for non-link content) |
| Just message the bot in DM | The LLM intent router decides whether it's a capture or a question |

---

### E. WhatsApp setup (optional second channel)

WhatsApp runs over Meta's **Business Cloud API** — Meta POSTs inbound messages to your HTTPS endpoint.
Review lead times are real, so start early.

1. **Meta developer app** — create one, add the **WhatsApp** product, note the **phone number id**, and
   generate a **permanent system-user token** (the API-Setup test token expires in 24h).
2. **App secret** — App Settings → Basic → copy into `WA_APP_SECRET`. Every webhook POST is
   signature-verified (`X-Hub-Signature-256`); unsigned traffic gets a 401.
3. **Expose the webhook** behind TLS, then in WhatsApp → Configuration set:
   - Callback URL: `https://<your-domain>/webhook/whatsapp`
   - Verify token: the same string as `WA_VERIFY_TOKEN`
   - Subscribe to the **messages** field.
4. **Fill all four `WA_*` vars** — a partial set fails fast at boot by design.

No Meta app yet? Drive the whole pipeline locally with the signed-payload simulator:

```bash
./scripts/simulate_whatsapp.sh text "check this https://youtu.be/dQw4w9WgXcQ"
./scripts/simulate_whatsapp.sh text "where was that pasta place?"
```

### F. GSuite integration (optional)

```bash
python3 scripts/google_oauth_consent.py ~/Downloads/client_secret_*.json
# prints GOOGLE_CLIENT_ID, GOOGLE_CLIENT_SECRET, GOOGLE_REFRESH_TOKEN — paste into .env
```

Stdlib-only, runs once, and the refresh token never leaves your bot.

---

## Environment Guide

Everything is configured through `.env` (loaded by `dotenvy` in dev, by systemd's `EnvironmentFile` in
production). The authoritative list is `src/config.rs`.

> ⚠️ **systemd's `EnvironmentFile` does not strip trailing `# comments`** — keep comments on their own
> lines. And never put real secrets in a committed template.

### Required

| Variable | Meaning |
|---|---|
| `TELEGRAM_BOT_TOKEN` | From @BotFather. The bot will not start without it. |
| `DATABASE_URL` | CockroachDB connection string (Postgres wire-compatible). Use `?sslmode=verify-full` for Cloud. |
| `HF_API_KEY` | Hugging Face token with **"Inference Providers"** scope — powers Tier-3 embeddings. |

### Chat models (Tiers 2 / 4 / 5 / router)

| Variable | Default | Meaning |
|---|---|---|
| `TIER2_PROVIDER` | `anthropic` | Summaries, tags, excerpts, query expansion — `anthropic` or `ollama`. |
| `GRAPH_PROVIDER` | `anthropic` | Tier-4 entity/edge extraction. |
| `RAG_PROVIDER` | `anthropic` | Tier-5 agent loop + answer synthesis. |
| `ROUTER_PROVIDER` | `anthropic` | Message intent router (runs on every inbound message — keep it cheap). |
| `RAG_MODE` | `agentic` | `agentic` (model picks tools per turn) or `pipeline` (fixed sequence). |
| `ANTHROPIC_API_KEY` | — | Required only if any tier routes to `anthropic`. Also enables photo vision. |
| `HAIKU_MODEL` | `claude-haiku-4-5-20251001` | Cheap tier model. |
| `SONNET_MODEL` | `claude-sonnet-4-6` | Reasoning tier model. |
| `ROUTER_MODEL` | `claude-haiku-4-5-20251001` | Intent-router model. |
| `OLLAMA_BASE_URL` | `http://localhost:11434/v1` | Any OpenAI-compatible chat endpoint (Ollama, DeepSeek, Groq, vLLM). |
| `OLLAMA_CHAT_MODEL` | `qwen2.5:3b-instruct` | Model name sent to that endpoint. |
| `OLLAMA_API_KEY` | — | Unset for local Ollama; set for hosted providers. |
| `OLLAMA_ROUTER_MODEL` | falls back to `OLLAMA_CHAT_MODEL` | A cheaper model name for routing on the same endpoint. |

### Embeddings (Tier 3)

| Variable | Default | Meaning |
|---|---|---|
| `EMBEDDING_MODEL` | `BAAI/bge-large-en-v1.5` | Hugging Face feature-extraction model. |
| `EMBEDDING_DIM` | `1024` | **Must match the model.** Changing it later requires a fresh database — vector columns are created at this dimension on first boot. |
| `EMBEDDING_BASE_URL` | `https://router.huggingface.co/hf-inference/models` | Models root for the feature-extraction API. |

### CockroachDB Cloud (MCP + CLI monitoring)

| Variable | Default | Meaning |
|---|---|---|
| `COCKROACH_CLOUD_API_KEY` | — | Cloud API key. Both this and the cluster id are required, or neither. |
| `COCKROACH_CLOUD_CLUSTER_ID` | — | Target cluster UUID. |
| `COCKROACH_CLOUD_MCP_SLUG` | `crdb` | Tool namespace → `crdb_list_tables`, etc. |
| `COCKROACH_CLOUD_MONITOR_CRON` | `0 0 * * * *` (hourly) | 6-field cron for the health + backup-freshness sweep. |

### MCP integrations & GSuite

| Variable | Default | Meaning |
|---|---|---|
| `MCP_SERVERS` | — | `slug=url,slug=url` — remote MCP servers over Streamable HTTP. |
| `MCP_TOKEN_<SLUG>` | — | Bearer token per server, uppercased slug (`MCP_TOKEN_LINEAR`). Kept separate from the URL on purpose. |
| `GOOGLE_CLIENT_ID` / `GOOGLE_CLIENT_SECRET` / `GOOGLE_REFRESH_TOKEN` | — | Built-in GSuite backend. All three or none. |
| `GOOGLE_MCP_SLUG` | `gsuite` | Tool namespace → `gsuite_gmail_search`, … |

### Web search

| Variable | Default | Meaning |
|---|---|---|
| `WEB_SEARCH_CMD` | — | Shell template with `{query}` / `{max}`, e.g. `ddgr --noua --json --num {max} {query}`. Unset disables web search gracefully. |
| `WEB_SEARCH_MAX_RESULTS` | `5` | Results per search. |

### WhatsApp channel

| Variable | Default | Meaning |
|---|---|---|
| `WA_ACCESS_TOKEN` | — | Permanent system-user token. |
| `WA_PHONE_NUMBER_ID` | — | Sender phone number **id**, not the display number. |
| `WA_APP_SECRET` | — | Meta app secret — verifies every webhook signature. |
| `WA_VERIFY_TOKEN` | — | Must match the string typed into the Meta webhook UI. |
| `WA_API_VERSION` | `v20.0` | Graph API version. |
| `GRAPH_BASE_URL` | `https://graph.facebook.com` | Overridable so local dev can point at a stub. |
| `WA_ACK_ON_CAPTURE` | `true` | Reply "✓ saved" after each capture. |

> All four core `WA_*` vars must be present together — a partial set fails fast at boot rather than
> half-enabling a channel.

### Speech-to-text (voice notes)

| Variable | Default | Meaning |
|---|---|---|
| `STT_BASE_URL` | `https://api.groq.com/openai/v1` | Any OpenAI-compatible `/audio/transcriptions` endpoint. |
| `STT_API_KEY` | — | Unset disables voice capture. |
| `STT_MODEL` | `whisper-large-v3-turbo` | Transcription model. |

### Server, ingestion & behaviour

| Variable | Default | Meaning |
|---|---|---|
| `PORT` | `8080` | HTTP port — `/health`, `/metrics`, `/api/dashboard`, and the WhatsApp webhook. |
| `ADMIN_CHAT_ID` | — | Telegram chat id that receives internal failure alerts. Unset disables alerting. |
| `TG_ACK_ON_CAPTURE` | `true` | React 👀 / reply "✓ saved" on Telegram captures. |
| `TELEGRAM_API_URL` | — | Override for testing against a stub Telegram API. |
| `YTDLP_PATH` | `yt-dlp` | Binary used for YouTube transcripts. |
| `CONTEXT_WINDOW_WAIT_SECS` | `60` | How long to wait for trailing context after a link. |
| `INGESTION_BATCH_SIZE` | `20` | Jobs claimed per worker sweep, and items per graph build. |
| `INGESTION_MAX_ATTEMPTS` | `5` | Retries (exponential backoff) before a job is marked exhausted. |
| `URL_DEDUP_DAYS` | `7` | Window in which a repeat URL is treated as a duplicate. |
| `GRAPH_CRON_SCHEDULE` | `0 0 */6 * * *` | 6-field cron (sec min hour dom mon dow). |
| `CLEANUP_CRON_SCHEDULE` | `0 0 0 * * *` | Buffer purge, job purge, stats, taste calibration. |
| `HEALTH_CRON_SCHEDULE` | `0 */15 * * * *` | Queue depth + stuck-item retry sweep. |

### Relevance interrupt

| Variable | Default | Meaning |
|---|---|---|
| `DEFAULT_RELEVANCE_THRESHOLD` | `0.72` | Cosine threshold for sending a relevance DM. |
| `MAX_VECTOR_WEIGHT` | `100.0` | Caps interest-vector accumulation so old items fade. |
| `TASTE_DECAY_LAMBDA` | `0.02` | Per-day exponential decay on taste weight. `0` disables decay. |
| `NOTIFICATION_SCORE_LOG` | `true` | Log every relevance score for threshold calibration. |

### Agent workspace (advanced)

| Variable | Default | Meaning |
|---|---|---|
| `AGENT_WORKSPACE_DIR` | `./agent_workspace` | Root for per-user agent workspaces. |
| `AGENT_SHELL_TOOLS_ENABLED` | `false` | ⚠️ Enables `bash_exec` / `file_read` / `file_write`. Real shell and filesystem access, jailed to the workspace but with **no human-in-the-loop approval**. Opt in deliberately. |

<details>
<summary><b>Minimal .env to get running</b></summary>

```env
# ── Required ───────────────────────────────────────────────
TELEGRAM_BOT_TOKEN=123456:ABC...
DATABASE_URL=postgresql://user:pass@cluster.cockroachlabs.cloud:26257/arnheid?sslmode=verify-full
HF_API_KEY=hf_...

# ── Chat models ────────────────────────────────────────────
ANTHROPIC_API_KEY=sk-ant-...
RAG_MODE=agentic

# ── Optional but recommended ───────────────────────────────
ADMIN_CHAT_ID=123456789
WEB_SEARCH_CMD=ddgr --noua --json --num {max} {query}
STT_API_KEY=gsk_...

# ── CockroachDB Cloud suites ───────────────────────────────
COCKROACH_CLOUD_API_KEY=...
COCKROACH_CLOUD_CLUSTER_ID=...

# ── GSuite (run scripts/google_oauth_consent.py) ───────────
GOOGLE_CLIENT_ID=...
GOOGLE_CLIENT_SECRET=...
GOOGLE_REFRESH_TOKEN=...
```

To run entirely on OSS models instead of Anthropic:

```env
TIER2_PROVIDER=ollama
GRAPH_PROVIDER=ollama
RAG_PROVIDER=ollama
ROUTER_PROVIDER=ollama
OLLAMA_BASE_URL=https://api.deepseek.com/v1
OLLAMA_CHAT_MODEL=deepseek-chat
OLLAMA_API_KEY=sk-...
```

</details>

---

## Demo & Video

<div align="center">

### 🎬 Demo video

> **📹 Walkthrough video — link to be added before submission.**

### 🤖 Try it live

**[t.me/arnheidgenbot](https://t.me/arnheidgenbot)**

DM it a link, then ask it a question about what you sent.

### 🖥️ Live dashboard

Run `cd web && npm run dev` — cluster health, backup freshness, and live memory counts stream from
`/api/dashboard`.

</div>

**A 60-second script if you're demoing it yourself:**

1. Send the bot a dense article link — no command. Watch the 👀 ack.
2. Send a voice note describing what you're working on.
3. `/ask what did that article say about X, and how does it relate to what I told you?`
   — one answer, cited, drawn from both the article passages and the transcribed voice note.
4. `/buildgraph`, then `/ask` something that only connects through an entity → graph expansion.
5. `/ping` → live DB, chat-model, and embedding canaries.
6. Open the dashboard → cluster `RUNNING`, backup fresh, memory counts climbing.

---

## Upcoming Features

```mermaid
timeline
    title Arnheid Roadmap
    section Shipped
        Core capture : Passive link ingestion : Article + YouTube extraction : Voice + photo capture
        Retrieval : Chunk-level RAG : Hybrid vector + FTS with RRF : Knowledge graph expansion
        Agent : 5-turn agentic loop : Universal MCP layer : GSuite integrations : Self-editing memory
        Platform : CockroachDB vector index : Cloud MCP server : ccloud CLI monitoring : Telegram + WhatsApp
    section Next
        Omnichannel : Slack : Microsoft Teams : Discord : Instagram : X/Twitter : Cross-channel identity linking
        MCP ecosystem : Google Docs live editing : Multi-engine search routing : Sandboxed code execution : Automated calendar scheduling
        Visualization : WebGL/Three.js 3D graph : Reasoning-path tracing : Manual memory curation : Vector-space browser
```

| # | Feature | What lands |
|---|---|---|
| **1** | **Omnichannel ingestion & integrations** | Expanding ingress beyond Telegram and WhatsApp to Slack, Microsoft Teams, Discord, Instagram, and X/Twitter. Unified webhooks passively scrape shared links, index text notes, and transcribe voice memos across every workspace — plus cross-channel identity linking so one human with many handles is one brain. |
| **2** | **Extended MCP tooling ecosystem** | Real-time Google Doc editing, multi-engine search routing (cross-verifying facts across Tavily, Google, and Bing), secure sandboxed code execution, and automated calendar slot scheduling. |
| **3** | **Dynamic graph visualization & vector space** | An interactive WebGL/Three.js visualizer on the dashboard mapping semantic connections between links, documents, and notes. Browse your second brain's relational network in 3D, trace reasoning paths, and manually curate or prune memory nodes. |

**Also on the list:** PDF ingestion · weekly resurfacing digest · cross-group convergence signals ·
session continuity for anchored and deictic queries on the agentic path.

> The schema already carries `channel`, `source`, and `content_type`, so new input channels slot in
> without migration pain.

---

## Credits

### Models

| Model | Role |
|---|---|
| **Claude Sonnet 4.6** (Anthropic) | Tier-5 agentic reasoning loop and cited answer synthesis |
| **Claude Haiku 4.5** (Anthropic) | Tier-2 summarization/tagging, Tier-4 graph extraction, message intent routing, photo vision |
| **BAAI/bge-large-en-v1.5** (Hugging Face Inference) | Tier-3 1024-d embeddings for passages, queries, and interest vectors |
| **Whisper Large v3 Turbo** (Groq) | Voice note transcription |
| **Qwen2.5 3B Instruct** (Ollama) | Default local/OSS chat model when tiers route away from Anthropic |
| **DeepSeek / Groq / any OpenAI-compatible endpoint** | Drop-in alternatives for every chat tier |

### Core infrastructure

| | |
|---|---|
| **[CockroachDB Cloud Serverless](https://www.cockroachlabs.com/)** | The memory layer — distributed SQL, native `VECTOR` type, C-SPANN vector indexes, Cloud Managed MCP server, `ccloud` CLI |
| **[AWS EC2](https://aws.amazon.com/ec2/)** | Compute host for the Rust binary, webhook server, and cron scheduler |
| **[Model Context Protocol](https://modelcontextprotocol.io/)** | The tool interoperability standard the whole integration layer speaks |

### Rust crates

`tokio` · `teloxide` · `axum` · `sqlx` · `reqwest` · `scraper` · `html2text` ·
`tokio-cron-scheduler` · `serde` / `serde_json` · `anyhow` · `thiserror` · `tracing` /
`tracing-subscriber` · `dotenvy` · `chrono` · `uuid` · `url` · `futures` · `hmac` / `sha2` / `hex` /
`base64`

### Web stack

`React 19` · `Vite 8` · `Tailwind CSS 4` · `lucide-react` · `oxlint` · `postcss` / `autoprefixer`

### Tools & services

`yt-dlp` (YouTube transcripts) · `ddgr` (web search) · `ccloud` CLI (cluster ops) ·
Docker & Docker Compose · systemd · Caddy (TLS) · Meta WhatsApp Cloud API · Telegram Bot API ·
Google Workspace APIs (Gmail, Calendar, Drive) · Prometheus text exposition

---

## License

Released under the **MIT License**. See [LICENSE](LICENSE).

```
MIT © Arnheid contributors
```

You're free to use, modify, distribute, and build on this — commercially or otherwise. Attribution
appreciated, not required.

---

## Contact & Thanks

<div align="center">

### Get in touch

[![GitHub](https://img.shields.io/badge/GitHub-mohitdixit02%2Farnheid-181717?style=for-the-badge&logo=github)](https://github.com/mohitdixit02/arnheid)
[![Telegram](https://img.shields.io/badge/Telegram-@arnheidgenbot-2AABEE?style=for-the-badge&logo=telegram&logoColor=white)](https://t.me/arnheidgenbot)
[![Email](https://img.shields.io/badge/Email-arnheid79%40gmail.com-EA4335?style=for-the-badge&logo=gmail&logoColor=white)](mailto:arnheid79@gmail.com)

**Issues & feature requests →** [GitHub Issues](https://github.com/mohitdixit02/arnheid/issues)
**Want to contribute? →** [CONTRIBUTING.md](CONTRIBUTING.md)

</div>

### Thanks

To the **CockroachDB** team — for a database that made "one store for relational state, vectors, a
graph, a durable queue, and agent memory" a real architectural choice instead of a compromise. The
native `VECTOR` type and C-SPANN indexes removed an entire moving part from this system, and
serializable transactions meant we never had to reason about a half-written memory.

To **AWS**, for infrastructure boring enough to forget about.

To **Anthropic**, **Hugging Face**, **Groq**, and the **Ollama** community — for making a five-tier
model routing strategy affordable enough that the cheap model does the cheap work.

To the maintainers of **teloxide**, **sqlx**, **axum**, **tokio**, **yt-dlp**, and every crate in the
dependency tree. This is a weekend's worth of visible surface sitting on years of someone else's
careful work.

And to everyone who forwarded a link into a group chat and never saw it again — this one's for you.

<div align="center">

**Two things that must never be refactored away**

*The `context_window` on every item* — the conversation around a shared link. "lol this is exactly
what we're building" next to a fundraise link is metadata no bookmarking tool captures.

*The relevance interrupt* — the personal DM with the exact relevant excerpt. Not a digest, not a
summary. The moment the product feels like magic.

---

<sub>Built with 🪳 CockroachDB Cloud and ☁️ AWS</sub>

</div>
