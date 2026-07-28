//! Claude (Anthropic Messages API) low-level client.
//! Prompt construction lives in `llm::chat`; this is just transport.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";

pub struct Anthropic {
    http: reqwest::Client,
    api_key: String,
    pub haiku_model: String,
    pub sonnet_model: String,
    /// Message-intent routing (capture vs. agent) — separate from
    /// `haiku_model` so it can be pointed at a cheaper/faster model without
    /// touching tier 2's summarize/excerpt quality.
    pub router_model: String,
}

#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(default)]
    text: String,
}

/// A tool definition for the Messages API's `tools` field. See
/// https://platform.claude.com/docs/en/agents-and-tools/tool-use/define-tools
/// — `input_schema` is a JSON Schema object (`{"type": "object", "properties": {...}, "required": [...]}`).
#[derive(Debug, Clone, Serialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// One block of a tool-use turn: either text Claude said, or a tool it wants
/// called. Distinct from the plain-completion `ContentBlock` above — this
/// path only ever runs with `tool_choice: {"type": "auto"}` and no extended
/// thinking, so these two variants are exhaustive for it (unlike the plain
/// path, which tolerates unknown block types by design).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

/// The result of one tool-enabled Messages call.
pub struct ToolTurn {
    pub content: Vec<ToolContentBlock>,
    pub stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct ToolMessagesResponse {
    content: Vec<ToolContentBlock>,
    #[serde(default)]
    stop_reason: Option<String>,
}

impl Anthropic {
    pub fn new(
        api_key: String,
        haiku_model: String,
        sonnet_model: String,
        router_model: String,
    ) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .expect("reqwest client");
        Self {
            http,
            api_key,
            haiku_model,
            sonnet_model,
            router_model,
        }
    }

    /// One Messages call with a single retry (2s backoff) on transient failure.
    pub async fn complete(
        &self,
        model: &str,
        system: Option<&str>,
        user: &str,
        max_tokens: u32,
    ) -> Result<String> {
        let mut body = json!({
            "model": model,
            "max_tokens": max_tokens,
            "messages": [{ "role": "user", "content": user }],
        });
        if let Some(s) = system {
            body["system"] = json!(s);
        }
        self.call(&body).await
    }

