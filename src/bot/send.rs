//! Retry wrapper for outbound Telegram API calls. Transient failures (flood
//! control, network blips, and Telegram's own unclassified errors) are
//! retried with backoff; permanent API errors (chat not found, bot blocked,
//! message too long, malformed entities, …) fail on the first attempt since
//! retrying can't fix them.

use std::time::Duration;
use teloxide::{backoff::exponential_backoff_strategy, ApiError, RequestError};

/// Total attempts made before giving up.
const MAX_ATTEMPTS: u32 = 3;

/// Retry a Telegram request up to [`MAX_ATTEMPTS`] times. `attempt` is called
/// fresh on every try — teloxide's request builders are one-shot futures
/// consumed by `.await`, so the caller passes a closure that rebuilds and
/// awaits the request rather than a request value itself:
///
/// ```ignore
/// send::with_retry(|| async {
///     bot.send_message(chat_id, text).await
/// }).await
/// ```
pub async fn with_retry<T, F, Fut>(mut attempt: F) -> Result<T, RequestError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, RequestError>>,
{
    for n in 1..=MAX_ATTEMPTS {
        match attempt().await {
            Ok(v) => return Ok(v),
            Err(e) if n < MAX_ATTEMPTS && is_retryable(&e) => {
                let delay = retry_after(&e).unwrap_or_else(|| exponential_backoff_strategy(n));
                tracing::warn!(
                    attempt = n,
                    error = %e,
                    delay_ms = delay.as_millis() as u64,
                    "telegram send failed, retrying"
                );
                eprintln!(
                    "[warn] telegram send failed, retrying: attempt={n} error={e} delay_ms={}",
                    delay.as_millis() as u64
                );
                tokio::time::sleep(delay).await;
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!("loop always returns within MAX_ATTEMPTS iterations")
}

/// Telegram's explicit flood-control wait, when the error carries one.
fn retry_after(e: &RequestError) -> Option<Duration> {
    match e {
        RequestError::RetryAfter(secs) => Some(secs.duration()),
        _ => None,
    }
}

/// Whether retrying stands a chance. teloxide-core already sleeps 10s
/// internally on any 5xx before parsing the body (see `process_response` in
/// `teloxide-core`'s `net::request`), then tries to JSON-decode it: a JSON
/// error body with a description teloxide doesn't recognize becomes
/// `Api(ApiError::Unknown)`; a non-JSON body (an HTML error page from an
/// intermediary proxy, a truncated response, …) fails to decode at all and
/// becomes `InvalidJson`. Both are worth retrying, same as a network blip or
/// explicit flood control. Everything else — bad chat id, blocked bot,
/// message too long, malformed entities — is a permanent rejection a retry
/// can't fix.
fn is_retryable(e: &RequestError) -> bool {
    matches!(
        e,
        RequestError::RetryAfter(_)
            | RequestError::Network(_)
            | RequestError::Api(ApiError::Unknown(_))
            | RequestError::InvalidJson { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn succeeds_without_retry() {
        let calls = AtomicU32::new(0);
        let result: Result<u32, RequestError> = with_retry(|| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Ok(7) }
        })
        .await;
        assert_eq!(result.unwrap(), 7);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retries_unknown_api_errors_then_succeeds() {
        let calls = AtomicU32::new(0);
        let result: Result<u32, RequestError> = with_retry(|| {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                if n < 2 {
                    Err(RequestError::Api(ApiError::Unknown("Bad Gateway".into())))
                } else {
                    Ok(42)
                }
            }
        })
        .await;
        assert_eq!(result.unwrap(), 42);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn honors_explicit_retry_after() {
        let calls = AtomicU32::new(0);
        let start = std::time::Instant::now();
        let result: Result<u32, RequestError> = with_retry(|| {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                if n == 0 {
                    Err(RequestError::RetryAfter(
                        teloxide::types::Seconds::from_seconds(1),
                    ))
                } else {
                    Ok(1)
                }
            }
        })
        .await;
        assert!(result.is_ok());
        assert!(start.elapsed() >= Duration::from_secs(1));
    }

    #[tokio::test]
    async fn retries_invalid_json_then_succeeds() {
        // A non-JSON 5xx body (an HTML error page from an intermediary
        // proxy, say) deserializes to `InvalidJson`, not `Api(Unknown)` —
        // this must be retried too, not just the JSON-shaped error case.
        let calls = AtomicU32::new(0);
        let result: Result<u32, RequestError> = with_retry(|| {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                if n == 0 {
                    let raw = "<html>502 Bad Gateway</html>";
                    let source = serde_json::from_str::<serde_json::Value>(raw).unwrap_err();
                    Err(RequestError::InvalidJson {
                        source,
                        raw: raw.into(),
                    })
                } else {
                    Ok(9)
                }
            }
        })
        .await;
        assert_eq!(result.unwrap(), 9);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn does_not_retry_permanent_errors() {
        let calls = AtomicU32::new(0);
        let result: Result<u32, RequestError> = with_retry(|| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err(RequestError::Api(ApiError::BotBlocked)) }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn gives_up_after_max_attempts() {
        // `ApiError::Unknown` exercises the same retryable branch as
        // `RequestError::Network` without needing a real `reqwest::Error` —
        // teloxide-core pins reqwest 0.11 while this crate depends on 0.12,
        // so the two `reqwest::Error` types are distinct and not
        // interchangeable even though they share a name.
        let calls = AtomicU32::new(0);
        let result: Result<u32, RequestError> = with_retry(|| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err(RequestError::Api(ApiError::Unknown("Bad Gateway".into()))) }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), MAX_ATTEMPTS);
    }
}
