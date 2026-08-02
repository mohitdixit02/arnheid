//! The relevance interrupt — taste-aware proactive notifications.

use crate::bot::formatter;
use crate::db::{self, events::event_type};
use crate::scorer::ranker;
use crate::state::AppState;
use anyhow::Result;
use chrono::{DateTime, Utc};
use teloxide::prelude::*;
use teloxide::types::ParseMode;
use uuid::Uuid;

pub struct ScoredItem<'a> {
    pub item_id: Uuid,
    pub group_id: i64,
    pub sharer_id: i64,
    pub embedding: &'a [f32],
    pub title: &'a str,
    pub url: &'a str,
    pub message_id: i64,
    pub raw_content: Option<&'a str>,
    pub tags: &'a [String],
    pub shared_at: DateTime<Utc>,
}

pub async fn run(state: &AppState, item: ScoredItem<'_>) {
    if let Err(e) = run_inner(state, &item).await {
        tracing::warn!(item_id = %item.item_id, error = %e, "relevance scorer failed");
        eprintln!("[warn] relevance scorer failed: item_id={} error={e}", item.item_id);
    }
}

async fn run_inner(state: &AppState, item: &ScoredItem<'_>) -> Result<()> {
    let profiles =
        db::taste::list_active_in_group(&state.pool, item.group_id, item.sharer_id).await?;
    if profiles.is_empty() {
        return Ok(());
    }
    tracing::debug!(item_id = %item.item_id, candidates = profiles.len(), "scoring relevance candidates");
    println!(
        "[debug] scoring relevance candidates: item_id={} candidates={}",
        item.item_id,
        profiles.len()
    );

    let now = Utc::now();

    for profile in profiles {
        let score = ranker::relevance_score(
            item.embedding,
            item.tags,
            item.shared_at,
            &profile,
        );

        if score < profile.notify_threshold {
            tracing::debug!(
                user_id = profile.user_id,
                item_id = %item.item_id,
                score,
                threshold = profile.notify_threshold,
                "below relevance threshold; not notifying"
            );
            println!(
                "[debug] below relevance threshold; not notifying: user_id={} item_id={} score={score} threshold={}",
                profile.user_id, item.item_id, profile.notify_threshold
            );
            if state.config.notification_score_log {
                if let Err(e) =
                    db::notifications::log(&state.pool, profile.user_id, item.item_id, score, false)
                        .await
                {
                    tracing::warn!(user_id = profile.user_id, error = %e, "logging below-threshold score failed");
                    eprintln!(
                        "[warn] logging below-threshold score failed: user_id={} error={e}",
                        profile.user_id
                    );
                }
            }
            continue;
        }

        if profile.muted_until.map(|m| m > now).unwrap_or(false) {
            tracing::debug!(user_id = profile.user_id, item_id = %item.item_id, "user muted; skipping notification");
            println!(
                "[debug] user muted; skipping notification: user_id={} item_id={}",
                profile.user_id, item.item_id
            );
            continue;
        }

        if db::notifications::already_notified(&state.pool, profile.user_id, item.item_id).await? {
            tracing::debug!(user_id = profile.user_id, item_id = %item.item_id, "already notified; skipping");
            println!(
                "[debug] already notified; skipping: user_id={} item_id={}",
                profile.user_id, item.item_id
            );
            continue;
        }

        let content = item.raw_content.unwrap_or(item.title);
        let excerpt = match state
            .chat
            .extract_excerpt(content, &profile.liked_tags)
            .await
        {
            Ok(e) if !e.trim().is_empty() => e,
            Err(e) => {
                tracing::warn!(item_id = %item.item_id, error = %e, "excerpt extraction failed; using title");
                eprintln!(
                    "[warn] excerpt extraction failed; using title: item_id={} error={e}",
                    item.item_id
                );
                item.title.to_string()
            }
            _ => item.title.to_string(),
        };

        let sharer_name = db::groups::first_name(&state.pool, item.sharer_id)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| "Someone".to_string());

        let link = formatter::message_link(item.group_id, item.message_id);
        let matching_tag = profile.liked_tags.first().map(|s| s.as_str());

        let text = formatter::notification(
            &sharer_name,
            item.title,
            item.url,
            &excerpt,
            score,
            matching_tag,
            link.as_deref(),
        );
        let keyboard = formatter::notification_keyboard(item.item_id);

        match state
            .bot
            .send_message(ChatId(profile.user_id), text)
            .parse_mode(ParseMode::Html)
            .link_preview_options(formatter::no_preview())
            .reply_markup(keyboard)
            .await
        {
            Ok(_) => {
                state.metrics.inc_relevance_dm_sent();
                tracing::info!(user_id = profile.user_id, item_id = %item.item_id, score, "relevance notification sent");
                println!(
                    "[info] relevance notification sent: user_id={} item_id={} score={score}",
                    profile.user_id, item.item_id
                );
                db::notifications::log(&state.pool, profile.user_id, item.item_id, score, true)
                    .await?;
                if let Err(e) = db::events::log(
                    &state.pool,
                    profile.user_id,
                    event_type::NOTIFY_SENT,
                    Some(item.item_id),
                    0.5,
                    serde_json::json!({ "score": score }),
                )
                .await
                {
                    tracing::warn!(user_id = profile.user_id, error = %e, "logging notify_sent event failed");
                    eprintln!(
                        "[warn] logging notify_sent event failed: user_id={} error={e}",
                        profile.user_id
                    );
                }
                if let Err(e) = db::taste::apply_signal(
                    &state.pool,
                    profile.user_id,
                    item.embedding,
                    0.5,
                    state.config.max_vector_weight,
                    state.config.default_relevance_threshold,
                    state.config.taste_decay_lambda,
                    &[],
                    false,
                    false,
                    false,
                )
                .await
                {
                    tracing::warn!(user_id = profile.user_id, error = %e, "taste signal update failed after notify");
                    eprintln!(
                        "[warn] taste signal update failed after notify: user_id={} error={e}",
                        profile.user_id
                    );
                }
            }
            Err(e) => {
                tracing::info!(user = profile.user_id, error = %e, "notification DM failed");
                println!(
                    "[info] notification DM failed: user={} error={e}",
                    profile.user_id
                );
            }
        }
    }

    Ok(())
}
