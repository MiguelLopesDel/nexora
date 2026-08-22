//! Screen-understanding configuration. `direct` sends the image to the task
//! model; `proxy` first converts it to text with a separate vision model.

use serde::Deserialize;

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
