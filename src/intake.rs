//! Channel-agnostic intake: every ingress (Telegram, WhatsApp, later
//! IG/Twitter) funnels captures through here into the one ingestion queue.
//! Channels resolve identities and transport; intake owns URL extraction,
//! context windows, and job scheduling.

use crate::db;
use crate::models::{ContextMessage, ContextPosition, ContextWindow};
use crate::state::AppState;
use chrono::Utc;

const CONTEXT_BEFORE: i64 = 3;
const CONTEXT_AFTER: i64 = 3;

/// Does a URL-free message read like a question about the brain rather
/// than a note to capture? Cheap heuristic: interrogative opener or "?".
pub fn is_question(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    if t.ends_with('?') {
        return true;
    }
    let first = t.split_whitespace().next().unwrap_or("").to_lowercase();
    const OPENERS: &[&str] = &[
        "have",
        "has",
        "had",
        "please",
        "send",
        "tell",
        "what",
        "whats",
        "what's",
        "where",
        "which",
        "who",
        "when",
        "how",
        "why",
        "find",
        "show",
        "search",
        "any",
        "got",
        "do",
        "does",
        "did",
        "is",
        "are",
        "was",
        "were",
        "can",
        "could",
        "recommend",
        "suggest",
        "list",
    ];
    OPENERS.contains(&first.as_str())
}

/// Does this message want the agent (a question, or an instruction like "send
/// the email"), or is it just something to save? LLM-routed so a leading
/// "@mention" the channel didn't strip, or an imperative with no opener word,
/// still lands on the agent instead of silently being captured as a note —
/// falls back to [`is_question`] if the model call itself fails, so a
/// provider outage degrades rather than blocking capture entirely.
pub async fn wants_agent(state: &AppState, text: &str) -> bool {
    if text.trim().is_empty() {
        return false;
    }
    match state.chat.classify_message_intent(text).await {
        Ok(crate::llm::chat::MessageIntent::Agent) => true,
        Ok(crate::llm::chat::MessageIntent::Capture) => false,
        Err(e) => {
            tracing::warn!(error = %e, "message-intent classification failed, using heuristic");
            eprintln!("[warn] message-intent classification failed, using heuristic: error={e}");
            is_question(text)
        }
    }
}

/// Word-boundary phrase match: `phrase` must appear as whole word(s) in
/// `text`, not merely as a run of letters inside a longer word. A naive
/// `text.contains("it")` matches "inviting" and "gsuite" — real messages
/// that tripped this exact bug and got misrouted as deictic.
fn contains_phrase(text: &str, phrase: &str) -> bool {
    let tokens = |s: &str| -> String {
        let words: Vec<&str> = s
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect();
        format!(" {} ", words.join(" "))
    };
    tokens(text).contains(&tokens(phrase))
}

/// Question refers to something just shared ("this link", "what do you think")?
pub fn is_deictic_query(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    if t.is_empty() {
        return false;
    }
    const PHRASES: &[&str] = &[
        "this",
        "that",
        "it",
        "the link",
        "the article",
        "the post",
        "the video",
        "the note",
        "the photo",
        "what i just",
        "what i shared",
        "what we just",
        "just shared",
        "just sent",
        "from this",
        "about this",
        "understand this",
        "help me understand",
        "pointers from",
    ];
    PHRASES.iter().any(|p| contains_phrase(&t, p))
}

/// Follow-up that should inherit the session's active referents ("tell me more").
pub fn is_follow_up_query(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    if t.is_empty() {
        return false;
    }
    if is_broad_brain_query(&t) {
        return false;
    }
    const PHRASES: &[&str] = &[
        "tell me more",
        "more about",
        "more detail",
        "more on",
        "go deeper",
        "elaborate",
        "expand on",
        "what about",
        "how about",
        "and the",
        "compare",
        "versus",
        "vs",
        "anything else",
        "explain that",
        "why is that",
        "can you explain",
        "what did you mean",
        "what do you mean",
        "keep going",
        "continue",
        "also",
    ];
    if PHRASES.iter().any(|p| contains_phrase(&t, p)) {
        return true;
    }
    // Short utterances in an active thread ("and pricing?", "the founder?").
    let word_count = t.split_whitespace().count();
    word_count <= 6 && (t.ends_with('?') || is_deictic_query(text))
}

