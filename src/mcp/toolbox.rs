//! The single tool surface the agent sees.
//!
//! Everything callable lives here: the built-in tools backed by this process
//! (`search_brain`, `graph_lookup`, `web_search`) and every MCP integration
//! from [`super::Registry`] (GSuite, remote servers). The agent asks for one
//! catalogue and dispatches through one `call` — it has no idea which side of
//! that line a tool came from.
//!
//! The built-ins can't live in `Registry` itself: they need per-query context
//! (who is asking, which scope) and they own citation numbering, while the
//! registry is built once at startup and shared. So the toolbox is created
//! per question and wraps the registry.

use crate::db;
use crate::llm::anthropic::ToolDef;
use crate::models::RetrievedItem;
use crate::query::handler::{rrf_fuse, QueryScope};
use crate::search::WebSnippet;
use crate::state::AppState;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Passages considered per search call.
const CHUNK_K: i64 = 12;
/// Vector floor for `search_brain` — lenient by design: precision comes from
/// the keyword half of hybrid search and the model's judgment, not this floor.
const MIN_SIMILARITY: f32 = 0.25;
/// Distinct brain items the accumulated context is capped at, so an
/// aggressive searcher can't grow the prompt without bound.
const MAX_SOURCES: usize = 8;
/// Chars of a web snippet shown to the model.
const WEB_SNIPPET_CHARS: usize = 400;
/// Facts considered per `archival_memory_search` call.
const ARCHIVAL_K: i64 = 8;
/// Chars kept from a `bash_exec`/`file_read` result — resent every turn, so
/// unbounded output would blow the transcript (and the bill) fast.
const MAX_TOOL_OUTPUT_CHARS: usize = 6_000;
/// A hung command must not stall the whole turn budget forever.
const BASH_TIMEOUT: Duration = Duration::from_secs(120);

/// Who is asking, and over which corpus.
#[derive(Debug, Clone, Copy)]
pub struct ToolCtx {
    pub chat_id: i64,
    pub user_id: i64,
    pub scope: QueryScope,
}

/// Sources accumulated across the whole loop for one question. Numbered the
/// moment each is first found; the number never changes once assigned, so
/// citations stay stable across tool calls within a turn.
#[derive(Default)]
pub struct Sources {
    pub items: Vec<RetrievedItem>,
    pub web: Vec<WebSnippet>,
}

impl Sources {
    pub fn new() -> Self {
        Self::default()
    }

    /// Absorb passage hits (deduped by item, one passage per item per call)
    /// and describe them for the model using the same `[n]` numbering the
    /// final answer must cite.
    fn absorb_items(&mut self, hits: Vec<db::chunks::ChunkHit>) -> String {
        if hits.is_empty() {
            return "No matching passages found.".to_string();
        }
        let mut out = String::new();
        let mut seen_this_call = std::collections::HashSet::new();
        for hit in hits {
            if !seen_this_call.insert(hit.item_id) {
                continue;
            }
            let Some(idx) = self.index_of_or_insert(&hit) else {
                continue; // at MAX_SOURCES cap; skip further growth
            };
            let n = idx + 1;
            let item = &self.items[idx];
            let label = item.title.clone().unwrap_or_else(|| item.url.clone());
            out.push_str(&format!(
                "[{n}] {label} ({url})\n   \"…{content}…\"\n\n",
                url = item.url,
                content = hit.content.trim(),
            ));
        }
        if out.is_empty() {
            "No new matching passages (already surfaced above).".to_string()
        } else {
            out
        }
    }

    /// Existing index for this item, or a freshly-inserted one — `None` only
    /// once `MAX_SOURCES` distinct items have already been found.
    fn index_of_or_insert(&mut self, hit: &db::chunks::ChunkHit) -> Option<usize> {
        if let Some(pos) = self.items.iter().position(|i| i.id == hit.item_id) {
            return Some(pos);
        }
        if self.items.len() >= MAX_SOURCES {
            return None;
        }
        self.items.push(chunk_hit_to_item(hit));
        Some(self.items.len() - 1)
    }

