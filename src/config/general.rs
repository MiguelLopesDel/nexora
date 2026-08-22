//! Window/runtime settings and the local relay's own configuration.

use serde::Deserialize;

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

/// Also used by `MeetingConfig` and `RelayConfig` fields that default to true.
pub(super) fn default_true() -> bool {
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

/// Settings for `nexora relay`: a local OpenAI-compatible intermediary that
/// adds web search, page reading, and history compaction on top of providers
/// that have none of it server-side (DeepSeek, OpenRouter, Ollama, …).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayConfig {
    /// Provider key under [providers] the relay forwards requests to.
    #[serde(default = "default_relay_upstream")]
    pub upstream: String,
    /// Listen port on 127.0.0.1. Point a provider at http://127.0.0.1:<port>/v1.
    #[serde(default = "default_relay_port")]
    pub port: u16,
    /// "auto" picks searxng when its URL is set, then brave when its key is
    /// set, then duckduckgo. Or force: "searxng", "duckduckgo", "brave", "off".
    #[serde(default = "default_relay_search")]
    pub search: String,
    /// Base URL of a SearxNG instance (JSON API), e.g. http://localhost:8888.
    #[serde(default)]
    pub searxng_url: String,
    /// Environment variable holding a Brave Search API key.
    #[serde(default = "default_brave_key_env")]
    pub brave_api_key_env: String,
    /// Maximum search tool rounds per question.
    #[serde(default = "default_search_rounds")]
    pub max_search_rounds: u32,
    /// Search hits handed to the model per query.
    #[serde(default = "default_search_results")]
    pub max_results: usize,
    /// Also fetch the top result and hand the model its extracted text.
    #[serde(default = "default_true")]
    pub fetch_pages: bool,
    /// Summarize older turns once the conversation exceeds this many
    /// characters, so long chats keep fitting the upstream context.
    #[serde(default = "default_compact_over_chars")]
    pub compact_over_chars: usize,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            upstream: default_relay_upstream(),
            port: default_relay_port(),
            search: default_relay_search(),
            searxng_url: String::new(),
            brave_api_key_env: default_brave_key_env(),
            max_search_rounds: default_search_rounds(),
            max_results: default_search_results(),
            fetch_pages: true,
            compact_over_chars: default_compact_over_chars(),
        }
    }
}

fn default_relay_upstream() -> String {
    "ollama".into()
}
fn default_relay_port() -> u16 {
    8787
}
fn default_relay_search() -> String {
    "auto".into()
}
fn default_brave_key_env() -> String {
    "BRAVE_API_KEY".into()
}
fn default_search_rounds() -> u32 {
    3
}
fn default_search_results() -> usize {
    5
}
fn default_compact_over_chars() -> usize {
    24_000
}
