//! GSuite (Gmail / Calendar / Drive) as a built-in MCP server.
//!
//! It speaks the same `list_tools`/`call_tool` surface as `client::HttpServer`
//! so `Registry` treats it identically — the agent loop can't tell a built-in
//! integration from a remote one. Running in-process rather than behind a
//! separate MCP server is the whole reason this exists: no sidecar to deploy,
//! and the Google refresh token never leaves the bot.
//!
//! Drive stays read-only. Gmail can also send, and Calendar can also create
//! events — with an optional auto-generated Google Meet link — the two write
//! paths this integration needs. The OAuth consent script's scopes must match
//! what's called here (`scripts/google_oauth_consent.py`) or Google 403s with
//! a message that doesn't mention scopes — and since a refresh token is
//! frozen to the scopes it was originally consented for, adding a write call
//! here means re-running that script and rotating GOOGLE_REFRESH_TOKEN, not
//! just redeploying.

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use super::{sanitize, McpTool, MAX_RESULT_CHARS};

const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GMAIL: &str = "https://gmail.googleapis.com/gmail/v1/users/me";
const CALENDAR: &str = "https://www.googleapis.com/calendar/v3";
const DRIVE: &str = "https://www.googleapis.com/drive/v3";
const TIMEOUT: Duration = Duration::from_secs(20);
/// Refresh a little before Google's stated expiry so an in-flight call can't
/// land on a just-expired token.
const EXPIRY_SLACK: Duration = Duration::from_secs(60);
/// Every list tool is capped: one Gmail search must not eat the turn budget.
const MAX_RESULTS: u64 = 25;
const DEFAULT_RESULTS: u64 = 10;
/// Body excerpt kept from `gmail_read` — enough to answer a question about a
/// mail, far short of pasting a whole newsletter into the prompt.
const MAX_BODY_CHARS: usize = 4_000;

pub struct GSuite {
    http: reqwest::Client,
    client_id: String,
    client_secret: String,
    refresh_token: String,
    /// Cached access token and the instant it stops being usable.
    token: Mutex<Option<(String, Instant)>>,
}

