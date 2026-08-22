//! Applying in-app preference-panel edits back to `config.toml`.

use anyhow::{Context, Result};
use toml_edit::{DocumentMut, Item, Table, value};

use super::meeting::MeetingConfig;
use super::provider::ProviderKind;
use super::vision::VisionConfig;
use super::{EXAMPLE_CONFIG, config_dir, config_path};

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

/// Ensure a table exists at `doc[key]`, creating it if missing.
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

fn apply_general(doc: &mut DocumentMut, update: &SettingsUpdate) {
    table(doc, "general")["hidden"] = value(update.hidden);
    table(doc, "general")["hyprland_rule"] = value(update.hyprland_rule.clone());
    table(doc, "general")["layer_shell"] = value(update.layer_shell.clone());
    table(doc, "general")["width"] = value(update.width as i64);
    table(doc, "general")["height"] = value(update.height as i64);
}

fn apply_task_and_provider(doc: &mut DocumentMut, update: &SettingsUpdate) {
    let tasks = table(doc, "tasks");
    let task = subtable(tasks, &update.task);
    task["provider"] = value(update.provider.clone());
    task["model"] = value(update.model.clone());

    let providers = table(doc, "providers");
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
}

fn apply_meeting(doc: &mut DocumentMut, update: &SettingsUpdate) {
    let meeting = table(doc, "meeting");
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
}

fn apply_vision(doc: &mut DocumentMut, update: &SettingsUpdate) {
    let vision = table(doc, "vision");
    vision["mode"] = value(update.vision.mode.clone());
    vision["provider"] = value(update.vision.provider.clone());
    vision["model"] = value(update.vision.model.clone());
    vision["prompt"] = value(update.vision.prompt.clone());
    vision["ollama_url"] = value(update.vision.ollama_url.clone());
}

fn apply_profile(doc: &mut DocumentMut, update: &SettingsUpdate) {
    let profiles = table(doc, "profiles");
    let profile = subtable(profiles, &update.profile_name);
    profile["system"] = value(update.profile_system.clone());
}

fn load_or_seed_document(path: &std::path::Path) -> Result<DocumentMut> {
    if path.exists() {
        std::fs::read_to_string(path)?
            .parse()
            .with_context(|| format!("parsing {}", path.display()))
    } else {
        std::fs::create_dir_all(config_dir())?;
        Ok(EXAMPLE_CONFIG.parse()?)
    }
}

fn write_document(path: &std::path::Path, doc: &DocumentMut) -> Result<()> {
    std::fs::write(path, doc.to_string()).with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Apply settings to config.toml without discarding comments or unrelated keys.
///
/// Creates the file from the bundled example if it does not exist yet.
pub fn apply_settings(update: &SettingsUpdate) -> Result<()> {
    let path = config_path();
    let mut doc = load_or_seed_document(&path)?;
    apply_general(&mut doc, update);
    apply_task_and_provider(&mut doc, update);
    apply_meeting(&mut doc, update);
    apply_vision(&mut doc, update);
    apply_profile(&mut doc, update);
    write_document(&path, &doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

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
}
