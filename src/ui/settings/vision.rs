//! "Vision & OCR" settings page: screen-understanding mode and the local
//! Ollama vision-model manager.

use gtk4 as gtk;
use gtk4::prelude::*;

use super::provider::ollama_library_controls;
use super::widgets::{entry_with, field_row, note_label, section_heading};
use crate::config::{Config, VisionConfig};
use crate::ui::overlay::Overlay;
use crate::vision;

pub(super) struct VisionSettingsWidgets {
    pub(super) mode: gtk::DropDown,
    pub(super) provider: gtk::DropDown,
    pub(super) model: gtk::Entry,
    pub(super) catalog: gtk::DropDown,
    pub(super) ollama_url: gtk::Entry,
    pub(super) prompt: gtk::TextView,
    pub(super) installed: gtk::DropDown,
    pub(super) refresh: gtk::Button,
    pub(super) download: gtk::Button,
    pub(super) delete: gtk::Button,
    pub(super) progress: gtk::ProgressBar,
    pub(super) status: gtk::Label,
}

impl Overlay {
    pub(super) fn build_vision_settings(
        &self,
        config: &Config,
        provider_names: &[String],
    ) -> VisionSettingsWidgets {
        let settings = &config.vision;
        let mode = gtk::DropDown::from_strings(&[
            "Direct to analysis model",
            "Vision/OCR proxy",
            "Screen analysis off",
        ]);
        mode.set_selected(match settings.mode.as_str() {
            "proxy" => 1,
            "off" => 2,
            _ => 0,
        });
        let provider_values: Vec<&str> = provider_names.iter().map(String::as_str).collect();
        let provider = gtk::DropDown::from_strings(&provider_values);
        if let Some(index) = provider_names
            .iter()
            .position(|name| *name == settings.provider)
        {
            provider.set_selected(index as u32);
        }
        let model = entry_with(&settings.model, "qwen3-vl:4b");
        let catalog_labels: Vec<String> = vision::PRESETS
            .iter()
            .map(|preset| {
                format!(
                    "{} · {} · {} — {}",
                    preset.id, preset.download, preset.size, preset.description
                )
            })
            .collect();
        let catalog_values: Vec<&str> = catalog_labels.iter().map(String::as_str).collect();
        let catalog = gtk::DropDown::from_strings(&catalog_values);
        catalog.set_enable_search(true);
        if let Some(index) = vision::PRESETS
            .iter()
            .position(|preset| preset.id == settings.model)
        {
            catalog.set_selected(index as u32);
        }
        let model_for_catalog = model.clone();
        catalog.connect_selected_notify(move |dropdown| {
            if let Some(item) = super::provider::dropdown_text(dropdown)
                && let Some(id) = item.split(" · ").next()
            {
                model_for_catalog.set_text(id);
            }
        });

        let ollama_url = entry_with(&settings.ollama_url, "http://localhost:11434");
        let prompt = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .height_request(100)
            .build();
        prompt.add_css_class("nexora-response");
        prompt.buffer().set_text(&settings.prompt);
        let library = ollama_library_controls(&ollama_url, &model);

        VisionSettingsWidgets {
            mode,
            provider,
            model,
            catalog,
            ollama_url,
            prompt,
            installed: library.installed,
            refresh: library.refresh,
            download: library.download,
            delete: library.delete,
            progress: library.progress,
            status: library.status,
        }
    }
}

pub(super) fn append_vision_fields(page: &gtk::Box, vision: &VisionSettingsWidgets) {
    page.append(&note_label(
        "Use a vision proxy when the main model is text-only (for example DeepSeek). The screenshot is converted locally or remotely into compact OCR text before the main request.",
    ));
    page.append(&field_row("Screen analysis mode", &vision.mode));
    page.append(&field_row("Vision provider", &vision.provider));
    page.append(&field_row("Vision model", &vision.model));
    page.append(&field_row("Curated local models", &vision.catalog));
    page.append(&field_row("Ollama URL", &vision.ollama_url));
    page.append(&field_row("Vision/OCR prompt", &vision.prompt));
    page.append(&section_heading("Local model manager"));
    page.append(&field_row("Installed models", &vision.installed));
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.append(&vision.refresh);
    actions.append(&vision.download);
    actions.append(&vision.delete);
    page.append(&actions);
    page.append(&vision.progress);
    page.append(&vision.status);
    page.append(&note_label(
        "Recommended: Qwen3-VL 4B. The 2B variant is faster; 8B improves small-text OCR. Download size is not the same as total RAM/VRAM use.",
    ));
}

/// Validate the vision settings widgets and build the config to save.
pub(super) fn validate_and_build_vision(
    widgets: &VisionSettingsWidgets,
    provider_names: &[String],
) -> Result<VisionConfig, &'static str> {
    let vision_provider_index = widgets.provider.selected() as usize;
    let Some(vision_provider) = provider_names.get(vision_provider_index).cloned() else {
        return Err("pick a vision provider");
    };
    let vision_prompt_buffer = widgets.prompt.buffer();
    let vision_prompt = vision_prompt_buffer
        .text(
            &vision_prompt_buffer.start_iter(),
            &vision_prompt_buffer.end_iter(),
            false,
        )
        .to_string();
    if widgets.model.text().trim().is_empty() || vision_prompt.trim().is_empty() {
        return Err("enter a vision model and OCR prompt");
    }
    Ok(VisionConfig {
        mode: match widgets.mode.selected() {
            1 => "proxy",
            2 => "off",
            _ => "direct",
        }
        .into(),
        provider: vision_provider,
        model: widgets.model.text().trim().into(),
        prompt: vision_prompt,
        ollama_url: widgets.ollama_url.text().trim().into(),
    })
}
