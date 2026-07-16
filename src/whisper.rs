//! Local transcription with whisper.cpp: curated model downloads kept in
//! `$XDG_DATA_HOME/nexora/whisper/` and CPU inference on raw PCM chunks.

use std::path::PathBuf;
use std::sync::{Mutex, Once};

use anyhow::{Context, Result, bail};
use async_channel::Sender;
use futures_util::StreamExt;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

const SAMPLE_RATE: usize = 16_000;
/// whisper.cpp degrades below roughly one second of audio; pad short chunks.
const MIN_SAMPLES: usize = SAMPLE_RATE + SAMPLE_RATE / 10;

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
        description: "Fastest on weak CPUs, least accurate",
    },
    WhisperModelPreset {
        id: "base",
        download: "148 MB",
        size: "Recommended",
        description: "Good balance of speed and accuracy on most CPUs",
    },
    WhisperModelPreset {
        id: "small",
        download: "488 MB",
        size: "Quality",
        description: "Noticeably better accuracy, needs a mid-range CPU",
    },
    WhisperModelPreset {
        id: "large-v3-turbo-q5_0",
        download: "574 MB",
        size: "Best",
        description: "Top accuracy (quantized large-v3-turbo), needs a fast CPU",
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

/// A loaded whisper.cpp model. Inference is CPU-bound and mutually exclusive,
/// so callers run `transcribe` inside `spawn_blocking` and share the value
/// behind an `Arc`.
pub struct Transcriber {
    state: Mutex<whisper_rs::WhisperState>,
    threads: i32,
}

impl Transcriber {
    pub fn load(path: &std::path::Path) -> Result<Self> {
        // Route whisper.cpp's chatty stderr output through the log crate
        // (which is a no-op here) exactly once.
        static HOOKS: Once = Once::new();
        HOOKS.call_once(whisper_rs::install_logging_hooks);

        let path = path
            .to_str()
            .context("whisper model path is not valid UTF-8")?;
        let context = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .with_context(|| format!("could not load whisper model at {path}"))?;
        let state = context.create_state().context("whisper state failed")?;
        let threads = std::thread::available_parallelism()
            .map(|cores| cores.get().min(8) as i32)
            .unwrap_or(4);
        Ok(Self {
            state: Mutex::new(state),
            threads,
        })
    }

    /// Transcribe a raw s16le 16 kHz mono chunk. An empty `language` lets the
    /// model detect it; anything else must be a Whisper language code.
    pub fn transcribe(&self, pcm_s16le: &[u8], language: &str) -> Result<String> {
        let mut samples: Vec<f32> = pcm_s16le
            .chunks_exact(2)
            .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 32_768.0)
            .collect();
        if samples.len() < MIN_SAMPLES {
            samples.resize(MIN_SAMPLES, 0.0);
        }

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(self.threads);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_no_timestamps(true);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        // Chunks arrive independently; carrying decoder context across them
        // makes hallucinations sticky.
        params.set_no_context(true);
        let language = language.trim().to_ascii_lowercase();
        params.set_language((!language.is_empty()).then_some(language.as_str()));

        let mut state = self.state.lock().expect("whisper state poisoned");
        state
            .full(params, &samples)
            .context("whisper inference failed")?;
        let mut text = String::new();
        for index in 0..state.full_n_segments() {
            if let Some(segment) = state.get_segment(index)
                && let Ok(piece) = segment.to_str_lossy()
            {
                text.push_str(&piece);
            }
        }
        Ok(text.trim().to_string())
    }
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

    /// Real end-to-end inference; needs `ggml-tiny.bin` downloaded first.
    /// Run with `cargo test -- --ignored`.
    #[test]
    #[ignore = "requires a downloaded whisper model"]
    fn loads_a_model_and_transcribes_silence() {
        let path = model_path("tiny");
        assert!(path.exists(), "download ggml-tiny.bin first");
        let transcriber = Transcriber::load(&path).expect("model should load");
        let silence = vec![0_u8; SAMPLE_RATE * 2 * 2];
        let text = transcriber
            .transcribe(&silence, "")
            .expect("inference should run");
        // Silence may decode to nothing or a short hallucination, never prose.
        assert!(text.len() < 200, "unexpected transcript: {text}");
    }

    /// Feeds real speech through the full path when `NEXORA_TEST_PCM` points
    /// at a raw s16le 16 kHz mono file; catches PCM conversion regressions
    /// that a silence-only test cannot see.
    #[test]
    #[ignore = "requires a downloaded whisper model and NEXORA_TEST_PCM"]
    fn transcribes_speech_from_env_pcm() {
        let pcm_path = std::env::var("NEXORA_TEST_PCM").expect("set NEXORA_TEST_PCM");
        let pcm = std::fs::read(pcm_path).expect("PCM file should be readable");
        let transcriber = Transcriber::load(&model_path("tiny")).expect("model should load");
        let text = transcriber
            .transcribe(&pcm, "en")
            .expect("inference should run");
        assert!(!text.is_empty(), "speech produced an empty transcript");
        println!("transcript: {text}");
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
