//! "AI providers" settings page: provider cards, model discovery, and the
//! shared local-Ollama-library widget used here and on the Vision page.

use std::rc::Rc;
use std::time::Duration;

use gtk4 as gtk;
use gtk4::glib;
use gtk4::prelude::*;

use super::widgets::{entry_with, field_row, note_label};
use crate::config::{Config, ProviderKind};
use crate::runtime;
use crate::ui::overlay::Overlay;
use crate::vision;

pub(super) struct ProviderSettingsWidgets {
    pub(super) cards: gtk::FlowBox,
    pub(super) kind: gtk::DropDown,
    pub(super) base_url: gtk::Entry,
    pub(super) api_key_env: gtk::Entry,
    pub(super) api_key: gtk::Entry,
    pub(super) clear_api_key: gtk::CheckButton,
    pub(super) model: gtk::Entry,
    pub(super) model_choices: gtk::DropDown,
    pub(super) refresh_models: gtk::Button,
    pub(super) model_status: gtk::Label,
    pub(super) thinking: gtk::DropDown,
    pub(super) reasoning_effort: gtk::DropDown,
    pub(super) token_notice: gtk::Label,
}

impl Overlay {
    pub(super) fn build_provider_settings(
        self: &Rc<Self>,
        config: &Config,
        provider_names: &[String],
    ) -> ProviderSettingsWidgets {
        let widgets = new_provider_widgets(provider_names);
        select_initial_provider(config, provider_names, &widgets);
        wire_provider_card_selection(config, provider_names, &widgets);
        wire_effort_controls(provider_names, &widgets);
        wire_model_entry(provider_names, &widgets);
        wire_model_choice_selection(&widgets);
        self.wire_refresh_models(provider_names, &widgets);
        widgets
    }

    /// Refresh-models button: pull the provider's live `/models` catalog.
    fn wire_refresh_models(
        self: &Rc<Self>,
        provider_names: &[String],
        widgets: &ProviderSettingsWidgets,
    ) {
        let this = Rc::clone(self);
        let cards = widgets.cards.clone();
        let names = provider_names.to_vec();
        let kind = widgets.kind.clone();
        let base_url = widgets.base_url.clone();
        let api_key_env = widgets.api_key_env.clone();
        let api_key = widgets.api_key.clone();
        let model_choices = widgets.model_choices.clone();
        let current_model = widgets.model.clone();
        let model_status = widgets.model_status.clone();
        widgets.refresh_models.connect_clicked(move |_| {
            let Some(child) = cards.selected_children().first().cloned() else {
                return;
            };
            let Some(name) = names.get(child.index() as usize).cloned() else {
                return;
            };
            let configured = this.config.borrow().provider(&name);
            let literal = api_key.text().trim().to_string();
            let provider = crate::config::ProviderConfig {
                kind: if kind.selected() == 1 {
                    ProviderKind::Anthropic
                } else {
                    ProviderKind::Openai
                },
                base_url: nonempty(base_url.text().as_str()),
                api_key: if literal.is_empty() {
                    configured
                        .as_ref()
                        .and_then(|provider| provider.api_key.clone())
                } else {
                    Some(literal)
                },
                api_key_env: nonempty(api_key_env.text().as_str()),
                default_model: None,
                thinking: None,
                reasoning_effort: None,
            };
            model_status.set_text("Loading models…");
            let (tx, rx) = async_channel::bounded(1);
            runtime().spawn(async move {
                let _ = tx
                    .send(crate::providers::list_models(&provider).await)
                    .await;
            });
            let choices = model_choices.clone();
            let current = current_model.clone();
            let status = model_status.clone();
            glib::spawn_future_local(async move {
                match rx.recv().await {
                    Ok(Ok(models)) => {
                        set_model_choices(&choices, &models, current.text().as_str());
                        status.set_text(&format!("{} models available", models.len()));
                    }
                    Ok(Err(err)) => status.set_text(&format!("Could not list models: {err:#}")),
                    Err(_) => status.set_text("Model lookup was interrupted"),
                }
            });
        });
    }
}