    fn absorb_web(&mut self, snippets: Vec<WebSnippet>) -> String {
        if snippets.is_empty() {
            return "No web results found.".to_string();
        }
        let mut out = String::new();
        for s in snippets {
            if self.web.iter().any(|w| w.url == s.url) {
                continue;
            }
            self.web.push(s);
            let n = self.web.len();
            let s = &self.web[n - 1];
            out.push_str(&format!(
                "[W{n}] {title} ({url})\n   \"…{content}…\"\n\n",
                title = s.title,
                url = s.url,
                content = truncate(&s.content, WEB_SNIPPET_CHARS),
            ));
        }
        if out.is_empty() {
            "No new web results (already surfaced above).".to_string()
        } else {
            out
        }
    }
}

pub struct ToolBox<'a> {
    state: &'a AppState,
    ctx: ToolCtx,
    defs: Vec<ToolDef>,
    pub sources: Sources,
    /// Set once an integration call succeeds. Integration answers cite
    /// nothing, so the caller can't tell "answered" from "found nothing"
    /// by looking at the source list alone.
    integration_used: bool,
}

impl<'a> ToolBox<'a> {
    /// `web_enabled` is the caller's policy (a channel may forbid web access
    /// outright); a configured search command is the capability.
    pub fn new(state: &'a AppState, ctx: ToolCtx, web_enabled: bool) -> Self {
        let mut defs = builtin_defs(
            web_enabled && state.web_search.is_some(),
            state.config.agent_shell_tools_enabled,
        );
        if let Some(mcp) = state.mcp.as_deref() {
            defs.extend(mcp.tool_defs().iter().cloned());
        }
        Self {
            state,
            ctx,
            defs,
            sources: Sources::new(),
            integration_used: false,
        }
    }

    pub fn defs(&self) -> &[ToolDef] {
        &self.defs
    }

    pub fn integration_used(&self) -> bool {
        self.integration_used
    }

    /// The tool list as the system prompt renders it: one block per tool with
    /// its argument schema, so the model can fill `arguments` correctly
    /// without native tool-calling support from the provider.
    pub fn catalogue(&self) -> String {
        render_catalogue(&self.defs)
    }