    /// Describe an image (base64 content block + instruction), on Haiku.
    pub async fn describe_image(&self, image: &[u8], media_type: &str, prompt: &str) -> Result<String> {
        use base64::{engine::general_purpose::STANDARD as B64, Engine};
        let body = json!({
            "model": &self.haiku_model,
            "max_tokens": 512,
            "messages": [{
                "role": "user",
                "content": [
                    {
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": media_type,
                            "data": B64.encode(image),
                        }
                    },
                    { "type": "text", "text": prompt }
                ]
            }],
        });
        self.call(&body).await
    }

    /// One Messages call with tool definitions in play, `tool_choice: auto`
    /// with parallel tool use disabled (one tool call per turn keeps the
    /// caller's loop — see `query::agent` — simple to reason about). An
    /// empty `tools` slice omits both `tools` and `tool_choice` entirely
    /// (rather than sending an empty array with a tool_choice), for a
    /// caller that wants to force a final text-only answer.
    /// `messages` are raw already-shaped `{"role": ..., "content": ...}`
    /// values; the caller owns the growing multi-turn conversation, this is
    /// transport only, same division of labor as `complete`.
    pub async fn complete_with_tools(
        &self,
        model: &str,
        system: &str,
        messages: &[serde_json::Value],
        tools: &[ToolDef],
        max_tokens: u32,
    ) -> Result<ToolTurn> {
        let mut body = json!({
            "model": model,
            "max_tokens": max_tokens,
            "system": system,
            "messages": messages,
        });
        if !tools.is_empty() {
            body["tools"] = json!(tools);
            body["tool_choice"] = json!({ "type": "auto", "disable_parallel_tool_use": true });
        }

        let mut last_err = None;
        for attempt in 0..2u8 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            match self.try_call_tools(&body).await {
                Ok(turn) => return Ok(turn),
                Err(e) => {
                    tracing::warn!(attempt, error = %e, "anthropic tool call failed");
                    eprintln!("[warn] anthropic tool call failed: attempt={attempt} error={e}");
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow!("anthropic tool call failed")))
    }

    async fn try_call_tools(&self, body: &serde_json::Value) -> Result<ToolTurn> {
        let resp = self
            .http
            .post(API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .json(body)
            .send()
            .await
            .context("anthropic tool request failed")?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!("anthropic {status}: {text}"));
        }

        let parsed: ToolMessagesResponse =
            serde_json::from_str(&text).context("decoding anthropic tool response")?;
        Ok(ToolTurn {
            content: parsed.content,
            stop_reason: parsed.stop_reason,
        })
    }

    async fn call(&self, body: &serde_json::Value) -> Result<String> {
        let mut last_err = None;
        for attempt in 0..2u8 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            match self.try_call(&body).await {
                Ok(text) => return Ok(text),
                Err(e) => {
                    tracing::warn!(attempt, error = %e, "anthropic call failed");
                    eprintln!("[warn] anthropic call failed: attempt={attempt} error={e}");
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow!("anthropic call failed")))
    }

    async fn try_call(&self, body: &serde_json::Value) -> Result<String> {
        let resp = self
            .http
            .post(API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .json(body)
            .send()
            .await
            .context("anthropic request failed")?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!("anthropic {status}: {text}"));
        }

        let parsed: MessagesResponse =
            serde_json::from_str(&text).context("decoding anthropic response")?;
        Ok(parsed
            .content
            .into_iter()
            .map(|c| c.text)
            .collect::<Vec<_>>()
            .join("")
            .trim()
            .to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shape taken verbatim from Anthropic's documented tool-use example
    /// (a `tool_use` block naming a tool and its arguments) — see
    /// https://platform.claude.com/docs/en/agents-and-tools/tool-use/overview.
    /// This is the only ground truth available in this environment (no live
    /// API key to integration-test against), so pinning the exact contract
    /// here is what stands in for that.
    #[test]
    fn deserializes_documented_tool_use_response() {
        let raw = r#"{
            "id": "msg_01",
            "type": "message",
            "role": "assistant",
            "content": [
                {
                    "type": "tool_use",
                    "id": "toolu_01A09q90qw90lq917835lq9",
                    "name": "get_weather",
                    "input": {"location": "San Francisco, CA"}
                }
            ],
            "stop_reason": "tool_use"
        }"#;
        let parsed: ToolMessagesResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(parsed.content.len(), 1);
        match &parsed.content[0] {
            ToolContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "toolu_01A09q90qw90lq917835lq9");
                assert_eq!(name, "get_weather");
                assert_eq!(input["location"], "San Francisco, CA");
            }
            ToolContentBlock::Text { .. } => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn deserializes_text_only_response() {
        let raw = r#"{
            "content": [{"type": "text", "text": "The weather is nice."}],
            "stop_reason": "end_turn"
        }"#;
        let parsed: ToolMessagesResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.stop_reason.as_deref(), Some("end_turn"));
        match &parsed.content[0] {
            ToolContentBlock::Text { text } => assert_eq!(text, "The weather is nice."),
            ToolContentBlock::ToolUse { .. } => panic!("expected Text"),
        }
    }

    #[test]
    fn deserializes_response_with_preamble_text_then_tool_use() {
        // Claude sometimes narrates before calling a tool.
        let raw = r#"{
            "content": [
                {"type": "text", "text": "Let me check that."},
                {"type": "tool_use", "id": "toolu_1", "name": "search_brain", "input": {"query": "x"}}
            ],
            "stop_reason": "tool_use"
        }"#;
        let parsed: ToolMessagesResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.content.len(), 2);
    }

    /// `ToolContentBlock` must serialize back to the exact shape the API
    /// expects when round-tripped as the `assistant` turn's content in the
    /// next request (see the documented round trip's `messages.push`).
    #[test]
    fn tool_use_block_serializes_to_documented_shape() {
        let block = ToolContentBlock::ToolUse {
            id: "toolu_1".to_string(),
            name: "get_weather".to_string(),
            input: serde_json::json!({"location": "SF"}),
        };
        let v = serde_json::to_value(&block).unwrap();
        assert_eq!(v["type"], "tool_use");
        assert_eq!(v["id"], "toolu_1");
        assert_eq!(v["name"], "get_weather");
        assert_eq!(v["input"]["location"], "SF");
    }

    #[test]
    fn tool_def_serializes_to_documented_shape() {
        let def = ToolDef {
            name: "get_weather".to_string(),
            description: "Get the current weather for a given location.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "location": {"type": "string", "description": "City and state"}
                },
                "required": ["location"]
            }),
        };
        let v = serde_json::to_value(&def).unwrap();
        assert_eq!(v["name"], "get_weather");
        assert_eq!(v["input_schema"]["type"], "object");
        assert_eq!(v["input_schema"]["required"][0], "location");
    }
}
