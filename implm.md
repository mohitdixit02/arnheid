# Simplified Implementation Plan - Arnhied × CockroachDB Memory Plane

This plan outlines the code changes required to implement the CockroachDB agentic memory plane on Arnhied, using the **simplified time-saving stack** (OpenAI embeddings, CockroachDB Cloud, and local/EC2 deployment).

---

## 1. Ground Rules & Simplifications

* **No AWS Bedrock:** We will use the existing OpenAI API client inside Arnhied to generate 1536-dimensional vector embeddings, avoiding AWS IAM and model request approval delays.
* **No AWS EKS:** We will run and test the application on the local Kind cluster configuration (`just up`) or a single AWS EC2 Ubuntu instance.
* **Database Platform:** CockroachDB Cloud Serverless (hosted on AWS).

---

## 2. File-by-File Code Changes

### A. Database Migrations
#### [NEW] `services/api-rs/crates/centaur-session-sqlx/memory-migrations/0001_agent_memory.sql`
Create the CockroachDB tables for long-term memory. We use 1536-dimensional vectors to match OpenAI's `text-embedding-3-small` dimensions:

```sql
-- 1. Table for long-term memories and facts
CREATE TABLE agent_memories (
    memory_id     UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    scope_key     STRING NOT NULL,          -- tenant identifier
    thread_key    STRING,                   -- source chat session ID
    kind          STRING NOT NULL,          -- 'fact' | 'decision' | 'preference'
    content       STRING NOT NULL,          -- actual text
    source_ref    JSONB NOT NULL DEFAULT '{}',
    embedding     VECTOR(1536),             -- OpenAI Embedding
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at  TIMESTAMPTZ,
    use_count     INT NOT NULL DEFAULT 0,
    superseded_by UUID REFERENCES agent_memories(memory_id),

    VECTOR INDEX agent_memories_scope_embedding_idx
        (scope_key, embedding vector_cosine_ops)
);

-- 2. Table for task outcomes
CREATE TABLE memory_episodes (
    episode_id  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    scope_key   STRING NOT NULL,
    thread_key  STRING NOT NULL,
    task        STRING NOT NULL,
    outcome     STRING NOT NULL,            -- 'succeeded' | 'failed'
    summary     STRING NOT NULL,
    embedding   VECTOR(1536),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),

    VECTOR INDEX memory_episodes_scope_embedding_idx
        (scope_key, embedding vector_cosine_ops)
);

-- 3. Row-Level Security Rules for isolation
ALTER TABLE agent_memories ENABLE ROW LEVEL SECURITY;
CREATE POLICY agent_memories_tenant ON agent_memories
    FOR SELECT TO centaur_memory_reader
    USING (scope_key = current_setting('centaur.scope_key', true));
```

---

### B. Python Workflows (Durable Workflows)
#### [NEW] `workflows/agent_memory_embeddings.py`
Workflow to generate embeddings using Arnhied's standard OpenAI client helpers:

```python
from centaur.workflows import WorkflowContext
from centaur.llm.openai import get_openai_client

async def handler(params: dict, ctx: WorkflowContext):
    text_to_embed = params["text"]
    
    # Run inside ctx.step to ensure idempotency and prevent double-billing
    async def call_openai():
        client = get_openai_client()
        resp = await client.embeddings.create(
            input=[text_to_embed],
            model="text-embedding-3-small"
        )
        return resp.data[0].embedding
        
    embedding = await ctx.step("generate_embedding", call_openai)
    return {"embedding": embedding}
```

#### [NEW] `workflows/agent_memory_capture.py`
Workflow triggered automatically upon session closure to summarize the session and write facts/episodes to CockroachDB:

```python
from centaur.workflows import WorkflowContext
from db import write_memory_to_crdb

async def handler(params: dict, ctx: WorkflowContext):
    session_id = params["session_id"]
    scope_key = params["scope_key"]
    
    # 1. Fetch transcript and extract facts
    summary, facts = await ctx.run_agent("summarize_and_extract_facts", {"session_id": session_id})
    
    # 2. Write to CockroachDB
    await ctx.step("save_to_crdb", lambda: write_memory_to_crdb(scope_key, session_id, summary, facts))
```

---

### C. Sandbox Memory Recall Tool
#### [NEW] `tools/productivity/memory/pyproject.toml` & `cli.py`
The tool that agents run inside sandboxes to perform vector queries against CockroachDB:

```python
# tools/productivity/memory/cli.py
import click
from centaur.db import get_crdb_connection

@click.group()
def main():
    pass

@main.command()
@click.argument("query")
@click.option("--limit", default=5)
def recall(query, limit):
    # Speaks standard pgwire connection
    conn = get_crdb_connection()
    results = conn.execute(
        """
        SELECT content, kind FROM agent_memories 
        WHERE scope_key = %s AND superseded_by IS NULL
        ORDER BY embedding <=> %s LIMIT %s
        """,
        (current_scope(), query_embedding(query), limit)
    )
    for r in results:
        click.echo(f"[{r.kind}] {r.content}")
```

---

### D. Agent Skills (Prompt Engineering)
#### [NEW] `.agents/skills/agent-memory/SKILL.md`
Instructs the Claude agent sandbox to query the memory tool before executing non-trivial tasks:

```markdown
---
name: agent-memory
description: Use this skill to recall past decisions and facts from CockroachDB memory.
---
# How to use Agentic Memory

1. When starting a task in a new session, run:
   `memory recall "<query_details>"`
2. If the user tells you to change a preference or decision, save it:
   `memory remember "<new_fact>" --kind fact`
```

---

### E. Admin Dashboard
#### [NEW] `services/console/app/controllers/console/memory_controller.rb`
Create the read-only controller bypassing authentication for evaluation:

```ruby
class Console::MemoryController < ApplicationController
  # Skip login requirements so judges can evaluate the metrics panel
  skip_before_action :require_login, only: [:index]

  def index
    @memories = AgentMemory.all.limit(50)
    @latencies = AgentMemory.recent_query_latencies
  end
end
```
