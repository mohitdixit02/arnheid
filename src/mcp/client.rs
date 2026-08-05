//! One MCP server, spoken to over JSON-RPC 2.0 / Streamable HTTP.
//!
//! Two things a hand-rolled MCP client usually gets wrong, both handled here:
//!
//! 1. **SSE framing.** A Streamable HTTP server may answer a POST with
//!    `content-type: text/event-stream`; the JSON-RPC payload is then on the
//!    last `data: ` line, not in the body.
//! 2. **Session binding.** A stateful server returns `Mcp-Session-Id` on
//!    `initialize` and rejects every later call that doesn't echo it back.

use anyhow::{anyhow, Context, Result};
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use super::{sanitize, McpTool, MAX_DESCRIPTION_CHARS, MAX_RESULT_CHARS, MAX_TOOLS};

const PROTOCOL_VERSION: &str = "2025-06-18";
const TIMEOUT: Duration = Duration::from_secs(30);
/// A server that keeps handing back a fresh cursor would otherwise page forever.
const MAX_PAGES: usize = 10;

pub struct HttpServer {
    url: String,
    http: reqwest::Client,
    rpc_id: AtomicU64,
    session: Mutex<Option<String>>,
}

impl HttpServer {
    pub fn new(url: String, token: Option<String>) -> Result<Self> {
        Self::new_with_headers(url, token, None)
    }

    pub fn new_with_headers(
        url: String,
        token: Option<String>,
        extra_headers: Option<std::collections::HashMap<String, String>>,
    ) -> Result<Self> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::ACCEPT,
            "application/json, text/event-stream".parse().unwrap(),
        );
        if let Some(token) = token.filter(|t| !t.is_empty()) {
            let mut value: reqwest::header::HeaderValue = format!("Bearer {token}")
                .parse()
                .context("MCP bearer token is not a valid header value")?;
            value.set_sensitive(true);
            headers.insert(reqwest::header::AUTHORIZATION, value);
        }
        if let Some(extras) = extra_headers {
            for (key, val) in extras {
                let name = reqwest::header::HeaderName::from_bytes(key.as_bytes())
                    .context("invalid header name")?;
                let value = reqwest::header::HeaderValue::from_bytes(val.as_bytes())
                    .context("invalid header value")?;
                headers.insert(name, value);
            }
        }
        Ok(Self {
            url,
            http: reqwest::Client::builder()
                .timeout(TIMEOUT)
                .default_headers(headers)
                .build()
                .context("building MCP http client")?,
            rpc_id: AtomicU64::new(0),
            session: Mutex::new(None),
        })
    }

    async fn rpc(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        let id = self.rpc_id.fetch_add(1, Ordering::Relaxed) + 1;
        let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});

        let mut req = self.http.post(&self.url).json(&body);
        // Lock scope ends before the await: the guard must not cross it.
        let session = self.session.lock().unwrap().clone();
        if let Some(s) = session {
            req = req.header("Mcp-Session-Id", s);
        }
        let resp = req
            .send()
            .await
            .with_context(|| format!("MCP {method} request"))?;

        // A stateful server hands out its session on `initialize` and 400s
        // every later call that omits it.
        if let Some(s) = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            *self.session.lock().unwrap() = Some(s.to_string());
        }
        let sse = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|c| c.starts_with("text/event-stream"));

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow!("MCP {method} HTTP {status}: {}", truncate(&text)));
        }
        decode(&text, sse, method)
    }

    /// Fire-and-forget by spec; a server that 4xxs a notification is still
    /// usable, so failure here is deliberately not an error.
    async fn notify(&self, method: &str) {
        let _ = self
            .http
            .post(&self.url)
            .json(&json!({"jsonrpc": "2.0", "method": method}))
            .send()
            .await;
    }

    /// Handshake, done once at startup by `Registry::connect`.
    // ponytail: the session is never renewed. A server that expires sessions
    // will 400 every call until the bot restarts; add a re-initialize-on-400
    // retry here if that ever shows up in the logs.
    pub async fn initialize(&self) -> Result<()> {
        self.rpc(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "arnheid", "version": env!("CARGO_PKG_VERSION")},
            }),
        )
        .await?;
        self.notify("notifications/initialized").await;
        Ok(())
    }

    /// Tools the server offers, sanitized. Paginates via `nextCursor`.
    pub async fn list_tools(&self) -> Result<Vec<McpTool>> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let params = match &cursor {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let result = self.rpc("tools/list", params).await?;
            for tool in result["tools"].as_array().unwrap_or(&Vec::new()) {
                let Some(name) = tool["name"].as_str().filter(|n| !n.is_empty()) else {
                    continue;
                };
                tools.push(McpTool {
                    name: sanitize(name, 128),
                    description: sanitize(
                        tool["description"].as_str().unwrap_or_default(),
                        MAX_DESCRIPTION_CHARS,
                    ),
                    input_schema: object_schema(&tool["inputSchema"]),
                });
                if tools.len() >= MAX_TOOLS {
                    return Ok(tools);
                }
            }
            let next = result["nextCursor"].as_str().map(str::to_string);
            // Same cursor twice means the server is looping us.
            if next.is_none() || next == cursor {
                break;
            }
            cursor = next;
        }
        Ok(tools)
    }

    pub async fn call_tool(&self, name: &str, arguments: &serde_json::Value) -> Result<String> {
        let result = self
            .rpc("tools/call", json!({"name": name, "arguments": arguments}))
            .await?;
        let text = result["content"]
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|b| b["type"] == "text")
                    .filter_map(|b| b["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        let text = sanitize(&text, MAX_RESULT_CHARS);
        // An error payload reads exactly like a normal answer; returning it
        // silently would hand the model a failure message as if it were data.
        if result["isError"].as_bool().unwrap_or(false) {
            return Err(anyhow!("MCP tool {name} failed: {text}"));
        }
        Ok(text)
    }
}

