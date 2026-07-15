//! Configuration loading for ~/.config/nexora/config.toml.

use std::collections::HashMap;
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
    620
}
fn default_height() -> i32 {
    440
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Anthropic,
    Openai,
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
        let mut names: Vec<String> = if self.providers.is_empty() {
            Self::example().providers.keys().cloned().collect()
        } else {
            self.providers.keys().cloned().collect()
        };
        names.sort();
        names
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
    /// Task being configured (usually "ask").
    pub task: String,
    pub provider: String,
    pub model: String,
    /// When `Some` and non-empty, stored as the provider's literal api_key.
    pub api_key: Option<String>,
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

    let tasks = table(&mut doc, "tasks");
    let task = subtable(tasks, &update.task);
    task["provider"] = value(update.provider.clone());
    task["model"] = value(update.model.clone());

    if let Some(key) = &update.api_key
        && !key.is_empty()
    {
        let providers = table(&mut doc, "providers");
        let provider = subtable(providers, &update.provider);
        provider["api_key"] = value(key.clone());
    }

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
        let task = config.task("ask").unwrap();
        config.provider_for(task).unwrap();
    }

    #[test]
    fn empty_config_uses_defaults() {
        let config: Config = toml::from_str("").unwrap();
        assert!(config.general.hidden);
        assert_eq!(config.general.layer_shell, "auto");
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
            task: "ask".into(),
            provider: "openrouter".into(),
            model: "some/model".into(),
            api_key: Some("sk-secret".into()),
        };
        apply_settings(&update).unwrap();

        let config = Config::load().unwrap();
        assert!(!config.general.hidden);
        let task = config.task("ask").unwrap();
        assert_eq!(task.provider, "openrouter");
        assert_eq!(task.model, "some/model");
        // The provider (seeded from the example) now carries the literal key.
        let provider = config.provider_for(task).unwrap();
        assert_eq!(provider.resolve_api_key().unwrap(), "sk-secret");

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
        };
        assert_eq!(provider.base_url(), "http://localhost:11434/v1");
    }
}
