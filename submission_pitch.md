# Arnheid: Production-Grade AI Agent Memory & Orchestration Engine

This document serves as the project submission pitch, outlining the product concept, technical architecture, CockroachDB memory layer integration, and AWS cloud deployment.

---

## 1. Product Pitch: What is Arnheid?

### The Problem
Teams coordinate and exchange critical information across messaging platforms (like Telegram and WhatsApp) every day. Ideas, documents, task lists, and timelines are constantly shared in high-volume group chats. However, this collective intelligence is lost almost immediately as it scrolls off the screen. Important links go unread, decisions are forgotten, and context-dependent tasks are lost in ambient chatter.

### The Solution: Arnheid
Arnheid is an autonomous, channel-agnostic AI agent that sits directly inside your messaging groups and DMs, passively capturing shared knowledge and transforming it into a structured, executable memory layer. 

By operating directly where teams communicate, Arnheid:
* **Captures ambient intelligence**: Automatically parses and cleans shared links, documents, voice notes (with STT transcription), and text updates.
* **Builds a Knowledge Graph**: Dynamically extracts entities and relationships from ingested content, mapping how concepts, people, and topics interconnect over time.
* **Orchestrates Action in the World**: Uses Model Context Protocol (MCP) and GSuite integrations to act on context. For example, if a team discusses targets in a chat, Arnheid can automatically draft and send progress emails or schedule calendar invites.
* **Understands Surrounding Context**: The agent does not just process isolated prompts; it observes the preceding group chat history, resolving ambiguous entities (like "the PO" or "the targets") before executing actions.

---

## 2. CockroachDB: The Memory & Vector Storage Layer

At the core of Arnheid’s reliability and intelligence sits **CockroachDB Cloud**. CockroachDB acts as the unified, distributed database powering the application state, transactional queues, and semantic retrieval systems.

### Where CockroachDB Sits in the Architecture
```
┌─────────────────────────────────────────────────────────────┐
│                       Messaging Clients                     │
│                     (Telegram & WhatsApp)                   │
└──────────────┬───────────────────────────────▲──────────────┘
               │ Ingest Messages               │ Responses & Alerts
               ▼                               │
┌──────────────────────────────────────────────┴──────────────┐
│                  Arnheid Rust Application                   │
│         (Agent Loop, MCP Registry, Cron Scheduler)           │
└──────────────┬───────────────────────────────▲──────────────┘
               │ Write Logs / Vector Queries   │ Schema & Chunks
               ▼                               │
┌──────────────────────────────────────────────┴──────────────┐
│                    CockroachDB Cloud                        │
│   (Vector Indexes, Chat Buffers, Ingestion Jobs, Memory)   │
└─────────────────────────────────────────────────────────────┘
```

### Key Capabilities Powered by CockroachDB

* **Distributed Vector Indexing (RAG)**:
  Arnheid stores and queries semantic embeddings of ingested web pages and notes directly inside CockroachDB using its native `VECTOR` data type. CockroachDB supports C-SPANN indexes, enabling low-latency semantic search at scale. This eliminates the need for a separate vector database, ensuring zero consistency gaps between data structures and vector spaces.
* **Short-Term Context Buffering**:
  Every incoming message across chats is buffered in a rolling `messages_buffer` table. When users trigger the agent, the system queries CockroachDB to extract the chronological history, providing the LLM with immediate surrounding context.
* **Transactional Queue Durability**:
  Webpage extraction, YouTube video transcript processing, and embedding generation are run asynchronously. CockroachDB hosts the durable `ingestion_jobs` queue, ensuring that background jobs are executed reliably with exponential backoff retries, even across application crashes.
* **CLI Auditing & Observability**:
  To protect this critical database layer, the bot runs a background cron service that periodically invokes the `ccloud` CLI. It queries the cluster health and backup status directly from Cockroach Cloud, verifying that backups are fresh (under 24 hours old) and the cluster state is active (`"RUNNING"`). If any anomalies occur, it instantly fires GSuite email alerts to the administrator.

---

## 3. AWS Cloud Infrastructure

Arnheid is deployed on **Amazon Web Services (AWS)** to achieve production-grade reliability, secure network boundaries, and high-performance message processing.

### EC2 (Elastic Compute Cloud) hosting
* **Host Engine**: The Rust application and its dependencies run inside an AWS EC2 instance. The Rust compiled binary runs inside Docker containers, providing a lightweight, portable environment.
* **Channel Webhooks**: The EC2 instance hosts an HTTP webhook server that listens for incoming Meta API payloads (WhatsApp Cloud API webhooks) and routes them to the Rust message parser in real-time.

### Secure Database Networking
* **Remote Access Management**: The EC2 instance connects to a secure CockroachDB Cloud Serverless instance. The connection is encrypted via SSL/TLS, utilizing native WebPKI verification (`sslmode=verify-full`) to protect data in transit.
* **Headless Operations**: All monitoring audits utilize secure API Keys to pull backup logs and metrics from the CockroachDB control plane to the EC2 host.
