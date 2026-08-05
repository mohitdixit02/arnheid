//! Universal MCP layer: one registry of third-party tools the agent loop can
//! call, whatever they're backed by.
//!
//! Two kinds of backend, same surface:
//!
//! - **Remote** — any MCP server reachable over Streamable HTTP, configured
//!   with `MCP_SERVERS=slug=url,…` (see `client::HttpServer`).
//! - **Built-in** — GSuite, implemented in-process against Google's REST APIs
//!   so there's no sidecar to run (see `gsuite::GSuite`).
//!
//! The registry flattens every backend's tools into one namespaced list of
//! Claude `ToolDef`s (`gsuite_gmail_search`, `linear_create_issue`, …) and
//! routes a call back to whichever backend owns that name. `query::agent`
//! therefore treats an integration exactly like its own `search_brain`.
//!
//! **Everything a server says is untrusted.** A tool description goes straight
//! into the model's context, so a hostile server's description is a prompt
//! injection vector: descriptions and results are stripped of control
//! characters and truncated here, at the boundary, and no server may claim a
//! name that would shadow one of the agent's own tools.

pub mod client;
pub mod gsuite;
pub mod toolbox;

use crate::config::Config;
use crate::llm::anthropic::ToolDef;
use std::collections::HashMap;
use std::time::Duration;

pub const MAX_DESCRIPTION_CHARS: usize = 2_000;
/// Cap on tools exposed to the model, across all servers — a server offering
/// hundreds would otherwise crowd out the prompt and the turn budget with it.
pub const MAX_TOOLS: usize = 32;
pub const MAX_RESULT_CHARS: usize = 8_000;
/// A slow or hung server must not stall startup.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// Names the built-in tools in `toolbox` define; an MCP server that claims
/// one would silently shadow brain retrieval. Kept in sync by a test there.
pub const RESERVED_TOOL_NAMES: [&str; 12] = [
    "search_brain",
    "graph_lookup",
    "web_search",
    "fetch_url",
    "search_chat_history",
    "core_memory_append",
    "core_memory_replace",
    "archival_memory_insert",
    "archival_memory_search",
    "bash_exec",
    "file_read",
    "file_write",
];

/// C0 controls minus \t \n \r, plus DEL. Terminal escapes and stray carriage
/// returns are how remote text forges structure in an agent's context.
fn sanitize(text: &str, limit: usize) -> String {
    let cleaned: String = text
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\t' | '\n' | '\r'))
        .collect();
    if cleaned.chars().count() > limit {
        format!("{}…", cleaned.chars().take(limit - 1).collect::<String>())
    } else {
        cleaned
    }
}

/// One tool as a backend describes it, before namespacing.
#[derive(Debug, Clone)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

enum Backend {
    Http(client::HttpServer),
    Gsuite(gsuite::GSuite),
}

impl Backend {
    async fn call(&self, tool: &str, args: &serde_json::Value) -> anyhow::Result<String> {
        match self {
            Backend::Http(s) => s.call_tool(tool, args).await,
            Backend::Gsuite(g) => g.call_tool(tool, args).await,
        }
    }
}

pub struct Registry {
    backends: Vec<Backend>,
    /// Exposed tool name → (backend index, the backend's own name for it).
    routes: HashMap<String, (usize, String)>,
    defs: Vec<ToolDef>,
}

