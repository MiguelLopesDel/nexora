//! Configuration loading for ~/.config/nexora/config.toml.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

pub const EXAMPLE_CONFIG: &str = include_str!("../config.example.toml");

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub general: General,
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    #[serde(default)]
    pub tasks: HashMap<String, TaskConfig>,
    #[serde(default)]
    pub presets: HashMap<String, PresetConfig>,
    #[serde(default)]
    pub meeting: MeetingConfig,
    #[serde(default)]
    pub vision: VisionConfig,
    #[serde(default)]
    pub profiles: HashMap<String, AssistantProfile>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct General {
    /// Request anti-capture (screen-share hiding) from the compositor.
    #[serde(default = "default_true")]
    pub hidden: bool,
    /// Use layer-shell overlay when the compositor supports it: "auto", "on", "off".
    #[serde(default = "default_layer_shell")]
    pub layer_shell: String,
    /// Hyprland window-rule keyword used for anti-capture. Depends on your
    /// Hyprland version; see `nexora hidden status`.
    #[serde(default = "default_hyprland_rule")]
    pub hyprland_rule: String,
    /// Window width in pixels.
    #[serde(default = "default_width")]
    pub width: i32,
    /// Window height in pixels.
    #[serde(default = "default_height")]
    pub height: i32,
}

impl Default for General {
    fn default() -> Self {
        Self {
            hidden: true,
            layer_shell: default_layer_shell(),
            hyprland_rule: default_hyprland_rule(),
            width: default_width(),
            height: default_height(),
        }
    }
}

fn default_hyprland_rule() -> String {
    crate::hidden::DEFAULT_HYPRLAND_RULE.to_string()
}

fn default_true() -> bool {
    true
}
fn default_layer_shell() -> String {
    "auto".into()
}
fn default_width() -> i32 {
    820
}
fn default_height() -> i32 {
    560
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    /// Wire protocol: "anthropic" or "openai" (OpenAI-compatible).
    pub kind: ProviderKind,
    /// Base URL override. Defaults depend on `kind`.
    pub base_url: Option<String>,
    /// API key, verbatim. Prefer `api_key_env` to keep secrets out of the file.
    pub api_key: Option<String>,
    /// Name of an environment variable holding the API key.
    pub api_key_env: Option<String>,
    /// Model preselected when this provider becomes the default chat provider.
    #[serde(default)]
    pub default_model: Option<String>,
    /// Provider-specific thinking mode; `None` keeps the provider default.
    #[serde(default)]
    pub thinking: Option<bool>,
    /// Provider-specific reasoning effort such as "high" or "max".
    #[serde(default)]
    pub reasoning_effort: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Anthropic,
    Openai,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Openai => "openai",
        }
    }
}

impl ProviderConfig {
    /// Resolve the API key from the literal value or the environment.
    pub fn resolve_api_key(&self) -> Result<String> {
        if let Some(key) = &self.api_key {
            return Ok(key.clone());
        }
        if let Some(var) = &self.api_key_env {
            return std::env::var(var)
                .with_context(|| format!("environment variable {var} is not set"));
        }
        bail!("provider has neither api_key nor api_key_env configured")
    }

