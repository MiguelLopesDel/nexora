//! Configuration loading for ~/.config/nexora/config.toml.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

mod general;
mod meeting;
mod provider;
mod settings_update;
mod vision;

pub use general::{General, RelayConfig};
pub use meeting::MeetingConfig;
pub use provider::{AssistantProfile, PresetConfig, ProviderConfig, ProviderKind, TaskConfig};
pub use settings_update::{SettingsUpdate, apply_settings};
pub use vision::VisionConfig;

pub const EXAMPLE_CONFIG: &str = include_str!("../../config.example.toml");

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
    #[serde(default)]
    pub relay: RelayConfig,
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
                task: provider::default_task(),
            });
        }
        let known: Vec<_> = self.presets.keys().map(String::as_str).collect();
        bail!(
            "preset \"{name}\" is not configured (configured presets: [{}], built-in: [explain-screen])",
            known.join(", ")
        )
    }
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
}