impl GSuite {
    pub fn new(client_id: String, client_secret: String, refresh_token: String) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(TIMEOUT)
                .build()
                .context("building gsuite http client")?,
            client_id,
            client_secret,
            refresh_token,
            token: Mutex::new(None),
        })
    }

    /// A valid access token, minted from the refresh token and cached until it
    /// expires. Held across the await so two concurrent calls don't both spend
    /// a refresh round-trip.
    async fn access_token(&self) -> Result<String> {
        let mut cached = self.token.lock().await;
        if let Some((token, expires_at)) = cached.as_ref() {
            if Instant::now() < *expires_at {
                return Ok(token.clone());
            }
        }
        let resp = self
            .http
            .post(TOKEN_URL)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("refresh_token", self.refresh_token.as_str()),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await
            .context("google token refresh")?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            // The body echoes `error_description`, never the secret itself.
            return Err(anyhow!("google token refresh HTTP {status}: {body}"));
        }
        let parsed: Value = serde_json::from_str(&body).context("google token response")?;
        let token = parsed["access_token"]
            .as_str()
            .ok_or_else(|| anyhow!("google token response has no access_token"))?
            .to_string();
        let ttl = Duration::from_secs(parsed["expires_in"].as_u64().unwrap_or(3600));
        *cached = Some((
            token.clone(),
            Instant::now() + ttl.saturating_sub(EXPIRY_SLACK),
        ));
        Ok(token)
    }

    async fn get(&self, url: &str, query: &[(&str, String)]) -> Result<Value> {
        let token = self.access_token().await?;
        let resp = self
            .http
            .get(url)
            .bearer_auth(token)
            .query(query)
            .send()
            .await
            .with_context(|| format!("google GET {url}"))?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            // 403 here is almost always a scope the credential never consented
            // to; Google's message says so, so pass it through verbatim.
            return Err(anyhow!(
                "google HTTP {status}: {}",
                body.chars().take(400).collect::<String>()
            ));
        }
        serde_json::from_str(&body).with_context(|| format!("decoding google response from {url}"))
    }

    async fn post(&self, url: &str, query: &[(&str, String)], body: Value) -> Result<Value> {
        let token = self.access_token().await?;
        let resp = self
            .http
            .post(url)
            .bearer_auth(token)
            .query(query)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("google POST {url}"))?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            // Same shape as `get`'s error: a write scope the credential was
            // never consented for shows up here, as a 403, same as a read.
            return Err(anyhow!(
                "google HTTP {status}: {}",
                body.chars().take(400).collect::<String>()
            ));
        }
        serde_json::from_str(&body).with_context(|| format!("decoding google response from {url}"))
    }

    pub fn list_tools(&self) -> Vec<McpTool> {
        vec![
            McpTool {
                name: "gmail_search".to_string(),
                description: "Search the user's Gmail. `query` uses Gmail's own search syntax \
                    (e.g. `from:alice newer_than:7d`, `is:unread subject:invoice`). Returns \
                    matching messages with sender, date, subject, snippet, and an id for \
                    gmail_read."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Gmail search query"},
                        "max_results": {"type": "integer", "description": "1-25, default 10"}
                    },
                    "required": ["query"]
                }),
            },
            McpTool {
                name: "gmail_read".to_string(),
                description: "Read the full body of one Gmail message by the id returned from \
                    gmail_search. Use when a snippet isn't enough to answer."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "message_id": {"type": "string", "description": "id from gmail_search"}
                    },
                    "required": ["message_id"]
                }),
            },
            McpTool {
                name: "gmail_send".to_string(),
                description: "Compose and send an email from the user's Gmail account. \
                    Use this whenever the user asks you to send, draft, or email someone."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "to": {"type": "string", "description": "Recipient email address"},
                        "subject": {"type": "string", "description": "Email subject line"},
                        "body": {"type": "string", "description": "Plain-text email body"}
                    },
                    "required": ["to", "subject", "body"]
                }),
            },
            McpTool {
                name: "calendar_events".to_string(),
                description: "The user's upcoming Google Calendar events, soonest first, with \
                    start/end times, location, and attendees. Use for anything about their \
                    schedule, meetings, or availability."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "days": {"type": "integer", "description": "Days ahead to look, 1-90, default 7"},
                        "max_results": {"type": "integer", "description": "1-25, default 10"}
                    }
                }),
            },
            McpTool {
                name: "calendar_create_event".to_string(),
                description: "Create an event on the user's primary Google Calendar and email \
                    invites to any attendees. Set add_meet_link (default true) to attach an \
                    auto-generated Google Meet link. `start`/`end` must be RFC3339 datetimes \
                    with a UTC offset, e.g. \"2026-08-16T18:00:00+05:30\" — resolve any \
                    relative date/time (\"tomorrow\", \"6pm IST\") against the current date \
                    given in your instructions before calling this."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "summary": {"type": "string", "description": "Event title"},
                        "start": {"type": "string", "description": "RFC3339 start datetime with UTC offset"},
                        "end": {"type": "string", "description": "RFC3339 end datetime with UTC offset"},
                        "attendees": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Attendee email addresses, invited automatically"
                        },
                        "description": {"type": "string", "description": "Optional event description/agenda"},
                        "add_meet_link": {"type": "boolean", "description": "Attach a Google Meet link, default true"}
                    },
                    "required": ["summary", "start", "end"]
                }),
            },
            McpTool {
                name: "drive_search".to_string(),
                description: "Search the user's Google Drive by file name and full-text content. \
                    Returns file names, types, links, and modification dates."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Words to look for in file names or contents"},
                        "max_results": {"type": "integer", "description": "1-25, default 10"}
                    },
                    "required": ["query"]
                }),
            },
        ]
    }

    pub async fn call_tool(&self, name: &str, args: &Value) -> Result<String> {
        let text = match name {
            "gmail_search" => {
                self.gmail_search(req_str(args, "query")?, count(args))
                    .await?
            }
            "gmail_read" => self.gmail_read(req_str(args, "message_id")?).await?,
            "gmail_send" => {
                self.gmail_send(
                    req_str(args, "to")?,
                    req_str(args, "subject")?,
                    req_str(args, "body")?,
                )
                .await?
            }
            "calendar_events" => {
                let days = clamped(args, "days", 7, 90);
                self.calendar_events(days, count(args)).await?
            }
            "calendar_create_event" => {
                self.calendar_create_event(
                    req_str(args, "summary")?,
                    req_str(args, "start")?,
                    req_str(args, "end")?,
                    &str_array(args, "attendees"),
                    args["description"].as_str(),
                    args["add_meet_link"].as_bool().unwrap_or(true),
                )
                .await?
            }
            "drive_search" => {
                self.drive_search(req_str(args, "query")?, count(args))
                    .await?
            }
            other => return Err(anyhow!("unknown gsuite tool '{other}'")),
        };
        // Google's own text is third-party content going into an agent prompt:
        // strip control characters here, at the boundary.
        Ok(sanitize(&text, MAX_RESULT_CHARS))
    }

    async fn gmail_search(&self, query: &str, max_results: u64) -> Result<String> {
        let list = self
            .get(
                &format!("{GMAIL}/messages"),
                &[
                    ("q", query.to_string()),
                    ("maxResults", max_results.to_string()),
                ],
            )
            .await?;
        let ids: Vec<String> = list["messages"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .filter_map(|m| m["id"].as_str().map(str::to_string))
            .collect();
        if ids.is_empty() {
            return Ok(format!("No Gmail messages match '{query}'."));
        }
        let mut out = String::new();
        // ponytail: N+1 fetches, sequential. Bounded by MAX_RESULTS (25), so
        // worst case is a couple of seconds; parallelize if that ever shows up
        // in the /ask latency metric.
        for id in &ids {
            let msg = match self
                .get(
                    &format!("{GMAIL}/messages/{id}"),
                    &[
                        ("format", "metadata".to_string()),
                        ("metadataHeaders", "Subject".to_string()),
                        ("metadataHeaders", "From".to_string()),
                        ("metadataHeaders", "Date".to_string()),
                    ],
                )
                .await
            {
                Ok(msg) => msg,
                // One message deleted between the list and the fetch must not
                // sink the other 24.
                Err(e) => {
                    tracing::warn!(id, error = %e, "skipping unreadable gmail message");
                    eprintln!("[warn] skipping unreadable gmail message: id={id} error={e}");
                    continue;
                }
            };
            let h = |name: &str| header(&msg, name);
            out.push_str(&format!(
                "- {date} | from {from} | {subject}\n  {snippet}\n  id: {id}\n",
                date = h("Date"),
                from = h("From"),
                subject = h("Subject"),
                snippet = msg["snippet"].as_str().unwrap_or_default().trim(),
            ));
        }
        if out.is_empty() {
            return Ok(format!(
                "Found {} message(s) for '{query}' but none could be read.",
                ids.len()
            ));
        }
        Ok(out)
    }

    async fn gmail_read(&self, message_id: &str) -> Result<String> {
        let msg = self
            .get(
                &format!("{GMAIL}/messages/{message_id}"),
                &[("format", "full".to_string())],
            )
            .await?;
        let body = plain_text_body(&msg["payload"]).unwrap_or_else(|| {
            msg["snippet"]
                .as_str()
                .unwrap_or("(no readable text body)")
                .to_string()
        });
        Ok(format!(
            "From: {}\nTo: {}\nDate: {}\nSubject: {}\n\n{}",
            header(&msg, "From"),
            header(&msg, "To"),
            header(&msg, "Date"),
            header(&msg, "Subject"),
            clip(body.trim(), MAX_BODY_CHARS),
        ))
    }

    async fn calendar_events(&self, days: i64, max_results: u64) -> Result<String> {
        let now = chrono::Utc::now();
        let events = self
            .get(
                &format!("{CALENDAR}/calendars/primary/events"),
                &[
                    ("timeMin", now.to_rfc3339()),
                    ("timeMax", (now + chrono::Duration::days(days)).to_rfc3339()),
                    ("singleEvents", "true".to_string()),
                    ("orderBy", "startTime".to_string()),
                    ("maxResults", max_results.to_string()),
                ],
            )
            .await?;
        let items = events["items"].as_array().cloned().unwrap_or_default();
        if items.is_empty() {
            return Ok(format!("No calendar events in the next {days} day(s)."));
        }
        let mut out = String::new();
        for e in items {
            // All-day events carry `date`; timed ones carry `dateTime`.
            let when = |k: &str| {
                e[k]["dateTime"]
                    .as_str()
                    .or_else(|| e[k]["date"].as_str())
                    .unwrap_or("?")
                    .to_string()
            };
            let attendees: Vec<&str> = e["attendees"]
                .as_array()
                .map(|a| a.iter().filter_map(|x| x["email"].as_str()).collect())
                .unwrap_or_default();
            out.push_str(&format!(
                "- {} → {} | {}",
                when("start"),
                when("end"),
                e["summary"].as_str().unwrap_or("(no title)"),
            ));
            if let Some(loc) = e["location"].as_str().filter(|l| !l.is_empty()) {
                out.push_str(&format!(" @ {loc}"));
            }
            if !attendees.is_empty() {
                out.push_str(&format!(" | attendees: {}", attendees.join(", ")));
            }
            out.push('\n');
        }
        Ok(out)
    }

    #[allow(clippy::too_many_arguments)]
    async fn calendar_create_event(
        &self,
        summary: &str,
        start: &str,
        end: &str,
        attendees: &[String],
        description: Option<&str>,
        add_meet_link: bool,
    ) -> Result<String> {
        // Validate up front — Google's 400 for a malformed RFC3339 string
        // doesn't say which field, so catch it here with a message that does.
        chrono::DateTime::parse_from_rfc3339(start)
            .with_context(|| format!("'start' is not a valid RFC3339 datetime: {start}"))?;
        chrono::DateTime::parse_from_rfc3339(end)
            .with_context(|| format!("'end' is not a valid RFC3339 datetime: {end}"))?;

        let mut body = json!({
            "summary": summary,
            "start": {"dateTime": start},
            "end": {"dateTime": end},
        });
        if let Some(desc) = description.filter(|d| !d.is_empty()) {
            body["description"] = json!(desc);
        }
        if !attendees.is_empty() {
            body["attendees"] = json!(attendees.iter().map(|e| json!({"email": e})).collect::<Vec<_>>());
        }
        let mut query = vec![("sendUpdates", "all".to_string())];
        if add_meet_link {
            // requestId just needs to be unique per create call, so Google can
            // tell a retry of this request apart from a brand new one.
            body["conferenceData"] = json!({
                "createRequest": {
                    "requestId": uuid::Uuid::new_v4().to_string(),
                    "conferenceSolutionKey": {"type": "hangoutsMeet"}
                }
            });
            query.push(("conferenceDataVersion", "1".to_string()));
        }

        let event = self
            .post(&format!("{CALENDAR}/calendars/primary/events"), &query, body)
            .await?;
        let link = event["htmlLink"].as_str().unwrap_or("");
        let meet = event["conferenceData"]["entryPoints"]
            .as_array()
            .and_then(|eps| eps.iter().find(|e| e["entryPointType"] == "video"))
            .and_then(|e| e["uri"].as_str())
            .unwrap_or("");
        let mut out = format!("Created \"{summary}\" ({start} \u{2192} {end}).\nCalendar link: {link}");
        if !meet.is_empty() {
            out.push_str(&format!("\nGoogle Meet: {meet}"));
        }
        if !attendees.is_empty() {
            out.push_str(&format!("\nInvited: {}", attendees.join(", ")));
        }
        Ok(out)
    }

    async fn gmail_send(&self, to: &str, subject: &str, body: &str) -> Result<String> {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        // Build a minimal RFC 2822 message.
        let raw_email = format!(
            "To: {to}\r\nSubject: {subject}\r\nContent-Type: text/plain; charset=UTF-8\r\n\r\n{body}"
        );
        let encoded = URL_SAFE_NO_PAD.encode(raw_email.as_bytes());
        let payload = json!({ "raw": encoded });
        let sent = self
            .post(&format!("{GMAIL}/messages/send"), &[], payload)
            .await?;
        let id = sent["id"].as_str().unwrap_or("(unknown id)");
        Ok(format!("Email sent successfully to {to}. Message id: {id}"))
    }

    async fn drive_search(&self, query: &str, max_results: u64) -> Result<String> {
        let q = escape_drive_literal(query);
        let files = self
            .get(
                &format!("{DRIVE}/files"),
                &[
                    (
                        "q",
                        format!(
                            "trashed = false and (name contains '{q}' or fullText contains '{q}')"
                        ),
                    ),
                    ("pageSize", max_results.to_string()),
                    ("orderBy", "modifiedTime desc".to_string()),
                    (
                        "fields",
                        "files(name,mimeType,modifiedTime,webViewLink)".to_string(),
                    ),
                ],
            )
            .await?;
        let items = files["files"].as_array().cloned().unwrap_or_default();
        if items.is_empty() {
            return Ok(format!("No Drive files match '{query}'."));
        }
        let mut out = String::new();
        for f in items {
            out.push_str(&format!(
                "- {} ({}) modified {}\n  {}\n",
                f["name"].as_str().unwrap_or("(unnamed)"),
                short_mime(f["mimeType"].as_str().unwrap_or("")),
                f["modifiedTime"].as_str().unwrap_or("?"),
                f["webViewLink"].as_str().unwrap_or(""),
            ));
        }
        Ok(out)
    }
}