impl Registry {
    /// Build the registry from config, listing each server's tools once at
    /// startup. A server that's down or misbehaving is logged and skipped —
    /// one broken integration must not stop the bot from booting. `None` when
    /// nothing is configured or nothing usable came back.
    pub async fn connect(config: &Config) -> Option<Self> {
        let mut registry = Self {
            backends: Vec::new(),
            routes: HashMap::new(),
            defs: Vec::new(),
        };

        if let Some(g) = config.google.as_ref() {
            match gsuite::GSuite::new(
                g.client_id.clone(),
                g.client_secret.clone(),
                g.refresh_token.clone(),
            ) {
                Ok(gs) => {
                    let tools = gs.list_tools();
                    registry.add(&g.slug, Backend::Gsuite(gs), tools);
                }
                Err(e) => {
                    tracing::error!(error = %e, "gsuite integration disabled");
                    eprintln!("[error] gsuite integration disabled: error={e}");
                }
            }
        }

        if let Some(cc) = config.cockroach_cloud.as_ref() {
            match connect_cockroach_cloud(&cc.api_key, &cc.cluster_id).await {
                Ok((backend, tools)) => registry.add(&cc.slug, backend, tools),
                Err(e) => {
                    tracing::error!(error = %e, "Cockroach Cloud MCP integration disabled");
                    eprintln!("[error] Cockroach Cloud MCP integration disabled: error={e}");
                }
            }
        }

        for server in &config.mcp_servers {
            match connect_http(server).await {
                Ok((backend, tools)) => registry.add(&server.slug, backend, tools),
                Err(e) => {
                    tracing::error!(slug = %server.slug, url = %server.url, error = %e,
                        "MCP server unreachable — its tools are unavailable this run");
                    eprintln!(
                        "[error] MCP server unreachable — its tools are unavailable this run: slug={} url={} error={e}",
                        server.slug, server.url
                    );
                }
            }
        }

        if registry.defs.is_empty() {
            return None;
        }
        tracing::info!(
            tools = registry.defs.len(),
            names = %registry.defs.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", "),
            "MCP layer ready"
        );
        println!(
            "[info] MCP layer ready: tools={} names={}",
            registry.defs.len(),
            registry.defs.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", ")
        );
        Some(registry)
    }

    fn add(&mut self, slug: &str, backend: Backend, tools: Vec<McpTool>) {
        let index = self.backends.len();
        self.backends.push(backend);
        let mut added = 0;
        for tool in tools {
            if self.defs.len() >= MAX_TOOLS {
                tracing::warn!(slug, "MCP tool cap reached — remaining tools dropped");
                eprintln!("[warn] MCP tool cap reached — remaining tools dropped: slug={slug}");
                break;
            }
            let Some(name) = exposed_name(slug, &tool.name) else {
                tracing::warn!(slug, tool = %tool.name, "skipping unusable MCP tool name");
                eprintln!("[warn] skipping unusable MCP tool name: slug={slug} tool={}", tool.name);
                continue;
            };
            if self.routes.contains_key(&name) {
                tracing::warn!(slug, tool = %name, "skipping duplicate MCP tool name");
                eprintln!("[warn] skipping duplicate MCP tool name: slug={slug} tool={name}");
                continue;
            }
            self.routes.insert(name.clone(), (index, tool.name.clone()));
            self.defs.push(ToolDef {
                name,
                description: tool.description,
                input_schema: tool.input_schema,
            });
            added += 1;
        }
        tracing::info!(slug, tools = added, "MCP backend registered");
        println!("[info] MCP backend registered: slug={slug} tools={added}");
    }

    pub fn tool_defs(&self) -> &[ToolDef] {
        &self.defs
    }

    pub fn handles(&self, name: &str) -> bool {
        self.routes.contains_key(name)
    }

    /// Call a tool and render the result for the model. A failure comes back
    /// as readable text, not an `Err`: the model can retry with different
    /// arguments or answer without it, which beats collapsing the whole turn.
    pub async fn call(&self, name: &str, args: &serde_json::Value) -> String {
        let Some((index, remote)) = self.routes.get(name) else {
            return format!("Error: unknown tool '{name}'.");
        };
        match self.backends[*index].call(remote, args).await {
            Ok(text) if text.trim().is_empty() => format!("{name} returned no content."),
            Ok(text) => text,
            Err(e) => {
                tracing::warn!(tool = name, error = %e, "MCP tool call failed");
                eprintln!("[warn] MCP tool call failed: tool={name} error={e}");
                format!("Error: {name} failed ({e}).")
            }
        }
    }
}

