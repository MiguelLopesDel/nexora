//! Curated model catalog, download, and on-disk management.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use async_channel::Sender;
use futures_util::StreamExt;

#[derive(Debug, Clone, Copy)]
pub struct WhisperModelPreset {
    pub id: &'static str,
    pub download: &'static str,
    pub size: &'static str,
    pub description: &'static str,
}

/// Curated multilingual ggml checkpoints from the official whisper.cpp
/// repository on Hugging Face.
pub const PRESETS: &[WhisperModelPreset] = &[
    WhisperModelPreset {
        id: "tiny",
        download: "78 MB",
        size: "Ultra-light",
        description: "Fastest on weak hardware, least accurate",
    },
    WhisperModelPreset {
        id: "base",
        download: "148 MB",
        size: "Recommended",
        description: "Good balance of speed and accuracy",
    },
    WhisperModelPreset {
        id: "small",
        download: "488 MB",
        size: "Quality",
        description: "Noticeably better accuracy, needs more compute",
    },
    WhisperModelPreset {
        id: "large-v3-turbo-q5_0",
        download: "574 MB",
        size: "Best",
        description: "Top accuracy (quantized large-v3-turbo), GPU recommended",
    },
];

#[derive(Debug, Clone)]
pub struct DownloadProgress {
    pub completed: u64,
    pub total: Option<u64>,
}

pub fn models_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("~/.local/share"))
        .join("nexora")
        .join("whisper")
}

pub fn model_path(id: &str) -> PathBuf {
    models_dir().join(format!("ggml-{id}.bin"))
}

pub fn model_url(id: &str) -> String {
    format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{id}.bin")
}

/// Preset ids that are present on disk.
pub fn installed_models() -> Vec<(String, u64)> {
    PRESETS
        .iter()
        .filter_map(|preset| {
            let metadata = std::fs::metadata(model_path(preset.id)).ok()?;
            Some((preset.id.to_string(), metadata.len()))
        })
        .collect()
}

/// Stream a checkpoint to disk. The file is written next to its final name
/// with a `.part` suffix and renamed on completion, so an interrupted
/// download is never mistaken for a usable model.
pub async fn download_model(id: &str, progress: Sender<DownloadProgress>) -> Result<()> {
    if !PRESETS.iter().any(|preset| preset.id == id) {
        bail!("unknown whisper model `{id}`");
    }
    let dir = models_dir();
    std::fs::create_dir_all(&dir)?;
    let partial = dir.join(format!("ggml-{id}.bin.part"));
    let response = reqwest::Client::new()
        .get(model_url(id))
        .send()
        .await
        .context("could not reach huggingface.co")?;
    if !response.status().is_success() {
        bail!("model download returned {}", response.status());
    }
    let total = response.content_length();
    let mut file = std::fs::File::create(&partial)?;
    let mut completed = 0_u64;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        std::io::Write::write_all(&mut file, &chunk)?;
        completed += chunk.len() as u64;
        let _ = progress.try_send(DownloadProgress { completed, total });
    }
    drop(file);
    if let Some(total) = total
        && completed != total
    {
        let _ = std::fs::remove_file(&partial);
        bail!("download ended early ({completed} of {total} bytes)");
    }
    std::fs::rename(&partial, model_path(id))?;
    Ok(())
}

pub fn remove_model(id: &str) -> Result<()> {
    let path = model_path(id);
    if !path.exists() {
        bail!("model `{id}` is not downloaded");
    }
    std::fs::remove_file(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_paths_follow_ggml_naming() {
        assert!(
            model_path("base")
                .to_string_lossy()
                .ends_with("nexora/whisper/ggml-base.bin")
        );
        assert_eq!(
            model_url("tiny"),
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin"
        );
    }

    #[test]
    fn presets_have_unique_known_ids() {
        let mut ids: Vec<_> = PRESETS.iter().map(|preset| preset.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), PRESETS.len());
        assert!(ids.contains(&"base"));
    }
}
