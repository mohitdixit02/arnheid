//! Typed configuration loaded from the environment (.env in dev).

use crate::llm::chat::ChatRoute;
use crate::llm::Provider;
use anyhow::{Context, Result};
use std::env;

/// How `/ask` retrieves and synthesizes. `Agentic` (default) gives the model
/// a bounded loop where it decides each turn whether to answer outright or
/// call a tool — see `query::agent`. Works on any chat provider: tools are
/// driven by a JSON protocol in the model's text, not provider-native tool
/// calling. `Pipeline` is the older fixed
/// expand→hybrid-retrieve→graph-expand→synthesize→self-correct sequence,
/// still the fallback whenever the agentic path errors out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RagMode {
    Pipeline,
    Agentic,
}

impl RagMode {
    fn parse(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "pipeline" | "fixed" => RagMode::Pipeline,
            _ => RagMode::Agentic,
        }
    }
}

/// One remote MCP server the agent may call tools on (`crate::mcp`).
#[derive(Debug, Clone)]
pub struct McpServerConfig {
    /// Namespace for this server's tools, e.g. `linear` → `linear_create_issue`.
    pub slug: String,
    /// Streamable HTTP endpoint.
    pub url: String,
    /// Bearer token from `MCP_TOKEN_<SLUG>`; None for unauthenticated servers.
    pub token: Option<String>,
}

/// Google OAuth credential behind the built-in GSuite MCP backend. An
/// installed-app client plus a refresh token consented once out-of-band —
/// see `scripts/google_oauth_consent.py`.
#[derive(Debug, Clone)]
pub struct GoogleConfig {
    pub client_id: String,
    pub client_secret: String,
    pub refresh_token: String,
    /// Tool namespace, `GOOGLE_MCP_SLUG` (default `gsuite`).
    pub slug: String,
}

/// CockroachDB Cloud Managed MCP backend settings.
#[derive(Debug, Clone)]
pub struct CockroachCloudConfig {
    pub api_key: String,
    pub cluster_id: String,
    pub slug: String,
}

/// WhatsApp Cloud API channel — present only when the WA_* env vars are set.
#[derive(Debug, Clone)]
pub struct WhatsAppConfig {
    /// Permanent system-user token with whatsapp_business_messaging scope.
    pub access_token: String,
    /// The sender phone number id (NOT the display number).
    pub phone_number_id: String,
    /// Meta app secret — signs every webhook POST (X-Hub-Signature-256).
    pub app_secret: String,
    /// Arbitrary string; must match what you type into the Meta webhook UI.
    pub verify_token: String,
    pub api_version: String,
    /// Meta Graph API root — overridable so local dev can point at a stub.
    pub graph_base_url: String,
    /// Reply "✓ saved" after each capture. Nice while testing; can go quiet.
    pub ack_on_capture: bool,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub telegram_bot_token: String,
    /// Override for local testing against a stub Telegram API; None = real.
    pub telegram_api_url: Option<String>,
    /// React 👀 or reply "✓ saved" after each Telegram capture.
    pub tg_ack_on_capture: bool,
    pub database_url: String,
    /// Telegram chat id the bot DMs when an internal action fails (query,
    /// command, or delivery). None disables alerting entirely.
    pub admin_chat_id: Option<i64>,

    // ── Webhook channels (WhatsApp now, IG/Twitter later) ───────────────
    pub whatsapp: Option<WhatsAppConfig>,
    /// Port for the webhook HTTP server (behind Caddy for TLS).
    pub http_port: u16,

    // ── Speech-to-text (voice note captures) ────────────────────────────
    /// Any OpenAI-compatible /v1/audio/transcriptions endpoint.
    pub stt_base_url: String,
    pub stt_api_key: Option<String>,
    pub stt_model: String,