async fn connect_http(
    server: &crate::config::McpServerConfig,
) -> anyhow::Result<(Backend, Vec<McpTool>)> {
    let client = client::HttpServer::new(server.url.clone(), server.token.clone())?;
    let tools = tokio::time::timeout(CONNECT_TIMEOUT, async {
        client.initialize().await?;
        client.list_tools().await
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out after {CONNECT_TIMEOUT:?}"))??;
    Ok((Backend::Http(client), tools))
}

async fn connect_cockroach_cloud(
    api_key: &str,
    cluster_id: &str,
) -> anyhow::Result<(Backend, Vec<McpTool>)> {
    let mut extra_headers = std::collections::HashMap::new();
    extra_headers.insert("mcp-cluster-id".to_string(), cluster_id.to_string());
    
    let client = client::HttpServer::new_with_headers(
        "https://cockroachlabs.cloud/mcp".to_string(),
        Some(api_key.to_string()),
        Some(extra_headers),
    )?;
    let tools = tokio::time::timeout(CONNECT_TIMEOUT, async {
        client.initialize().await?;
        client.list_tools().await
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out after {CONNECT_TIMEOUT:?}"))??;
    Ok((Backend::Http(client), tools))
}

/// `slug` + the server's tool name, reduced to what Claude accepts for a tool
/// name (`[a-zA-Z0-9_-]{1,128}`). `None` if nothing usable survives, or if the
/// result would shadow one of the agent's own tools.
fn exposed_name(slug: &str, tool: &str) -> Option<String> {
    let keep = |s: &str| -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    };
    // The tool half must survive on its own: a name of only punctuation would
    // otherwise reduce to the bare slug and collide with its siblings.
    if !tool.chars().any(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let name = format!("{}_{}", keep(slug), keep(tool));
    let name: String = name.chars().take(128).collect();
    let trimmed = name.trim_matches('_').to_string();
    if trimmed.is_empty() || RESERVED_TOOL_NAMES.contains(&trimmed.as_str()) {
        return None;
    }
    Some(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposed_names_are_namespaced_and_legal() {
        assert_eq!(
            exposed_name("gsuite", "gmail_search").unwrap(),
            "gsuite_gmail_search"
        );
        // Anything outside Claude's charset becomes an underscore.
        assert_eq!(
            exposed_name("my.server", "list files!").unwrap(),
            "my_server_list_files"
        );
        assert!(exposed_name("gsuite", "x").unwrap().len() <= 128);
        // A blank/punctuation-only tool name would reduce to the bare slug.
        assert!(exposed_name("acme", "  ").is_none());
        assert!(exposed_name("acme", "!!").is_none());
        assert!(exposed_name("_", "  ").is_none());
        let long = exposed_name("s", &"x".repeat(400)).unwrap();
        assert_eq!(long.chars().count(), 128);
    }

    #[test]
    fn a_server_cannot_shadow_the_agents_own_tools() {
        // Only reachable by a server naming itself to collide on purpose.
        assert!(exposed_name("", "search_brain").is_none());
        assert!(exposed_name("_", "web_search").is_none());
        // The namespaced form is fine — it can't be confused for the builtin.
        assert_eq!(
            exposed_name("evil", "search_brain").unwrap(),
            "evil_search_brain"
        );
    }

    #[test]
    fn sanitize_strips_control_chars_and_truncates() {
        assert_eq!(sanitize("a\x1b[31mred\x00", 100), "a[31mred");
        assert_eq!(sanitize("keep\ttabs\nand\r\n", 100), "keep\ttabs\nand\r\n");
        assert_eq!(sanitize("abcdef", 4), "abc…");
        assert_eq!(sanitize("abcd", 4), "abcd");
    }

    #[test]
    fn duplicate_and_illegal_tool_names_are_dropped() {
        let mut r = Registry {
            backends: Vec::new(),
            routes: HashMap::new(),
            defs: Vec::new(),
        };
        let tool = |name: &str| McpTool {
            name: name.to_string(),
            description: String::new(),
            input_schema: serde_json::json!({"type": "object"}),
        };
        let gs = gsuite::GSuite::new("a".into(), "b".into(), "c".into()).unwrap();
        r.add(
            "acme",
            Backend::Gsuite(gs),
            vec![tool("go"), tool("go"), tool("  ")],
        );
        assert_eq!(r.tool_defs().len(), 1);
        assert!(r.handles("acme_go"));
        assert!(!r.handles("go"));
    }
}
