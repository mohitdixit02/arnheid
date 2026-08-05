//! Agentic `/ask`: the model decides, turn by turn, whether it can answer the
//! user outright or needs a tool first — so "hey, you there?" costs one call
//! and no searches, while "what did I save about X" pays for retrieval.
//!
//! The protocol is JSON in the model's text output, not provider-native tool
//! calling: every turn the model returns `{reasoning, message, tool_calls,
//! final_response}`. That keeps the loop working on any chat provider
//! (Groq/DeepSeek/Ollama, not just Claude) and makes the model's thinking
//! loggable. `reasoning` and `tool_calls` go to the logs; only the `message`
//! of the final turn reaches the user.
//!
//! Every tool — brain search, knowledge graph, web, and every MCP integration
//! — is dispatched through `mcp::toolbox`, so this module never special-cases
//! a backend.
//!
//! Scope, deliberately narrow: anchored ("about this link"), deictic ("what
//! about that"), and follow-up queries need session/thread continuity this
//! module doesn't have, so `handle_or_fallback` routes those to the fixed
//! pipeline regardless of `RAG_MODE`.

use crate::mcp::toolbox::{Sources, ToolBox, ToolCtx};
use crate::query::handler::{cited_in_text, cited_web_in_text, QueryReply, QueryScope};
use crate::state::AppState;
use anyhow::{anyhow, Result};
use serde::Deserialize;
use uuid::Uuid;

/// Hard cap on turns per question. A confused model can't loop forever, and
/// can't run up an API bill forever either.
const MAX_TURNS: u32 = 5;
/// Tool calls honoured per turn; extras are dropped with a note back to the
/// model, which keeps one turn from fanning out into a dozen searches.
const MAX_CALLS_PER_TURN: usize = 3;
/// Chars of one tool result kept in the transcript. The transcript is resent
/// every turn, so an unbounded result would blow a small model's context
/// window (and the bill) by turn three.
const MAX_RESULT_CHARS: usize = 4_000;

const OUTPUT_CONTRACT: &str = "\
Reply with a single JSON object and nothing else — no prose, no code fences:\n\
{\n\
  \"reasoning\": \"one or two sentences: what the user is asking for and why you are \
doing what you are doing. Never shown to the user.\",\n\
  \"message\": \"what to say to the user — fill this in only when final_response is true\",\n\
  \"tool_calls\": [{\"name\": \"tool_name\", \"arguments\": {…}}],\n\
  \"final_response\": true|false\n\
}\n\
Rules:\n\
- final_response=true: you are done. \"message\" holds the complete answer and \
\"tool_calls\" MUST be empty.\n\
- final_response=false: \"tool_calls\" MUST list at least one call; \"message\" is ignored.\n\
- You can list multiple tool calls (up to 3) in the \"tool_calls\" array to execute them in parallel (e.g. searching both email and calendar at the same time) or searching knowledge graph and chats.\n\
- Never repeat a call you already made with identical arguments — read the result \
you already have and move on.\n\
- Use only the tool names listed above, with exactly the arguments in their schema.";

const DECISION_POLICY: &str = "\
Decide FIRST whether you need a tool at all:\n\
- Greetings, small talk, thanks, questions about you or what you can do, and \
anything you can answer from your own general knowledge with confidence: answer \
immediately with final_response=true and NO tool calls. Do not search.\n\
- Anything about what the user saved, captured, shared, read, or discussed — or \
any question you cannot answer without their material: call search_brain (and \
graph_lookup for people/companies/topics and their connections).\n\
- Details, names, emails, timelines, or subjects discussed earlier in the ongoing group/private conversation that are not present in your immediate context: call search_chat_history. Check this if web-search, gsuite, or other lookups fail or return empty.\n\
- Current events, prices, releases, or public facts that would not be in their \
saved material and that you cannot answer reliably from memory: call web_search. \
Never call it for chit-chat or opinions.\n\
- Their own connected accounts (mail, calendar, files, issue trackers): call the \
matching integration tool. That data is live and is NOT in the saved material, so \
search_brain will never find it.\n\
- An instruction to DO something on a connected account — send an email, create \
an event — is an action, not a lookup: call the matching write tool directly with \
what they gave you. Do not search or list first to check whether it was already \
done unless they explicitly ask you to check; that only delays the thing they \
asked for and searching cannot substitute for acting.\n\
Searching when you did not need to wastes the user's time; guessing when a tool \
would have known is worse. Prefer one well-aimed call over three vague ones. If a \
tool call errors (including a permission/access error), do not paper over it by \
retrying with a different tool — say plainly, in your final message, what failed \
and why, so the user can fix it on their end.";