fn new_provider_widgets(provider_names: &[String]) -> ProviderSettingsWidgets {
    let cards = gtk::FlowBox::new();
    cards.set_selection_mode(gtk::SelectionMode::Single);
    cards.set_min_children_per_line(2);
    cards.set_max_children_per_line(3);
    cards.set_row_spacing(8);
    cards.set_column_spacing(8);
    cards.add_css_class("provider-grid");
    for name in provider_names {
        cards.insert(&provider_card(name), -1);
    }

    let kind = gtk::DropDown::from_strings(&["OpenAI-compatible", "Anthropic"]);
    let base_url = entry_with("", "Blank = protocol default endpoint");
    let api_key_env = entry_with("", "For example OPENAI_API_KEY");
    let api_key = gtk::Entry::builder()
        .placeholder_text("Leave blank to keep the stored key")
        .visibility(false)
        .hexpand(true)
        .build();
    api_key.add_css_class("nexora-entry");
    let clear_api_key = gtk::CheckButton::with_label("Remove stored literal API key");
    let model = entry_with("", "Default chat model for this provider");
    let model_choices = gtk::DropDown::from_strings(&["Select a discovered model…"]);
    model_choices.set_enable_search(true);
    let refresh_models = gtk::Button::with_label("Refresh model list");
    refresh_models.add_css_class("nexora-attach");
    let model_status = note_label("Use Refresh to query this provider's current /models catalog.");
    let thinking =
        gtk::DropDown::from_strings(&["Provider default", "Thinking enabled", "Thinking disabled"]);
    let reasoning_effort = gtk::DropDown::from_strings(&["Provider default"]);
    let token_notice = note_label("");
    token_notice.add_css_class("token-notice");

    ProviderSettingsWidgets {
        cards,
        kind,
        base_url,
        api_key_env,
        api_key,
        clear_api_key,
        model,
        model_choices,
        refresh_models,
        model_status,
        thinking,
        reasoning_effort,
        token_notice,
    }
}

fn select_initial_provider(
    config: &Config,
    provider_names: &[String],
    widgets: &ProviderSettingsWidgets,
) {
    let selected = config
        .task("ask")
        .ok()
        .and_then(|task| {
            provider_names
                .iter()
                .position(|name| *name == task.provider)
        })
        .unwrap_or(0) as i32;
    if let Some(child) = widgets.cards.child_at_index(selected) {
        widgets.cards.select_child(&child);
    }
    if let Some(name) = provider_names.get(selected as usize) {
        populate_provider_fields(config, name, widgets);
    }
}

fn wire_provider_card_selection(
    config: &Config,
    provider_names: &[String],
    widgets: &ProviderSettingsWidgets,
) {
    let config_for_selection = config.clone();
    let names_for_selection = provider_names.to_vec();
    let kind_for_selection = widgets.kind.clone();
    let base_url_for_selection = widgets.base_url.clone();
    let api_key_env_for_selection = widgets.api_key_env.clone();
    let api_key_for_selection = widgets.api_key.clone();
    let clear_key_for_selection = widgets.clear_api_key.clone();
    let model_for_selection = widgets.model.clone();
    let model_choices_for_selection = widgets.model_choices.clone();
    let thinking_for_selection = widgets.thinking.clone();
    let reasoning_for_selection = widgets.reasoning_effort.clone();
    let token_notice_for_selection = widgets.token_notice.clone();
    widgets
        .cards
        .connect_selected_children_changed(move |cards| {
            let Some(child) = cards.selected_children().first().cloned() else {
                return;
            };
            let Some(name) = names_for_selection.get(child.index() as usize) else {
                return;
            };
            let temporary = ProviderSettingsWidgets {
                cards: cards.clone(),
                kind: kind_for_selection.clone(),
                base_url: base_url_for_selection.clone(),
                api_key_env: api_key_env_for_selection.clone(),
                api_key: api_key_for_selection.clone(),
                clear_api_key: clear_key_for_selection.clone(),
                model: model_for_selection.clone(),
                model_choices: model_choices_for_selection.clone(),
                refresh_models: gtk::Button::new(),
                model_status: gtk::Label::new(None),
                thinking: thinking_for_selection.clone(),
                reasoning_effort: reasoning_for_selection.clone(),
                token_notice: token_notice_for_selection.clone(),
            };
            populate_provider_fields(&config_for_selection, name, &temporary);
        });
}

