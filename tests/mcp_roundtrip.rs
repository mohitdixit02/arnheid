//! End-to-end check of the MCP client against a stub server that behaves like
//! the awkward half of the spec: SSE-framed replies, a session id every later
//! call must echo back, paginated `tools/list`, and an `isError` result.
//!
//! Runs with no network and no credentials:
//!
//!   cargo test --test mcp_roundtrip

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{routing::post, Json, Router};
use arnheid::mcp::client::HttpServer;
use serde_json::{json, Value};

const SESSION: &str = "sess-abc123";

/// Stub MCP server. Answers `initialize` over SSE (the framing a hand-rolled
/// client usually gets wrong) and rejects any later call that drops the
/// session id, so a client that ignores the header fails the test loudly.
async fn mcp(headers: HeaderMap, Json(body): Json<Value>) -> Response {
    let method = body["method"].as_str().unwrap_or_default().to_string();
    let id = body["id"].clone();

    if method == "initialize" {
        let payload =
            json!({"jsonrpc": "2.0", "id": id, "result": {"protocolVersion": "2025-06-18"}});
        return (
            StatusCode::OK,
            [
                ("content-type", "text/event-stream"),
                ("mcp-session-id", SESSION),
            ],
            format!("event: message\ndata: {payload}\n\n"),
        )
            .into_response();
    }
    if method.starts_with("notifications/") {
        return StatusCode::ACCEPTED.into_response();
    }
    if headers.get("mcp-session-id").and_then(|v| v.to_str().ok()) != Some(SESSION) {
        return (StatusCode::BAD_REQUEST, "missing session").into_response();
    }

    let result = match method.as_str() {
        "tools/list" => match body["params"]["cursor"].as_str() {
            None => json!({
                "tools": [{
                    "name": "alpha",
                    // Control characters here are the prompt-injection vector
                    // the client must strip at the boundary.
                    "description": "first \x1b[31mtool\x07",
                    "inputSchema": {"type": "object", "properties": {"q": {"type": "string"}}},
                }],
                "nextCursor": "page2",
            }),
            Some("page2") => json!({
                // No inputSchema at all: must be substituted, not dropped.
                "tools": [{"name": "beta", "description": "second tool"}],
            }),
            Some(other) => panic!("unexpected cursor {other}"),
        },
        "tools/call" => match body["params"]["name"].as_str() {
            Some("boom") => json!({
                "isError": true,
                "content": [{"type": "text", "text": "upstream exploded"}],
            }),
            Some(name) => json!({
                "content": [
                    {"type": "text", "text": format!("{name} ran with {}", body["params"]["arguments"])},
                    {"type": "image", "data": "ignored"},
                ],
            }),
            None => panic!("tools/call with no name"),
        },
        "explode" => {
            return Json(json!({
                "jsonrpc": "2.0", "id": id,
                "error": {"code": -32601, "message": "no such method"},
            }))
            .into_response()
        }
        other => panic!("unexpected method {other}"),
    };
    Json(json!({"jsonrpc": "2.0", "id": id, "result": result})).into_response()
}

async fn spawn_stub() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/mcp", post(mcp)))
            .await
            .unwrap();
    });
    format!("http://{addr}/mcp")
}

#[tokio::test]
async fn lists_and_calls_tools_across_sse_pagination_and_sessions() {
    let url = spawn_stub().await;
    let server = HttpServer::new(url, Some("test-token".to_string())).unwrap();

    // Reaching page 2 at all proves the SSE-framed session id was captured
    // from `initialize` and echoed on every later call.
    server.initialize().await.unwrap();
    let tools = server.list_tools().await.unwrap();
    assert_eq!(
        tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        ["alpha", "beta"]
    );
    assert_eq!(
        tools[0].description, "first [31mtool",
        "control characters must be stripped before a description reaches the model"
    );
    assert_eq!(tools[0].input_schema["properties"]["q"]["type"], "string");
    assert_eq!(
        tools[1].input_schema,
        json!({"type": "object", "properties": {}}),
        "a missing inputSchema must be substituted, not passed through"
    );

    // Text blocks are joined; non-text blocks are dropped rather than rendered.
    let out = server
        .call_tool("alpha", &json!({"q": "hello"}))
        .await
        .unwrap();
    assert_eq!(out, r#"alpha ran with {"q":"hello"}"#);

    // isError must surface as an error, not as a result that reads like data.
    let err = server.call_tool("boom", &json!({})).await.unwrap_err();
    assert!(err.to_string().contains("upstream exploded"), "{err}");
}

#[tokio::test]
async fn a_dropped_session_fails_loudly() {
    let url = spawn_stub().await;
    // No `initialize`, so no session id — the stub 400s, and that must be an
    // error rather than a silently empty tool list.
    let server = HttpServer::new(url, None).unwrap();
    let err = server.list_tools().await.unwrap_err();
    assert!(err.to_string().contains("400"), "{err}");
}

/// The whole layer as `main` builds it: env vars → `Config` → `Registry` →
/// a routed tool call. Sets process-wide env, so it must stay the only test
/// in this binary that touches the environment.
#[tokio::test]
async fn registry_wires_config_through_to_a_routed_call() {
    let url = spawn_stub().await;
    // Required by Config::from_env; nothing here opens a database.
    std::env::set_var("TELEGRAM_BOT_TOKEN", "test");
    std::env::set_var("DATABASE_URL", "postgresql://root@localhost:26257/arnheid");
    std::env::set_var("HF_API_KEY", "hf_test");
    std::env::set_var("GOOGLE_CLIENT_ID", "cid");
    std::env::set_var("GOOGLE_CLIENT_SECRET", "csecret");
    std::env::set_var("GOOGLE_REFRESH_TOKEN", "rtoken");
    std::env::set_var("MCP_SERVERS", format!("stub={url}"));
    std::env::set_var("MCP_TOKEN_STUB", "tok");

    let config = arnheid::config::Config::from_env().expect("config");
    let registry = arnheid::mcp::Registry::connect(&config)
        .await
        .expect("registry with gsuite + stub");

    let names: Vec<&str> = registry
        .tool_defs()
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    for expected in [
        "gsuite_gmail_search",
        "gsuite_gmail_read",
        "gsuite_calendar_events",
        "gsuite_drive_search",
        "stub_alpha",
        "stub_beta",
    ] {
        assert!(
            registry.handles(expected),
            "missing {expected} in {names:?}"
        );
    }
    // Every exposed name must be legal as a Claude tool name.
    assert!(names.iter().all(|n| !n.is_empty()
        && n.len() <= 128
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')));

    // Routed to the right backend, with the slug stripped back off.
    let out = registry.call("stub_alpha", &json!({"q": "x"})).await;
    assert_eq!(out, r#"alpha ran with {"q":"x"}"#);

    // A failing tool comes back as text the model can read, never a panic or
    // a collapsed turn.
    let boom = registry.call("stub_boom", &json!({})).await;
    assert!(boom.starts_with("Error: unknown tool"), "{boom}");
}

#[tokio::test]
async fn unreachable_server_errors_rather_than_hanging() {
    // Port 1 on loopback: connection refused, immediately.
    let server = HttpServer::new("http://127.0.0.1:1/mcp".to_string(), None).unwrap();
    assert!(server.initialize().await.is_err());
}