    // ── Chat (Tiers 2/4/5) ──────────────────────────────────────────────
    /// Optional — only required if any tier routes to Anthropic.
    pub anthropic_api_key: Option<String>,
    pub haiku_model: String,
    pub sonnet_model: String,
    /// OpenAI-compatible chat base, e.g. `http://localhost:11434/v1` (Ollama)
    /// or `https://api.groq.com/openai/v1` (Groq), `https://api.deepseek.com` (DeepSeek).
    pub ollama_base_url: String,
    pub ollama_chat_model: String,
    /// None for local Ollama; set for hosted providers (Groq/DeepSeek/…).
    pub ollama_api_key: Option<String>,
    pub tier2_provider: Provider,
    pub graph_provider: Provider, // tier 4
    pub rag_provider: Provider,   // tier 5
    /// Message-intent router (capture vs. agent) — runs on every inbound
    /// message, so it's split from tier 2 to allow a cheaper/faster model
    /// without touching summarize/excerpt quality.
    pub router_provider: Provider,
    pub router_model: String,
    /// `None` falls back to `ollama_chat_model` — same server, just a
    /// different model name in the request, no second endpoint needed.
    pub ollama_router_model: Option<String>,
    /// `RAG_MODE=agentic` opts into the tool-calling retrieval loop instead
    /// of the fixed pipeline. See `RagMode` doc comment.
    pub rag_mode: RagMode,

    // ── Embeddings (Tier 3) — Hugging Face hosted inference ─────────────
    /// Models root for the HF feature-extraction API; the model path is
    /// appended by `Embedder`.
    pub embedding_url: String,
    pub embedding_api_key: String,
    pub embedding_model: String,
    pub embedding_dim: usize,

    // ── Ingestion ───────────────────────────────────────────────────────
    /// Path to the yt-dlp binary used for YouTube transcript fetching.
    pub ytdlp_path: String,

    // ── Behaviour ───────────────────────────────────────────────────────
    pub context_window_wait_secs: u64,
    pub ingestion_batch_size: usize,
    /// Retries per ingestion job (exponential backoff) before giving up for good.
    pub ingestion_max_attempts: i64,
    pub graph_cron_schedule: String,
    pub cleanup_cron_schedule: String,
    pub health_cron_schedule: String,
    pub default_relevance_threshold: f32,
    pub max_vector_weight: f32,
    /// Exponential decay on taste vector weight (per day). 0 disables decay.
    pub taste_decay_lambda: f32,
    pub notification_score_log: bool,
    pub url_dedup_days: i64,

    // ── Web search (automatic on every /ask) ────────────────────────────────
    /// Shell command template for web search. Supports {query} and {max}
    /// placeholders. Omitting this disables web search gracefully.
    /// Example: `ddgr --noua --json --num {max} {query}`
    pub web_search_cmd: Option<String>,
    pub web_search_max_results: usize,

    // ── MCP integrations (see `crate::mcp`) ───────────────────────────────
    /// Remote MCP servers from `MCP_SERVERS`. Empty = none configured.
    pub mcp_servers: Vec<McpServerConfig>,
    /// Built-in GSuite backend — None without the GOOGLE_* credential.
    pub google: Option<GoogleConfig>,
    /// CockroachDB Cloud Managed MCP backend.
    pub cockroach_cloud: Option<CockroachCloudConfig>,
    /// Background cron schedule for monitoring cluster health and backups.
    pub cockroach_cloud_monitor_cron: String,

    // ── Agent memory + act-in-the-world tools (agentic /ask only) ────────
    /// Root directory for per-user agent workspaces (bash_exec cwd,
    /// file_read/file_write jail). Created on first use.
    pub agent_workspace_dir: String,
    /// Off by default: bash_exec/file_read/file_write give the model real
    /// shell and filesystem access, scoped to its own workspace directory
    /// but otherwise unconfirmed (the agentic loop has no human-in-the-loop
    /// approval step). Opt in deliberately once you trust what you're
    /// pointing this at.
    pub agent_shell_tools_enabled: bool,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let ollama_base_url = opt("OLLAMA_BASE_URL", "http://localhost:11434/v1");

