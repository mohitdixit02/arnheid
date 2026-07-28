//! Tiered chat tasks (summarize, excerpt, graph, synthesis) with per-tier
//! provider routing between Claude and a local Ollama model.

use crate::llm::anthropic::Anthropic;
use crate::llm::ollama::Ollama;
use crate::llm::Provider;
use crate::models::Summary;
use anyhow::{anyhow, Context, Result};

/// Which provider serves each chat tier. Defaults can be set per tier in config.
#[derive(Debug, Clone, Copy)]
pub struct ChatRoute {
    pub tier2: Provider,  // summarize + excerpt
    pub tier4: Provider,  // graph extraction
    pub tier5: Provider,  // RAG synthesis
    pub router: Provider, // message-intent routing (capture vs. agent)
}

pub struct Chat {
    anthropic: Option<Anthropic>,
    ollama: Option<Ollama>,
    route: ChatRoute,
}

impl Chat {
    pub fn new(
        anthropic: Option<Anthropic>,
        ollama: Option<Ollama>,
        route: ChatRoute,
    ) -> Result<Self> {
        // Validate that every routed provider is actually configured.
        for (tier, p) in [
            ("tier2", route.tier2),
            ("tier4", route.tier4),
            ("tier5", route.tier5),
            ("router", route.router),
        ] {
            match p {
                Provider::Anthropic if anthropic.is_none() => {
                    return Err(anyhow!(
                        "{tier} routed to anthropic but ANTHROPIC_API_KEY is not set"
                    ));
                }
                Provider::Ollama if ollama.is_none() => {
                    return Err(anyhow!(
                        "{tier} routed to ollama but Ollama is not configured"
                    ));
                }
                _ => {}
            }
        }
        tracing::info!(
            tier2 = ?route.tier2,
            tier4 = ?route.tier4,
            tier5 = ?route.tier5,
            router = ?route.router,
            "chat provider routing"
        );
        println!(
            "[info] chat provider routing: tier2={:?} tier4={:?} tier5={:?} router={:?}",
            route.tier2, route.tier4, route.tier5, route.router
        );
        Ok(Self {
            anthropic,
            ollama,
            route,
        })
    }