const ANSWER_STYLE: &str = "\
Writing the final message: plain text only, no markdown (no **bold**, #headings, \
or backticks). Cite facts that came from tool results as [n] for saved sources and \
[Wn] for web sources, matching the numbers in the tool output. Facts from \
integration tools are live account data, not numbered sources — report them \
directly (dates, times, names, subjects) with no citation marker. Never invent a \
citation number. If you searched and nothing relevant turned up, say so plainly \
instead of guessing. When a result already contains the concrete answer (a number, \
date, name, current status), lead with that fact directly — never deflect to \
'check the link' or describe what a page covers instead of just answering. If the \
results don't actually contain live data the question needs (e.g. current weather, \
a live price), say plainly that you don't have current data for it rather than \
writing around the gap.";

const WRAP_UP: &str = "\n\nYou have used your entire tool budget for this question. \
Reply now with final_response=true and no tool calls, answering from what you found \
above — or saying plainly that nothing relevant turned up.";

const REPAIR: &str = "\n\nYour last reply was not valid JSON. Reply again with ONLY \
the JSON object described above — no prose, no code fences.";

/// One decoded turn of the protocol.
#[derive(Debug, Default)]
struct AgentStep {
    reasoning: String,
    message: String,
    tool_calls: Vec<ToolCall>,
    final_response: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct ToolCall {
    #[serde(alias = "tool", alias = "tool_name")]
    name: String,
    #[serde(default, alias = "args", alias = "parameters", alias = "input")]
    arguments: serde_json::Value,
}

/// Models are loose with this contract: `"true"` and `1` show up as often as
/// `true`, and rejecting them would burn a repair turn over nothing.
fn lenient_bool(v: Option<&serde_json::Value>) -> bool {
    match v {
        Some(serde_json::Value::Bool(b)) => *b,
        Some(serde_json::Value::String(s)) => s.trim().eq_ignore_ascii_case("true"),
        Some(serde_json::Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        _ => false,
    }
}

/// Pull the step out of whatever the model wrapped it in — a bare object, a
/// ```json fence, or a sentence of preamble.
fn parse_step(raw: &str) -> Result<AgentStep> {
    let json = extract_json_object(raw).ok_or_else(|| anyhow!("no JSON object in model reply"))?;
    let v: serde_json::Value = serde_json::from_str(json)?;
    let obj = v.as_object().ok_or_else(|| anyhow!("not a JSON object"))?;

    let tool_calls: Vec<ToolCall> = obj
        .get("tool_calls")
        .filter(|v| !v.is_null())
        .map(|v| serde_json::from_value(v.clone()).unwrap_or_default())
        .unwrap_or_default();

    Ok(AgentStep {
        reasoning: string_field(obj, "reasoning"),
        message: string_field(obj, "message"),
        final_response: lenient_bool(obj.get("final_response")),
        tool_calls: tool_calls
            .into_iter()
            .filter(|c| !c.name.trim().is_empty())
            .map(|mut c| {
                // Some models hand back `arguments` as a JSON-encoded string
                // rather than an object; unwrap it before the tool sees it.
                if let Some(s) = c.arguments.as_str() {
                    if let Ok(v) = serde_json::from_str(s) {
                        c.arguments = v;
                    }
                }
                c
            })
            .collect(),
    })
}

fn string_field(obj: &serde_json::Map<String, serde_json::Value>, key: &str) -> String {
    obj.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// First balanced `{…}` run in the text, brace-counted outside of strings so a
/// `"}"` inside a message doesn't cut the object short.
fn extract_json_object(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    let start = s.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for i in start..bytes.len() {
        let c = bytes[i];
        if in_string {
            match c {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

const MEMORY_POLICY: &str = "\
Your CORE MEMORY above is you — it persists across every future conversation with this user, \
even on a different machine or a different underlying model. It isn't a suggestion to consult, \
it's the only place your identity and what you know about this user actually live. Keep it \
current: the moment you learn something durable about the user (name, role, preferences, an \
ongoing project) or about your own persona, call core_memory_append or core_memory_replace — \
don't wait to be asked, and don't let a block go stale or contradict what you just learned. \
For detailed or one-off facts too granular for core memory, use archival_memory_insert; \
retrieve those later with archival_memory_search rather than guessing.";

/// Render core memory blocks the way the model sees them: labelled,
/// budget-visible (so it knows when it's near the limit), always present
/// even before anything has been written.
fn render_memory(blocks: &[crate::db::agent_memory::MemoryBlock]) -> String {
    if blocks.is_empty() {
        return "(none yet)".to_string();
    }
    blocks
        .iter()
        .map(|b| {
            format!(
                "<{label}> ({used}/{limit} chars)\n{value}\n</{label}>",
                label = b.label,
                used = b.value.chars().count(),
                limit = b.char_limit,
                value = b.value,
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn system_prompt(
    tools: &ToolBox<'_>,
    scope: QueryScope,
    memory: &[crate::db::agent_memory::MemoryBlock],
) -> String {
    let audience = match scope {
        QueryScope::Personal => "one user's personal second brain",
        QueryScope::Group(_) => "a group's shared second brain",
    };
    // Tool arguments like calendar start/end times need an absolute date to
    // resolve "tomorrow" / "6pm IST" against — the model has no other way to
    // know today's date.
    let now = chrono::Utc::now().format("%A, %Y-%m-%d %H:%M UTC");
    format!(
        "You are Arnheid, a sharp research analyst wired into {audience}. You are talking \
         to the user in a chat app, so be direct and conversational. Right now it is {now} — \
         resolve any relative date or time in the user's request against this before calling \
         a tool that needs one.\n\n\
         CORE MEMORY:\n{memory}\n\n\
         {MEMORY_POLICY}\n\n\
         {DECISION_POLICY}\n\n\
         TOOLS AVAILABLE:\n{catalogue}\n\n\
         {ANSWER_STYLE}\n\n\
         {OUTPUT_CONTRACT}",
        catalogue = tools.catalogue(),
        memory = render_memory(memory),
    )
}

/// Run the agentic loop for one question. Returns `Err` on transport/API
/// failure or an unparseable model, so the caller (`handle_or_fallback`) can
/// fall back to the fixed pipeline — a broken agentic path never means a
/// broken `/ask`.
pub async fn handle(
    state: &AppState,
    chat_id: i64,
    user_id: i64,
    scope: QueryScope,
    web: super::WebMode,
    anchor: &super::QueryAnchor,
    query_text: &str,
) -> Result<(QueryReply, crate::metrics::AskOutcome)> {
    let query_text = query_text.trim();
    let ctx = ToolCtx {
        chat_id,
        user_id,
        scope,
    };
    let mut tools = ToolBox::new(state, ctx, web != super::WebMode::Off);
    let session = crate::db::sessions::get_or_create(&state.pool, user_id, chat_id)
        .await
        .ok();
    let recent_turns = if let Some(ref s) = session {
        crate::db::sessions::recent_turns(&state.pool, s.id)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    // Never blocks `/ask` on memory being unavailable — an empty block list
    // just means the model sees "(none yet)" and starts writing to it.
    let memory = crate::db::agent_memory::get_or_seed_blocks(&state.pool, user_id)
        .await
        .unwrap_or_default();
    let system = system_prompt(&tools, scope, &memory);

    let mut transcript = String::new();
    
    // Load recent group chat messages (surrounding context)
    let raw_msgs = if chat_id < 0 {
        let target_msg_id = anchor.query_message_id.unwrap_or(i64::MAX);
        crate::db::groups::messages_before(&state.pool, chat_id, target_msg_id, 10)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    if !raw_msgs.is_empty() {
        transcript.push_str("Surrounding group chat conversation context:\n");
        for m in &raw_msgs {
            let sender = m.username.as_deref().unwrap_or("User");
            transcript.push_str(&format!("{}: {}\n", sender, m.text));
        }
        transcript.push_str("\n");
    }

    if !recent_turns.is_empty() {
        transcript.push_str("Recent conversation history in this chat:\n");
        for t in &recent_turns {
            let role = if t.role == "user" { "User" } else { "Arnheid" };
            transcript.push_str(&format!("{role}: {}\n", t.text));
        }
        transcript.push_str("\n");
    }
    transcript.push_str(&format!("User question: {query_text}"));
    let mut called: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut repaired = false;
    let mut any_tool_called = false;

    for turn in 0..MAX_TURNS {
        let raw = state.chat.synthesize(&system, &transcript).await?;
        let step = match parse_step(&raw) {
            Ok(step) => step,
            // One repair attempt; a model that can't produce JSON twice is
            // not going to on the third try, so hand back to the pipeline.
            Err(e) if !repaired => {
                tracing::warn!(chat_id, user_id, error = %e, raw = %truncate(&raw, 400),
                    "agent reply was not valid JSON, asking for a repair");
                eprintln!(
                    "[warn] agent reply was not valid JSON, asking for a repair: chat_id={chat_id} user_id={user_id} error={e} raw={}",
                    truncate(&raw, 400)
                );
                repaired = true;
                transcript.push_str(REPAIR);
                continue;
            }
            Err(e) => return Err(anyhow!("agent protocol broken after repair: {e}")),
        };

        tracing::info!(
            chat_id,
            user_id,
            turn,
            reasoning = %step.reasoning,
            message = %truncate(&step.message, 200),
            tool_calls = %describe_calls(&step.tool_calls),
            final_response = step.final_response,
            "agent step"
        );
        println!(
            "[info] agent step: chat_id={chat_id} user_id={user_id} turn={turn} reasoning={} message={} tool_calls={} final_response={}",
            step.reasoning,
            truncate(&step.message, 200),
            describe_calls(&step.tool_calls),
            step.final_response
        );

        // No calls requested means the model is done, whatever the flag says
        // — honouring only `final_response` here would let a model that
        // forgot the flag spin until the turn budget ran out.
        if step.final_response || step.tool_calls.is_empty() {
            if !step.message.is_empty() {
                let reply = build_reply(&step.message, &tools.sources);
                let outcome = outcome_of(&reply, tools.integration_used(), any_tool_called);
                
                let next_state = crate::models::ThreadState {
                    active_item_ids: reply.cited_item_ids.clone(),
                    mode: crate::models::ThreadMode::Open,
                    last_intent: Some(query_text.chars().take(120).collect()),
                };
                if let Some(ref s) = session {
                    let user_item_ids = vec![];
                    if let Err(e) = crate::db::sessions::append_exchange(
                        &state.pool,
                        s.id,
                        query_text,
                        &user_item_ids,
                        &reply.html,
                        &reply.cited_item_ids,
                        &next_state,
                    )
                    .await {
                        tracing::warn!(error = %e, "agent failed to append session turn");
                        eprintln!("[warn] agent failed to append session turn: error={e}");
                    }
                }
                return Ok((reply, outcome));
            }
            tracing::warn!(chat_id, user_id, turn, "agent finished with an empty message");
            eprintln!("[warn] agent finished with an empty message: chat_id={chat_id} user_id={user_id} turn={turn}");
            transcript.push_str(WRAP_UP);
            continue;
        }

        for call in step.tool_calls.iter().take(MAX_CALLS_PER_TURN) {
            let key = format!("{}:{}", call.name, call.arguments);
            let result = if !called.insert(key) {
                "Error: you already made this exact call — use the result above, \
                 refine the arguments, or answer now."
                    .to_string()
            } else {
                any_tool_called = true;
                tools.call(&call.name, &call.arguments).await
            };
            transcript.push_str(&format!(
                "\n\nTool call: {name}({args})\nResult:\n{result}",
                name = call.name,
                args = call.arguments,
                result = truncate(&result, MAX_RESULT_CHARS),
            ));
        }
        if step.tool_calls.len() > MAX_CALLS_PER_TURN {
            transcript.push_str(&format!(
                "\n\n(Only the first {MAX_CALLS_PER_TURN} calls of that turn ran. \
                 Ask again next turn if you still need the rest.)"
            ));
        }
    }

    // Budget exhausted: one last turn with the wrap-up instruction. Whatever
    // comes back is the answer, JSON or not — the user gets something.
    transcript.push_str(WRAP_UP);
    let raw = state.chat.synthesize(&system, &transcript).await?;
    let text = match parse_step(&raw) {
        Ok(step) if !step.message.is_empty() => step.message,
        _ => raw.trim().to_string(),
    };
    tracing::info!(chat_id, user_id, "agent hit the turn budget, wrapped up");
    println!("[info] agent hit the turn budget, wrapped up: chat_id={chat_id} user_id={user_id}");
    let reply = build_reply(&text, &tools.sources);
    let outcome = outcome_of(&reply, tools.integration_used(), any_tool_called);

    let next_state = crate::models::ThreadState {
        active_item_ids: reply.cited_item_ids.clone(),
        mode: crate::models::ThreadMode::Open,
        last_intent: Some(query_text.chars().take(120).collect()),
    };
    if let Some(ref s) = session {
        let user_item_ids = vec![];
        if let Err(e) = crate::db::sessions::append_exchange(
            &state.pool,
            s.id,
            query_text,
            &user_item_ids,
            &reply.html,
            &reply.cited_item_ids,
            &next_state,
        )
        .await {
            tracing::warn!(error = %e, "agent failed to append session turn");
            eprintln!("[warn] agent failed to append session turn: error={e}");
        }
    }
    Ok((reply, outcome))
}

fn describe_calls(calls: &[ToolCall]) -> String {
    if calls.is_empty() {
        return "none".to_string();
    }
    calls
        .iter()
        .map(|c| format!("{}({})", c.name, truncate(&c.arguments.to_string(), 200)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A cited source means an answer; so does live integration content, which is
/// never cited; so does a direct reply that needed no tools at all. Only a
/// search that came back empty is `NoResults`.
fn outcome_of(
    reply: &QueryReply,
    integration_used: bool,
    any_tool_called: bool,
) -> crate::metrics::AskOutcome {
    if !reply.cited_item_ids.is_empty() || integration_used || !any_tool_called {
        crate::metrics::AskOutcome::Answered
    } else {
        crate::metrics::AskOutcome::NoResults
    }
}

fn build_reply(text: &str, sources: &Sources) -> QueryReply {
    if text.is_empty() {
        return QueryReply {
            html: "Nothing relevant turned up.".to_string(),
            cited_item_ids: vec![],
        };
    }
    let cited = cited_in_text(text, sources.items.len());
    let web_cited = cited_web_in_text(text, sources.web.len());
    let cited_item_ids: Vec<Uuid> = cited
        .iter()
        .filter_map(|&n| sources.items.get(n - 1).map(|i| i.id))
        .collect();
    let html =
        crate::bot::formatter::query_answer(text, &sources.items, &cited, &sources.web, &web_cited);
    QueryReply {
        html,
        cited_item_ids,
    }
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
    fn parses_a_plain_step() {
        let step = parse_step(
            r#"{"reasoning":"casual hello","message":"Hey! What's up?","tool_calls":[],"final_response":true}"#,
        )
        .unwrap();
        assert!(step.final_response);
        assert_eq!(step.message, "Hey! What's up?");
        assert!(step.tool_calls.is_empty());
    }

    #[test]
    fn parses_through_fences_and_preamble() {
        let step = parse_step(
            "Sure! Here's my step:\n```json\n{\"reasoning\":\"need the brain\",\
             \"tool_calls\":[{\"name\":\"search_brain\",\"arguments\":{\"query\":\"rust async\"}}],\
             \"final_response\":false}\n```\nhope that helps",
        )
        .unwrap();
        assert!(!step.final_response);
        assert_eq!(step.tool_calls.len(), 1);
        assert_eq!(step.tool_calls[0].name, "search_brain");
        assert_eq!(step.tool_calls[0].arguments["query"], "rust async");
    }

    #[test]
    fn accepts_loose_field_shapes() {
        // "true" as a string, `args` instead of `arguments`, `tool` instead of `name`.
        let step = parse_step(
            r#"{"message":"done","tool_calls":null,"final_response":"true"}"#,
        )
        .unwrap();
        assert!(step.final_response);

        let step =
            parse_step(r#"{"tool_calls":[{"tool":"web_search","args":{"query":"x"}}],"final_response":false}"#)
                .unwrap();
        assert_eq!(step.tool_calls[0].name, "web_search");
        assert_eq!(step.tool_calls[0].arguments["query"], "x");
    }

    #[test]
    fn braces_inside_strings_dont_truncate_the_object() {
        let step = parse_step(r#"{"message":"use {this} literally","final_response":true}"#).unwrap();
        assert_eq!(step.message, "use {this} literally");
    }

    #[test]
    fn nameless_calls_are_dropped() {
        let step =
            parse_step(r#"{"tool_calls":[{"name":"  ","arguments":{}}],"final_response":false}"#)
                .unwrap();
        assert!(step.tool_calls.is_empty());
    }

    #[test]
    fn prose_only_reply_is_an_error() {
        assert!(parse_step("I think the answer is 42.").is_err());
    }

    #[test]
    fn direct_answers_count_as_answered() {
        let reply = QueryReply {
            html: "hi".to_string(),
            cited_item_ids: vec![],
        };
        assert_eq!(
            outcome_of(&reply, false, false),
            crate::metrics::AskOutcome::Answered
        );
        // Searched, cited nothing, no integration → genuinely empty-handed.
        assert_eq!(
            outcome_of(&reply, false, true),
            crate::metrics::AskOutcome::NoResults
        );
    }

    #[test]
    fn build_reply_with_no_text_says_so() {
        let reply = build_reply("", &Sources::new());
        assert_eq!(reply.html, "Nothing relevant turned up.");
        assert!(reply.cited_item_ids.is_empty());
    }
}
