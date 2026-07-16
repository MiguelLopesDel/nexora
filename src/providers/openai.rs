//! OpenAI-compatible Chat Completions API (streaming).
//!
//! Covers OpenAI, OpenRouter, DeepSeek, Gemini's compatibility endpoint,
//! Ollama, llama.cpp and anything else speaking this dialect.

use anyhow::Result;
use async_channel::Sender;
use base64::Engine;
use serde_json::{Value, json};

use super::{ChatRequest, StreamEvent, check_status, consume_sse, http_client};
use crate::config::ProviderConfig;
use crate::conversation::Role;

pub fn build_body(request: &ChatRequest) -> Value {
    let mut messages = Vec::new();
    if let Some(system) = &request.system {
        messages.push(json!({ "role": "system", "content": system }));
    }

    let last = request.messages.len().saturating_sub(1);
    for (i, (role, text)) in request.messages.iter().enumerate() {
        // Plain string content has the widest server compatibility; only use
        // the multimodal array form for the final turn when an image is set.
        let content = match (i == last, &request.image_png) {
            (true, Some(png)) => {
                let data_uri = format!(
                    "data:image/png;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(png)
                );
                json!([
                    { "type": "text", "text": text },
                    { "type": "image_url", "image_url": { "url": data_uri } },
                ])
            }
            _ => json!(text),
        };
        messages.push(json!({ "role": role_str(*role), "content": content }));
    }

    json!({
        "model": request.model,
        "max_tokens": request.max_tokens,
        "stream": true,
        "messages": messages,
    })
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

pub async fn stream(
    provider: &ProviderConfig,
    request: &ChatRequest,
    tx: &Sender<StreamEvent>,
) -> Result<()> {
    let url = format!("{}/chat/completions", provider.base_url());
    let mut body = build_body(request);
    apply_reasoning_options(provider, &mut body);
    let response = http_client()?
        .post(url)
        .bearer_auth(provider.resolve_api_key()?)
        .json(&body)
        .send()
        .await?;
    let response = check_status(response).await?;

    consume_sse(response, |event| {
        if event.data == "[DONE]" {
            return Ok(());
        }
        let value: Value = serde_json::from_str(&event.data)?;
        if let Some(message) = value["error"]["message"].as_str() {
            anyhow::bail!("API stream error: {message}");
        }
        if let Some(text) = value["choices"][0]["delta"]["content"].as_str() {
            let _ = tx.try_send(StreamEvent::Delta(text.to_string()));
        }
        Ok(())
    })
    .await
}

fn apply_reasoning_options(provider: &ProviderConfig, body: &mut Value) {
    if let Some(thinking) = provider.thinking {
        body["thinking"] = json!({
            "type": if thinking { "enabled" } else { "disabled" }
        });
    }
    // An effort level is meaningless when thinking is explicitly disabled,
    // and some OpenAI-compatible providers reject the contradictory pair.
    if provider.thinking != Some(false)
        && let Some(effort) = &provider.reasoning_effort
    {
        body["reasoning_effort"] = json!(effort);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_only_uses_string_content() {
        let request = ChatRequest {
            model: "gpt-test".into(),
            system: None,
            messages: vec![(Role::User, "hi".into())],
            image_png: None,
            max_tokens: 32,
        };
        let body = build_body(&request);
        assert_eq!(body["messages"][0]["content"], "hi");
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn image_uses_multimodal_array_and_system_message() {
        let request = ChatRequest {
            model: "gpt-test".into(),
            system: Some("sys".into()),
            messages: vec![(Role::User, "what?".into())],
            image_png: Some(vec![9]),
            max_tokens: 32,
        };
        let body = build_body(&request);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages[0]["role"], "system");
        let content = messages[1]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "text");
        assert!(
            content[1]["image_url"]["url"]
                .as_str()
                .unwrap()
                .starts_with("data:image/png;base64,")
        );
    }

    #[test]
    fn multi_turn_history_maps_roles_in_order() {
        let request = ChatRequest {
            model: "gpt-test".into(),
            system: Some("sys".into()),
            messages: vec![
                (Role::User, "a".into()),
                (Role::Assistant, "b".into()),
                (Role::User, "c".into()),
            ],
            image_png: None,
            max_tokens: 32,
        };
        let messages = build_body(&request);
        let messages = messages["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 4); // system + 3 turns
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[2]["role"], "assistant");
        assert_eq!(messages[3]["content"], "c");
    }

    #[test]
    fn meeting_transcript_context_reaches_request_body() {
        let contextual_prompt = "sobre o que eles tao conversando?\n\nLive meeting transcript (most recent speech, for context):\nQual que é a pessoa?";
        let request = ChatRequest {
            model: "gpt-test".into(),
            system: None,
            messages: vec![(Role::User, contextual_prompt.into())],
            image_png: None,
            max_tokens: 32,
        };

        let body = build_body(&request);

        assert_eq!(body["messages"][0]["content"], contextual_prompt);
    }

    #[test]
    fn disabled_thinking_omits_stale_reasoning_effort() {
        let provider = ProviderConfig {
            kind: crate::config::ProviderKind::Openai,
            base_url: None,
            api_key: None,
            api_key_env: None,
            default_model: None,
            thinking: Some(false),
            reasoning_effort: Some("high".into()),
        };
        let mut body = json!({});

        apply_reasoning_options(&provider, &mut body);

        assert_eq!(body["thinking"]["type"], "disabled");
        assert!(body.get("reasoning_effort").is_none());
    }
}