fn wire_effort_controls(provider_names: &[String], widgets: &ProviderSettingsWidgets) {
    for control in [&widgets.thinking, &widgets.reasoning_effort] {
        let cards = widgets.cards.clone();
        let names = provider_names.to_vec();
        let model = widgets.model.clone();
        let thinking = widgets.thinking.clone();
        let effort = widgets.reasoning_effort.clone();
        let notice = widgets.token_notice.clone();
        control.connect_selected_notify(move |_| {
            if let Some(name) = selected_provider_name(&cards, &names) {
                update_token_notice(&name, model.text().as_str(), &thinking, &effort, &notice);
            }
        });
    }
}

fn wire_model_entry(provider_names: &[String], widgets: &ProviderSettingsWidgets) {
    let cards_for_model = widgets.cards.clone();
    let names_for_model = provider_names.to_vec();
    let thinking_for_model = widgets.thinking.clone();
    let effort_for_model = widgets.reasoning_effort.clone();
    let notice_for_model = widgets.token_notice.clone();
    widgets.model.connect_changed(move |model| {
        if let Some(name) = selected_provider_name(&cards_for_model, &names_for_model) {
            let selected = selected_effort(&effort_for_model);
            set_effort_choices(
                &effort_for_model,
                &name,
                model.text().as_str(),
                selected.as_deref(),
            );
            update_token_notice(
                &name,
                model.text().as_str(),
                &thinking_for_model,
                &effort_for_model,
                &notice_for_model,
            );
        }
    });
}

fn wire_model_choice_selection(widgets: &ProviderSettingsWidgets) {
    let model_entry = widgets.model.clone();
    widgets
        .model_choices
        .connect_selected_notify(move |dropdown| {
            let Some(item) = dropdown.selected_item() else {
                return;
            };
            let Ok(item) = item.downcast::<gtk::StringObject>() else {
                return;
            };
            let selected = item.string();
            if !selected.starts_with("Select ") {
                model_entry.set_text(&selected);
            }
        });
}

fn provider_card(name: &str) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    card.add_css_class("provider-card");
    let icon = gtk::Label::new(Some(provider_figure(name)));
    icon.add_css_class("provider-figure");
    icon.add_css_class(&format!("provider-{}", css_name(name)));
    let label = gtk::Label::new(Some(&provider_title(name)));
    label.add_css_class("provider-name");
    label.set_xalign(0.0);
    card.append(&icon);
    card.append(&label);
    card
}

fn provider_figure(name: &str) -> &'static str {
    match name {
        "anthropic" => "A",
        "openai" => "◎",
        "openrouter" => "↗",
        "deepseek" => "D",
        "gemini" => "✦",
        "ollama" => "◉",
        _ => "AI",
    }
}

fn provider_title(name: &str) -> String {
    match name {
        "openai" => "OpenAI".into(),
        "openrouter" => "OpenRouter".into(),
        "deepseek" => "DeepSeek".into(),
        "gemini" => "Gemini".into(),
        "ollama" => "Ollama".into(),
        "anthropic" => "Anthropic".into(),
        other => other.to_string(),
    }
}

fn css_name(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect()
}

