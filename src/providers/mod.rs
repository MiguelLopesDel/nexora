//! Provider-agnostic chat streaming.

mod anthropic;
mod openai;
mod sse;

use anyhow::Result;
use async_channel::Sender;

use crate::config::{ProviderConfig, ProviderKind, TaskConfig};

/// One user turn. The MVP is single-shot (no conversation history yet).
#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    pub system: Option<String>,
    pub prompt: String,
    /// PNG screenshot to attach, if any.
    pub image_png: Option<Vec<u8>>,
    pub max_tokens: u32,
}

impl ChatRequest {
    pub fn from_task(task: &TaskConfig, prompt: String, image_png: Option<Vec<u8>>) -> Self {
        Self {
            model: task.model.clone(),
            system: task.system.clone(),
            prompt,
            image_png,
            max_tokens: task.max_tokens,
        }
    }
}

#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// A chunk of assistant text.
    Delta(String),
    /// Stream finished successfully.
    Done,
    /// Stream failed; message is user-facing.
    Error(String),
}

/// Stream a chat completion, forwarding events to `tx`.
///
/// Always terminates the channel with `Done` or `Error`.
pub async fn stream_chat(provider: &ProviderConfig, request: ChatRequest, tx: Sender<StreamEvent>) {
    let result = match provider.kind {
        ProviderKind::Anthropic => anthropic::stream(provider, &request, &tx).await,
        ProviderKind::Openai => openai::stream(provider, &request, &tx).await,
    };
    let last = match result {
        Ok(()) => StreamEvent::Done,
        Err(err) => StreamEvent::Error(format!("{err:#}")),
    };
    let _ = tx.send(last).await;
}

fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()?)
}

/// Read an SSE response body, invoking `on_event` per event.
async fn consume_sse(
    response: reqwest::Response,
    mut on_event: impl FnMut(sse::SseEvent) -> Result<()>,
) -> Result<()> {
    use futures_util::StreamExt;

    let mut parser = sse::SseParser::new();
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        for event in parser.push(&chunk?) {
            on_event(event)?;
        }
    }
    Ok(())
}

/// Turn a non-2xx response into a readable error with body excerpt.
async fn check_status(response: reqwest::Response) -> Result<reqwest::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    let excerpt: String = body.chars().take(400).collect();
    anyhow::bail!("API returned {status}: {excerpt}")
}
