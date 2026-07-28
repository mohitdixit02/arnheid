//! Cron job bodies: graph building, cleanup, and health/retry sweep.

use crate::db;
use crate::graph::builder;
use crate::ingestion::{fetcher, youtube};
use crate::state::AppState;
use anyhow::Result;

/// Every 6h — build graph edges from unprocessed items.
pub async fn graph_builder(state: &AppState) {
    match builder::run(state, state.config.ingestion_batch_size).await {
        Ok(s) => {
            tracing::info!(
                items = s.items,
                entities = s.entities,
                edges = s.edges,
                "graph build complete"
            );
            println!(
                "[info] graph build complete: items={} entities={} edges={}",
                s.items, s.entities, s.edges
            );
        }
        Err(e) => {
            tracing::error!(error = %e, "graph build failed");
            eprintln!("[error] graph build failed: error={e}");
        }
    }
}

/// Every 24h — purge the messages buffer, old finished ingestion jobs, and
/// log per-group stats.
pub async fn cleanup(state: &AppState) {
    match db::groups::purge_old_buffer(&state.pool).await {
        Ok(n) => {
            tracing::info!(removed = n, "buffer purge complete");
            println!("[info] buffer purge complete: removed={n}");
        }
        Err(e) => {
            tracing::error!(error = %e, "buffer purge failed");
            eprintln!("[error] buffer purge failed: error={e}");
        }
    }

    match db::ingestion_jobs::purge_old(&state.pool).await {
        Ok(n) => {
            tracing::info!(removed = n, "ingestion job purge complete");
            println!("[info] ingestion job purge complete: removed={n}");
        }
        Err(e) => {
            tracing::warn!(error = %e, "ingestion job purge failed");
            eprintln!("[warn] ingestion job purge failed: error={e}");
        }
    }

    if let Err(e) = log_group_stats(state).await {
        tracing::warn!(error = %e, "group stats logging failed");
        eprintln!("[warn] group stats logging failed: error={e}");
    }

    match db::taste::calibrate_all(&state.pool, state.config.default_relevance_threshold).await {
        Ok(n) if n > 0 => {
            tracing::info!(raised = n, "taste threshold calibration");
            println!("[info] taste threshold calibration: raised={n}");
        }
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(error = %e, "taste calibration failed");
            eprintln!("[warn] taste calibration failed: error={e}");
        }
    }
}

async fn log_group_stats(state: &AppState) -> Result<()> {
    let groups: Vec<(i64, Option<String>)> =
        sqlx::query_as("SELECT id, name FROM groups").fetch_all(&state.pool).await?;

    for (group_id, name) in groups {
        let (total, week): (i64, i64) = sqlx::query_as(
            r#"
            SELECT
              COUNT(*),
              COUNT(*) FILTER (WHERE shared_at > NOW() - INTERVAL '7 days')
            FROM items WHERE group_id = $1
            "#,
        )
        .bind(group_id)
        .fetch_one(&state.pool)
        .await?;

        let notifications = match db::notifications::count_for_group(&state.pool, group_id).await {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(group_id, error = %e, "notification count failed; reporting 0");
                eprintln!("[warn] notification count failed; reporting 0: group_id={group_id} error={e}");
                0
            }
        };

        tracing::info!(
            group_id,
            name = name.as_deref().unwrap_or("?"),
            total_items = total,
            items_this_week = week,
            notifications,
            "group stats"
        );
        println!(
            "[info] group stats: group_id={group_id} name={} total_items={total} items_this_week={week} notifications={notifications}",
            name.as_deref().unwrap_or("?")
        );
    }
    Ok(())
}

/// Every 15 min — log queue depth and retry items stuck in pending_retry.
///
/// This retries the URL *fetch* for an item that already exists as a stub
/// (`items.fetch_status = 'pending_retry'`) — a narrower, item-level concern
/// than the durable `ingestion_jobs` queue's own job-level retry, and not yet
/// folded into it (see migration 014's comment).
pub async fn health_and_retry(state: &AppState) {
    match db::ingestion_jobs::queue_depth(&state.pool).await {
        Ok(depth) => {
            tracing::info!(queue_depth = depth, "health check");
            println!("[info] health check: queue_depth={depth}");
        }
        Err(e) => {
            tracing::warn!(error = %e, "queue depth check failed");
            eprintln!("[warn] queue depth check failed: error={e}");
        }
    }

    let pending = match db::items::pending_retries(&state.pool, 20).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "fetching pending retries failed");
            eprintln!("[warn] fetching pending retries failed: error={e}");
            return;
        }
    };

    for (id, _group_id, url) in pending {
        let fetched = if youtube::is_youtube(&url) {
            youtube::fetch(&url, &state.config.ytdlp_path).await
        } else {
            fetcher::fetch(&url).await
        };
        match fetched {
            Ok(content) if content.available => {
                let body = fetcher::truncate_for_llm(&content.text, 4000);
                let title = content.title.clone().unwrap_or_default();
                let (summary, tags, category) =
                    match state.chat.summarize(&title, &body).await {
                        Ok(s) => (s.summary, s.tags, s.category),
                        Err(e) => {
                            tracing::warn!(item_id = %id, error = %e, "summarize failed during retry; using raw excerpt");
                            eprintln!("[warn] summarize failed during retry; using raw excerpt: item_id={id} error={e}");
                            (content.text.chars().take(300).collect(), vec![], None)
                        }
                    };
                let embed_input = format!("{title}\n{summary}\n{}", tags.join(" "));
                let embedding = match state.embedder.embed(&embed_input).await {
                    Ok(e) => Some(e),
                    Err(e) => {
                        tracing::warn!(item_id = %id, error = %e, "embedding failed during retry");
                        eprintln!("[warn] embedding failed during retry: item_id={id} error={e}");
                        None
                    }
                };

                if let Err(e) = db::items::update_after_fetch(
                    &state.pool,
                    id,
                    content.title.as_deref(),
                    &content.text,
                    &summary,
                    &tags,
                    category.as_deref(),
                    embedding.as_deref(),
                )
                .await
                {
                    tracing::warn!(item_id = %id, error = %e, "retry update failed");
                    eprintln!("[warn] retry update failed: item_id={id} error={e}");
                } else {
                    tracing::info!(item_id = %id, "retry succeeded");
                    println!("[info] retry succeeded: item_id={id}");
                }
            }
            _ => {
                // Still unavailable — stop retrying to avoid spamming the origin.
                tracing::info!(item_id = %id, "retry exhausted; marking unavailable");
                println!("[info] retry exhausted; marking unavailable: item_id={id}");
                if let Err(e) = db::items::mark_retry_exhausted(&state.pool, id).await {
                    tracing::warn!(item_id = %id, error = %e, "marking retry exhausted failed");
                    eprintln!("[warn] marking retry exhausted failed: item_id={id} error={e}");
                }
            }
        }
    }
}