        // Embeddings run on Hugging Face's hosted inference — local Ollama
        // embedding was too slow. The token needs the "Inference Providers"
        // permission: https://huggingface.co/settings/tokens
        let embedding_url = opt(
            "EMBEDDING_BASE_URL",
            "https://router.huggingface.co/hf-inference/models",
        );
        let embedding_api_key = env::var("HF_API_KEY")
            .ok()
            .filter(|s| !s.is_empty())
            .context("missing required env var HF_API_KEY (Hugging Face token for embeddings)")?;

        let config = Self {
            telegram_bot_token: req("TELEGRAM_BOT_TOKEN")?,
            telegram_api_url: env::var("TELEGRAM_API_URL").ok().filter(|s| !s.is_empty()),
            tg_ack_on_capture: opt("TG_ACK_ON_CAPTURE", "true") == "true",
            database_url: req("DATABASE_URL")?,
            admin_chat_id: env::var("ADMIN_CHAT_ID")
                .ok()
                .filter(|s| !s.is_empty())
                .map(|s| s.parse())
                .transpose()
                .context("ADMIN_CHAT_ID must be a valid integer chat id")?,

            whatsapp: whatsapp_from_env()?,
            http_port: opt("PORT", "8080").parse().unwrap_or(8080),

            stt_base_url: opt("STT_BASE_URL", "https://api.groq.com/openai/v1"),
            stt_api_key: env::var("STT_API_KEY").ok().filter(|s| !s.is_empty()),
            stt_model: opt("STT_MODEL", "whisper-large-v3-turbo"),

            anthropic_api_key: env::var("ANTHROPIC_API_KEY").ok().filter(|s| !s.is_empty()),
            haiku_model: opt("HAIKU_MODEL", "claude-haiku-4-5-20251001"),
            sonnet_model: opt("SONNET_MODEL", "claude-sonnet-4-6"),
            ollama_base_url,
            ollama_chat_model: opt("OLLAMA_CHAT_MODEL", "qwen2.5:3b-instruct"),
            ollama_api_key: env::var("OLLAMA_API_KEY").ok().filter(|s| !s.is_empty()),
            tier2_provider: Provider::parse(&opt("TIER2_PROVIDER", "anthropic")),
            graph_provider: Provider::parse(&opt("GRAPH_PROVIDER", "anthropic")),
            rag_provider: Provider::parse(&opt("RAG_PROVIDER", "anthropic")),
            router_provider: Provider::parse(&opt("ROUTER_PROVIDER", "anthropic")),
            router_model: opt("ROUTER_MODEL", "claude-haiku-4-5-20251001"),
            ollama_router_model: env::var("OLLAMA_ROUTER_MODEL").ok().filter(|s| !s.is_empty()),
            rag_mode: RagMode::parse(&opt("RAG_MODE", "agentic")),

            embedding_url,
            embedding_api_key,
            embedding_model: opt("EMBEDDING_MODEL", "BAAI/bge-large-en-v1.5"),
            embedding_dim: opt("EMBEDDING_DIM", "1024").parse().unwrap_or(1024),

            ytdlp_path: opt("YTDLP_PATH", "yt-dlp"),

            context_window_wait_secs: opt("CONTEXT_WINDOW_WAIT_SECS", "60").parse().unwrap_or(60),
            ingestion_batch_size: opt("INGESTION_BATCH_SIZE", "20").parse().unwrap_or(20),
            ingestion_max_attempts: opt("INGESTION_MAX_ATTEMPTS", "5").parse().unwrap_or(5),
            graph_cron_schedule: opt("GRAPH_CRON_SCHEDULE", "0 0 */6 * * *"),
            cleanup_cron_schedule: opt("CLEANUP_CRON_SCHEDULE", "0 0 0 * * *"),
            health_cron_schedule: opt("HEALTH_CRON_SCHEDULE", "0 */15 * * * *"),
            default_relevance_threshold: opt("DEFAULT_RELEVANCE_THRESHOLD", "0.72")
                .parse()
                .unwrap_or(0.72),
            max_vector_weight: opt("MAX_VECTOR_WEIGHT", "100.0").parse().unwrap_or(100.0),
            taste_decay_lambda: opt("TASTE_DECAY_LAMBDA", "0.02").parse().unwrap_or(0.02),
            notification_score_log: opt("NOTIFICATION_SCORE_LOG", "true") == "true",
            url_dedup_days: opt("URL_DEDUP_DAYS", "7").parse().unwrap_or(7),

            web_search_cmd: env::var("WEB_SEARCH_CMD").ok().filter(|s| !s.is_empty()),
            web_search_max_results: opt("WEB_SEARCH_MAX_RESULTS", "5").parse().unwrap_or(5),

            mcp_servers: parse_mcp_servers(&opt("MCP_SERVERS", ""))?,
            google: google_from_env()?,
            cockroach_cloud: cockroach_cloud_from_env()?,
            cockroach_cloud_monitor_cron: opt("COCKROACH_CLOUD_MONITOR_CRON", "0 0 * * * *"),

            agent_workspace_dir: opt("AGENT_WORKSPACE_DIR", "./agent_workspace"),
            agent_shell_tools_enabled: opt("AGENT_SHELL_TOOLS_ENABLED", "false") == "true",
        };

