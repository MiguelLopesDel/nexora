//! The settings page: sidebar-navigated sections for general window
//! behavior, AI providers, the live meeting assistant, vision/OCR, assistant
//! profiles, and shortcuts/privacy.

mod general;
mod meeting;
mod provider;
mod vision;
mod widgets;

use std::rc::Rc;

use gtk4 as gtk;
use gtk4::prelude::*;

use general::{GeneralSettingsWidgets, append_general_fields, build_general_settings};
use meeting::{
    MeetingSettingsWidgets, append_meeting_fields, append_profile_fields,
    meeting_config_from_widgets, validate_meeting_widgets,
};
use provider::{ProviderSettingsWidgets, append_provider_fields, nonempty};
use vision::{VisionSettingsWidgets, append_vision_fields, validate_and_build_vision};
use widgets::{note_label, settings_page, settings_scroll};

use crate::config::{Config, ProviderKind, SettingsUpdate};
use crate::hidden::{self, HiddenState};
use crate::ui::overlay::Overlay;
use crate::ui::window::apply_badge_style;

/// Widgets of the settings panel, kept so Save can read them back.
pub(super) struct SettingsWidgets {
    provider_names: Vec<String>,
    provider: ProviderSettingsWidgets,
    general: GeneralSettingsWidgets,
    meeting: MeetingSettingsWidgets,
    vision: VisionSettingsWidgets,
    feedback: gtk::Label,
}

impl Overlay {
    pub(super) fn open_settings(self: &Rc<Self>) {
        if self.settings.borrow().is_none() {
            let widgets = self.build_settings_page();
            *self.settings.borrow_mut() = Some(widgets);
        }
        self.stack.set_visible_child_name("settings");
    }

    fn build_settings_page(self: &Rc<Self>) -> SettingsWidgets {
        let config = self.config.borrow().clone();
        let provider_names = config.provider_names();
        let provider = self.build_provider_settings(&config, &provider_names);
        let general = build_general_settings(&config);
        let meeting = self.build_meeting_settings(&config, &provider_names);
        let vision = self.build_vision_settings(&config, &provider_names);

        let pages = gtk::Stack::builder()
            .hexpand(true)
            .vexpand(true)
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();

        let general_page = settings_page("Window behavior");
        append_general_fields(&general_page, &general);
        let interaction_note = note_label(
            "On Wayland the overlay now requests keyboard focus only while you interact with it. Click another window to keep Nexora visible while returning keyboard input to that app. Window mode changes apply after restart.",
        );
        general_page.append(&interaction_note);
        pages.add_titled(&settings_scroll(&general_page), Some("general"), "General");

        let provider_page = settings_page("AI providers");
        provider_page.append(&note_label(
            "Select a provider card, then configure only that provider below.",
        ));
        provider_page.append(&provider.cards);
        provider_page.append(&widgets::section_heading("Selected provider"));
        append_provider_fields(&provider_page, &provider);
        meeting::append_local_chat_models(&provider_page, &config.vision.ollama_url);
        pages.add_titled(
            &settings_scroll(&provider_page),
            Some("providers"),
            "Providers",
        );

        let meeting_page = settings_page("Live meeting assistant");
        append_meeting_fields(&meeting_page, &meeting);
        pages.add_titled(&settings_scroll(&meeting_page), Some("meeting"), "Meeting");

        let vision_page = settings_page("Vision & OCR");
        append_vision_fields(&vision_page, &vision);
        pages.add_titled(
            &settings_scroll(&vision_page),
            Some("vision"),
            "Vision & OCR",
        );

        let profile_page = settings_page("Assistant profiles");
        profile_page.append(&note_label(
            "Pick a template or enter a new profile name, edit its prompt, and save.",
        ));
        append_profile_fields(&profile_page, &meeting);
        pages.add_titled(
            &settings_scroll(&profile_page),
            Some("profiles"),
            "Profiles",
        );

        let privacy_page = settings_page("Shortcuts and privacy");
        let hidden_note = note_label(&hidden::status_report());
        privacy_page.append(&hidden_note);
        privacy_page.append(&note_label(
            "Esc hides Nexora. Reopen it with the same global shortcut bound to `nexora toggle`. Hyprland handles compositor shortcuts before applications, so Nexora cannot suppress a window-management bind aimed at the focused window below.",
        ));
        privacy_page.append(&self.keybind_section());
        pages.add_titled(
            &settings_scroll(&privacy_page),
            Some("shortcuts"),
            "Shortcuts",
        );

        let sidebar = gtk::StackSidebar::new();
        sidebar.set_stack(&pages);
        sidebar.set_width_request(150);
        sidebar.add_css_class("nexora-sidebar");

        let body = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        body.set_vexpand(true);
        body.append(&sidebar);
        body.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        body.append(&pages);

        let feedback = gtk::Label::new(None);
        feedback.add_css_class("nexora-status");
        feedback.set_xalign(0.0);
        feedback.set_wrap(true);

        let save = gtk::Button::with_label("Save settings");
        save.add_css_class("nexora-attach");
        save.set_halign(gtk::Align::End);
        let this = Rc::clone(self);
        save.connect_clicked(move |_| this.save_settings());

        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        footer.append(&feedback);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        footer.append(&spacer);
        footer.append(&save);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
        root.set_size_request(740, 450);
        root.append(&body);
        root.append(&footer);
        self.stack.add_named(&root, Some("settings"));

        SettingsWidgets {
            provider,
            provider_names,
            general,
            meeting,
            vision,
            feedback,
        }
    }

