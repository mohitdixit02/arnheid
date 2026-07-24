//! WhatsApp Business Cloud API client — outbound messages and media download.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

pub struct WhatsApp {
    http: reqwest::Client,
    base_url: String,
    access_token: String,
    phone_number_id: String,
    api_version: String,
}

#[derive(Deserialize)]
struct MediaInfo {
    url: String,
    #[serde(default)]
    mime_type: Option<String>,
}

/// Whether a failed send is worth retrying, for [`WhatsApp::send_text`]'s
/// internal retry loop.
enum SendOutcome {
    Retryable(anyhow::Error),
    Permanent(anyhow::Error),
}

impl WhatsApp {
    pub fn new(
        base_url: String,
        access_token: String,
        phone_number_id: String,
        api_version: String,
    ) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client");
        Self {
            http,
            base_url,
            access_token,
            phone_number_id,
            api_version,
        }
    }

    fn graph(&self, path: &str) -> String {
        format!(
            "{}/{}/{path}",
            self.base_url.trim_end_matches('/'),
            self.api_version
        )
    }

    /// Free-form text reply. Only valid inside the 24h customer-service
    /// window, which is always open here because the user just messaged us.
    ///
    /// Retries transient failures (network errors, 429, 5xx) with backoff;
    /// gives up immediately on permanent rejections (bad request, invalid
    /// token, recipient not opted in, …) since retrying can't fix those.
    pub async fn send_text(&self, to: &str, body: &str) -> Result<()> {
        const MAX_ATTEMPTS: u32 = 3;
        let url = self.graph(&format!("{}/messages", self.phone_number_id));
        let payload = json!({
            "messaging_product": "whatsapp",
            "to": to,
            "type": "text",
            "text": { "body": body, "preview_url": true },
        });

        for attempt in 1..=MAX_ATTEMPTS {
            match self.try_send_text(&url, &payload).await {
                Ok(()) => return Ok(()),
                Err(SendOutcome::Retryable(e)) if attempt < MAX_ATTEMPTS => {
                    let delay = Duration::from_secs(1 << (attempt - 1));
                    tracing::warn!(attempt, error = %e, "whatsapp send failed, retrying");
                    eprintln!("[warn] whatsapp send failed, retrying: attempt={attempt} error={e}");
                    tokio::time::sleep(delay).await;
                }
                Err(SendOutcome::Retryable(e)) | Err(SendOutcome::Permanent(e)) => return Err(e),
            }
        }
        unreachable!("loop always returns within MAX_ATTEMPTS iterations")
    }

    async fn try_send_text(
        &self,
        url: &str,
        payload: &serde_json::Value,
    ) -> Result<(), SendOutcome> {
        let resp = self
            .http
            .post(url)
            .bearer_auth(&self.access_token)
            .json(payload)
            .send()
            .await
            .map_err(|e| SendOutcome::Retryable(anyhow!("whatsapp send request failed: {e}")))?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let text = resp.text().await.unwrap_or_default();
        let err = anyhow!("whatsapp send {status}: {text}");
        if status.as_u16() == 429 || status.is_server_error() {
            Err(SendOutcome::Retryable(err))
        } else {
            Err(SendOutcome::Permanent(err))
        }
    }

    /// Blue-ticks the inbound message so the sender sees the bot is alive.
    /// Best-effort — failures are logged by callers, never fatal.
    pub async fn mark_read(&self, message_id: &str) -> Result<()> {
        let url = self.graph(&format!("{}/messages", self.phone_number_id));
        let payload = json!({
            "messaging_product": "whatsapp",
            "status": "read",
            "message_id": message_id,
        });
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.access_token)
            .json(&payload)
            .send()
            .await
            .context("whatsapp mark_read failed")?;
        if !resp.status().is_success() {
            return Err(anyhow!("whatsapp mark_read {}", resp.status()));
        }
        Ok(())
    }

    /// Resolve a media id to bytes. Two-step: GET /{media_id} returns a
    /// short-lived (~5 min) CDN url, then GET that url with the same token.
    pub async fn download_media(&self, media_id: &str) -> Result<(Vec<u8>, String)> {
        let info: MediaInfo = self
            .http
            .get(self.graph(media_id))
            .bearer_auth(&self.access_token)
            .send()
            .await
            .context("media info request failed")?
            .error_for_status()
            .context("media info request rejected")?
            .json()
            .await
            .context("decoding media info")?;

        let bytes = self
            .http
            .get(&info.url)
            .bearer_auth(&self.access_token)
            .send()
            .await
            .context("media download failed")?
            .error_for_status()
            .context("media download rejected")?
            .bytes()
            .await
            .context("reading media body")?;

        let mime = info
            .mime_type
            .unwrap_or_else(|| "application/octet-stream".into());
        Ok((bytes.to_vec(), mime))
    }
}