    /// Anthropic model id to use for a tier (Haiku for 2, Sonnet for 4/5,
    /// its own configurable model for the intent router).
    fn anthropic_model<'a>(a: &'a Anthropic, tier: Tier) -> &'a str {
        match tier {
            Tier::Two => &a.haiku_model,
            Tier::Four | Tier::Five => &a.sonnet_model,
            Tier::Router => &a.router_model,
        }
    }

    /// Same idea for Ollama — the router gets its own model only if
    /// `OLLAMA_ROUTER_MODEL` overrides it; otherwise it shares tier 2/4/5's.
    fn ollama_model(o: &Ollama, tier: Tier) -> &str {
        match tier {
            Tier::Router => o.router_model(),
            Tier::Two | Tier::Four | Tier::Five => o.model(),
        }
    }

    async fn complete(
        &self,
        tier: Tier,
        system: Option<&str>,
        user: &str,
        max_tokens: u32,
    ) -> Result<String> {
        let provider = match tier {
            Tier::Two => self.route.tier2,
            Tier::Four => self.route.tier4,
            Tier::Five => self.route.tier5,
            Tier::Router => self.route.router,
        };
        match provider {
            Provider::Anthropic => {
                let a = self
                    .anthropic
                    .as_ref()
                    .ok_or_else(|| anyhow!("anthropic unset"))?;
                let model = Self::anthropic_model(a, tier);
                a.complete(model, system, user, max_tokens).await
            }
            Provider::Ollama => {
                let o = self
                    .ollama
                    .as_ref()
                    .ok_or_else(|| anyhow!("ollama unset"))?;
                let model = Self::ollama_model(o, tier);
                o.complete(model, system, user, max_tokens).await
            }
        }
    }

    // ── Vision: describe a captured photo (WhatsApp/IG ingress) ─────────────
    /// Requires Claude — the local Ollama route has no vision model wired up.
    pub async fn describe_image(&self, image: &[u8], media_type: &str) -> Result<String> {
        let a = self.anthropic.as_ref().ok_or_else(|| {
            anyhow!("image capture requires ANTHROPIC_API_KEY (vision)")
        })?;
        a.describe_image(
            image,
            media_type,
            "Describe this image in 2-3 sentences for a personal knowledge archive: \
             what it shows, any readable text, and why someone might have saved it.",
        )
        .await
    }

    // ── Tier 2c: context envelope → structured user signals ───────────────
    pub async fn parse_context_signals(
        &self,
        title: &str,
        context_text: &str,
        user_comment: &str,
    ) -> Result<crate::models::ContextSignals> {
        let user = format!(
            "Content title: {title}\n\
             Conversation around the share:\n{context_text}\n\
             User's own message/comment: {user_comment}\n\n\
             Extract why the user saved this and how they feel about the topic. \
             Return JSON only: \
             {{\"intent\": \"brief why they saved it\", \
             \"sentiment\": -1.0 to 1.0, \
             \"entities\": [\"topics/people/companies they care about here\"], \
             \"signal_strength\": 0.5 to 2.0}}"
        );
        let raw = self.complete(Tier::Two, None, &user, 512).await?;
        parse_json(&raw).context("parsing context signals JSON")
    }

    // ── Tier 2: summarize + tag + classify ──────────────────────────────────
    pub async fn summarize(&self, title: &str, content: &str) -> Result<Summary> {
        let user = format!(
            "Title: {title}\n\nContent:\n{content}\n\n\
             Summarize this content in 3 sentences. Extract 5 topic tags \
             (lowercase, single words or short phrases). Classify into one category. \
             Return JSON only, no prose: \
             {{\"summary\": \"...\", \"tags\": [\"...\"], \"category\": \"...\"}}"
        );
        let raw = self.complete(Tier::Two, None, &user, 1024).await?;
        parse_json(&raw).context("parsing summary JSON")
    }

    // ── Tier 2: relevant excerpt for a notification ─────────────────────────
    pub async fn extract_excerpt(&self, content: &str, interests: &[String]) -> Result<String> {
        let interest_list = if interests.is_empty() {
            "general technology and startups".to_string()
        } else {
            interests.join(", ")
        };
        let user = format!(
            "Article:\n{content}\n\n\
             A user's interests: [{interest_list}]. \
             Extract the 1-2 sentences from the article most relevant to those \
             interests. Return only those sentences, no preamble."
        );
        self.complete(Tier::Two, None, &user, 256).await
    }

    // ── Query expansion (HyDE + multi-query) for retrieval ─────────────────
    pub async fn expand_query(&self, question: &str) -> Result<QueryExpansion> {
        let user = format!(
            "User question: {question}\n\n\
             Generate retrieval aids to find documents that answer it. \
             Return JSON only: \
             {{\"probes\": [\"2-3 alternative phrasings or focused sub-questions\"], \
             \"hyde\": \"1-3 sentences of a plausible hypothetical answer such a document might contain\"}}"
        );
        let raw = self.complete(Tier::Two, None, &user, 400).await?;
        parse_json(&raw).context("parsing query expansion")
    }

    // ── Intent router for /ask --web ────────────────────────────────────────
    pub async fn classify_query_intent(&self, question: &str) -> Result<QueryIntent> {
        let user = format!(
            "Classify this question for a personal knowledge assistant that has \
             (a) the user's saved links/notes and (b) optional live web search.\n\
             Question: {question}\n\n\
             Return JSON only: \
             {{\"intent\": \"recall|synthesize|augment|open|hybrid\", \
             \"web_queries\": [\"0-2 focused web search queries; empty for recall/synthesize\"]}}"
        );
        let raw = self.complete(Tier::Two, None, &user, 256).await?;
        parse_json(&raw).context("parsing query intent")
    }

    // ── Intent router: capture vs. agent (question or action request) ───────
    /// Replaces a fixed heuristic that missed imperatives with no opener word
    /// and messages carrying a leading "@mention" the channel didn't strip.
    /// Callers fall back to that heuristic if this errors, so a provider
    /// hiccup degrades gracefully instead of blocking capture.
    pub async fn classify_message_intent(&self, text: &str) -> Result<MessageIntent> {
        let user = format!(
            "Message: {text}\n\n\
             Classify it for a personal-assistant bot that either SAVES a message as a \
             note for later, or hands it to an agent that can answer questions and act on \
             connected accounts (search saved material, check email/calendar/drive, send \
             an email, create a calendar event).\n\
             - \"agent\": directed at the assistant — a question, request, or instruction \
             (find/check/send/create/tell/explain/etc.), even imperative with no question \
             mark, even with a leading \"@mention\".\n\
             - \"capture\": a statement, fact, or link the user wants saved, not directed \
             at the assistant.\n\
             Return JSON only: {{\"intent\": \"agent|capture\"}}"
        );
        let raw = self.complete(Tier::Router, None, &user, 40).await?;
        let parsed: IntentField = parse_json(&raw).context("parsing message intent")?;
        Ok(parsed.intent)
    }

    // ── Tier 4: entity + edge extraction ────────────────────────────────────
    pub async fn extract_graph(&self, batch_prompt: &str) -> Result<String> {
        self.complete(Tier::Four, None, batch_prompt, 4096).await
    }

    // ── Tier 5: RAG synthesis ───────────────────────────────────────────────
    pub async fn synthesize(&self, system: &str, context_and_question: &str) -> Result<String> {
        self.complete(Tier::Five, Some(system), context_and_question, 2000)
            .await
    }
}

