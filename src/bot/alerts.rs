//! Rate-limited admin alerting: DM the configured admin chat when a
//! user-facing action fails internally, instead of only logging it.
//!
//! Before this existed, every failure inside a query or command handler was
//! logged and otherwise silent — nobody but whoever happened to be tailing
//! the process logs would know the bot had stopped answering. `notify` is the
//! deliberate exception to "never let the bot go silent": it always tries to
//! tell someone, but never itself becomes a source of failure (a broken alert
//! is only ever logged, never propagated).

use crate::state::AppState;
use std::time::{Duration, Instant};
use teloxide::prelude::*;
use teloxide::types::ParseMode;

/// Minimum gap between two alerts sharing the same `tag`, so a backend that's
/// down for an hour sends one DM, not one per failed message.
const COOLDOWN: Duration = Duration::from_secs(15 * 60);
/// Telegram messages cap at 4096 chars; keep alerts well under that.
const MAX_DETAIL_CHARS: usize = 500;

/// DM the admin chat (if `ADMIN_CHAT_ID` is configured) about `tag`, at most
/// once per [`COOLDOWN`] window per distinct tag. Never returns an error —
/// alert delivery failing is itself only logged.
pub async fn notify(state: &AppState, tag: &str, detail: &str) {
    let Some(chat_id) = state.config.admin_chat_id else {
        return;
    };

    if !should_send(state, tag) {
        return;
    }

    let text = format!(
        "⚠️ <b>Arnheid error</b>\n<code>{}</code>\n{}",
        crate::bot::formatter::esc(tag),
        crate::bot::formatter::esc(&clip(detail, MAX_DETAIL_CHARS)),
    );
    if let Err(e) = state
        .bot
        .send_message(ChatId(chat_id), text)
        .parse_mode(ParseMode::Html)
        .await
    {
        tracing::warn!(error = %e, tag, "admin alert delivery failed");
        eprintln!("[warn] admin alert delivery failed: tag={tag} error={e}");
    }
}

/// True at most once per [`COOLDOWN`] per `tag`; updates the cooldown clock
/// as a side effect when it returns true.
fn should_send(state: &AppState, tag: &str) -> bool {
    let mut cooldowns = state
        .alert_cooldowns
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(last) = cooldowns.get(tag) {
        if last.elapsed() < COOLDOWN {
            return false;
        }
    }
    cooldowns.insert(tag.to_string(), Instant::now());
    true
}

/// Truncate `s` to at most `max` chars, appending `…` if it was longer.
/// Shared by admin alerts and `/ping`'s error-detail display.
pub(crate) fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_leaves_short_strings_untouched() {
        assert_eq!(clip("hello", 10), "hello");
    }

    #[test]
    fn clip_truncates_long_strings() {
        let long = "x".repeat(600);
        let clipped = clip(&long, MAX_DETAIL_CHARS);
        assert_eq!(clipped.chars().count(), MAX_DETAIL_CHARS + 1); // + '…'
        assert!(clipped.ends_with('…'));
    }

    #[test]
    fn clip_handles_multibyte_chars_without_panicking() {
        let s = "🧠".repeat(10);
        let clipped = clip(&s, 3);
        assert_eq!(clipped.chars().count(), 4); // 3 + '…'
    }
}
