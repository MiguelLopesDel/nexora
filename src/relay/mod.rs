//! `nexora relay`: a local OpenAI-compatible intermediary that gives any
//! upstream provider (DeepSeek, OpenRouter, Ollama, …) capabilities their
//! APIs lack server-side — web search with page reading, conversation
//! compaction, and prompt enrichment — so a small or remote model behaves
//! closer to a full assistant. Point any OpenAI-compatible client at
//! `http://127.0.0.1:<port>/v1`.

mod http;
mod search;

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use crate::config::{Config, ProviderConfig, RelayConfig};

const PAGE_EXTRACT_CHARS: usize = 4_000;

struct State {
    relay: RelayConfig,
    upstream: ProviderConfig,
    backend: search::Backend,
    client: reqwest::Client,
    /// Compaction summaries keyed by a hash of the summarized turns, so a
    /// growing conversation only pays for each stretch once.
    summaries: Mutex<HashMap<u64, String>>,
}

pub async fn serve(config: Config) -> Result<()> {
    let relay = config.relay.clone();
    let upstream = config
        .providers
        .get(&relay.upstream)
        .cloned()
        .with_context(|| {
            format!(
                "[relay] upstream provider `{}` is not configured",
                relay.upstream
            )
        })?;
    if upstream.kind != crate::config::ProviderKind::Openai {
        bail!("[relay] upstream must be an OpenAI-compatible provider");
    }
    let backend = search::resolve(&relay)?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .context("could not build upstream HTTP client")?;

    let listener = TcpListener::bind(("127.0.0.1", relay.port))
        .await
        .with_context(|| format!("could not bind 127.0.0.1:{}", relay.port))?;
    println!(
        "nexora relay listening on http://127.0.0.1:{}/v1 → upstream `{}` ({}) · search: {} · compaction over {} chars",
        relay.port,
        relay.upstream,
        upstream.base_url(),
        backend.label(),
        relay.compact_over_chars,
    );

    let state = Arc::new(State {
        relay,
        upstream,
        backend,
        client,
        summaries: Mutex::new(HashMap::new()),
    });
    loop {
        let (stream, _) = listener.accept().await.context("accept failed")?;
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(err) = handle_connection(state, stream).await {
                eprintln!("relay: {err:#}");
            }
        });
    }
}

async fn handle_connection(state: Arc<State>, mut stream: tokio::net::TcpStream) -> Result<()> {
    let request = http::read_request(&mut stream).await?;
    match (request.method.as_str(), request.path.as_str()) {
        ("POST", path) if path.ends_with("/chat/completions") => {
            let body: Value = match serde_json::from_slice(&request.body) {
                Ok(body) => body,
                Err(err) => {
                    let error = json!({"error": {"message": format!("invalid JSON: {err}")}});
                    return http::respond_json(&mut stream, 400, &error.to_string()).await;
                }
            };
            match handle_chat(&state, &body).await {
                Ok(answer) => respond_chat(&mut stream, &body, &answer).await,
                Err(err) => {
                    let error = json!({"error": {"message": format!("{err:#}")}});
                    http::respond_json(&mut stream, 500, &error.to_string()).await
                }
            }
        }
        ("GET", path) if path.ends_with("/models") => {
            let model = state
                .upstream
                .default_model
                .clone()
                .unwrap_or_else(|| "relay".into());
            let body = json!({"object": "list", "data": [{"id": model, "object": "model", "owned_by": "nexora-relay"}]});
            http::respond_json(&mut stream, 200, &body.to_string()).await
        }
        _ => {
            let error = json!({"error": {"message": "unknown route"}});
            http::respond_json(&mut stream, 404, &error.to_string()).await
        }
    }
}