/// The JSON-RPC result out of a response body, unwrapping SSE framing and
/// surfacing a JSON-RPC-level `error` as an `Err`.
fn decode(body: &str, sse: bool, method: &str) -> Result<serde_json::Value> {
    let payload = if sse {
        body.lines()
            .filter_map(|l| l.strip_prefix("data: "))
            .next_back()
            .ok_or_else(|| anyhow!("empty MCP event stream for {method}"))?
    } else {
        body
    };
    let message: serde_json::Value =
        serde_json::from_str(payload).with_context(|| format!("decoding MCP {method} response"))?;
    if let Some(error) = message.get("error").filter(|e| !e.is_null()) {
        return Err(anyhow!("MCP error from {method}: {error}"));
    }
    message
        .get("result")
        .cloned()
        .ok_or_else(|| anyhow!("MCP response for {method} has no result"))
}

/// Claude rejects a tool whose `input_schema` isn't an object schema, and a
/// server is free to omit or malform it — substitute an empty one rather than
/// dropping an otherwise usable tool.
fn object_schema(raw: &serde_json::Value) -> serde_json::Value {
    if raw.get("type").and_then(|t| t.as_str()) == Some("object") {
        raw.clone()
    } else {
        json!({"type": "object", "properties": {}})
    }
}

fn truncate(s: &str) -> String {
    s.chars().take(300).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_reads_the_last_sse_data_line() {
        let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":1}}\n\n";
        assert_eq!(decode(body, true, "m").unwrap(), json!({"ok": 1}));
    }

    #[test]
    fn decode_surfaces_jsonrpc_errors_and_missing_results() {
        let err = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"nope"}}"#;
        assert!(decode(err, false, "m")
            .unwrap_err()
            .to_string()
            .contains("nope"));
        assert!(decode(r#"{"jsonrpc":"2.0","id":1}"#, false, "m").is_err());
        assert!(decode("not json", false, "m").is_err());
        // A null `error` alongside a result is legal-ish and must not trip us.
        assert_eq!(
            decode(r#"{"error":null,"result":{"ok":1}}"#, false, "m").unwrap(),
            json!({"ok": 1})
        );
    }

    #[test]
    fn object_schema_substitutes_for_junk() {
        let good = json!({"type": "object", "properties": {"q": {"type": "string"}}});
        assert_eq!(object_schema(&good), good);
        assert_eq!(
            object_schema(&json!(null)),
            json!({"type": "object", "properties": {}})
        );
        assert_eq!(
            object_schema(&json!({"type": "string"})),
            json!({"type": "object", "properties": {}})
        );
    }
}