        Ok(config)
    }

    pub fn chat_route(&self) -> ChatRoute {
        ChatRoute {
            tier2: self.tier2_provider,
            tier4: self.graph_provider,
            tier5: self.rag_provider,
            router: self.router_provider,
        }
    }

    /// True if any chat tier needs a local Ollama chat model.
    pub fn uses_ollama_chat(&self) -> bool {
        [
            self.tier2_provider,
            self.graph_provider,
            self.rag_provider,
            self.router_provider,
        ]
        .iter()
        .any(|p| *p == Provider::Ollama)
    }
}

/// All four WA_* vars present → Some; none present → None; a partial set
/// is a config mistake and fails fast.
fn whatsapp_from_env() -> Result<Option<WhatsAppConfig>> {
    const KEYS: [&str; 4] = [
        "WA_ACCESS_TOKEN",
        "WA_PHONE_NUMBER_ID",
        "WA_APP_SECRET",
        "WA_VERIFY_TOKEN",
    ];
    let set: Vec<&str> = KEYS
        .iter()
        .copied()
        .filter(|k| env::var(k).map(|v| !v.is_empty()).unwrap_or(false))
        .collect();
    if set.is_empty() {
        return Ok(None);
    }
    if set.len() < KEYS.len() {
        anyhow::bail!(
            "partial WhatsApp config: {} set but all of {} are required",
            set.join(", "),
            KEYS.join(", ")
        );
    }
    Ok(Some(WhatsAppConfig {
        access_token: req("WA_ACCESS_TOKEN")?,
        phone_number_id: req("WA_PHONE_NUMBER_ID")?,
        app_secret: req("WA_APP_SECRET")?,
        verify_token: req("WA_VERIFY_TOKEN")?,
        api_version: opt("WA_API_VERSION", "v20.0"),
        graph_base_url: opt("GRAPH_BASE_URL", "https://graph.facebook.com"),
        ack_on_capture: opt("WA_ACK_ON_CAPTURE", "true") == "true",
    }))
}

