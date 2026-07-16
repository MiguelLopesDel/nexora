//! Screen understanding and local Ollama vision-model management.

use anyhow::{Context, Result, bail};
use async_channel::Sender;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::config::ProviderConfig;
use crate::conversation::Role;
use crate::providers::{ChatRequest, complete_chat};

#[derive(Debug, Clone, Copy)]
pub struct VisionModelPreset {
    pub id: &'static str,
    pub download: &'static str,
    pub size: &'static str,
    pub description: &'static str,
}

pub const PRESETS: &[VisionModelPreset] = &[
    VisionModelPreset {
        id: "qwen3-vl:2b",
        download: "1.9 GB",
        size: "Light",
        description: "Fast screen descriptions and OCR on modest hardware",
    },
    VisionModelPreset {
        id: "qwen3-vl:4b",
        download: "3.3 GB",
        size: "Recommended",
        description: "Best default balance for UI understanding and OCR",
    },
    VisionModelPreset {
        id: "qwen3-vl:8b",
        download: "6.1 GB",
        size: "Quality",
        description: "More accurate small text and complex screen layouts",
    },
    VisionModelPreset {
        id: "minicpm-v",
        download: "5.5 GB",
        size: "OCR",
        description: "Strong OCR-focused alternative for dense documents",
    },
    VisionModelPreset {
        id: "moondream",
        download: "1.7 GB",
        size: "Ultra-light",
        description: "Very fast, but less reliable on dense or nuanced screens",
    },
];

#[derive(Debug, Clone)]
pub struct InstalledModel {
    pub name: String,
    pub bytes: u64,
}

#[derive(Debug, Clone)]
pub struct PullProgress {
    pub status: String,
    pub completed: Option<u64>,
    pub total: Option<u64>,
}

pub async fn describe_screen(
    provider: &ProviderConfig,
    model: &str,
    prompt: &str,
    png: Vec<u8>,
) -> Result<String> {
    let request = ChatRequest {
        model: model.to_string(),
        system: Some(
            "You are a private screen-understanding component. Return only a compact, factual description and OCR text for another assistant."
                .into(),
        ),
        messages: vec![(Role::User, prompt.to_string())],
        image_png: Some(png),
        max_tokens: 1200,
    };
    let description = complete_chat(provider, request).await?;
    if description.trim().is_empty() {
        bail!("vision model returned an empty screen description")
    }
    Ok(description.trim().to_string())
}

pub async fn list_ollama_models(base_url: &str) -> Result<Vec<InstalledModel>> {
    let response = reqwest::Client::new()
        .get(format!("{}/api/tags", ollama_root(base_url)))
        .send()
        .await
        .context("could not connect to Ollama")?;
    let response = checked(response).await?;
    let value: Value = response.json().await?;
    let mut models: Vec<InstalledModel> = value["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|model| {
            Some(InstalledModel {
                name: model["name"].as_str()?.to_string(),
                bytes: model["size"].as_u64().unwrap_or(0),
            })
        })
        .collect();
    models.sort_by_key(|model| model.name.to_ascii_lowercase());
    Ok(models)
}

pub async fn pull_ollama_model(
    base_url: &str,
    model: &str,
    progress: Sender<PullProgress>,
) -> Result<()> {
    let response = reqwest::Client::new()
        .post(format!("{}/api/pull", ollama_root(base_url)))
        .json(&json!({ "model": model, "stream": true }))
        .send()
        .await
        .context("could not connect to Ollama")?;
    let response = checked(response).await?;
    let mut stream = response.bytes_stream();
    let mut pending = String::new();
    while let Some(chunk) = stream.next().await {
        pending.push_str(&String::from_utf8_lossy(&chunk?));
        while let Some(newline) = pending.find('\n') {
            let line = pending[..newline].trim().to_string();
            pending.drain(..=newline);
            if line.is_empty() {
                continue;
            }
            let update: PullUpdate = serde_json::from_str(&line)
                .with_context(|| format!("invalid Ollama progress response: {line}"))?;
            if let Some(error) = update.error {
                bail!("Ollama pull failed: {error}")
            }
            let _ = progress
                .send(PullProgress {
                    status: update.status.unwrap_or_else(|| "downloading".into()),
                    completed: update.completed,
                    total: update.total,
                })
                .await;
        }
    }
    Ok(())
}

pub async fn delete_ollama_model(base_url: &str, model: &str) -> Result<()> {
    let response = reqwest::Client::new()
        .delete(format!("{}/api/delete", ollama_root(base_url)))
        .json(&json!({ "model": model }))
        .send()
        .await
        .context("could not connect to Ollama")?;
    checked(response).await?;
    Ok(())
}

pub fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", bytes as f64 / 1_000_000_000.0)
    } else if bytes >= 1_000_000 {
        format!("{:.0} MB", bytes as f64 / 1_000_000.0)
    } else {
        format!("{bytes} B")
    }
}

fn ollama_root(base_url: &str) -> String {
    base_url
        .trim_end_matches('/')
        .strip_suffix("/v1")
        .unwrap_or_else(|| base_url.trim_end_matches('/'))
        .to_string()
}

async fn checked(response: reqwest::Response) -> Result<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    bail!(
        "Ollama returned {status}: {}",
        body.chars().take(300).collect::<String>()
    )
}

#[derive(Debug, Deserialize)]
struct PullUpdate {
    status: Option<String>,
    completed: Option<u64>,
    total: Option<u64>,
    error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_openai_suffix_from_ollama_url() {
        assert_eq!(
            ollama_root("http://localhost:11434/v1/"),
            "http://localhost:11434"
        );
        assert_eq!(ollama_root("http://host:11434"), "http://host:11434");
    }

    #[test]
    fn formats_model_sizes() {
        assert_eq!(format_bytes(3_300_000_000), "3.3 GB");
        assert_eq!(format_bytes(900_000_000), "900 MB");
    }
}