    /// Run one tool call. Never fails: a tool error comes back as text for the
    /// model to read and react to, exactly like a result.
    pub async fn call(&mut self, name: &str, args: &serde_json::Value) -> String {
        match name {
            "search_brain" => match string_arg(args, "query") {
                Some(q) => self.search_brain(q).await,
                None => "Error: 'query' is required and must be non-empty.".to_string(),
            },
            "graph_lookup" => match string_arg(args, "topic") {
                Some(t) => self.graph_lookup(t).await,
                None => "Error: 'topic' is required and must be non-empty.".to_string(),
            },
            "web_search" => match string_arg(args, "query") {
                Some(q) => self.web_search(q).await,
                None => "Error: 'query' is required and must be non-empty.".to_string(),
            },
            "fetch_url" => match string_arg(args, "url") {
                Some(u) => self.fetch_url(u).await,
                None => "Error: 'url' is required and must be non-empty.".to_string(),
            },
            "search_chat_history" => match string_arg(args, "query") {
                Some(q) => self.search_chat_history(q).await,
                None => "Error: 'query' is required and must be non-empty.".to_string(),
            },
            "core_memory_append" => match (string_arg(args, "label"), string_arg(args, "content"))
            {
                (Some(l), Some(c)) => self.core_memory_append(l, c).await,
                _ => "Error: 'label' and 'content' are required and must be non-empty.".to_string(),
            },
            "core_memory_replace" => match (
                string_arg(args, "label"),
                string_arg(args, "old_content"),
                string_arg(args, "new_content"),
            ) {
                (Some(l), Some(o), Some(n)) => self.core_memory_replace(l, o, n).await,
                _ => "Error: 'label', 'old_content', and 'new_content' are required."
                    .to_string(),
            },
            "archival_memory_insert" => match string_arg(args, "content") {
                Some(c) => self.archival_memory_insert(c).await,
                None => "Error: 'content' is required and must be non-empty.".to_string(),
            },
            "archival_memory_search" => match string_arg(args, "query") {
                Some(q) => self.archival_memory_search(q).await,
                None => "Error: 'query' is required and must be non-empty.".to_string(),
            },
            "bash_exec" if self.state.config.agent_shell_tools_enabled => {
                match string_arg(args, "command") {
                    Some(c) => self.bash_exec(c).await,
                    None => "Error: 'command' is required and must be non-empty.".to_string(),
                }
            }
            "file_read" if self.state.config.agent_shell_tools_enabled => {
                match string_arg(args, "path") {
                    Some(p) => self.file_read(p).await,
                    None => "Error: 'path' is required and must be non-empty.".to_string(),
                }
            }
            "file_write" if self.state.config.agent_shell_tools_enabled => {
                match (string_arg(args, "path"), args.get("content").and_then(|v| v.as_str())) {
                    (Some(p), Some(c)) => self.file_write(p, c).await,
                    _ => "Error: 'path' and 'content' are required.".to_string(),
                }
            }
            // Anything else is an integration. Its output is context, not a
            // numbered source: nothing here touches `sources`, so citations
            // stay owned by the brain and web tools.
            other => match self.state.mcp.as_ref().filter(|m| m.handles(other)) {
                Some(mcp) => {
                    let out = mcp.call(other, args).await;
                    if !out.starts_with("Error:") {
                        self.integration_used = true;
                    }
                    out
                }
                None => format!("Error: unknown tool '{other}'."),
            },
        }
    }

    async fn search_brain(&mut self, query: &str) -> String {
        let qvec = match self.state.embedder.embed(query).await {
            Ok(v) => v,
            Err(e) => return format!("Error: search failed ({e})."),
        };
        let pool = &self.state.pool;
        let mut lists = Vec::new();
        match self.ctx.scope {
            QueryScope::Personal => {
                let uid = self.ctx.user_id;
                if let Ok(h) =
                    db::chunks::search_by_owner(pool, uid, &qvec, CHUNK_K, MIN_SIMILARITY).await
                {
                    lists.push(h);
                }
                if let Ok(h) = db::chunks::keyword_search_by_owner(pool, uid, query, CHUNK_K).await {
                    lists.push(h);
                }
            }
            QueryScope::Group(group_id) => {
                if let Ok(h) =
                    db::chunks::search(pool, group_id, &qvec, CHUNK_K, MIN_SIMILARITY).await
                {
                    lists.push(h);
                }
                if let Ok(h) = db::chunks::keyword_search(pool, group_id, query, CHUNK_K).await {
                    lists.push(h);
                }
            }
        }
        self.sources.absorb_items(rrf_fuse(lists, CHUNK_K as usize))
    }

