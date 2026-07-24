//! Inline-keyboard callback handling — relevance DM dismissal.

use crate::db::{self, events::event_type};
use crate::state::AppState;
use teloxide::prelude::*;
use teloxide::types::InlineKeyboardMarkup;
use uuid::Uuid;

/// teloxide endpoint for callback queries.
pub async fn handle_callback(_bot: Bot, q: CallbackQuery, state: AppState) -> ResponseResult<()> {
    if let Err(e) = handle_inner(&state, &q).await {
        tracing::warn!(error = %e, "callback handler error");
        eprintln!("[warn] callback handler error: error={e}");
    }
    Ok(())
}

async fn handle_inner(state: &AppState, q: &CallbackQuery) -> anyhow::Result<()> {
    let Some(data) = q.data.as_deref() else {
        return Ok(());
    };
    let user_id = q.from.id.0 as i64;

    let toast = match data {
        d if d.starts_with("nd:") => {
            let id = parse_uuid(d.strip_prefix("nd:").unwrap_or(""))?;
            handle_notify_dismiss(state, user_id, id).await?
        }
        _ => return Ok(()),
    };

    state.bot.answer_callback_query(q.id.clone()).text(toast).await?;

    if let Some(message) = q.regular_message() {
        let _ = state
            .bot
            .edit_message_reply_markup(message.chat.id, message.id)
            .reply_markup(InlineKeyboardMarkup::default())
            .await;
    }

    Ok(())
}

async fn handle_notify_dismiss(
    state: &AppState,
    user_id: i64,
    item_id: Uuid,
) -> anyhow::Result<&'static str> {
    if let Err(e) = db::events::log(
        &state.pool,
        user_id,
        event_type::NOTIFY_DISMISS,
        Some(item_id),
        -1.0,
        serde_json::json!({}),
    )
    .await
    {
        tracing::warn!(user_id, item_id = %item_id, error = %e, "logging notify_dismiss event failed");
        eprintln!("[warn] logging notify_dismiss event failed: user_id={user_id} item_id={item_id} error={e}");
    }

    if let Some((embedding, tags)) = db::items::signal_source(&state.pool, item_id).await? {
        if let Err(e) = db::taste::apply_signal(
            &state.pool,
            user_id,
            &embedding,
            -1.0,
            state.config.max_vector_weight,
            state.config.default_relevance_threshold,
            state.config.taste_decay_lambda,
            &tags,
            true,
            false,
            false,
        )
        .await
        {
            tracing::warn!(user_id, item_id = %item_id, error = %e, "taste signal update failed after dismiss");
            eprintln!("[warn] taste signal update failed after dismiss: user_id={user_id} item_id={item_id} error={e}");
        }
    }

    let toast = if db::taste::calibrate_threshold(
        &state.pool,
        user_id,
        state.config.default_relevance_threshold,
        3,
        0.05,
    )
    .await?
    .is_some()
    {
        "Marked not relevant — I'll ping you less often."
    } else {
        "Marked not relevant."
    };

    Ok(toast)
}

fn parse_uuid(s: &str) -> anyhow::Result<Uuid> {
    Uuid::parse_str(s).map_err(|e| anyhow::anyhow!("invalid callback uuid: {e}"))
}