fn populate_provider_fields(config: &Config, name: &str, widgets: &ProviderSettingsWidgets) {
    let Some(provider) = config.provider(name) else {
        return;
    };
    widgets.kind.set_selected(match provider.kind {
        ProviderKind::Openai => 0,
        ProviderKind::Anthropic => 1,
    });
    widgets
        .base_url
        .set_text(provider.base_url.as_deref().unwrap_or(""));
    widgets
        .api_key_env
        .set_text(provider.api_key_env.as_deref().unwrap_or(""));
    widgets.api_key.set_text("");
    widgets
        .api_key
        .set_placeholder_text(Some(if provider.api_key.is_some() {
            "A literal key is stored; leave blank to keep it"
        } else {
            "Paste a literal API key (optional)"
        }));
    widgets.clear_api_key.set_active(false);
    let task_model = config
        .task("ask")
        .ok()
        .filter(|task| task.provider == name)
        .map(|task| task.model.as_str());
    let current_model = provider
        .default_model
        .as_deref()
        .or(task_model)
        .unwrap_or("");
    widgets.model.set_text(current_model);
    let mut choices: Vec<String> = curated_models(name)
        .iter()
        .map(|model| (*model).to_string())
        .collect();
    if !current_model.is_empty() && !choices.iter().any(|model| model == current_model) {
        choices.insert(0, current_model.to_string());
    }
    set_model_choices(&widgets.model_choices, &choices, current_model);
    widgets.thinking.set_sensitive(matches!(
        name.to_ascii_lowercase().as_str(),
        "anthropic" | "deepseek"
    ));
    widgets.thinking.set_selected(match provider.thinking {
        None => 0,
        Some(true) => 1,
        Some(false) => 2,
    });
    set_effort_choices(
        &widgets.reasoning_effort,
        name,
        current_model,
        provider.reasoning_effort.as_deref(),
    );
    update_token_notice(
        name,
        current_model,
        &widgets.thinking,
        &widgets.reasoning_effort,
        &widgets.token_notice,
    );
}

pub(super) fn append_provider_fields(page: &gtk::Box, provider: &ProviderSettingsWidgets) {
    page.append(&field_row("Protocol", &provider.kind));
    page.append(&field_row("Base URL", &provider.base_url));
    page.append(&field_row("Environment key", &provider.api_key_env));
    page.append(&field_row("API key", &provider.api_key));
    page.append(&field_row("Default model", &provider.model));
    page.append(&field_row("Available models", &provider.model_choices));
    page.append(&provider.refresh_models);
    page.append(&provider.model_status);
    page.append(&field_row("Thinking mode", &provider.thinking));
    page.append(&field_row("Reasoning effort", &provider.reasoning_effort));
    page.append(&provider.token_notice);
    page.append(&provider.clear_api_key);
    page.append(&note_label(
        "Literal keys are stored in config.toml with mode 0600. Environment variables are preferred.",
    ));
}

pub(super) fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

pub(super) fn selected_provider_name(cards: &gtk::FlowBox, names: &[String]) -> Option<String> {
    let child = cards.selected_children().first().cloned()?;
    names.get(child.index() as usize).cloned()
}

pub(super) fn dropdown_text(dropdown: &gtk::DropDown) -> Option<String> {
    dropdown
        .selected_item()?
        .downcast::<gtk::StringObject>()
        .ok()
        .map(|item| item.string().to_string())
}

pub(super) fn selected_effort(dropdown: &gtk::DropDown) -> Option<String> {
    let label = dropdown_text(dropdown)?;
    if label.starts_with("None") {
        Some("none".into())
    } else if label.starts_with("Low") {
        Some("low".into())
    } else if label.starts_with("Medium") {
        Some("medium".into())
    } else if label.starts_with("High") {
        Some("high".into())
    } else if label.starts_with("Extra high") {
        Some("xhigh".into())
    } else if label.starts_with("Maximum") {
        Some("max".into())
    } else {
        None
    }
}

