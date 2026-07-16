//! Local transcription with whisper.cpp: curated model downloads kept in
//! `$XDG_DATA_HOME/nexora/whisper/` and local inference on raw PCM chunks.

use std::path::PathBuf;
use std::sync::{Mutex, Once};

use anyhow::{Context, Result, bail};
use async_channel::Sender;
use futures_util::StreamExt;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

#[cfg(any(
    all(feature = "whisper-vulkan", feature = "whisper-cuda"),
    all(feature = "whisper-vulkan", feature = "whisper-rocm"),
    all(feature = "whisper-cuda", feature = "whisper-rocm")
))]
compile_error!("enable only one whisper GPU backend at a time");

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputePreference {
    Auto,
    Gpu,
    Cpu,
}

impl ComputePreference {
    pub fn from_config(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "gpu" => Ok(Self::Gpu),
            "cpu" => Ok(Self::Cpu),
            other => bail!("unknown transcription compute mode `{other}`"),
        }
    }
}

pub fn compiled_gpu_backend() -> Option<&'static str> {
    #[cfg(feature = "whisper-vulkan")]
    return Some("Vulkan");
    #[cfg(all(not(feature = "whisper-vulkan"), feature = "whisper-cuda"))]
    return Some("CUDA");
    #[cfg(all(
        not(feature = "whisper-vulkan"),
        not(feature = "whisper-cuda"),
        feature = "whisper-rocm"
    ))]
    return Some("ROCm/HIP");
    #[cfg(not(any(
        feature = "whisper-vulkan",
        feature = "whisper-cuda",
        feature = "whisper-rocm"
    )))]
    None
}

/// Return only the new suffix from two transcripts of overlapping audio.
/// Exact multi-word overlap is intentionally conservative: when Whisper
/// revises a phrase, retaining a little duplication is safer than deleting it.
pub fn novel_transcript(previous: &str, current: &str) -> String {
    let previous_words: Vec<(&str, String)> = previous
        .split_whitespace()
        .map(|word| (word, normalize_word(word)))
        .filter(|(_, normalized)| !normalized.is_empty())
        .collect();
    let current_words: Vec<(&str, String)> = current
        .split_whitespace()
        .map(|word| (word, normalize_word(word)))
        .filter(|(_, normalized)| !normalized.is_empty())
        .collect();
    let max_overlap = previous_words.len().min(current_words.len());
    for overlap in (2..=max_overlap).rev() {
        let previous_start = previous_words.len() - overlap;
        for current_start in 0..=current_words.len() - overlap {
            let matches = previous_words[previous_start..]
                .iter()
                .zip(&current_words[current_start..current_start + overlap])
                .all(|(left, right)| left.1 == right.1);
            if matches {
                return current_words[current_start + overlap..]
                    .iter()
                    .map(|(word, _)| *word)
                    .collect::<Vec<_>>()
                    .join(" ");
            }
        }
    }
    current.trim().to_string()
}

fn normalize_word(word: &str) -> String {
    word.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn context_parameters(preference: ComputePreference) -> Result<WhisperContextParameters<'static>> {
    let use_gpu = match preference {
        ComputePreference::Auto => compiled_gpu_backend().is_some(),
        ComputePreference::Gpu => {
            if compiled_gpu_backend().is_none() {
                bail!(
                    "GPU transcription was requested, but this Nexora binary has no whisper GPU backend; rebuild with `--features whisper-vulkan`, `whisper-cuda`, or `whisper-rocm`"
                );
            }
            true
        }
        ComputePreference::Cpu => false,
    };
    let mut params = WhisperContextParameters::default();
    params.use_gpu(use_gpu);
    Ok(params)
}

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

/// A loaded whisper.cpp model. Inference is mutually exclusive, so callers run
/// `transcribe` inside `spawn_blocking` and share the value behind an `Arc`.
pub struct Transcriber {
    state: Mutex<whisper_rs::WhisperState>,
    threads: i32,
    compute_label: String,
}

