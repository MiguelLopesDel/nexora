//! Continuous meeting assistant settings. All fields are exposed in Settings.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::general::default_true;
use super::provider::default_task;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeetingConfig {
    /// "microphone", "system", "both", or "custom".
    #[serde(default = "default_audio_source")]
    pub audio_source: String,
    /// PulseAudio/PipeWire source name when `audio_source = "custom"`.
    #[serde(default)]
    pub audio_device: String,
    #[serde(default = "default_chunk_seconds")]
    pub chunk_seconds: u64,
    /// Rolling local Whisper window. It may be longer than the capture stride
    /// so words cut at chunk boundaries are heard again with surrounding audio.
    #[serde(default = "default_transcription_window_seconds")]
    pub transcription_window_seconds: u64,
    /// Manual questions are sent immediately with the transcript available at
    /// Enter. Only a session with no transcript at all yet waits up to this
    /// long for the first line. Zero never waits.
    #[serde(default = "default_question_context_wait_ms")]
    pub question_context_wait_ms: u64,
    /// Maximum transcript characters attached to a manual question. Context
    /// selection combines recent speech with relevant older fragments.
    #[serde(default = "default_question_context_chars")]
    pub question_context_chars: usize,
    /// RMS-like PCM amplitude below which a chunk is skipped. Zero disables it.
    #[serde(default = "default_silence_threshold")]
    pub silence_threshold: u16,
    /// "local" runs whisper.cpp on this computer; "remote" uploads audio to
    /// the provider's /audio/transcriptions API.
    #[serde(default = "default_transcription_backend")]
    pub transcription_backend: String,
    /// Curated whisper.cpp checkpoint used when the backend is "local".
    #[serde(default = "default_whisper_model")]
    pub whisper_model: String,
    /// "auto" prefers an available compiled GPU backend, "gpu" requires it,
    /// and "cpu" disables GPU use even in a GPU-enabled build.
    #[serde(default = "default_transcription_compute")]
    pub transcription_compute: String,
    #[serde(default = "default_transcription_provider")]
    pub transcription_provider: String,
    #[serde(default = "default_transcription_model")]
    pub transcription_model: String,
    #[serde(default)]
    pub input_language: String,
    #[serde(default)]
    pub translate: bool,
    #[serde(default = "default_target_language")]
    pub target_language: String,
    #[serde(default = "default_true")]
    pub suggestions: bool,
    #[serde(default = "default_true")]
    pub objection_handling: bool,
    #[serde(default = "default_true")]
    pub automatic_notes: bool,
    #[serde(default)]
    pub screen_context: bool,
    #[serde(default = "default_screen_interval")]
    pub screen_interval_chunks: u32,
    #[serde(default = "default_true")]
    pub summary: bool,
    #[serde(default = "default_true")]
    pub save_session: bool,
    #[serde(default = "default_task")]
    pub analysis_task: String,
    #[serde(default = "default_profile")]
    pub profile: String,
    /// Post-transcription fixes for slang, jargon, and names the transcriber
    /// keeps getting wrong. Keys match whole words (or word sequences)
    /// ignoring case: `"clod" = "Claude"` never rewrites part of a word.
    #[serde(default)]
    pub corrections: BTreeMap<String, String>,
}

impl Default for MeetingConfig {
    fn default() -> Self {
        Self {
            audio_source: default_audio_source(),
            audio_device: String::new(),
            chunk_seconds: default_chunk_seconds(),
            transcription_window_seconds: default_transcription_window_seconds(),
            question_context_wait_ms: default_question_context_wait_ms(),
            question_context_chars: default_question_context_chars(),
            silence_threshold: default_silence_threshold(),
            transcription_backend: default_transcription_backend(),
            whisper_model: default_whisper_model(),
            transcription_compute: default_transcription_compute(),
            transcription_provider: default_transcription_provider(),
            transcription_model: default_transcription_model(),
            input_language: String::new(),
            translate: false,
            target_language: default_target_language(),
            suggestions: true,
            objection_handling: true,
            automatic_notes: true,
            screen_context: false,
            screen_interval_chunks: default_screen_interval(),
            summary: true,
            save_session: true,
            analysis_task: default_task(),
            profile: default_profile(),
            corrections: BTreeMap::new(),
        }
    }
}

fn default_audio_source() -> String {
    "system".into()
}
fn default_chunk_seconds() -> u64 {
    2
}
fn default_transcription_window_seconds() -> u64 {
    4
}
fn default_question_context_wait_ms() -> u64 {
    1_200
}
fn default_question_context_chars() -> usize {
    12_000
}
fn default_silence_threshold() -> u16 {
    180
}
fn default_transcription_backend() -> String {
    "local".into()
}
fn default_whisper_model() -> String {
    "base".into()
}
fn default_transcription_compute() -> String {
    "auto".into()
}
fn default_transcription_provider() -> String {
    "openai".into()
}
fn default_transcription_model() -> String {
    "gpt-4o-mini-transcribe".into()
}
fn default_target_language() -> String {
    "Portuguese (Brazil)".into()
}
fn default_screen_interval() -> u32 {
    3
}
pub(super) fn default_profile() -> String {
    "general".into()
}