pub(super) fn set_effort_choices(
    dropdown: &gtk::DropDown,
    provider: &str,
    model: &str,
    selected: Option<&str>,
) {
    let provider = provider.to_ascii_lowercase();
    let model = model.to_ascii_lowercase();
    let options: &[&str] = match provider.as_str() {
        "deepseek" => &[
            "Provider default (High)",
            "High · more reasoning tokens",
            "Maximum · highest token use",
        ],
        "anthropic" => &[
            "Provider default",
            "Low · fewer reasoning tokens",
            "Medium · balanced",
            "High · more reasoning tokens",
            "Extra high · much higher use",
            "Maximum · highest token use",
        ],
        "openai" if model.contains("pro") => &[
            "Provider/model default",
            "Medium · lowest supported level",
            "High · more reasoning tokens",
            "Extra high · much higher use",
        ],
        "openai" if model.contains("gpt-5.1") => &[
            "Provider/model default",
            "None · least token use",
            "Low · fewer reasoning tokens",
            "Medium · balanced",
            "High · more reasoning tokens",
        ],
        "openai"
            if model.contains("gpt-5.2")
                || model.contains("gpt-5.4")
                || model.contains("gpt-5.5") =>
        {
            &[
                "Provider/model default",
                "None · least token use",
                "Low · fewer reasoning tokens",
                "Medium · balanced",
                "High · more reasoning tokens",
                "Extra high · highest supported level",
            ]
        }
        "openai" => &[
            "Provider/model default",
            "None · least token use",
            "Low · fewer reasoning tokens",
            "Medium · balanced",
            "High · more reasoning tokens",
            "Extra high · much higher use",
            "Maximum · highest token use",
        ],
        "openrouter" => &[
            "Provider/model default",
            "None · least token use",
            "Low · fewer reasoning tokens",
            "Medium · balanced",
            "High · more reasoning tokens",
            "Extra high · much higher use",
            "Maximum · highest token use",
        ],
        _ => &["Provider/model default (capability unknown)"],
    };
    let store = gtk::StringList::new(options);
    dropdown.set_model(Some(&store));
    dropdown.set_sensitive(options.len() > 1);
    let wanted = selected.unwrap_or_default();
    let selected_index = options
        .iter()
        .position(|label| match wanted {
            "none" => label.starts_with("None"),
            "low" => label.starts_with("Low"),
            "medium" => label.starts_with("Medium"),
            "high" => label.starts_with("High"),
            "xhigh" => label.starts_with("Extra high"),
            "max" => label.starts_with("Maximum"),
            _ => label.starts_with("Provider"),
        })
        .unwrap_or(0);
    dropdown.set_selected(selected_index as u32);
}

pub(super) fn update_token_notice(
    provider: &str,
    model: &str,
    thinking: &gtk::DropDown,
    effort: &gtk::DropDown,
    notice: &gtk::Label,
) {
    for class in ["token-low", "token-medium", "token-high"] {
        notice.remove_css_class(class);
    }
    let provider = provider.to_ascii_lowercase();
    let effort = selected_effort(effort);
    let thinking_disabled = thinking.is_sensitive() && thinking.selected() == 2;
    let (class, message) = if thinking_disabled || effort.as_deref() == Some("none") {
        (
            "token-low",
            "LOWER TOKEN USE · Internal reasoning is disabled when this model honors the setting. Faster and cheaper, but complex answers may be weaker.".to_string(),
        )
    } else if provider == "deepseek" {
        if effort.as_deref() == Some("max") {
            (
                "token-high",
                "HIGH TOKEN USE · DeepSeek Maximum can generate substantially more billed reasoning/output tokens and adds latency. Reserve it for difficult tasks.".to_string(),
            )
        } else {
            (
                "token-medium",
                "MORE TOKEN USE · DeepSeek thinking defaults to enabled with High effort. Reasoning tokens are included in completion usage; Low/Medium are not real levels and map to High.".to_string(),
            )
        }
    } else if matches!(effort.as_deref(), Some("high" | "xhigh" | "max")) {
        (
            "token-high",
            "HIGHER TOKEN USE · This effort level allows more internal reasoning. It can improve difficult answers, but usually increases billed output tokens and latency.".to_string(),
        )
    } else if matches!(effort.as_deref(), Some("low" | "medium"))
        || (thinking.is_sensitive() && thinking.selected() == 1)
    {
        (
            "token-medium",
            "MODERATE TOKEN USE · Thinking is enabled. Internal reasoning counts toward output usage even when the full reasoning text is not visible.".to_string(),
        )
    } else {
        let support = match provider.as_str() {
            "openai" => "Support and the default effort depend on the selected OpenAI model.",
            "anthropic" => {
                "Adaptive thinking decides how much reasoning is useful for each request."
            }
            "openrouter" => "Support and billing depend on the routed model and provider.",
            "gemini" => {
                "This compatibility adapter does not expose Gemini-specific thinking controls yet."
            }
            "ollama" => {
                "Local models do not incur API charges, but reasoning still uses time and compute."
            }
            _ => "The endpoint does not advertise a standard reasoning-effort capability.",
        };
        (
            "token-medium",
            format!("MODEL DEFAULT · {support} Current model: {model}."),
        )
    };
    notice.add_css_class(class);
    notice.set_text(&message);
}

