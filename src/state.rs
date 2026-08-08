//! Shared, cheaply-clonable application state injected into every handler/task.

use crate::config::Config;
use crate::llm::chat::Chat;
use crate::llm::embeddings::Embedder;
use crate::llm::stt::Stt;
use crate::metrics::Metrics;
use crate::search::ShellSearch;
use crate::whatsapp::WhatsApp;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use teloxide::Bot;
use tokio::sync::Notify;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub pool: PgPool,
    pub bot: Bot,
    pub chat: Arc<Chat>,
    pub embedder: Arc<Embedder>,
    /// The bot's own @username (lowercased, no '@'), for mention detection.
    pub bot_username: Arc<String>,
    /// Woken by `intake::schedule_link`/`schedule_note` after enqueuing a
    /// durable `ingestion_jobs` row, so the consumer reacts promptly instead
    /// of waiting for its next poll tick. The row itself (not this signal)
    /// is what makes a capture durable — a missed or coalesced notification
    /// only costs a little latency, never a lost job.
    pub ingestion_notify: Arc<Notify>,
    /// WhatsApp Cloud API client — None unless the channel is configured.
    pub wa: Option<Arc<WhatsApp>>,
    /// Speech-to-text for voice note captures — None without STT_API_KEY.
    pub stt: Option<Arc<Stt>>,
    /// Shell-based web search — None without WEB_SEARCH_CMD.
    pub web_search: Option<Arc<ShellSearch>>,
    /// Third-party tools (GSuite, remote MCP servers) the agentic `/ask` loop
    /// can call. None when nothing is configured or nothing connected — see
    /// `crate::mcp`.
    pub mcp: Option<Arc<crate::mcp::Registry>>,
    /// Last-sent timestamp per admin-alert tag, so `bot::alerts::notify`
    /// sends at most one DM per tag per cooldown window.
    pub alert_cooldowns: Arc<Mutex<HashMap<String, Instant>>>,
    /// In-process counters/gauges rendered as Prometheus text on `GET /metrics`.
    pub metrics: Arc<Metrics>,
    /// Thread-safe cached data for database health and backup monitoring.
    pub db_monitor: Arc<tokio::sync::RwLock<Option<DbMonitorData>>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DbMonitorData {
    pub status: String,
    pub last_backup: String,
}