#[derive(Clone, Copy)]
enum Tier {
    Two,
    Four,
    Five,
    /// Message-intent routing (capture vs. agent) — its own tier so it can
    /// be pointed at a different, cheaper/faster model than tier 2 without
    /// affecting summarize/excerpt quality.
    Router,
}

/// Retrieval probes generated from a user question (HyDE + multi-query).
#[derive(serde::Deserialize, Default)]
pub struct QueryExpansion {
    #[serde(default)]
    pub probes: Vec<String>,
    #[serde(default)]
    pub hyde: String,
}

/// How much external web context a question likely needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QueryIntentKind {
    #[default]
    Recall,
    Synthesize,
    Augment,
    Open,
    Hybrid,
}

#[derive(serde::Deserialize, Default)]
pub struct QueryIntent {
    #[serde(default)]
    pub intent: QueryIntentKind,
    #[serde(default)]
    pub web_queries: Vec<String>,
}

/// Does an inbound message want the agent (a question or an action request),
/// or is it just something to save?
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageIntent {
    #[default]
    Capture,
    Agent,
}

#[derive(serde::Deserialize, Default)]
struct IntentField {
    #[serde(default)]
    intent: MessageIntent,
}

/// Tolerantly pull a JSON object out of an LLM response (handles ```json fences).
fn parse_json<T: serde::de::DeserializeOwned>(raw: &str) -> Result<T> {
    let cleaned = raw.trim();
    let cleaned = cleaned
        .strip_prefix("```json")
        .or_else(|| cleaned.strip_prefix("```"))
        .unwrap_or(cleaned)
        .trim_end_matches("```")
        .trim();

    if let Ok(v) = serde_json::from_str::<T>(cleaned) {
        return Ok(v);
    }
    let start = cleaned.find('{');
    let end = cleaned.rfind('}');
    if let (Some(s), Some(e)) = (start, end) {
        if e > s {
            return serde_json::from_str::<T>(&cleaned[s..=e])
                .context("no valid JSON object found");
        }
    }
    Err(anyhow!("no JSON object in response: {cleaned}"))
}