/// `MCP_SERVERS=linear=https://mcp.linear.app/mcp,notion=https://…` — a
/// comma-separated list of `slug=url`. Each server's bearer token, if it needs
/// one, comes from `MCP_TOKEN_<SLUG>` (uppercased, non-alphanumerics → `_`)
/// so credentials never sit in the same variable as the URL.
fn parse_mcp_servers(raw: &str) -> Result<Vec<McpServerConfig>> {
    let mut servers = Vec::new();
    for entry in raw.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let (slug, url) = entry
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("MCP_SERVERS entry '{entry}' must be slug=url"))?;
        let (slug, url) = (slug.trim(), url.trim());
        if slug.is_empty() || url.is_empty() {
            anyhow::bail!("MCP_SERVERS entry '{entry}' must be slug=url");
        }
        // A typo'd scheme would otherwise fail once per tool call at runtime
        // instead of once, loudly, at boot.
        if !url.starts_with("http://") && !url.starts_with("https://") {
            anyhow::bail!("MCP server '{slug}' URL must be http(s): got '{url}'");
        }
        let token_key: String = format!("MCP_TOKEN_{}", slug.to_uppercase())
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        servers.push(McpServerConfig {
            slug: slug.to_string(),
            url: url.to_string(),
            token: env::var(&token_key).ok().filter(|s| !s.is_empty()),
        });
    }
    Ok(servers)
}

/// All three GOOGLE_* vars present → Some; none → None; a partial set is a
/// config mistake and fails fast, same contract as `whatsapp_from_env`.
fn google_from_env() -> Result<Option<GoogleConfig>> {
    const KEYS: [&str; 3] = [
        "GOOGLE_CLIENT_ID",
        "GOOGLE_CLIENT_SECRET",
        "GOOGLE_REFRESH_TOKEN",
    ];
    let set: Vec<&str> = KEYS
        .iter()
        .copied()
        .filter(|k| env::var(k).map(|v| !v.is_empty()).unwrap_or(false))
        .collect();
    if set.is_empty() {
        return Ok(None);
    }
    if set.len() < KEYS.len() {
        anyhow::bail!(
            "partial Google config: {} set but all of {} are required",
            set.join(", "),
            KEYS.join(", ")
        );
    }
    Ok(Some(GoogleConfig {
        client_id: req("GOOGLE_CLIENT_ID")?,
        client_secret: req("GOOGLE_CLIENT_SECRET")?,
        refresh_token: req("GOOGLE_REFRESH_TOKEN")?,
        slug: opt("GOOGLE_MCP_SLUG", "gsuite"),
    }))
}

fn cockroach_cloud_from_env() -> Result<Option<CockroachCloudConfig>> {
    let api_key = env::var("COCKROACH_CLOUD_API_KEY").ok().filter(|s| !s.is_empty());
    let cluster_id = env::var("COCKROACH_CLOUD_CLUSTER_ID").ok().filter(|s| !s.is_empty());
    match (api_key, cluster_id) {
        (Some(key), Some(cid)) => Ok(Some(CockroachCloudConfig {
            api_key: key,
            cluster_id: cid,
            slug: env::var("COCKROACH_CLOUD_MCP_SLUG")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "crdb".to_string()),
        })),
        (None, None) => Ok(None),
        _ => anyhow::bail!("partial Cockroach Cloud MCP config: both COCKROACH_CLOUD_API_KEY and COCKROACH_CLOUD_CLUSTER_ID are required"),
    }
}


fn req(key: &str) -> Result<String> {
    env::var(key).with_context(|| format!("missing required env var {key}"))
}

fn opt(key: &str, default: &str) -> String {
    env::var(key)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_servers_parse_and_reject_junk() {
        let ok =
            parse_mcp_servers(" linear=https://mcp.linear.app/mcp , notion=http://x.dev/mcp ").unwrap();
        assert_eq!(ok.len(), 2);
        assert_eq!(ok[0].slug, "linear");
        assert_eq!(ok[0].url, "https://mcp.linear.app/mcp");
        assert_eq!(ok[1].url, "http://x.dev/mcp");

        assert!(parse_mcp_servers("").unwrap().is_empty());
        assert!(parse_mcp_servers(" , ,").unwrap().is_empty());
        assert!(parse_mcp_servers("noequals").is_err());
        assert!(parse_mcp_servers("slug=").is_err());
        assert!(parse_mcp_servers("=https://x.dev").is_err());
        // A scheme typo fails at boot, not once per tool call.
        assert!(parse_mcp_servers("slug=ftp://x.dev").is_err());
    }
}