/// Explicit whole-brain search — do not inherit session referents.
pub fn is_broad_brain_query_pub(t: &str) -> bool {
    is_broad_brain_query(t)
}

/// A bare greeting or bit of small talk — no informational need, so `/ask`
/// should answer directly instead of spending a search (brain or web) on it.
/// Exact match on the whole trimmed message, not a substring test like the
/// other heuristics here: "hi" should match, but "hi, any news on the
/// robotics deal" must not, so a loose `contains` would be wrong here.
pub fn is_chitchat_query(text: &str) -> bool {
    let t = text
        .trim()
        .trim_end_matches(['?', '!', '.'])
        .trim()
        .to_lowercase();
    if t.is_empty() || t.split_whitespace().count() > 6 {
        return false;
    }
    const PHRASES: &[&str] = &[
        "hi", "hello", "hey", "yo", "sup", "hiya", "howdy", "hi there", "hey there",
        "how are you", "how are u", "how r u", "hows it going", "how's it going",
        "how you doing", "how are you doing", "whats up", "what's up",
        "good morning", "good afternoon", "good evening", "good night",
        "who are you", "what are you", "what can you do", "what do you do",
        "thanks", "thank you", "ty", "thx", "cheers", "cool thanks",
        "lol", "haha", "lmao", "nice", "cool", "ok", "okay", "yep", "nope",
        "test", "testing",
    ];
    PHRASES.contains(&t.as_str())
}

fn is_broad_brain_query(t: &str) -> bool {
    const PHRASES: &[&str] = &[
        "what have we",
        "what have i",
        "what did we",
        "what did i",
        "show me all",
        "search my",
        "search our",
        "search the",
        "find everything",
        "list everything",
        "everything we",
        "everything i",
    ];
    PHRASES.iter().any(|p| contains_phrase(t, p))
}

/// Extract http(s) URLs from free text (whitespace-token scan + validation).
pub fn extract_urls(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for tok in text.split_whitespace() {
        let tok = tok.trim_matches(|c: char| {
            matches!(
                c,
                '(' | ')' | '[' | ']' | '<' | '>' | ',' | '"' | '\'' | '.' | '!' | '?'
            )
        });
        if (tok.starts_with("http://") || tok.starts_with("https://"))
            && url::Url::parse(tok).is_ok()
            && !out.contains(&tok.to_string())
        {
            out.push(tok.to_string());
        }
    }
    out
}

/// Schedule a shared link: enqueue a durable row now, due after the
/// trailing-chatter wait. The context window (which needs that trailing
/// chatter) is built later, at process time — see `ingestion::pipeline`.
#[allow(clippy::too_many_arguments)]
pub async fn schedule_link(
    state: &AppState,
    url: String,
    group_id: i64,
    group_name: Option<String>,
    shared_by: i64,
    message_id: i64,
    forwarded: bool,
    forward_origin: Option<String>,
    source_channel: &str,
) -> anyhow::Result<()> {
    let run_after =
        Utc::now() + chrono::Duration::seconds(state.config.context_window_wait_secs as i64);
    db::ingestion_jobs::enqueue_link(
        &state.pool,
        &url,
        group_id,
        group_name.as_deref(),
        shared_by,
        message_id,
        forwarded,
        forward_origin.as_deref(),
        source_channel,
        run_after,
    )
    .await?;
    state.ingestion_notify.notify_one();
    Ok(())
}

/// Enqueue pre-extracted content (a text note, voice transcript, or image
/// description). No fetch, no wait — the content is already in hand.
#[allow(clippy::too_many_arguments)]
pub async fn schedule_note(
    state: &AppState,
    group_id: i64,
    group_name: Option<String>,
    shared_by: i64,
    message_id: i64,
    forwarded: bool,
    content_type: &str,
    text: String,
    source_channel: &str,
) -> anyhow::Result<()> {
    let title = derive_title(&text, content_type);
    // Pseudo-URL: unique per capture, satisfies items.url NOT NULL and
    // the (group, url, day) dedup index without colliding.
    let url = format!("{content_type}://{group_id}/{message_id}");
    db::ingestion_jobs::enqueue_note(
        &state.pool,
        content_type,
        &url,
        group_id,
        group_name.as_deref(),
        shared_by,
        message_id,
        forwarded,
        source_channel,
        &title,
        &text,
    )
    .await?;
    state.ingestion_notify.notify_one();
    Ok(())
}