fn header(msg: &Value, name: &str) -> String {
    msg["payload"]["headers"]
        .as_array()
        .and_then(|hs| {
            hs.iter().find(|h| {
                h["name"]
                    .as_str()
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
            })
        })
        .and_then(|h| h["value"].as_str())
        .unwrap_or("")
        .to_string()
}

/// First `text/plain` part of a MIME tree, base64url-decoded. Multipart mail
/// nests arbitrarily deep, so this recurses rather than checking `parts[0]`.
fn plain_text_body(payload: &Value) -> Option<String> {
    if payload["mimeType"] == "text/plain" {
        if let Some(data) = payload["body"]["data"].as_str() {
            return decode_b64url(data);
        }
    }
    payload["parts"]
        .as_array()?
        .iter()
        .find_map(plain_text_body)
}

fn decode_b64url(data: &str) -> Option<String> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    // Gmail sends unpadded base64url; strip any padding a variant adds so the
    // NO_PAD engine doesn't reject an otherwise-valid body.
    let bytes = URL_SAFE_NO_PAD.decode(data.trim_end_matches('=')).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Drive's query language is a string grammar, so a `'` in user text would
/// otherwise close the literal and let the rest be read as query syntax.
fn escape_drive_literal(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

fn short_mime(mime: &str) -> &str {
    match mime {
        "application/vnd.google-apps.document" => "Google Doc",
        "application/vnd.google-apps.spreadsheet" => "Google Sheet",
        "application/vnd.google-apps.presentation" => "Google Slides",
        "application/vnd.google-apps.folder" => "folder",
        "application/pdf" => "PDF",
        other => other,
    }
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    format!("{}…", s.chars().take(max).collect::<String>())
}

fn req_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args[key]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("'{key}' is required and must be a non-empty string"))
}

/// Best-effort string array: non-string entries are dropped rather than
/// failing the whole call over one malformed attendee.
fn str_array(args: &Value, key: &str) -> Vec<String> {
    args[key]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

fn count(args: &Value) -> u64 {
    clamped(
        args,
        "max_results",
        DEFAULT_RESULTS as i64,
        MAX_RESULTS as i64,
    ) as u64
}

/// A model that guesses `max_results: 500` gets clamped rather than an error.
fn clamped(args: &Value, key: &str, default: i64, max: i64) -> i64 {
    args[key].as_i64().unwrap_or(default).clamp(1, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_literals_cannot_escape_the_quoted_string() {
        assert_eq!(escape_drive_literal("o'brien"), "o\\'brien");
        assert_eq!(
            escape_drive_literal("x' or name contains 'y"),
            "x\\' or name contains \\'y"
        );
        // A trailing backslash must not escape our own closing quote.
        assert_eq!(escape_drive_literal("a\\"), "a\\\\");
    }

    #[test]
    fn counts_are_clamped_into_range() {
        assert_eq!(count(&json!({})), DEFAULT_RESULTS);
        assert_eq!(count(&json!({"max_results": 0})), 1);
        assert_eq!(count(&json!({"max_results": 5000})), MAX_RESULTS);
        assert_eq!(count(&json!({"max_results": "ten"})), DEFAULT_RESULTS);
        assert_eq!(clamped(&json!({"days": -3}), "days", 7, 90), 1);
        assert_eq!(clamped(&json!({"days": 400}), "days", 7, 90), 90);
    }

    #[test]
    fn str_array_drops_non_string_entries() {
        assert_eq!(
            str_array(&json!({"attendees": ["a@x.com", 5, "b@x.com", null]}), "attendees"),
            vec!["a@x.com".to_string(), "b@x.com".to_string()]
        );
        assert!(str_array(&json!({}), "attendees").is_empty());
    }

    #[test]
    fn required_string_args_reject_blanks() {
        assert!(req_str(&json!({"query": "  "}), "query").is_err());
        assert!(req_str(&json!({"query": 7}), "query").is_err());
        assert_eq!(
            req_str(&json!({"query": " mail "}), "query").unwrap(),
            "mail"
        );
    }

    #[test]
    fn plain_text_body_walks_nested_multipart() {
        // "hi there" as unpadded base64url.
        let payload = json!({
            "mimeType": "multipart/mixed",
            "parts": [
                {"mimeType": "text/html", "body": {"data": "PGI-eDwvYj4"}},
                {"mimeType": "multipart/alternative", "parts": [
                    {"mimeType": "text/plain", "body": {"data": "aGkgdGhlcmU"}}
                ]}
            ]
        });
        assert_eq!(plain_text_body(&payload).unwrap(), "hi there");
        assert!(plain_text_body(&json!({"mimeType": "text/html"})).is_none());
    }

    /// `gmail_search` asks for three metadata headers by repeating the same
    /// query key. If reqwest collapsed repeats, every subject/from/date would
    /// come back blank and nothing would fail loudly — so pin the assumption.
    #[test]
    fn repeated_query_keys_survive_url_encoding() {
        let url = reqwest::Client::new()
            .get("https://example.test/m")
            .query(&[
                ("format", "metadata".to_string()),
                ("metadataHeaders", "Subject".to_string()),
                ("metadataHeaders", "From".to_string()),
                ("metadataHeaders", "Date".to_string()),
            ])
            .build()
            .unwrap()
            .url()
            .to_string();
        assert_eq!(
            url,
            "https://example.test/m?format=metadata&metadataHeaders=Subject\
             &metadataHeaders=From&metadataHeaders=Date"
        );
    }

    /// Gmail's `q` and Drive's `q` carry spaces, quotes and colons; they must
    /// reach Google percent-encoded, not as raw URL syntax.
    #[test]
    fn query_values_are_percent_encoded() {
        let url = reqwest::Client::new()
            .get("https://example.test/f")
            .query(&[(
                "q",
                "trashed = false and (name contains 'a b&c')".to_string(),
            )])
            .build()
            .unwrap()
            .url()
            .to_string();
        // Form encoding: spaces become `+`, which Google decodes back to a
        // space. What matters is that nothing stays raw.
        assert_eq!(
            url,
            "https://example.test/f?q=trashed+%3D+false+and+%28name+contains+%27a+b%26c%27%29"
        );
    }

    #[test]
    fn headers_are_found_case_insensitively() {
        let msg = json!({"payload": {"headers": [{"name": "subject", "value": "Q3 plan"}]}});
        assert_eq!(header(&msg, "Subject"), "Q3 plan");
        assert_eq!(header(&msg, "From"), "");
    }
}