pub(super) fn set_model_choices(dropdown: &gtk::DropDown, models: &[String], selected: &str) {
    let values: Vec<&str> = if models.is_empty() {
        vec!["Refresh to discover models…"]
    } else {
        models.iter().map(String::as_str).collect()
    };
    let store = gtk::StringList::new(&values);
    dropdown.set_model(Some(&store));
    let selected = values
        .iter()
        .position(|model| *model == selected)
        .unwrap_or(0);
    dropdown.set_selected(selected as u32);
}

fn curated_models(provider: &str) -> &'static [&'static str] {
    match provider {
        "anthropic" => &[
            "claude-opus-4-8",
            "claude-sonnet-4-6",
            "claude-haiku-4-5-20251001",
        ],
        "deepseek" => &["deepseek-v4-pro", "deepseek-v4-flash"],
        "openai" => &[
            "gpt-5.6",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
            "gpt-5.4",
            "gpt-5-mini",
        ],
        "gemini" => &[
            "gemini-3.5-flash",
            "gemini-3.1-pro-preview",
            "gemini-2.5-flash",
            "gemini-2.5-pro",
        ],
        _ => &[],
    }
}

/// List/download/remove widgets for a local Ollama model library, shared by
/// the vision page and the chat-model manager. `url` is the registry
/// endpoint; `model` holds the tag the download button pulls.
pub(super) struct OllamaLibrary {
    pub(super) installed: gtk::DropDown,
    pub(super) refresh: gtk::Button,
    pub(super) download: gtk::Button,
    pub(super) delete: gtk::Button,
    pub(super) progress: gtk::ProgressBar,
    pub(super) status: gtk::Label,
}

pub(super) fn ollama_library_controls(
    ollama_url: &gtk::Entry,
    model: &gtk::Entry,
) -> OllamaLibrary {
    let installed = gtk::DropDown::from_strings(&["Refresh to list installed models…"]);
    installed.set_enable_search(true);
    let refresh = gtk::Button::with_label("Refresh installed models");
    refresh.add_css_class("nexora-attach");
    let download = gtk::Button::with_label("Download selected model");
    download.add_css_class("nexora-attach");
    let delete = gtk::Button::with_label("Remove installed model");
    delete.add_css_class("nexora-attach");
    let progress = gtk::ProgressBar::new();
    progress.set_show_text(true);
    progress.set_visible(false);
    let status = note_label(
        "Ollama must be running. Downloads come from its registry and remain on this computer.",
    );

    wire_ollama_refresh(&refresh, ollama_url, &installed, &status);
    wire_ollama_download(&download, ollama_url, model, &status, &progress);
    wire_ollama_delete(&delete, ollama_url, &installed, &status);

    OllamaLibrary {
        installed,
        refresh,
        download,
        delete,
        progress,
        status,
    }
}

fn wire_ollama_refresh(
    refresh: &gtk::Button,
    ollama_url: &gtk::Entry,
    installed: &gtk::DropDown,
    status: &gtk::Label,
) {
    let url_for_refresh = ollama_url.clone();
    let installed_for_refresh = installed.clone();
    let status_for_refresh = status.clone();
    refresh.connect_clicked(move |_| {
        status_for_refresh.set_text("Connecting to Ollama…");
        let url = url_for_refresh.text().to_string();
        let (tx, rx) = async_channel::bounded(1);
        runtime().spawn(async move {
            let _ = tx.send(vision::list_ollama_models(&url).await).await;
        });
        let dropdown = installed_for_refresh.clone();
        let status = status_for_refresh.clone();
        glib::spawn_future_local(async move {
            match rx.recv().await {
                Ok(Ok(models)) => {
                    let labels: Vec<String> = models
                        .iter()
                        .map(|model| {
                            format!("{} · {}", model.name, vision::format_bytes(model.bytes))
                        })
                        .collect();
                    let values: Vec<&str> = labels.iter().map(String::as_str).collect();
                    dropdown.set_model(Some(&gtk::StringList::new(&values)));
                    status.set_text(&format!("{} local models installed", models.len()));
                }
                Ok(Err(err)) => status.set_text(&format!("Ollama unavailable: {err:#}")),
                Err(_) => status.set_text("Ollama lookup was interrupted"),
            }
        });
    });
}