    async fn graph_lookup(&mut self, topic: &str) -> String {
        // The knowledge graph is per-group, not per-owner — Personal scope
        // falls back to the current chat's graph, same as the fixed pipeline.
        let group_id = match self.ctx.scope {
            QueryScope::Group(g) => g,
            QueryScope::Personal => self.ctx.chat_id,
        };
        let pool = &self.state.pool;
        let seeds = db::graph::entities_in_query(pool, group_id, topic, 8)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(group_id, error = %e, "entities_in_query failed");
                eprintln!("[warn] entities_in_query failed: group_id={group_id} error={e}");
                Vec::new()
            });
        if seeds.is_empty() {
            return format!("No known entities matching '{topic}' in the knowledge graph.");
        }
        let mut entities = seeds.clone();
        match db::graph::related_entities(pool, group_id, &seeds, 12).await {
            Ok(related) => entities.extend(related),
            Err(e) => {
                tracing::warn!(group_id, error = %e, "related_entities failed");
                eprintln!("[warn] related_entities failed: group_id={group_id} error={e}");
            }
        }
        let item_ids = db::graph::items_mentioning(pool, group_id, &entities, 12)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(group_id, error = %e, "items_mentioning failed");
                eprintln!("[warn] items_mentioning failed: group_id={group_id} error={e}");
                Vec::new()
            });
        if item_ids.is_empty() {
            return format!("No captures found connected to '{topic}'.");
        }
        let qvec = match self.state.embedder.embed(topic).await {
            Ok(v) => v,
            Err(e) => return format!("Error: graph lookup failed ({e})."),
        };
        let hits = db::chunks::search_within_items(pool, group_id, &qvec, &item_ids, CHUNK_K)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(group_id, error = %e, "search_within_items failed");
                eprintln!("[warn] search_within_items failed: group_id={group_id} error={e}");
                Vec::new()
            });
        self.sources.absorb_items(hits)
    }

    async fn web_search(&mut self, query: &str) -> String {
        let Some(search) = self.state.web_search.as_ref() else {
            return "Error: web search is not configured.".to_string();
        };
        let snippets = search.search(query).await.unwrap_or_else(|e| {
            tracing::warn!(error = %e, "agent tool web_search failed");
            eprintln!("[warn] agent tool web_search failed: error={e}");
            Vec::new()
        });
        self.sources.absorb_web(snippets)
    }

    async fn fetch_url(&mut self, url: &str) -> String {
        match crate::ingestion::fetcher::fetch(url).await {
            Ok(content) => {
                if content.available {
                    let max_chars = 12000;
                    let text = if content.text.chars().count() > max_chars {
                        let truncated: String = content.text.chars().take(max_chars).collect();
                        format!("{}\n\n[Content truncated due to length limits]", truncated)
                    } else {
                        content.text
                    };

                    let title = content.title.clone().unwrap_or_else(|| url.to_string());
                    let citation_marker = self.sources.absorb_web(vec![crate::search::WebSnippet {
                        title: title.clone(),
                        url: url.to_string(),
                        content: if text.chars().count() > 100 {
                            let snip: String = text.chars().take(100).collect();
                            format!("{}…", snip)
                        } else {
                            text.clone()
                        },
                    }]);

                    format!(
                        "Webpage fetched successfully.\n\
                         Title: {}\n\
                         URL: {}\n\
                         Citation Marker: {}\n\n\
                         Full Content:\n{}",
                        title, url, citation_marker.trim(), text
                    )
                } else {
                    format!("Error: the page content could not be extracted (possibly JavaScript-only or paywalled).")
                }
            }
            Err(e) => format!("Error fetching URL: {}", e),
        }
    }

    async fn search_chat_history(&mut self, query: &str) -> String {
        let pool = &self.state.pool;
        let group_id = self.ctx.chat_id;
        
        match db::groups::search_buffer(pool, group_id, query, 15).await {
            Ok(hits) => {
                if hits.is_empty() {
                    return format!("No matching messages found in past chat logs for query: '{}'", query);
                }
                let mut out = String::from("Matching past chat logs:\n");
                for h in hits {
                    let sender = h.username.as_deref().unwrap_or("User");
                    let time = h.created_at.format("%Y-%m-%d %H:%M UTC");
                    out.push_str(&format!("[{time}] {sender}: {text}\n", text = h.text));
                }
                out
            }
            Err(e) => format!("Error searching chat history: {e}"),
        }
    }



    async fn core_memory_append(&mut self, label: &str, content: &str) -> String {
        render_edit(
            db::agent_memory::append_block(&self.state.pool, self.ctx.user_id, label, content)
                .await,
            self.ctx.user_id,
            label,
        )
    }

    async fn core_memory_replace(&mut self, label: &str, old: &str, new: &str) -> String {
        render_edit(
            db::agent_memory::replace_block(&self.state.pool, self.ctx.user_id, label, old, new)
                .await,
            self.ctx.user_id,
            label,
        )
    }

    async fn archival_memory_insert(&mut self, content: &str) -> String {
        let embedding = match self.state.embedder.embed(content).await {
            Ok(v) => v,
            Err(e) => return format!("Error: embedding failed ({e})."),
        };
        match db::agent_memory::insert_archival(
            &self.state.pool,
            self.ctx.user_id,
            content,
            &embedding,
        )
        .await
        {
            Ok(id) => {
                tracing::debug!(user_id = self.ctx.user_id, archival_id = %id, "archival memory stored");
                println!("[debug] archival memory stored: user_id={} archival_id={id}", self.ctx.user_id);
                "Stored.".to_string()
            }
            Err(e) => {
                tracing::warn!(user_id = self.ctx.user_id, error = %e, "archival insert failed");
                eprintln!("[warn] archival insert failed: user_id={} error={e}", self.ctx.user_id);
                format!("Error: archival insert failed ({e}).")
            }
        }
    }

    async fn archival_memory_search(&mut self, query: &str) -> String {
        let embedding = match self.state.embedder.embed(query).await {
            Ok(v) => v,
            Err(e) => return format!("Error: embedding failed ({e})."),
        };
        match db::agent_memory::search_archival(
            &self.state.pool,
            self.ctx.user_id,
            &embedding,
            ARCHIVAL_K,
        )
        .await
        {
            Ok(hits) if hits.is_empty() => "No archival facts match that.".to_string(),
            Ok(hits) => hits
                .iter()
                .enumerate()
                .map(|(i, h)| format!("{}. ({}) {}", i + 1, h.created_at.format("%Y-%m-%d"), h.content))
                .collect::<Vec<_>>()
                .join("\n"),
            Err(e) => format!("Error: archival search failed ({e})."),
        }
    }

    /// This user's private jail directory for `bash_exec`/`file_read`/
    /// `file_write` — never a path outside it.
    fn workspace_dir(&self) -> PathBuf {
        Path::new(&self.state.config.agent_workspace_dir).join(self.ctx.user_id.to_string())
    }

    /// Resolve a model-given relative path inside this user's workspace.
    /// `..` components and absolute paths are refused outright rather than
    /// "resolved and checked" — the target may not exist on disk yet, so
    /// there's nothing to canonicalize against. This defends against a
    /// wayward path string, not a symlink-level filesystem attacker; that's
    /// a different threat model this tool doesn't try to cover.
    fn resolve_in_workspace(&self, rel: &str) -> Result<PathBuf, String> {
        let rel = rel.trim();
        if rel.is_empty() {
            return Err("path must not be empty".to_string());
        }
        if Path::new(rel).is_absolute() || rel.split('/').any(|part| part == "..") {
            return Err(
                "path must be relative and stay inside the workspace (no absolute paths, no '..')"
                    .to_string(),
            );
        }
        let root = self.workspace_dir();
        std::fs::create_dir_all(&root).map_err(|e| format!("workspace unavailable: {e}"))?;
        let target = root.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("could not create parent dir: {e}"))?;
        }
        Ok(target)
    }

    async fn file_read(&mut self, path: &str) -> String {
        let target = match self.resolve_in_workspace(path) {
            Ok(p) => p,
            Err(e) => return format!("Error: {e}"),
        };
        match tokio::fs::read_to_string(&target).await {
            Ok(content) => truncate(&content, MAX_TOOL_OUTPUT_CHARS),
            Err(e) => format!("Error: could not read '{path}' ({e})."),
        }
    }

    async fn file_write(&mut self, path: &str, content: &str) -> String {
        let target = match self.resolve_in_workspace(path) {
            Ok(p) => p,
            Err(e) => return format!("Error: {e}"),
        };
        match tokio::fs::write(&target, content).await {
            Ok(()) => format!("Wrote {} bytes to {path}.", content.len()),
            Err(e) => format!("Error: could not write '{path}' ({e})."),
        }
    }

    /// Runs with cwd scoped to this user's workspace, otherwise the full
    /// shell — clone/build/`gh pr create`/whatever the task needs.
    /// GH_TOKEN/GITHUB_TOKEN, if set in this process's own environment,
    /// are inherited by the child automatically; nothing extra to wire.
    async fn bash_exec(&mut self, command: &str) -> String {
        let cwd = self.workspace_dir();
        if let Err(e) = std::fs::create_dir_all(&cwd) {
            return format!("Error: workspace unavailable ({e}).");
        }
        let run = tokio::process::Command::new("bash")
            .arg("-lc")
            .arg(command)
            .current_dir(&cwd)
            .kill_on_drop(true)
            .output();
        match tokio::time::timeout(BASH_TIMEOUT, run).await {
            Err(_) => format!("Error: command timed out after {}s.", BASH_TIMEOUT.as_secs()),
            Ok(Err(e)) => format!("Error: failed to run command ({e})."),
            Ok(Ok(out)) => {
                let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
                if !out.stderr.is_empty() {
                    text.push_str("\n[stderr]\n");
                    text.push_str(&String::from_utf8_lossy(&out.stderr));
                }
                if !out.status.success() {
                    text.push_str(&format!("\n[exit code {}]", out.status.code().unwrap_or(-1)));
                }
                truncate(&text, MAX_TOOL_OUTPUT_CHARS)
            }
        }
    }
}