/// Hourly — run ccloud CLI checks for cluster health and backup freshness.
pub async fn cockroach_cloud_monitor(state: &AppState) {
    let Some(cc) = &state.config.cockroach_cloud else {
        return;
    };

    let mut errors = Vec::new();
    let mut status = "RUNNING".to_string();
    let mut last_backup = "Unknown".to_string();

    // 1. Check Cluster Health
    match run_ccloud(&["cluster", "info", &cc.cluster_id], &cc.api_key).await {
        Ok(json) => {
            let state_val = json["state"].as_str().unwrap_or("");
            if state_val != "RUNNING" {
                status = "ERROR".to_string();
                errors.push(format!(
                    "Cluster state is abnormal: expected RUNNING, got '{state_val}'"
                ));
            }
        }
        Err(e) => {
            status = "ERROR".to_string();
            errors.push(format!("Failed to query cluster info: {e}"));
        }
    }

    // 2. Check Backup Freshness
    match run_ccloud(&["cluster", "backup", "list", &cc.cluster_id], &cc.api_key).await {
        Ok(json) => {
            if let Some(backups) = json.as_array() {
                if let Some(latest) = backups.iter().max_by_key(|b| b["created_at"].as_str().unwrap_or("")) {
                    if let Some(created_at_str) = latest["created_at"].as_str() {
                        last_backup = created_at_str.to_string();
                        if let Ok(created_at) = chrono::DateTime::parse_from_rfc3339(created_at_str) {
                            let duration = chrono::Utc::now().signed_duration_since(created_at.with_timezone(&chrono::Utc));
                            if duration.num_hours() > 24 {
                                errors.push(format!(
                                    "Backup is stale: latest backup was created {} hours ago (limit: 24h)",
                                    duration.num_hours()
                                ));
                            }
                        }
                    }
                } else {
                    errors.push("No database backups found in ccloud registry".to_string());
                }
            } else {
                errors.push("Failed to parse ccloud backup list as a JSON array".to_string());
            }
        }
        Err(e) => {
            errors.push(format!("Failed to list backups: {e}"));
        }
    }

    // Update in-memory dashboard cache
    {
        let mut monitor = state.db_monitor.write().await;
        *monitor = Some(crate::state::DbMonitorData {
            status: status.clone(),
            last_backup: last_backup.clone(),
        });
    }

    // 3. Dispatch Email Alert if Errors Found
    if !errors.is_empty() {
        let subject = "CRITICAL: CockroachDB Cloud Cluster Alert";
        let body = format!(
            "CockroachDB Cloud CLI Monitor detected the following issues with cluster '{cid}':\n\n\
             {err_list}\n\n\
             Please inspect your CockroachDB Cloud Console immediately.",
            cid = cc.cluster_id,
            err_list = errors.iter().map(|e| format!("- {e}")).collect::<Vec<_>>().join("\n")
        );

        if let Some(mcp) = &state.mcp {
            let email_args = serde_json::json!({
                "to": "arnheid79@gmail.com",
                "subject": subject,
                "body": body
            });
            let res = mcp.call("gsuite_gmail_send", &email_args).await;
            tracing::error!(errors = ?errors, email_result = %res, "CockroachDB Cloud cluster monitoring alerts triggered");
            eprintln!("[error] CockroachDB Cloud cluster monitoring alerts triggered: {res}");
        } else {
            tracing::error!(errors = ?errors, "CockroachDB Cloud cluster monitoring alerts triggered (GSuite email failed: MCP registry not configured)");
            eprintln!("[error] CockroachDB Cloud cluster monitoring alerts triggered (GSuite email failed: MCP registry not configured)");
        }
    }
}

async fn run_ccloud(args: &[&str], api_key: &str) -> Result<serde_json::Value> {
    let mut cmd_args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    cmd_args.push("--output".to_string());
    cmd_args.push("json".to_string());

    let output = tokio::process::Command::new("ccloud")
        .args(&cmd_args)
        .env("COCKROACH_CLOUD_API_KEY", api_key)
        .output()
        .await?;

    if !output.status.success() {
        anyhow::bail!(
            "ccloud exited with status {code}: {err}",
            code = output.status.code().unwrap_or(-1),
            err = String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let stdout_str = String::from_utf8_lossy(&output.stdout);
    let json_start = stdout_str.find(|c| c == '{' || c == '[');
    let parsed: serde_json::Value = match json_start {
        Some(idx) => serde_json::from_str(&stdout_str[idx..])?,
        None => anyhow::bail!("No JSON object or array found in ccloud output: {}", stdout_str),
    };
    Ok(parsed)
}
