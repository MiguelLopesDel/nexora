//! Anthropic Messages API (streaming).

use anyhow::Result;
use async_channel::Sender;
use base64::Engine;
use serde_json::{Value, json};

use super::{ChatRequest, StreamEvent, check_status, consume_sse, http_client};
use crate::config::ProviderConfig;

pub fn build_body(request: &ChatRequest) -> Value {
    let mut content = Vec::new();
    if let Some(png) = &request.image_png {
        content.push(json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": "image/png",
                "data": base64::engine::general_purpose::STANDARD.encode(png),
            }
        }));
    }
    content.push(json!({ "type": "text", "text": request.prompt }));

    let mut body = json!({
        "model": request.model,
        "max_tokens": request.max_tokens,
        "stream": true,
        "messages": [{ "role": "user", "content": content }],
    });
    if let Some(system) = &request.system {
        body["system"] = json!(system);
    }
    body
}

pub async fn stream(
    provider: &ProviderConfig,
    request: &ChatRequest,
    tx: &Sender<StreamEvent>,
) -> Result<()> {
    let url = format!("{}/v1/messages", provider.base_url());
    let response = http_client()?
        .post(url)
        .header("x-api-key", provider.resolve_api_key()?)
        .header("anthropic-version", "2023-06-01")
        .json(&build_body(request))
        .send()
        .await?;
    let response = check_status(response).await?;

    consume_sse(response, |event| {
        match event.event.as_deref() {
            Some("content_block_delta") => {
                let value: Value = serde_json::from_str(&event.data)?;
                if let Some(text) = value["delta"]["text"].as_str() {
                    let _ = tx.try_send(StreamEvent::Delta(text.to_string()));
                }
            }
            Some("error") => {
                let value: Value = serde_json::from_str(&event.data)?;
                let message = value["error"]["message"].as_str().unwrap_or(&event.data);
                anyhow::bail!("Anthropic stream error: {message}");
            }
            _ => {}
        }
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_with_image() -> ChatRequest {
        ChatRequest {
            model: "claude-sonnet-5".into(),
            system: Some("be brief".into()),
            prompt: "what is this?".into(),
            image_png: Some(vec![1, 2, 3]),
            max_tokens: 64,
        }
    }

    #[test]
    fn body_includes_image_block_before_text() {
        let body = build_body(&request_with_image());
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "image");
        assert_eq!(content[0]["source"]["media_type"], "image/png");
        assert_eq!(content[1]["type"], "text");
        assert_eq!(body["system"], "be brief");
        assert_eq!(body["stream"], true);
    }

    #[test]
    fn body_omits_system_when_absent() {
        let mut request = request_with_image();
        request.system = None;
        request.image_png = None;
        let body = build_body(&request);
        assert!(body.get("system").is_none());
        assert_eq!(body["messages"][0]["content"].as_array().unwrap().len(), 1);
    }
}
