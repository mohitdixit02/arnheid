//! Tier 3 embeddings via Hugging Face's hosted Inference API
//! (`hf-inference` feature-extraction). No local model — set `HF_API_KEY` and
//! an `EMBEDDING_MODEL` whose dimension matches `EMBEDDING_DIM`.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

pub struct Embedder {
    http: reqwest::Client,
    /// Full feature-extraction URL for one model, e.g.
    /// `https://router.huggingface.co/hf-inference/models/BAAI/bge-large-en-v1.5/pipeline/feature-extraction`.
    url: String,
    api_key: String,
    dim: usize,
}

/// HF returns a bare array: flat for a single pooled sentence embedding,
/// nested when the model emits one row per input.
#[derive(Deserialize)]
#[serde(untagged)]
enum EmbedResponse {
    Flat(Vec<f32>),
    Nested(Vec<Vec<f32>>),
}

impl Embedder {
    /// `base_url` is the models root; the model path is appended.
    pub fn new(base_url: String, api_key: String, model: String, dim: usize) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .expect("reqwest client");
        Self {
            http,
            url: format!(
                "{}/{}/pipeline/feature-extraction",
                base_url.trim_end_matches('/'),
                model.trim_matches('/')
            ),
            api_key,
            dim,
        }
    }

    #[allow(dead_code)] // exposed for callers that validate dimensions
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Embed a single string. `truncate` lets the server cut at the model's
    /// token limit instead of rejecting the request; the char cap just keeps
    /// the payload sane.
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let body = json!({ "inputs": truncate_chars(text, 8000), "truncate": true });

        let mut last_err = None;
        // ponytail: 3 tries covers HF cold-start (503 while the model loads).
        for attempt in 0..3u8 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(3 * attempt as u64)).await;
            }
            match self.try_embed(&body).await {
                Ok(v) => return Ok(v),
                Err(e) => {
                    tracing::warn!(attempt, error = %e, "embedding call failed");
                    eprintln!("[warn] embedding call failed: attempt={attempt} error={e}");
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow!("embedding failed")))
    }

    async fn try_embed(&self, body: &serde_json::Value) -> Result<Vec<f32>> {
        let resp = self
            .http
            .post(&self.url)
            .bearer_auth(&self.api_key)
            .json(body)
            .send()
            .await
            .context("embedding request failed")?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!("huggingface {status}: {text}"));
        }

        let vec = match serde_json::from_str(&text).context("decoding embedding response")? {
            EmbedResponse::Flat(v) => v,
            EmbedResponse::Nested(rows) => rows
                .into_iter()
                .next()
                .ok_or_else(|| anyhow!("empty embedding response"))?,
        };

        if vec.len() != self.dim {
            return Err(anyhow!(
                "embedding dim mismatch: got {}, expected {} — check EMBEDDING_MODEL/EMBEDDING_DIM",
                vec.len(),
                self.dim
            ));
        }
        Ok(vec)
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_joins_without_double_slash() {
        let e = Embedder::new(
            "https://router.huggingface.co/hf-inference/models/".into(),
            "k".into(),
            "BAAI/bge-large-en-v1.5".into(),
            1024,
        );
        assert_eq!(
            e.url,
            "https://router.huggingface.co/hf-inference/models/BAAI/bge-large-en-v1.5/pipeline/feature-extraction"
        );
    }

    #[test]
    fn parses_both_response_shapes() {
        let flat: EmbedResponse = serde_json::from_str("[0.1,0.2]").unwrap();
        assert!(matches!(flat, EmbedResponse::Flat(v) if v.len() == 2));
        let nested: EmbedResponse = serde_json::from_str("[[0.1,0.2]]").unwrap();
        assert!(matches!(nested, EmbedResponse::Nested(v) if v[0].len() == 2));
    }
}
