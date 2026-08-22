//! Chat providers, tasks (provider+model bindings), presets, and profiles.

use anyhow::{Context, Result, bail};
use serde::Deserialize;

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

/// Also used by `MeetingConfig::analysis_task`.
pub(super) fn default_task() -> String {
    "ask".into()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssistantProfile {
    pub system: String,
}

#[cfg(test)]
mod tests {
    use super::*;

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