/// Run the full pipeline for one request and return the final answer text.
async fn handle_chat(state: &State, body: &Value) -> Result<String> {
    let model = requested_model(state, body);
    let mut messages = normalized_messages(body)?;
    let compacted = compact(state, &model, &mut messages).await?;
    enrich(state, &mut messages);

    let searching = !matches!(state.backend, search::Backend::Off);
    println!(
        "relay: {} message(s), model {model}{}{}",
        messages.len(),
        if compacted { ", history compacted" } else { "" },
        if searching { "" } else { ", search off" },
    );

    let mut tools_enabled = searching;
    let mut rounds = 0_u32;
    loop {
        let offer_tools = tools_enabled && rounds < state.relay.max_search_rounds;
        let response = match upstream_chat(state, &model, &messages, offer_tools, body).await {
            Ok(response) => response,
            // Some OpenAI-compatible servers reject the tools parameter
            // entirely; degrade to a plain relay rather than failing.
            Err(err) if offer_tools && format!("{err:#}").contains("tool") => {
                tools_enabled = false;
                upstream_chat(state, &model, &messages, false, body).await?
            }
            Err(err) => return Err(err),
        };

        let message = response["choices"][0]["message"].clone();
        let calls = message["tool_calls"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if calls.is_empty() || !offer_tools {
            return Ok(message["content"].as_str().unwrap_or_default().to_string());
        }

        rounds += 1;
        messages.push(message);
        for call in calls {
            let id = call["id"].as_str().unwrap_or_default().to_string();
            let result = run_tool(state, &call).await;
            let content = result.unwrap_or_else(|err| format!("tool failed: {err:#}"));
            messages.push(json!({"role": "tool", "tool_call_id": id, "content": content}));
        }
    }
}

fn requested_model(state: &State, body: &Value) -> String {
    let requested = body["model"].as_str().unwrap_or_default().trim();
    if requested.is_empty() || requested == "relay" {
        state.upstream.default_model.clone().unwrap_or_default()
    } else {
        requested.to_string()
    }
}

/// Flatten message content to plain strings (multimodal parts keep only
/// text; the relay targets text upstreams).
fn normalized_messages(body: &Value) -> Result<Vec<Value>> {
    let raw = body["messages"]
        .as_array()
        .context("request has no messages array")?;
    let mut messages = Vec::with_capacity(raw.len());
    for message in raw {
        let role = message["role"].as_str().unwrap_or("user");
        let content = match &message["content"] {
            Value::String(text) => text.clone(),
            Value::Array(parts) => parts
                .iter()
                .filter_map(|part| part["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };
        messages.push(json!({"role": role, "content": content}));
    }
    Ok(messages)
}

/// Keep the system prompt and the recent turns verbatim; summarize the long
/// middle once the conversation outgrows the configured budget.
async fn compact(state: &State, model: &str, messages: &mut Vec<Value>) -> Result<bool> {
    let budget = state.relay.compact_over_chars;
    if budget == 0 || total_chars(messages) <= budget {
        return Ok(false);
    }

    let system_end = messages
        .iter()
        .position(|message| message["role"] != "system")
        .unwrap_or(0);
    // Walk back from the end until the kept tail uses half the budget.
    let mut tail_start = messages.len();
    let mut tail_chars = 0_usize;
    while tail_start > system_end {
        let next = content_len(&messages[tail_start - 1]);
        if tail_chars + next > budget / 2 && tail_start < messages.len() {
            break;
        }
        tail_start -= 1;
        tail_chars += next;
    }
    if tail_start <= system_end + 1 {
        return Ok(false);
    }

    let middle: Vec<Value> = messages[system_end..tail_start].to_vec();
    let key = hash_messages(&middle);
    let cached = state.summaries.lock().await.get(&key).cloned();
    let summary = match cached {
        Some(summary) => summary,
        None => {
            let transcript = middle
                .iter()
                .map(|message| {
                    format!(
                        "{}: {}",
                        message["role"].as_str().unwrap_or("user"),
                        message["content"].as_str().unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let prompt = format!(
                "Summarize this conversation compactly for use as context. Keep decisions, facts, names, numbers, and open questions. Do not invent details.\n\n{transcript}"
            );
            let request = vec![json!({"role": "user", "content": prompt})];
            let response = upstream_chat(state, model, &request, false, &Value::Null).await?;
            let summary = response["choices"][0]["message"]["content"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            state.summaries.lock().await.insert(key, summary.clone());
            summary
        }
    };

    let mut compacted = messages[..system_end].to_vec();
    compacted.push(json!({
        "role": "system",
        "content": format!("Summary of the earlier conversation (compacted to fit context):\n{summary}"),
    }));
    compacted.extend_from_slice(&messages[tail_start..]);
    *messages = compacted;
    Ok(true)
}

fn total_chars(messages: &[Value]) -> usize {
    messages.iter().map(content_len).sum()
}

fn content_len(message: &Value) -> usize {
    message["content"]
        .as_str()
        .map_or(0, |text| text.chars().count())
}

fn hash_messages(messages: &[Value]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for message in messages {
        message.to_string().hash(&mut hasher);
    }
    hasher.finish()
}

/// Give the model what it needs to use its tools well: today's date and
/// clear guidance on when to search and how to cite.
fn enrich(state: &State, messages: &mut Vec<Value>) {
    let mut note = format!(
        "Today's date is {} (UTC).",
        crate::conversation::utc_date_string()
    );
    if !matches!(state.backend, search::Backend::Off) {
        note.push_str(
            " You can call the web_search tool. Use it for anything recent, niche, or uncertain instead of guessing; cite source URLs briefly in the answer. For questions you can answer confidently from general knowledge, answer directly without searching.",
        );
    }
    if let Some(first) = messages.first_mut()
        && first["role"] == "system"
    {
        let existing = first["content"].as_str().unwrap_or_default();
        first["content"] = Value::String(format!("{existing}\n\n{note}"));
        return;
    }
    messages.insert(0, json!({"role": "system", "content": note}));
}

fn web_search_tool() -> Value {
    json!([{
        "type": "function",
        "function": {
            "name": "web_search",
            "description": "Search the live web. Returns result titles, URLs, snippets, and an extract of the top page.",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "The search query"}
                },
                "required": ["query"]
            }
        }
    }])
}

async fn run_tool(state: &State, call: &Value) -> Result<String> {
    let name = call["function"]["name"].as_str().unwrap_or_default();
    if name != "web_search" {
        bail!("unknown tool `{name}`");
    }
    let arguments = call["function"]["arguments"].as_str().unwrap_or("{}");
    let arguments: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    let query = arguments["query"].as_str().unwrap_or_default();
    if query.is_empty() {
        bail!("web_search called without a query");
    }
    println!("relay: web_search({query:?}) via {}", state.backend.label());

    let hits = search::search(&state.backend, query, state.relay.max_results).await?;
    if hits.is_empty() {
        return Ok(format!("No results for \"{query}\"."));
    }
    let mut result = format!("Results for \"{query}\":\n");
    for (index, hit) in hits.iter().enumerate() {
        result.push_str(&format!(
            "{}. {} — {}\n   {}\n",
            index + 1,
            hit.title,
            hit.url,
            hit.snippet
        ));
    }
    if state.relay.fetch_pages
        && let Some(top) = hits.first()
    {
        match search::fetch_page_text(&top.url, PAGE_EXTRACT_CHARS).await {
            Ok(text) if !text.is_empty() => {
                result.push_str(&format!("\nExtract of {}:\n{}\n", top.url, text));
            }
            Ok(_) => {}
            Err(err) => result.push_str(&format!("\n(top page could not be read: {err:#})\n")),
        }
    }
    Ok(result)
}

/// One non-streaming call to the upstream provider. Streaming to the client
/// is produced by the relay itself after the final answer is known.
async fn upstream_chat(
    state: &State,
    model: &str,
    messages: &[Value],
    offer_tools: bool,
    original: &Value,
) -> Result<Value> {
    let mut request = json!({
        "model": model,
        "messages": messages,
        "stream": false,
    });
    if offer_tools {
        request["tools"] = web_search_tool();
    }
    for passthrough in ["max_tokens", "temperature", "top_p"] {
        if !original[passthrough].is_null() {
            request[passthrough] = original[passthrough].clone();
        }
    }

    let url = format!("{}/chat/completions", state.upstream.base_url());
    let mut call = state.client.post(&url).json(&request);
    if let Ok(key) = state.upstream.resolve_api_key() {
        call = call.bearer_auth(key);
    }
    let response = call.send().await.context("upstream request failed")?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        let excerpt: String = body.chars().take(400).collect();
        bail!("upstream returned {status}: {excerpt}");
    }
    serde_json::from_str(&body).context("upstream sent invalid JSON")
}

/// Answer the client in whichever shape it asked for.
async fn respond_chat(
    stream: &mut tokio::net::TcpStream,
    body: &Value,
    answer: &str,
) -> Result<()> {
    let model = body["model"].as_str().unwrap_or("relay");
    let id = format!(
        "chatcmpl-relay-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    if body["stream"].as_bool().unwrap_or(false) {
        http::start_sse(stream).await?;
        let first = json!({
            "id": id, "object": "chat.completion.chunk", "created": created, "model": model,
            "choices": [{"index": 0, "delta": {"role": "assistant"}, "finish_reason": null}]
        });
        http::sse_event(stream, &first.to_string()).await?;
        // Chunked delivery keeps clients' incremental rendering working.
        let characters: Vec<char> = answer.chars().collect();
        for piece in characters.chunks(400) {
            let text: String = piece.iter().collect();
            let chunk = json!({
                "id": id, "object": "chat.completion.chunk", "created": created, "model": model,
                "choices": [{"index": 0, "delta": {"content": text}, "finish_reason": null}]
            });
            http::sse_event(stream, &chunk.to_string()).await?;
        }
        let last = json!({
            "id": id, "object": "chat.completion.chunk", "created": created, "model": model,
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
        });
        http::sse_event(stream, &last.to_string()).await?;
        http::sse_event(stream, "[DONE]").await
    } else {
        let response = json!({
            "id": id, "object": "chat.completion", "created": created, "model": model,
            "choices": [{"index": 0, "message": {"role": "assistant", "content": answer}, "finish_reason": "stop"}],
        });
        http::respond_json(stream, 200, &response.to_string()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(role: &str, content: &str) -> Value {
        json!({"role": role, "content": content})
    }

    #[test]
    fn multimodal_content_is_flattened_to_text() {
        let body = json!({"messages": [
            {"role": "user", "content": [
                {"type": "text", "text": "look"},
                {"type": "image_url", "image_url": {"url": "data:..."}},
                {"type": "text", "text": "closer"}
            ]}
        ]});
        let messages = normalized_messages(&body).unwrap();
        assert_eq!(messages[0]["content"], "look\ncloser");
    }

    #[test]
    fn compaction_hash_is_stable_per_prefix() {
        let a = vec![message("user", "hello"), message("assistant", "hi")];
        let b = vec![message("user", "hello"), message("assistant", "hi")];
        assert_eq!(hash_messages(&a), hash_messages(&b));
        let c = vec![message("user", "hello"), message("assistant", "yo")];
        assert_ne!(hash_messages(&a), hash_messages(&c));
    }
}
