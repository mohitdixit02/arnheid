//! HTTP surface: always-on `/health` and `/metrics`, plus the WhatsApp
//! webhook when that channel is configured. Telegram remains long-polling
//! and needs no inbound HTTP of its own.
//!
//! The webhook POST path only verifies, parses, and spawns — every slow
//! thing (media download, STT, LLM calls) happens off the request path.

use crate::db;
use crate::state::AppState;
use crate::whatsapp::{handler, signature, types::WebhookPayload};
use anyhow::{Context, Result};
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use std::collections::HashMap;

pub async fn serve(state: AppState) -> Result<()> {
    let port = state.config.http_port;
    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/metrics", get(metrics_handler))
        .route("/api/dashboard", get(dashboard_handler))
        .route(
            "/webhook/whatsapp",
            get(verify_webhook).post(receive_webhook),
        )
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .context("binding http listener")?;
    tracing::info!(port, "http server listening");
    println!("[info] http server listening: port={port}");
    axum::serve(listener, app).await.context("http server")
}

/// Prometheus text exposition. Queue depth is queried live (not cached)
/// since it's a cheap COUNT and scrapes are infrequent — always current
/// beats whatever the last 15-minute cron tick happened to see.
async fn metrics_handler(State(state): State<AppState>) -> String {
    let depth = db::ingestion_jobs::queue_depth(&state.pool).await.ok();
    state.metrics.render(depth)
}

/// Meta's one-time subscription handshake: echo hub.challenge back iff
/// the verify token matches.
async fn verify_webhook(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let expected = state.config.whatsapp.as_ref().map(|w| w.verify_token.as_str());
    let mode_ok = params.get("hub.mode").map(String::as_str) == Some("subscribe");
    let token_ok = params.get("hub.verify_token").map(String::as_str) == expected && expected.is_some();

    match (mode_ok && token_ok, params.get("hub.challenge")) {
        (true, Some(challenge)) => (StatusCode::OK, challenge.clone()),
        _ => {
            tracing::warn!("webhook verification rejected");
            eprintln!("[warn] webhook verification rejected");
            (StatusCode::FORBIDDEN, String::new())
        }
    }
}

/// Inbound webhook. Signature failures get 401; everything after that
/// returns 200 no matter what — Meta disables webhooks that keep failing,
/// and redeliveries are already deduped on the message id.
async fn receive_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let Some(wa_config) = state.config.whatsapp.as_ref() else {
        return StatusCode::NOT_FOUND;
    };
    let sig = headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok());
    if !signature::verify(&wa_config.app_secret, &body, sig) {
        tracing::warn!("webhook signature verification failed");
        eprintln!("[warn] webhook signature verification failed");
        return StatusCode::UNAUTHORIZED;
    }

    let payload: WebhookPayload = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "unparseable webhook payload");
            eprintln!("[warn] unparseable webhook payload: error={e}");
            return StatusCode::OK;
        }
    };

    for entry in payload.entry {
        for change in entry.changes {
            let value = change.value;
            for message in &value.messages {
                let name = value.profile_name(&message.from);
                // Spawn per message: the webhook must return immediately.
                tokio::spawn(handler::process_message(
                    state.clone(),
                    message.clone(),
                    name,
                ));
            }
        }
    }
    StatusCode::OK
}

/// Serve live dashboard database metrics and Cockroach Cloud monitor status.
async fn dashboard_handler(
    State(state): State<AppState>,
) -> impl IntoResponse {
    let total_items: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM items")
        .fetch_one(&state.pool)
        .await
        .unwrap_or(0);

    let total_entities: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM entities")
        .fetch_one(&state.pool)
        .await
        .unwrap_or(0);

    let total_edges: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM edges")
        .fetch_one(&state.pool)
        .await
        .unwrap_or(0);

    let total_chunks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chunks")
        .fetch_one(&state.pool)
        .await
        .unwrap_or(0);

    let monitor_data = {
        let monitor = state.db_monitor.read().await;
        monitor.clone()
    };

    let (status, last_backup) = match monitor_data {
        Some(data) => (data.status, data.last_backup),
        None => ("UNKNOWN".to_string(), "Unknown".to_string()),
    };

    // Query recent items
    let recent_items_rows = sqlx::query(
        "SELECT title, url, source, shared_at FROM items ORDER BY shared_at DESC LIMIT 5"
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let recent_items: Vec<serde_json::Value> = recent_items_rows
        .into_iter()
        .map(|row| {
            use sqlx::Row;
            let title: Option<String> = row.try_get("title").ok();
            let url: String = row.try_get("url").unwrap_or_default();
            let source: String = row.try_get("source").unwrap_or_default();
            let shared_at: chrono::DateTime<chrono::Utc> = row.try_get("shared_at").unwrap_or_else(|_| chrono::Utc::now());
            serde_json::json!({
                "title": title.unwrap_or_else(|| "Untitled Ingestion".to_string()),
                "url": url,
                "source": source,
                "shared_at": shared_at
            })
        })
        .collect();

    // Query recent entities
    let recent_entities_rows = sqlx::query(
        "SELECT name, \"type\", first_seen FROM entities ORDER BY first_seen DESC LIMIT 5"
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let recent_entities: Vec<serde_json::Value> = recent_entities_rows
        .into_iter()
        .map(|row| {
            use sqlx::Row;
            let name: String = row.try_get("name").unwrap_or_default();
            let type_val: String = row.try_get("type").unwrap_or_default();
            let first_seen: chrono::DateTime<chrono::Utc> = row.try_get("first_seen").unwrap_or_else(|_| chrono::Utc::now());
            serde_json::json!({
                "name": name,
                "type": type_val,
                "first_seen": first_seen
            })
        })
        .collect();

    let cluster_id = state.config.cockroach_cloud.as_ref()
        .map(|cc| cc.cluster_id.clone())
        .unwrap_or_default();

    let response_body = serde_json::json!({
        "database": {
            "status": status,
            "last_backup": last_backup,
            "cluster_id": cluster_id
        },
        "memory_stats": {
            "total_captured_items": total_items,
            "total_extracted_entities": total_entities,
            "total_knowledge_edges": total_edges,
            "total_vector_chunks": total_chunks
        },
        "recent_items": recent_items,
        "recent_entities": recent_entities
    });

    let mut headers = HeaderMap::new();
    headers.insert("Access-Control-Allow-Origin", "*".parse().unwrap());
    headers.insert("Access-Control-Allow-Methods", "GET".parse().unwrap());
    headers.insert("Content-Type", "application/json".parse().unwrap());

    (StatusCode::OK, headers, axum::Json(response_body))
}