impl Transcriber {
    pub fn load(path: &std::path::Path, preference: ComputePreference) -> Result<Self> {
        // Route whisper.cpp's chatty stderr output through the log crate
        // (which is a no-op here) exactly once.
        static HOOKS: Once = Once::new();
        HOOKS.call_once(whisper_rs::install_logging_hooks);

        let path = path
            .to_str()
            .context("whisper model path is not valid UTF-8")?;
        let params = context_parameters(preference)?;
        let gpu_requested = params.use_gpu;
        let (context, compute_label) = match WhisperContext::new_with_params(path, params) {
            Ok(context) => {
                let label = if gpu_requested {
                    format!(
                        "GPU ({})",
                        compiled_gpu_backend().unwrap_or("unknown backend")
                    )
                } else if preference == ComputePreference::Cpu {
                    "CPU (forced)".into()
                } else {
                    "CPU (this build has no GPU backend)".into()
                };
                (context, label)
            }
            Err(gpu_error) if preference == ComputePreference::Auto && gpu_requested => {
                let mut cpu_params = WhisperContextParameters::default();
                cpu_params.use_gpu(false);
                let context = WhisperContext::new_with_params(path, cpu_params).with_context(|| {
                    format!(
                        "could not load whisper model at {path}; GPU initialization also failed: {gpu_error}"
                    )
                })?;
                (
                    context,
                    format!(
                        "CPU fallback ({} initialization failed)",
                        compiled_gpu_backend().unwrap_or("GPU")
                    ),
                )
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("could not load whisper model at {path}"));
            }
        };
        let state = context.create_state().context("whisper state failed")?;
        let threads = std::thread::available_parallelism()
            .map(|cores| cores.get().min(8) as i32)
            .unwrap_or(4);
        Ok(Self {
            state: Mutex::new(state),
            threads,
            compute_label,
        })
    }

    pub fn compute_label(&self) -> &str {
        &self.compute_label
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
        let transcriber =
            Transcriber::load(&path, ComputePreference::Auto).expect("model should load");
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
        let transcriber = Transcriber::load(&model_path("tiny"), ComputePreference::Auto)
            .expect("model should load");
        println!("compute: {}", transcriber.compute_label());
        let text = transcriber
            .transcribe(&pcm, "en")
            .expect("inference should run");
        assert!(!text.is_empty(), "speech produced an empty transcript");
        println!("transcript: {text}");
    }

    /// Replays speech through independent live-sized windows. This catches
    /// boundary loss that full-file transcription cannot reveal.
    #[test]
    #[ignore = "requires a downloaded whisper model and NEXORA_TEST_PCM"]
    fn transcribes_speech_in_live_windows() {
        let pcm_path = std::env::var("NEXORA_TEST_PCM").expect("set NEXORA_TEST_PCM");
        let seconds: usize = std::env::var("NEXORA_TEST_CHUNK_SECONDS")
            .unwrap_or_else(|_| "2".into())
            .parse()
            .expect("NEXORA_TEST_CHUNK_SECONDS must be a number");
        let stride_seconds: usize = std::env::var("NEXORA_TEST_STRIDE_SECONDS")
            .unwrap_or_else(|_| seconds.to_string())
            .parse()
            .expect("NEXORA_TEST_STRIDE_SECONDS must be a number");
        let pcm = std::fs::read(pcm_path).expect("PCM file should be readable");
        let transcriber = Transcriber::load(&model_path("tiny"), ComputePreference::Cpu)
            .expect("model should load");
        let bytes_per_window = SAMPLE_RATE * 2 * seconds;
        let bytes_per_stride = SAMPLE_RATE * 2 * stride_seconds;
        let raw_transcript: Vec<String> = (0..pcm.len())
            .step_by(bytes_per_stride)
            .map(|start| &pcm[start..(start + bytes_per_window).min(pcm.len())])
            .map(|window| transcriber.transcribe(window, "en").expect("window failed"))
            .filter(|text| !text.is_empty())
            .collect();
        let mut previous = None;
        let transcript: Vec<String> = raw_transcript
            .into_iter()
            .filter_map(|current| {
                let novel = previous
                    .as_deref()
                    .map_or_else(|| current.clone(), |old| novel_transcript(old, &current));
                previous = Some(current);
                (!novel.is_empty()).then_some(novel)
            })
            .collect();
        let joined = transcript.join(" ").to_ascii_lowercase();
        println!("live transcript ({seconds}s/{stride_seconds}s): {joined}");
        assert!(
            joined.contains("country can do for you"),
            "first clause lost"
        );
        assert!(
            joined.contains("you can do for your country"),
            "second clause lost"
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

    #[test]
    fn overlapping_transcripts_emit_only_the_new_suffix() {
        assert_eq!(
            novel_transcript(
                "country can do for you",
                "one pre can do for you ask what you can do"
            ),
            "ask what you can do"
        );
        assert_eq!(
            novel_transcript(
                "ask what you can do for your",
                "ask what you can do for your country."
            ),
            "country."
        );
    }

    #[test]
    fn cpu_compute_never_enables_gpu() {
        let params = context_parameters(ComputePreference::Cpu).unwrap();
        assert!(!params.use_gpu);
    }

    #[test]
    fn automatic_compute_matches_the_compiled_backend() {
        let params = context_parameters(ComputePreference::Auto).unwrap();
        assert_eq!(params.use_gpu, compiled_gpu_backend().is_some());
    }

    #[test]
    fn forced_gpu_requires_a_gpu_enabled_build() {
        let result = context_parameters(ComputePreference::Gpu);
        assert_eq!(result.is_ok(), compiled_gpu_backend().is_some());
    }
}