/// Build the context window around a pivot message from the rolling buffer.
pub async fn build_context_window(
    state: &AppState,
    group_id: i64,
    message_id: i64,
    forwarded: bool,
    forward_origin: Option<String>,
) -> ContextWindow {
    let mut messages: Vec<ContextMessage> =
        db::groups::messages_before(&state.pool, group_id, message_id, CONTEXT_BEFORE)
            .await
            .unwrap_or_default();

    let after = db::groups::messages_after(&state.pool, group_id, message_id, CONTEXT_AFTER)
        .await
        .unwrap_or_default();
    messages.extend(after);

    // Ensure the pivot itself is represented.
    if !messages.iter().any(|m| m.message_id == message_id) {
        messages.push(ContextMessage {
            user_id: None,
            username: None,
            message_id,
            text: String::new(),
            position: ContextPosition::Pivot,
        });
        messages.sort_by_key(|m| m.message_id);
    }

    ContextWindow {
        messages,
        forwarded,
        forward_origin,
    }
}

/// A display title for content that has none: first line, clipped.
fn derive_title(text: &str, content_type: &str) -> String {
    let first_line = text.lines().next().unwrap_or("").trim();
    let clipped: String = first_line.chars().take(60).collect();
    if clipped.is_empty() {
        match content_type {
            "voice" => "Voice note".to_string(),
            "image" => "Photo".to_string(),
            _ => "Note".to_string(),
        }
    } else if first_line.chars().count() > 60 {
        format!("{clipped}…")
    } else {
        clipped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_extracted_and_deduped() {
        let urls = extract_urls("see https://a.io/x and (https://b.io/y) plus https://a.io/x");
        assert_eq!(urls, vec!["https://a.io/x", "https://b.io/y"]);
    }

    #[test]
    fn titles_derived_sanely() {
        assert_eq!(derive_title("short note", "note"), "short note");
        assert_eq!(derive_title("", "voice"), "Voice note");
        assert!(derive_title(&"x".repeat(100), "note").ends_with('…'));
    }

    #[test]
    fn question_detection() {
        assert!(is_question("where was that ramen place"));
        assert!(is_question("that pasta recipe?"));
        assert!(!is_question("great pasta at Roscioli, get the carbonara"));
        assert!(!is_question(""));
    }

    #[test]
    fn deictic_queries_detected() {
        assert!(is_deictic_query("what do you think about this?"));
        assert!(is_deictic_query("thoughts on that link"));
        assert!(!is_deictic_query("what have we saved on robotics?"));
    }

    /// Regression: "it" as a bare phrase was matched with naive `.contains`,
    /// so any word containing the letters "it" (inviting, gsuite, credit,
    /// legitimate...) was misrouted to the deictic no-referent fallback
    /// instead of the agentic tool loop. These are the two real messages
    /// that hit this bug in production.
    #[test]
    fn deictic_does_not_false_positive_on_substrings_of_it() {
        assert!(!is_deictic_query(
            "can you setup a google meet for tomorrow 6 pm ist inviting a@b.com and yourself"
        ));
        assert!(!is_deictic_query("do you have gsuite access?"));
        assert!(!is_deictic_query("what's a good credit limit to request"));
    }

    #[test]
    fn follow_up_queries_detected() {
        assert!(is_follow_up_query("tell me more"));
        assert!(is_follow_up_query("what about the pricing?"));
        assert!(is_follow_up_query("versus the other one"));
        assert!(!is_follow_up_query("what have we saved on robotics?"));
        assert!(!is_follow_up_query("can you setup a meeting for tomorrow"));
    }

    #[test]
    fn chitchat_detected() {
        assert!(is_chitchat_query("how are u?"));
        assert!(is_chitchat_query("How are you?"));
        assert!(is_chitchat_query("hi"));
        assert!(is_chitchat_query("thanks!"));
        assert!(is_chitchat_query("  Whats up  "));
    }

    #[test]
    fn real_questions_are_not_chitchat() {
        assert!(!is_chitchat_query("what is the weather in bangalore"));
        assert!(!is_chitchat_query("hi, any news on the robotics deal?"));
        assert!(!is_chitchat_query("what have we saved on robotics?"));
        assert!(!is_chitchat_query(""));
    }
}