/// Shared rendering for the two core-memory edit tools: their DB layer
/// already distinguishes a real failure from a plain rejection (over
/// budget, no match) — both become tool-result text either way.
fn render_edit(
    outcome: anyhow::Result<db::agent_memory::EditOutcome>,
    user_id: i64,
    label: &str,
) -> String {
    match outcome {
        Ok(db::agent_memory::EditOutcome::Applied(v)) => {
            tracing::debug!(user_id, label, "core memory block updated");
            println!("[debug] core memory block updated: user_id={user_id} label={label}");
            format!("Updated. '{label}' is now:\n{v}")
        }
        Ok(db::agent_memory::EditOutcome::Rejected(msg)) => {
            tracing::debug!(user_id, label, reason = %msg, "core memory edit rejected");
            println!("[debug] core memory edit rejected: user_id={user_id} label={label} reason={msg}");
            format!("Error: {msg}")
        }
        Err(e) => {
            tracing::warn!(user_id, label, error = %e, "core memory update failed");
            eprintln!("[warn] core memory update failed: user_id={user_id} label={label} error={e}");
            format!("Error: memory update failed ({e}).")
        }
    }
}

fn render_catalogue(defs: &[ToolDef]) -> String {
    defs.iter()
        .map(|d| {
            format!(
                "- {name}\n  purpose: {desc}\n  arguments: {schema}",
                name = d.name,
                desc = d.description.trim(),
                schema = d.input_schema,
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Tools this process backs itself. `web_search` only exists when a search
/// command is configured — a tool the model can't successfully call is worse
/// than no tool at all. `shell_enabled` gates bash_exec/file_read/file_write
/// — off by default (`AGENT_SHELL_TOOLS_ENABLED`), since those run with no
/// confirmation step.
fn builtin_defs(web_enabled: bool, shell_enabled: bool) -> Vec<ToolDef> {
    let mut tools = vec![
        ToolDef {
            name: "search_brain".to_string(),
            description: "Hybrid semantic + keyword search over what THIS user has saved \
                (links, notes, voice memos, photos they captured). The only way to see \
                their saved material. Returns numbered passages to cite as [n]. Call again \
                with different phrasing if the first search comes up short."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "What to search for"}
                },
                "required": ["query"]
            }),
        },
        ToolDef {
            name: "graph_lookup".to_string(),
            description: "Knowledge-graph lookup: captures connected to a named person, \
                company, or topic. Use for cross-referential questions ('what have we saved \
                that relates to X') that a plain search would miss. Returns passages to cite as [n]."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "topic": {"type": "string", "description": "A person, company, or topic name"}
                },
                "required": ["topic"]
            }),
        },
        ToolDef {
            name: "fetch_url".to_string(),
            description: "Scrape and read the clean text content of a specific webpage URL. Use this to read the details of links, repositories, or documentation."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "The exact URL to scrape and read"}
                },
                "required": ["url"]
            }),
        },
        ToolDef {
            name: "search_chat_history".to_string(),
            description: "Search past chat logs and conversations in this specific group or DM chat. Use this to retrieve details, names, emails, or timelines mentioned earlier in the conversation that are no longer visible in the immediate context window."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "The keyword or topic to search for in past messages"}
                },
                "required": ["query"]
            }),
        },
    ];
    if web_enabled {
        tools.push(ToolDef {
            name: "web_search".to_string(),
            description: "Live web search. Use ONLY for current events, prices, releases, or \
                public facts that the user's saved material would not contain — never for \
                chit-chat, opinions, or questions about their own captures. Cite results as [Wn]."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "What to search the web for"}
                },
                "required": ["query"]
            }),
        });
    }

    tools.push(ToolDef {
        name: "core_memory_append".to_string(),
        description: "Append a note to one of your own core memory blocks (\"persona\", \
            \"human\", or any label you choose) — this is how you remember things about \
            yourself and this user across every future conversation, not just this one. Use \
            it the moment you learn something worth keeping (their name, role, a preference, \
            an ongoing project), not just at the end of a conversation. Fails if the block is \
            already full; use core_memory_replace to edit it down first."
            .to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "label": {"type": "string", "description": "Which block, e.g. \"human\" or \"persona\" (or a new label you choose)"},
                "content": {"type": "string", "description": "The note to append"}
            },
            "required": ["label", "content"]
        }),
    });
    tools.push(ToolDef {
        name: "core_memory_replace".to_string(),
        description: "Replace an exact substring of one of your core memory blocks with new \
            text — use this to correct or edit down something you previously remembered, \
            rather than letting a block go stale or overflow its budget."
            .to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "label": {"type": "string", "description": "Which block to edit"},
                "old_content": {"type": "string", "description": "Exact existing text to find"},
                "new_content": {"type": "string", "description": "Text to replace it with"}
            },
            "required": ["label", "old_content", "new_content"]
        }),
    });
    tools.push(ToolDef {
        name: "archival_memory_insert".to_string(),
        description: "Save a fact to long-term archival memory — for things worth keeping \
            permanently but too detailed or numerous for a core memory block (specific dates, \
            decisions, one-off facts). Retrieved later with archival_memory_search; never \
            shown automatically, so don't use it for things that need to always be visible — \
            that's what core memory is for."
            .to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "content": {"type": "string", "description": "The fact to remember, written so it stands alone out of context"}
            },
            "required": ["content"]
        }),
    });
    tools.push(ToolDef {
        name: "archival_memory_search".to_string(),
        description: "Search your long-term archival memory for facts saved with \
            archival_memory_insert. Use when a question needs something you filed away, not \
            something already visible in a core memory block."
            .to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "What to search for"}
            },
            "required": ["query"]
        }),
    });

    if shell_enabled {
        tools.push(ToolDef {
            name: "bash_exec".to_string(),
            description: "Run a shell command in your own persistent workspace directory \
                (survives between calls and across conversations with this user). Full shell \
                access: git clone, build tools, `gh pr create`, etc. all work, and GITHUB_TOKEN \
                is available in the environment if configured. No confirmation step — the \
                command runs immediately, so be as careful as you would running it for real, \
                because you are."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "Shell command to run"}
                },
                "required": ["command"]
            }),
        });
        tools.push(ToolDef {
            name: "file_read".to_string(),
            description: "Read a text file from your workspace directory. Path is relative to \
                the workspace root."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Relative path within your workspace"}
                },
                "required": ["path"]
            }),
        });
        tools.push(ToolDef {
            name: "file_write".to_string(),
            description: "Write (create or overwrite) a text file in your workspace \
                directory. Path is relative to the workspace root; parent directories are \
                created as needed."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Relative path within your workspace"},
                    "content": {"type": "string", "description": "Full file content to write"}
                },
                "required": ["path", "content"]
            }),
        });
    }
    tools
}