    pub fn base_url(&self) -> String {
        let url = self.base_url.clone().unwrap_or_else(|| match self.kind {
            ProviderKind::Anthropic => "https://api.anthropic.com".into(),
            ProviderKind::Openai => "https://api.openai.com/v1".into(),
        });
        url.trim_end_matches('/').to_string()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskConfig {
    /// Key into `[providers]`.
    pub provider: String,
    pub model: String,
    /// Optional system prompt override for this task.
    pub system: Option<String>,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
}

fn default_max_tokens() -> u32 {
    2048
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresetConfig {
    /// Prompt sent when the preset fires.
    pub prompt: String,
    /// Attach a screenshot of the current screen.
    #[serde(default)]
    pub attach_screen: bool,
    /// Task (provider+model) to use. Defaults to "ask".
    #[serde(default = "default_task")]
    pub task: String,
}

fn default_task() -> String {
    "ask".into()
}

/// Continuous meeting assistant settings. All fields are exposed in Settings.
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
    /// Maximum time to wait for an in-flight transcript update before sending
    /// a manual question. Zero sends immediately.
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
fn default_profile() -> String {
    "general".into()
}

/// Screen-understanding configuration. `direct` sends the image to the task
/// model; `proxy` first converts it to text with a separate vision model.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VisionConfig {
    #[serde(default = "default_vision_mode")]
    pub mode: String,
    #[serde(default = "default_vision_provider")]
    pub provider: String,
    #[serde(default = "default_vision_model")]
    pub model: String,
    #[serde(default = "default_vision_prompt")]
    pub prompt: String,
    #[serde(default = "default_ollama_url")]
    pub ollama_url: String,
}

impl Default for VisionConfig {
    fn default() -> Self {
        Self {
            mode: default_vision_mode(),
            provider: default_vision_provider(),
            model: default_vision_model(),
            prompt: default_vision_prompt(),
            ollama_url: default_ollama_url(),
        }
    }
}

fn default_vision_mode() -> String {
    "direct".into()
}
fn default_vision_provider() -> String {
    "ollama".into()
}
fn default_vision_model() -> String {
    "qwen3-vl:4b".into()
}
fn default_vision_prompt() -> String {
    "Describe the visible screen for another AI. Extract important text with OCR, application names, errors, numbers, UI state, and conversation-relevant details. Be factual and compact; do not guess hidden content.".into()
}
fn default_ollama_url() -> String {
    "http://localhost:11434".into()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssistantProfile {
    pub system: String,
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("nexora")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = config_path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let config: Config =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        Ok(config)
    }

    /// The bundled example configuration, parsed. Used to seed the settings
    /// panel with provider choices before the user has a config file.
    pub fn example() -> Self {
        toml::from_str(EXAMPLE_CONFIG).expect("bundled example config must parse")
    }

    /// Sorted provider names, falling back to the example's when none are
    /// configured yet.
    pub fn provider_names(&self) -> Vec<String> {
        let mut names: Vec<String> = Self::example().providers.keys().cloned().collect();
        names.extend(self.providers.keys().cloned());
        names.sort();
        names.dedup();
        names
    }

    /// Provider lookup with bundled providers available before config creation.
    pub fn provider(&self, name: &str) -> Option<ProviderConfig> {
        self.providers
            .get(name)
            .cloned()
            .or_else(|| Self::example().providers.remove(name))
    }

    /// Task lookup with a clear error listing what is configured.
    pub fn task(&self, name: &str) -> Result<&TaskConfig> {
        self.tasks.get(name).with_context(|| {
            let known: Vec<_> = self.tasks.keys().map(String::as_str).collect();
            format!(
                "task \"{name}\" is not configured (configured tasks: [{}]) — edit {}",
                known.join(", "),
                config_path().display()
            )
        })
    }

    pub fn provider_for(&self, task: &TaskConfig) -> Result<&ProviderConfig> {
        self.providers.get(&task.provider).with_context(|| {
            format!(
                "provider \"{}\" is not configured under [providers]",
                task.provider
            )
        })
    }

    pub fn profile(&self, name: &str) -> Result<AssistantProfile> {
        if let Some(profile) = self.profiles.get(name) {
            return Ok(profile.clone());
        }
        Self::example()
            .profiles
            .remove(name)
            .with_context(|| format!("assistant profile \"{name}\" is not configured"))
    }

    pub fn profile_names(&self) -> Vec<String> {
        let mut names: Vec<String> = Self::example().profiles.keys().cloned().collect();
        names.extend(self.profiles.keys().cloned());
        names.sort();
        names.dedup();
        names
    }

    /// Preset lookup; "explain-screen" has a built-in fallback.
    pub fn preset(&self, name: &str) -> Result<PresetConfig> {
        if let Some(preset) = self.presets.get(name) {
            return Ok(preset.clone());
        }
        if name == "explain-screen" {
            return Ok(PresetConfig {
                prompt: "Explain what is on my screen. Be concise; focus on unusual terms, \
                         errors, and anything I would likely want clarified."
                    .into(),
                attach_screen: true,
                task: default_task(),
            });
        }
        let known: Vec<_> = self.presets.keys().map(String::as_str).collect();
        bail!(
            "preset \"{name}\" is not configured (configured presets: [{}], built-in: [explain-screen])",
            known.join(", ")
        )
    }
}

/// Settings the in-app preferences panel can change.
pub struct SettingsUpdate {
    pub hidden: bool,
    pub hyprland_rule: String,
    pub layer_shell: String,
    pub width: i32,
    pub height: i32,
    /// Task being configured (usually "ask").
    pub task: String,
    pub provider: String,
    pub provider_kind: ProviderKind,
    pub provider_base_url: Option<String>,
    pub provider_api_key_env: Option<String>,
    pub provider_thinking: Option<bool>,
    pub provider_reasoning_effort: Option<String>,
    pub model: String,
    /// When `Some` and non-empty, stored as the provider's literal api_key.
    pub api_key: Option<String>,
    pub clear_api_key: bool,
    pub meeting: MeetingConfig,
    pub vision: VisionConfig,
    pub profile_name: String,
    pub profile_system: String,
}

/// Apply settings to config.toml without discarding comments or unrelated keys.
///
/// Creates the file from the bundled example if it does not exist yet.
pub fn apply_settings(update: &SettingsUpdate) -> Result<()> {
    use toml_edit::{DocumentMut, Item, Table, value};

    let path = config_path();
    let mut doc: DocumentMut = if path.exists() {
        std::fs::read_to_string(&path)?
            .parse()
            .with_context(|| format!("parsing {}", path.display()))?
    } else {
        std::fs::create_dir_all(config_dir())?;
        EXAMPLE_CONFIG.parse()?
    };

    // Ensure a table exists at `doc[key]`, creating it if missing.
    fn table<'a>(doc: &'a mut DocumentMut, key: &str) -> &'a mut Table {
        doc.entry(key)
            .or_insert_with(|| Item::Table(Table::new()))
            .as_table_mut()
            .expect("config section must be a table")
    }
    fn subtable<'a>(parent: &'a mut Table, key: &str) -> &'a mut Table {
        parent
            .entry(key)
            .or_insert_with(|| Item::Table(Table::new()))
            .as_table_mut()
            .expect("config subsection must be a table")
    }

    table(&mut doc, "general")["hidden"] = value(update.hidden);
    table(&mut doc, "general")["hyprland_rule"] = value(update.hyprland_rule.clone());
    table(&mut doc, "general")["layer_shell"] = value(update.layer_shell.clone());
    table(&mut doc, "general")["width"] = value(update.width as i64);
    table(&mut doc, "general")["height"] = value(update.height as i64);

    let tasks = table(&mut doc, "tasks");
    let task = subtable(tasks, &update.task);
    task["provider"] = value(update.provider.clone());
    task["model"] = value(update.model.clone());

    let providers = table(&mut doc, "providers");
    let provider = subtable(providers, &update.provider);
    provider["kind"] = value(update.provider_kind.as_str());
    provider["default_model"] = value(update.model.clone());
    match &update.provider_base_url {
        Some(url) if !url.is_empty() => provider["base_url"] = value(url.clone()),
        _ => {
            provider.remove("base_url");
        }
    }
    match &update.provider_api_key_env {
        Some(name) if !name.is_empty() => provider["api_key_env"] = value(name.clone()),
        _ => {
            provider.remove("api_key_env");
        }
    }
    match update.provider_thinking {
        Some(enabled) => provider["thinking"] = value(enabled),
        None => {
            provider.remove("thinking");
        }
    }
    match &update.provider_reasoning_effort {
        Some(effort) if !effort.is_empty() => provider["reasoning_effort"] = value(effort.clone()),
        _ => {
            provider.remove("reasoning_effort");
        }
    }
    if update.clear_api_key {
        provider.remove("api_key");
    } else if let Some(key) = &update.api_key
        && !key.is_empty()
    {
        provider["api_key"] = value(key.clone());
    }

    let meeting = table(&mut doc, "meeting");
    meeting["audio_source"] = value(update.meeting.audio_source.clone());
    meeting["audio_device"] = value(update.meeting.audio_device.clone());
    meeting["chunk_seconds"] = value(update.meeting.chunk_seconds as i64);
    meeting["transcription_window_seconds"] =
        value(update.meeting.transcription_window_seconds as i64);
    meeting["question_context_wait_ms"] = value(update.meeting.question_context_wait_ms as i64);
    meeting["question_context_chars"] = value(update.meeting.question_context_chars as i64);
    meeting["silence_threshold"] = value(update.meeting.silence_threshold as i64);
    meeting["transcription_backend"] = value(update.meeting.transcription_backend.clone());
    meeting["whisper_model"] = value(update.meeting.whisper_model.clone());
    meeting["transcription_compute"] = value(update.meeting.transcription_compute.clone());
    meeting["transcription_provider"] = value(update.meeting.transcription_provider.clone());
    meeting["transcription_model"] = value(update.meeting.transcription_model.clone());
    meeting["input_language"] = value(update.meeting.input_language.clone());
    meeting["translate"] = value(update.meeting.translate);
    meeting["target_language"] = value(update.meeting.target_language.clone());
    meeting["suggestions"] = value(update.meeting.suggestions);
    meeting["objection_handling"] = value(update.meeting.objection_handling);
    meeting["automatic_notes"] = value(update.meeting.automatic_notes);
    meeting["screen_context"] = value(update.meeting.screen_context);
    meeting["screen_interval_chunks"] = value(update.meeting.screen_interval_chunks as i64);
    meeting["summary"] = value(update.meeting.summary);
    meeting["save_session"] = value(update.meeting.save_session);
    meeting["analysis_task"] = value(update.meeting.analysis_task.clone());
    meeting["profile"] = value(update.profile_name.clone());

    let vision = table(&mut doc, "vision");
    vision["mode"] = value(update.vision.mode.clone());
    vision["provider"] = value(update.vision.provider.clone());
    vision["model"] = value(update.vision.model.clone());
    vision["prompt"] = value(update.vision.prompt.clone());
    vision["ollama_url"] = value(update.vision.ollama_url.clone());

    let profiles = table(&mut doc, "profiles");
    let profile = subtable(profiles, &update.profile_name);
    profile["system"] = value(update.profile_system.clone());

    std::fs::write(&path, doc.to_string())
        .with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Write the example config to the standard path, never overwriting.
