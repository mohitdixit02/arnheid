//! Shell-based web search backend.
//!
//! Runs a user-configured command and parses its JSON output. Supports ddgr,
//! googler, or any tool that emits the same JSON schema:
//!   [{"title":"…","url":"…","abstract":"…"}, …]
//!
//! The command template supports two placeholders:
//!   {query}  — shell-escaped search query
//!   {max}    — max results (from WEB_SEARCH_MAX_RESULTS)
//!
//! Example values for WEB_SEARCH_CMD:
//!   ddgr --noua --json --num {max} {query}
//!   googler --noua --json --count {max} {query}

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashSet;
use tokio::process::Command;

/// A single web search result passed to Tier 5 synthesis.
#[derive(Debug, Clone)]
pub struct WebSnippet {
    pub title: String,
    pub url: String,
    pub content: String,
}

pub struct ShellSearch {
    cmd_template: String,
    max_results: usize,
}

/// JSON shape emitted by ddgr / googler.
#[derive(Deserialize)]
struct SearchResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    /// ddgr uses "abstract", googler uses "abstract" too; fallback to "snippet"
    /// or "content" for other tools.
    #[serde(
        alias = "abstract",
        alias = "snippet",
        alias = "content",
        default
    )]
    abstract_: String,
}

impl ShellSearch {
    pub fn new(cmd_template: String, max_results: usize) -> Self {
        Self { cmd_template, max_results: max_results.max(1) }
    }

    /// Build the final shell command, substituting `{query}` and `{max}`.
    fn build_cmd(&self, query: &str) -> String {
        self.cmd_template
            .replace("{query}", &shell_quote(query))
            .replace("{max}", &self.max_results.to_string())
    }

    pub async fn search(&self, query: &str) -> Result<Vec<WebSnippet>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(vec![]);
        }

        let cmd = self.build_cmd(query);
        tracing::info!(query = %query, cmd = %cmd, "executing web search command");
        println!("[info] executing web search command: query={query} cmd={cmd}");

        #[cfg(windows)]
        let output = Command::new("cmd")
            .env("PYTHONIOENCODING", "utf-8")
            .args(["/C", &cmd])
            .output()
            .await
            .with_context(|| format!("web search command failed to start: {cmd}"))?;

        #[cfg(not(windows))]
        let output = Command::new("sh")
            .env("PYTHONIOENCODING", "utf-8")
            .args(["-c", &cmd])
            .output()
            .await
            .with_context(|| format!("web search command failed to start: {cmd}"))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        tracing::info!(
            success = output.status.success(),
            code = ?output.status.code(),
            stdout_len = stdout.len(),
            stderr_len = stderr.len(),
            "web search command finished"
        );
        println!(
            "[info] web search command finished: success={} code={:?} stdout_len={} stderr_len={}",
            output.status.success(),
            output.status.code(),
            stdout.len(),
            stderr.len()
        );

        if !output.status.success() {
            tracing::error!(stderr = %stderr, "web search command failed");
            eprintln!("[error] web search command failed: stderr={stderr}");
            anyhow::bail!(
                "web search command exited {}: {}",
                output.status,
                stderr.chars().take(300).collect::<String>()
            );
        }

        let results: Vec<SearchResult> = serde_json::from_str(stdout.trim())
            .with_context(|| {
                format!(
                    "web search output is not valid JSON array. \
                     Ensure WEB_SEARCH_CMD outputs \
                     [{{\"title\":\"…\",\"url\":\"…\",\"abstract\":\"…\"}}] format. \
                     Got: {}",
                    stdout.chars().take(200).collect::<String>()
                )
            })?;

        tracing::info!(results_count = results.len(), "parsed web search results");
        println!("[info] parsed web search results: results_count={}", results.len());

        let mut snippets: Vec<WebSnippet> = results
            .into_iter()
            .filter(|r| !r.url.is_empty())
            .map(|r| WebSnippet {
                title: r.title,
                url: r.url,
                content: r.abstract_,
            })
            .collect();

        if let Some(weather) = fetch_weather_snippet(query).await {
            tracing::info!(weather_info = %weather.content, "prepended weather snippet");
            println!("[info] prepended weather snippet: {}", weather.content);
            snippets.insert(0, weather);
        }

        Ok(snippets)
    }


    /// Run multiple queries, deduplicate by URL, return up to `cap` snippets.
    pub async fn search_many(&self, queries: &[String], cap: usize) -> Result<Vec<WebSnippet>> {
        let cap = cap.max(1);
        let mut seen = HashSet::new();
        let mut out = Vec::new();

        for q in queries {
            if out.len() >= cap {
                break;
            }
            let q = q.trim();
            if q.is_empty() {
                continue;
            }
            match self.search(q).await {
                Ok(snippets) => {
                    for s in snippets {
                        if seen.insert(s.url.clone()) {
                            out.push(s);
                            if out.len() >= cap {
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, query = %q, "web search query failed, skipping");
                    eprintln!("[warn] web search query failed, skipping: query={q} error={e}");
                }
            }
        }
        Ok(out)
    }
}

/// Wraps a query in single quotes and escapes embedded single quotes (POSIX style).
/// On Windows this is still reasonable for cmd.exe given typical search queries.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Helper function to query wttr.in for real-time weather details when query mentions weather.
async fn fetch_weather_snippet(query: &str) -> Option<WebSnippet> {
    let lowercase = query.to_lowercase();
    if !lowercase.contains("weather") {
        return None;
    }

    let clean = lowercase
        .replace("what is the", "")
        .replace("what is", "")
        .replace("current", "")
        .replace("weather", "")
        .replace("in ", "")
        .replace("of ", "")
        .replace("for ", "")
        .trim()
        .to_string();

    let location = if clean.is_empty() {
        "delhi".to_string()
    } else {
        clean
    };

    let url = format!("https://wttr.in/{}", location);

    #[cfg(windows)]
    let output = Command::new("cmd")
        .args(["/C", &format!("curl.exe -s \"wttr.in/{}?format=4\"", location)])
        .output()
        .await
        .ok()?;

    #[cfg(not(windows))]
    let output = Command::new("sh")
        .args(["-c", &format!("curl -s \"wttr.in/{}?format=4\"", location)])
        .output()
        .await
        .ok()?;

    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !text.is_empty() && !text.contains("Unknown location") && !text.contains("Error") {
            return Some(WebSnippet {
                title: format!("Real-time Weather Info for {}", location),
                url,
                content: text,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_basic() {
        assert_eq!(shell_quote("weather in delhi"), "'weather in delhi'");
    }

    #[test]
    fn shell_quote_with_single_quote() {
        assert_eq!(shell_quote("what's the weather"), "'what'\\''s the weather'");
    }

    #[test]
    fn build_cmd_substitutes_placeholders() {
        let s = ShellSearch::new("ddgr --json --num {max} {query}".into(), 5);
        let cmd = s.build_cmd("weather in delhi");
        assert_eq!(cmd, "ddgr --json --num 5 'weather in delhi'");
    }
}