fn chunk_hit_to_item(hit: &db::chunks::ChunkHit) -> RetrievedItem {
    RetrievedItem {
        id: hit.item_id,
        url: hit.url.clone(),
        title: hit.title.clone(),
        summary: hit.summary.clone(),
        raw_content: None,
        tags: Vec::new(),
        category: None,
        context_window: None,
        shared_by: hit.shared_by,
        shared_by_username: hit.username.clone(),
        message_id: hit.message_id,
        shared_at: hit.shared_at,
        similarity: hit.similarity,
        source_channel: hit.source_channel.clone(),
        content_type: hit.content_type.clone(),
        group_id: hit.group_id,
    }
}

fn string_arg<'a>(input: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    input
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.trim().to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{}…", t.trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_names_are_reserved_against_mcp_shadowing() {
        for def in builtin_defs(true, true) {
            assert!(
                super::super::RESERVED_TOOL_NAMES.contains(&def.name.as_str()),
                "{} is not in mcp::RESERVED_TOOL_NAMES — an MCP server could shadow it",
                def.name
            );
        }
    }

    #[test]
    fn web_search_appears_only_when_configured() {
        assert!(builtin_defs(false, false).iter().all(|t| t.name != "web_search"));
        assert!(builtin_defs(true, false).iter().any(|t| t.name == "web_search"));
    }

    #[test]
    fn shell_tools_appear_only_when_enabled() {
        let off = builtin_defs(true, false);
        assert!(["bash_exec", "file_read", "file_write"]
            .iter()
            .all(|n| off.iter().all(|t| &t.name != n)));
        let on = builtin_defs(true, true);
        assert!(["bash_exec", "file_read", "file_write"]
            .iter()
            .all(|n| on.iter().any(|t| &t.name == n)));
    }

    #[test]
    fn catalogue_lists_every_tool_with_its_arguments() {
        let rendered = render_catalogue(&builtin_defs(true, true));
        for name in super::super::RESERVED_TOOL_NAMES {
            assert!(rendered.contains(name), "{name} missing from the catalogue");
        }
        // The model fills `arguments` from this, so the schema has to survive.
        assert!(rendered.contains("\"query\""));
        assert!(rendered.contains("\"topic\""));
        assert!(rendered.contains("\"label\""));
        assert!(rendered.contains("\"path\""));
        assert_eq!(rendered.matches("purpose:").count(), 12);
    }

    #[test]
    fn string_arg_rejects_blank_and_missing() {
        let v = serde_json::json!({"query": "  ", "other": 5});
        assert_eq!(string_arg(&v, "query"), None);
        assert_eq!(string_arg(&v, "missing"), None);
        assert_eq!(string_arg(&v, "other"), None);

        let v = serde_json::json!({"query": " robotics funding "});
        assert_eq!(string_arg(&v, "query"), Some("robotics funding"));
    }

    #[test]
    fn truncate_appends_ellipsis_only_when_over_limit() {
        assert_eq!(truncate("short", 100), "short");
        assert_eq!(truncate(&"x".repeat(10), 3), "xxx…");
    }
}