pub fn init_config_file() -> Result<PathBuf> {
    let path = config_path();
    if path.exists() {
        bail!("{} already exists, not overwriting", path.display());
    }
    std::fs::create_dir_all(config_dir())?;
    std::fs::write(&path, EXAMPLE_CONFIG)?;
    // The config may hold API keys; keep it private to the user.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_config_parses() {
        let config: Config = toml::from_str(EXAMPLE_CONFIG).expect("example config must parse");
        assert!(config.providers.contains_key("anthropic"));
        assert!(config.tasks.contains_key("ask"));
        assert!(config.profiles.contains_key("sales"));
        assert_eq!(config.meeting.audio_source, "system");
        assert!(config.meeting.suggestions);
        let task = config.task("ask").unwrap();
        config.provider_for(task).unwrap();
    }

    #[test]
    fn empty_config_uses_defaults() {
        let config: Config = toml::from_str("").unwrap();
        assert!(config.general.hidden);
        assert_eq!(config.general.layer_shell, "auto");
        assert_eq!(config.meeting.chunk_seconds, 2);
        assert_eq!(config.meeting.transcription_window_seconds, 4);
        assert_eq!(config.meeting.question_context_wait_ms, 1_200);
        assert_eq!(config.meeting.question_context_chars, 12_000);
        assert_eq!(config.meeting.transcription_compute, "auto");
        assert_eq!(config.meeting.profile, "general");
        assert_eq!(config.vision.model, "qwen3-vl:4b");
        assert!(config.preset("explain-screen").is_ok());
        assert!(config.preset("nope").is_err());
    }

    #[test]
    fn api_key_resolution_prefers_literal() {
        let provider = ProviderConfig {
            kind: ProviderKind::Openai,
            base_url: None,
            api_key: Some("sk-test".into()),
            api_key_env: Some("DEFINITELY_NOT_SET_12345".into()),
            default_model: None,
            thinking: None,
            reasoning_effort: None,
        };
        assert_eq!(provider.resolve_api_key().unwrap(), "sk-test");
    }

    #[test]
    fn apply_settings_writes_and_reloads() {
        // Isolate config_path() to a temp dir for this test.
        let dir = std::env::temp_dir().join(format!("nexora-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &dir);
        }

        let update = SettingsUpdate {
            hidden: false,
            hyprland_rule: "noscreencopy".into(),
            layer_shell: "auto".into(),
            width: 700,
            height: 500,
            task: "ask".into(),
            provider: "openrouter".into(),
            provider_kind: ProviderKind::Openai,
            provider_base_url: Some("https://openrouter.ai/api/v1".into()),
            provider_api_key_env: Some("OPENROUTER_API_KEY".into()),
            provider_thinking: Some(true),
            provider_reasoning_effort: Some("high".into()),
            model: "some/model".into(),
            api_key: Some("sk-secret".into()),
            clear_api_key: false,
            meeting: MeetingConfig::default(),
            vision: VisionConfig::default(),
            profile_name: "general".into(),
            profile_system: "Be concise.".into(),
        };
        apply_settings(&update).unwrap();

        let config = Config::load().unwrap();
        assert!(!config.general.hidden);
        assert_eq!(config.general.width, 700);
        let task = config.task("ask").unwrap();
        assert_eq!(task.provider, "openrouter");
        assert_eq!(task.model, "some/model");
        // The provider (seeded from the example) now carries the literal key.
        let provider = config.provider_for(task).unwrap();
        assert_eq!(provider.resolve_api_key().unwrap(), "sk-secret");
        assert_eq!(provider.thinking, Some(true));
        assert_eq!(provider.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(config.vision.provider, "ollama");

        let _ = std::fs::remove_dir_all(&dir);
        unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
        }
    }

    #[test]
    fn base_url_trims_trailing_slash() {
        let provider = ProviderConfig {
            kind: ProviderKind::Openai,
            base_url: Some("http://localhost:11434/v1/".into()),
            api_key: Some("x".into()),
            api_key_env: None,
            default_model: None,
            thinking: None,
            reasoning_effort: None,
        };
        assert_eq!(provider.base_url(), "http://localhost:11434/v1");
    }
}
