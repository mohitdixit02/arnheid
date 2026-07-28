//! Arnheid — a group ambient intelligence layer for Telegram.
//!
//! Single binary: Telegram dispatcher, ingestion worker, relevance scorer,
//! query handler and cron scheduler all run in-process on one Tokio runtime.

use anyhow::{Context, Result};
use arnheid::config::Config;
use arnheid::llm::anthropic::Anthropic;
use arnheid::llm::chat::Chat;
use arnheid::llm::embeddings::Embedder;
use arnheid::llm::ollama::Ollama;
use arnheid::llm::stt::Stt;
use arnheid::search::ShellSearch;
use arnheid::state::AppState;
use arnheid::whatsapp::WhatsApp;
use arnheid::{bot, cron, db, http, ingestion};
use std::sync::Arc;
use teloxide::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    init_tracing();

    let config = Arc::new(Config::from_env()?);
    tracing::info!(
        web_search_cmd = ?config.web_search_cmd,
        "starting Arnheid config check"
    );
    println!("[info] starting Arnheid config check: web_search_cmd={:?}", config.web_search_cmd);

    // ── Database ────────────────────────────────────────────────────────────
    let pool = db::init_pool(&config.database_url).await?;
    db::run_migrations(&pool).await?;
    db::ensure_vector_schema(&pool, config.embedding_dim).await?;
    tracing::info!(dim = config.embedding_dim, "database ready");
    println!("[info] database ready: dim={}", config.embedding_dim);

    // ── Model clients ───────────────────────────────────────────────────────
    let anthropic = config.anthropic_api_key.clone().map(|key| {
        Anthropic::new(
            key,
            config.haiku_model.clone(),
            config.sonnet_model.clone(),
            config.router_model.clone(),
        )
    });
    let ollama = if config.uses_ollama_chat() {
        Some(Ollama::new(
            config.ollama_base_url.clone(),
            config.ollama_chat_model.clone(),
            config.ollama_router_model.clone(),
            config.ollama_api_key.clone(),
        ))
    } else {
        None
    };
    let chat = Arc::new(Chat::new(anthropic, ollama, config.chat_route())?);

    let embedder = Arc::new(Embedder::new(
        config.embedding_url.clone(),
        config.embedding_api_key.clone(),
        config.embedding_model.clone(),
        config.embedding_dim,
    ));
    tracing::info!(
        model = %config.embedding_model,
        url = %config.embedding_url,
        "embedder ready"
    );
    println!("[info] embedder ready: model={} url={}", config.embedding_model, config.embedding_url);

    // ── Telegram ────────────────────────────────────────────────────────────
    let mut bot = Bot::new(&config.telegram_bot_token);
    if let Some(api_url) = &config.telegram_api_url {
        bot = bot.set_api_url(api_url.parse().context("parsing TELEGRAM_API_URL")?);
    }
    let me = bot
        .get_me()
        .await
        .context("get_me failed — check TELEGRAM_BOT_TOKEN")?;
    let bot_username = me.username().to_string();
    tracing::info!(bot = %bot_username, "telegram connected");
    println!("[info] telegram connected: bot={bot_username}");

    // ── Channel clients ─────────────────────────────────────────────────────
    let wa = config.whatsapp.as_ref().map(|w| {
        Arc::new(WhatsApp::new(
            w.graph_base_url.clone(),
            w.access_token.clone(),
            w.phone_number_id.clone(),
            w.api_version.clone(),
        ))
    });
    let stt = config.stt_api_key.clone().map(|key| {
        Arc::new(Stt::new(
            config.stt_base_url.clone(),
            key,
            config.stt_model.clone(),
        ))
    });
    let web_search = config.web_search_cmd.as_ref().map(|cmd| {
        tracing::info!(cmd = %cmd, max = config.web_search_max_results, "web search enabled");
        println!("[info] web search enabled: cmd={cmd} max={}", config.web_search_max_results);
        Arc::new(ShellSearch::new(cmd.clone(), config.web_search_max_results))
    });
    if web_search.is_none() {
        tracing::warn!("WEB_SEARCH_CMD not set — /ask will answer from internal brain only");
        eprintln!("[warn] WEB_SEARCH_CMD not set — /ask will answer from internal brain only");
    }

    // ── MCP integrations ────────────────────────────────────────────────────
    // Tool lists are fetched once here rather than per question; an
    // unreachable server is logged and skipped inside `connect`.
    let mcp = arnheid::mcp::Registry::connect(&config).await.map(Arc::new);
    if mcp.is_some() && config.rag_mode != arnheid::config::RagMode::Agentic {
        tracing::warn!(
            "MCP integrations are configured but RAG_MODE is not `agentic` — \
             only the agentic /ask loop calls tools, so they will sit unused"
        );
        eprintln!(
            "[warn] MCP integrations are configured but RAG_MODE is not `agentic` — \
             only the agentic /ask loop calls tools, so they will sit unused"
        );
    }

    // ── Wiring ──────────────────────────────────────────────────────────────
    let state = AppState {
        config: config.clone(),
        pool: pool.clone(),
        bot: bot.clone(),
        chat,
        embedder,
        bot_username: Arc::new(bot_username.to_lowercase()),
        ingestion_notify: Arc::new(tokio::sync::Notify::new()),
        wa,
        stt,
        web_search,
        mcp,
        alert_cooldowns: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        metrics: Arc::new(arnheid::metrics::Metrics::new()),
        db_monitor: Arc::new(tokio::sync::RwLock::new(None)),
    };

    // Ingestion consumer (continuous background task) — polls the durable
    // `ingestion_jobs` table rather than draining an in-memory channel.
    {
        let state = state.clone();
        tokio::spawn(async move { ingestion::run_consumer(state).await });
    }

    // HTTP server: /health and /metrics always; the WhatsApp webhook route
    // only actually does anything once that channel is configured.
    {
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(e) = http::serve(state).await {
                tracing::error!(error = %e, "http server died — webhook/metrics endpoint is down");
                eprintln!("[error] http server died — webhook/metrics endpoint is down: error={e}");
            }
        });
    }
    if state.config.whatsapp.is_some() {
        tracing::info!("whatsapp channel enabled");
        println!("[info] whatsapp channel enabled");
    }

    // Cron scheduler. Keep the handle alive for the process lifetime.
    let _scheduler = cron::start(state.clone()).await?;

    // Run initial CockroachDB Cloud monitor check immediately on startup
    {
        let state = state.clone();
        tokio::spawn(async move {
            tracing::info!("running initial cockroach cloud monitor sweep on startup");
            println!("[info] running initial cockroach cloud monitor sweep on startup");
            cron::jobs::cockroach_cloud_monitor(&state).await;
        });
    }

    // ── Run the dispatcher (blocks until Ctrl-C) ───────────────────────────
    tracing::info!("Arnheid is live");
    println!("[info] Arnheid is live");
    bot::run(state).await;

    tracing::info!("shutting down");
    println!("[info] shutting down");
    Ok(())
}

fn init_tracing() {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("arnheid=info,sqlx=warn"));
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(filter)
        .init();
}