    /// Keybinds are bound in the compositor, not the app (no portable global
    /// hotkey exists on Wayland); show copyable snippets instead.
    fn keybind_section(self: &Rc<Self>) -> gtk::Box {
        let section = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let heading = gtk::Label::new(Some("Keybinds"));
        heading.add_css_class("nexora-title");
        heading.set_xalign(0.0);
        section.append(&heading);
        let note = gtk::Label::new(Some(
            "Global shortcuts are set in your compositor. Copy a snippet and paste it into your \
             config:",
        ));
        note.add_css_class("nexora-status");
        note.set_xalign(0.0);
        note.set_wrap(true);
        section.append(&note);

        let executable_path = std::env::current_exe()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "nexora".into());
        let executable = shell_quote(&executable_path);
        let hypr = format!(
            "bind = SUPER, A, exec, {executable} toggle\n\
             bind = SUPER+SHIFT, A, exec, {executable} run explain-screen"
        );
        let niri_executable = executable_path.replace('"', "\\\"");
        let niri = format!(
            "Mod+A {{ spawn \"{niri_executable}\" \"toggle\"; }}\n\
             Mod+Shift+A {{ spawn \"{niri_executable}\" \"run\" \"explain-screen\"; }}"
        );
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.append(&self.copy_button("Copy Hyprland binds", hypr));
        row.append(&self.copy_button("Copy niri binds", niri));
        section.append(&row);
        section.append(&note_label(&format!(
            "Current executable: {}. The generated binds use this absolute path, so they also work when Nexora is launched with `cargo run`.",
            std::env::current_exe()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| "nexora".into())
        )));
        section
    }

    fn copy_button(self: &Rc<Self>, label: &str, payload: String) -> gtk::Button {
        let button = gtk::Button::with_label(label);
        button.add_css_class("nexora-attach");
        let this = Rc::clone(self);
        button.connect_clicked(move |_| {
            WidgetExt::display(&this.window)
                .clipboard()
                .set_text(&payload);
            this.set_settings_feedback("copied to clipboard");
        });
        button
    }

    fn save_settings(self: &Rc<Self>) {
        let settings = self.settings.borrow();
        let Some(widgets) = settings.as_ref() else {
            return;
        };
        let Some(provider) =
            provider::selected_provider_name(&widgets.provider.cards, &widgets.provider_names)
        else {
            self.set_settings_feedback("pick a provider first");
            return;
        };
        let model = widgets.provider.model.text().trim().to_string();
        if model.is_empty() {
            self.set_settings_feedback("enter a model name");
            return;
        }
        let key = widgets.provider.api_key.text().to_string();

        let (profile_name, transcription_provider, profile_system) =
            match validate_meeting_widgets(&widgets.meeting, &widgets.provider_names) {
                Ok(values) => values,
                Err(message) => {
                    drop(settings);
                    self.set_settings_feedback(message);
                    return;
                }
            };
        let vision = match validate_and_build_vision(&widgets.vision, &widgets.provider_names) {
            Ok(vision) => vision,
            Err(message) => {
                drop(settings);
                self.set_settings_feedback(message);
                return;
            }
        };
        let meeting = meeting_config_from_widgets(
            &widgets.meeting,
            transcription_provider,
            profile_name.clone(),
            self.config.borrow().meeting.corrections.clone(),
        );

        let provider_thinking = match widgets.provider.thinking.selected() {
            1 => Some(true),
            2 => Some(false),
            _ => None,
        };
        let provider_reasoning_effort = if provider_thinking == Some(false) {
            None
        } else {
            provider::selected_effort(&widgets.provider.reasoning_effort)
        };
        let update = SettingsUpdate {
            hidden: widgets.general.hidden.is_active(),
            hyprland_rule: widgets.general.hyprland_rule.text().trim().into(),
            layer_shell: match widgets.general.layer_shell.selected() {
                1 => "on",
                2 => "off",
                _ => "auto",
            }
            .into(),
            width: widgets.general.width.value_as_int(),
            height: widgets.general.height.value_as_int(),
            task: "ask".to_string(),
            provider,
            provider_kind: if widgets.provider.kind.selected() == 1 {
                ProviderKind::Anthropic
            } else {
                ProviderKind::Openai
            },
            provider_base_url: nonempty(widgets.provider.base_url.text().as_str()),
            provider_api_key_env: nonempty(widgets.provider.api_key_env.text().as_str()),
            provider_thinking,
            provider_reasoning_effort,
            model,
            api_key: (!key.is_empty()).then_some(key),
            clear_api_key: widgets.provider.clear_api_key.is_active(),
            meeting,
            vision,
            profile_name,
            profile_system,
        };
        drop(settings);

        if let Err(err) = crate::config::apply_settings(&update) {
            self.set_settings_feedback(&format!("save failed: {err:#}"));
            return;
        }
        self.reload_config();
        self.apply_hidden_change(update.hidden);
        self.window.set_default_size(update.width, update.height);
        self.rebuild_settings();
        self.set_settings_feedback("saved");
    }

    fn rebuild_settings(self: &Rc<Self>) {
        self.settings.borrow_mut().take();
        if let Some(page) = self.stack.child_by_name("settings") {
            self.stack.remove(&page);
        }
        let widgets = self.build_settings_page();
        *self.settings.borrow_mut() = Some(widgets);
        self.stack.set_visible_child_name("settings");
    }

    fn reload_config(&self) {
        match Config::load() {
            Ok(config) => *self.config.borrow_mut() = config,
            Err(err) => eprintln!("nexora: reload failed: {err:#}"),
        }
    }

    fn apply_hidden_change(&self, want_hidden: bool) {
        let currently_active = *self.hidden_state.borrow() == HiddenState::Active;
        let new_state = if want_hidden {
            if currently_active {
                return;
            }
            let config = self.config.borrow();
            hidden::apply(
                &config.general.hyprland_rule,
                config.general.layer_shell != "off",
            )
        } else {
            HiddenState::Unsupported(
                "disabled — takes full effect on next start on some compositors".into(),
            )
        };
        self.badge.set_text(new_state.badge());
        apply_badge_style(&self.badge, &new_state);
        *self.hidden_state.borrow_mut() = new_state;
    }

    fn set_settings_feedback(&self, text: &str) {
        if let Some(widgets) = self.settings.borrow().as_ref() {
            widgets.feedback.set_text(text);
        }
    }
}

fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "/._-".contains(character))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}