fn wire_ollama_download(
    download: &gtk::Button,
    ollama_url: &gtk::Entry,
    model: &gtk::Entry,
    status: &gtk::Label,
    progress: &gtk::ProgressBar,
) {
    let url_for_download = ollama_url.clone();
    let model_for_download = model.clone();
    let status_for_download = status.clone();
    let progress_for_download = progress.clone();
    let download_button = download.clone();
    download.connect_clicked(move |_| {
        let model = model_for_download.text().trim().to_string();
        if model.is_empty() {
            status_for_download.set_text("Select a model first");
            return;
        }
        let url = url_for_download.text().to_string();
        status_for_download.set_text(&format!("Downloading {model}…"));
        progress_for_download.set_fraction(0.0);
        progress_for_download.set_text(Some("Starting…"));
        progress_for_download.set_visible(true);
        download_button.set_sensitive(false);
        let (progress_tx, progress_rx) = async_channel::unbounded();
        let (done_tx, done_rx) = async_channel::bounded(1);
        runtime().spawn(async move {
            let result = vision::pull_ollama_model(&url, &model, progress_tx).await;
            let _ = done_tx.send(result).await;
        });
        let status = status_for_download.clone();
        let bar = progress_for_download.clone();
        let button = download_button.clone();
        glib::spawn_future_local(async move {
            loop {
                while let Ok(update) = progress_rx.try_recv() {
                    let text = match (update.completed, update.total) {
                        (Some(done), Some(total)) if total > 0 => {
                            bar.set_fraction(done as f64 / total as f64);
                            format!(
                                "{} · {:.0}%",
                                update.status,
                                done as f64 * 100.0 / total as f64
                            )
                        }
                        _ => update.status,
                    };
                    bar.set_text(Some(&text));
                }
                if let Ok(result) = done_rx.try_recv() {
                    button.set_sensitive(true);
                    match result {
                        Ok(()) => {
                            bar.set_fraction(1.0);
                            bar.set_text(Some("Complete"));
                            status.set_text(
                                "Model downloaded. Refresh the installed list to verify it.",
                            );
                        }
                        Err(err) => {
                            bar.set_visible(false);
                            status.set_text(&format!("Download failed: {err:#}"));
                        }
                    }
                    break;
                }
                glib::timeout_future(Duration::from_millis(100)).await;
            }
        });
    });
}

fn wire_ollama_delete(
    delete: &gtk::Button,
    ollama_url: &gtk::Entry,
    installed: &gtk::DropDown,
    status: &gtk::Label,
) {
    let url_for_delete = ollama_url.clone();
    let installed_for_delete = installed.clone();
    let status_for_delete = status.clone();
    delete.connect_clicked(move |_| {
        let Some(label) = dropdown_text(&installed_for_delete) else {
            status_for_delete.set_text("Refresh and select an installed model first");
            return;
        };
        let Some(model) = label.split(" · ").next().map(str::to_string) else {
            return;
        };
        if model.starts_with("Refresh ") {
            status_for_delete.set_text("Refresh and select an installed model first");
            return;
        }
        let url = url_for_delete.text().to_string();
        status_for_delete.set_text(&format!("Removing {model}…"));
        let (tx, rx) = async_channel::bounded(1);
        runtime().spawn(async move {
            let _ = tx
                .send(vision::delete_ollama_model(&url, &model).await)
                .await;
        });
        let status = status_for_delete.clone();
        glib::spawn_future_local(async move {
            match rx.recv().await {
                Ok(Ok(())) => status.set_text("Model removed"),
                Ok(Err(err)) => status.set_text(&format!("Remove failed: {err:#}")),
                Err(_) => status.set_text("Remove operation was interrupted"),
            }
        });
    });
}
