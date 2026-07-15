//! The overlay window: a chat view with conversation history and a settings
//! panel, switched by a header toggle.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4_layer_shell::{KeyboardMode, Layer, LayerShell};

use crate::config::{Config, SettingsUpdate};
use crate::conversation::{Conversation, Role};
use crate::hidden::{self, HiddenState};
use crate::providers::{ChatRequest, StreamEvent, stream_chat};
use crate::{runtime, screenshot};

/// Widgets of the settings panel, kept so Save can read them back.
struct SettingsWidgets {
    provider: gtk::DropDown,
    provider_names: Vec<String>,
    model: gtk::Entry,
    api_key: gtk::Entry,
    hidden: gtk::Switch,
    feedback: gtk::Label,
}

pub struct Overlay {
    pub window: gtk::ApplicationWindow,
    config: RefCell<Config>,
    hidden_state: RefCell<HiddenState>,
    badge: gtk::Label,
    stack: gtk::Stack,
    gear: gtk::ToggleButton,
    // Chat view.
    entry: gtk::Entry,
    attach: gtk::ToggleButton,
    response: gtk::TextView,
    end_mark: gtk::TextMark,
    status: gtk::Label,
    conversation: RefCell<Conversation>,
    busy: Cell<bool>,
    // Settings view.
    settings: RefCell<Option<SettingsWidgets>>,
}

impl Overlay {
    pub fn new(app: &gtk::Application, config: Config, hidden_state: HiddenState) -> Rc<Self> {
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("Nexora")
            .default_width(config.general.width)
            .default_height(config.general.height)
            .decorated(false)
            .resizable(false)
            .build();
        window.add_css_class("nexora");
        setup_layer_shell(&window, &config);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.set_margin_top(14);
        root.set_margin_bottom(14);
        root.set_margin_start(16);
        root.set_margin_end(16);

        // Header: title, anti-capture badge, settings gear.
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let title = gtk::Label::new(Some("Nexora"));
        title.add_css_class("nexora-title");
        title.set_hexpand(true);
        title.set_xalign(0.0);
        let badge = gtk::Label::new(Some(hidden_state.badge()));
        badge.add_css_class("nexora-badge");
        apply_badge_style(&badge, &hidden_state);
        let gear = gtk::ToggleButton::builder()
            .label("⚙")
            .tooltip_text("Settings")
            .build();
        gear.add_css_class("nexora-attach");
        header.append(&title);
        header.append(&badge);
        header.append(&gear);
        root.append(&header);

        // Chat page.
        let response = gtk::TextView::builder()
            .editable(false)
            .cursor_visible(false)
            .wrap_mode(gtk::WrapMode::WordChar)
            .build();
        response.add_css_class("nexora-response");
        install_tags(&response);
        let end_mark = response
            .buffer()
            .create_mark(None, &response.buffer().end_iter(), false);
        let scroll = gtk::ScrolledWindow::builder()
            .child(&response)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();

        let status = gtk::Label::new(None);
        status.add_css_class("nexora-status");
        status.set_xalign(0.0);

        let input_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let attach = gtk::ToggleButton::builder()
            .label("📷")
            .tooltip_text("Attach a screenshot of your screen")
            .build();
        attach.add_css_class("nexora-attach");
        let entry = gtk::Entry::builder()
            .placeholder_text("Ask anything… (Enter to send · Ctrl+N new chat · Esc hide)")
            .hexpand(true)
            .build();
        entry.add_css_class("nexora-entry");
        input_row.append(&attach);
        input_row.append(&entry);

        let chat_page = gtk::Box::new(gtk::Orientation::Vertical, 8);
        chat_page.append(&scroll);
        chat_page.append(&status);
        chat_page.append(&input_row);

        let stack = gtk::Stack::builder()
            .vexpand(true)
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        stack.add_named(&chat_page, Some("chat"));
        root.append(&stack);

        window.set_child(Some(&root));

        // Restore the most recent conversation; drop a dangling user turn so
        // history always alternates and stays a valid API request.
        let mut conversation = Conversation::load_latest().unwrap_or_default();
        if conversation.turns.last().map(|t| t.role) == Some(Role::User) {
            conversation.turns.pop();
        }

        let overlay = Rc::new(Self {
            window,
            config: RefCell::new(config),
            hidden_state: RefCell::new(hidden_state),
            badge,
            stack,
            gear,
            entry,
            attach,
            response,
            end_mark,
            status,
            conversation: RefCell::new(conversation),
            busy: Cell::new(false),
            settings: RefCell::new(None),
        });

        overlay.render_conversation();
        overlay.wire_events();
        overlay
    }

    fn wire_events(self: &Rc<Self>) {
        // Enter sends the prompt.
        let this = Rc::clone(self);
        self.entry.connect_activate(move |entry| {
            let prompt = entry.text().trim().to_string();
            if prompt.is_empty() {
                return;
            }
            entry.set_text("");
            this.ask(prompt, this.attach.is_active(), "ask".to_string());
        });

        // Gear toggles the settings page.
        let this = Rc::clone(self);
        self.gear.connect_toggled(move |gear| {
            if gear.is_active() {
                this.open_settings();
            } else {
                this.stack.set_visible_child_name("chat");
                this.entry.grab_focus();
            }
        });

        // Esc hides; Ctrl+N starts a new conversation.
        let keys = gtk::EventControllerKey::new();
        let this = Rc::clone(self);
        keys.connect_key_pressed(move |_, key, _, mods| {
            if key == gdk::Key::Escape {
                this.window.set_visible(false);
                return glib::Propagation::Stop;
            }
            if key == gdk::Key::n && mods.contains(gdk::ModifierType::CONTROL_MASK) {
                this.new_conversation();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.window.add_controller(keys);
    }

    pub fn present(&self) {
        self.window.present();
        self.entry.grab_focus();
    }

    pub fn toggle(&self) {
        if self.window.is_visible() {
            self.window.set_visible(false);
        } else {
            self.present();
        }
    }

    fn new_conversation(&self) {
        *self.conversation.borrow_mut() = Conversation::new();
        self.render_conversation();
        self.set_status("new conversation");
        self.entry.grab_focus();
    }

    /// Send a prompt in the current conversation, optionally attaching a shot.
    pub fn ask(self: &Rc<Self>, prompt: String, attach_screen: bool, task_name: String) {
        if self.busy.get() {
            self.set_status("still answering — wait for the current response");
            return;
        }
        self.busy.set(true);
        self.stack.set_visible_child_name("chat");
        self.gear.set_active(false);

        let this = Rc::clone(self);
        glib::spawn_future_local(async move {
            this.run_ask(prompt, attach_screen, task_name).await;
            this.busy.set(false);
        });
    }

    async fn run_ask(self: &Rc<Self>, prompt: String, attach_screen: bool, task_name: String) {
        // Capture first so a failure doesn't leave a half-formed turn.
        let image = if attach_screen {
            self.set_status("capturing screen…");
            match self.capture_avoiding_self().await {
                Ok(png) => Some(png),
                Err(err) => {
                    self.set_status(&format!("screenshot failed: {err:#}"));
                    return;
                }
            }
        } else {
            None
        };
        self.present();

        // Resolve provider/model up front; on config error keep the turn out.
        let outcome = {
            let config = self.config.borrow();
            config
                .task(&task_name)
                .and_then(|task| Ok((task.clone(), config.provider_for(task)?.clone())))
        };
        let (task, provider) = match outcome {
            Ok(pair) => pair,
            Err(err) => {
                self.set_status("not configured");
                self.show_system_line(&format!(
                    "{err:#}\nOpen ⚙ Settings (or run `nexora config init`) to set a provider."
                ));
                return;
            }
        };

        self.conversation
            .borrow_mut()
            .push_user(prompt, image.is_some());
        self.render_conversation();
        self.begin_assistant_line();
        self.set_status(&format!("{} · {}", task.provider, task.model));

        let messages = self.conversation.borrow().api_messages();
        let request = ChatRequest::new(&task, messages, image);
        let (tx, rx) = async_channel::unbounded::<StreamEvent>();
        runtime().spawn(async move { stream_chat(&provider, request, tx).await });

        let mut answer = String::new();
        while let Ok(event) = rx.recv().await {
            match event {
                StreamEvent::Delta(text) => {
                    answer.push_str(&text);
                    self.append_text(&text);
                }
                StreamEvent::Done => break,
                StreamEvent::Error(message) => {
                    // Drop the user turn so history stays valid for a retry.
                    self.conversation.borrow_mut().turns.pop();
                    self.render_conversation();
                    self.show_system_line(&format!("error: {message}"));
                    self.set_status("error");
                    return;
                }
            }
        }

        let mut conversation = self.conversation.borrow_mut();
        conversation.push_assistant(answer);
        if let Err(err) = conversation.save() {
            eprintln!("nexora: could not save history: {err:#}");
        }
        drop(conversation);
        self.render_conversation();
        self.set_status("");
    }

    async fn capture_avoiding_self(&self) -> anyhow::Result<Vec<u8>> {
        let must_hide =
            self.window.is_visible() && *self.hidden_state.borrow() != HiddenState::Active;
        if must_hide {
            self.window.set_visible(false);
            glib::timeout_future(Duration::from_millis(300)).await;
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        runtime().spawn(async move {
            let _ = tx.send(screenshot::capture_png().await);
        });
        let result = rx
            .await
            .map_err(|_| anyhow::anyhow!("capture task dropped"))?;
        if must_hide {
            self.window.set_visible(true);
        }
        result
    }

    // --- Conversation rendering -------------------------------------------

    fn render_conversation(&self) {
        let buffer = self.response.buffer();
        buffer.set_text("");
        let conversation = self.conversation.borrow();
        if conversation.is_empty() {
            self.insert_tagged("Ask anything to begin.\n", "dim");
            return;
        }
        for turn in &conversation.turns {
            self.insert_tagged(&format!("{}\n", turn.role.label()), "role");
            if turn.had_image {
                self.insert_tagged("[screenshot attached] ", "dim");
            }
            let mut iter = buffer.end_iter();
            buffer.insert(&mut iter, &format!("{}\n\n", turn.text));
        }
        self.scroll_to_end();
    }

    /// Print the "Nexora" label so streamed deltas append under it.
    fn begin_assistant_line(&self) {
        self.insert_tagged(&format!("{}\n", Role::Assistant.label()), "role");
        self.scroll_to_end();
    }

    fn append_text(&self, text: &str) {
        let buffer = self.response.buffer();
        buffer.insert(&mut buffer.end_iter(), text);
        self.scroll_to_end();
    }

    /// A dim, non-conversation line (errors, hints).
    fn show_system_line(&self, text: &str) {
        self.insert_tagged(&format!("{text}\n"), "dim");
        self.scroll_to_end();
    }

    fn insert_tagged(&self, text: &str, tag: &str) {
        let buffer = self.response.buffer();
        let mut iter = buffer.end_iter();
        buffer.insert_with_tags_by_name(&mut iter, text, &[tag]);
    }

    fn scroll_to_end(&self) {
        let buffer = self.response.buffer();
        buffer.move_mark(&self.end_mark, &buffer.end_iter());
        self.response.scroll_mark_onscreen(&self.end_mark);
    }

    fn set_status(&self, text: &str) {
        self.status.set_text(text);
    }

    // --- Settings ----------------------------------------------------------

    fn open_settings(self: &Rc<Self>) {
        if self.settings.borrow().is_none() {
            let widgets = self.build_settings_page();
            *self.settings.borrow_mut() = Some(widgets);
        }
        self.stack.set_visible_child_name("settings");
    }

    fn build_settings_page(self: &Rc<Self>) -> SettingsWidgets {
        let config = self.config.borrow();
        let page = gtk::Box::new(gtk::Orientation::Vertical, 10);
        page.set_margin_top(4);

        // Provider + model for the default "ask" task.
        let provider_names = config.provider_names();
        let strs: Vec<&str> = provider_names.iter().map(String::as_str).collect();
        let provider = gtk::DropDown::from_strings(&strs);
        let model = gtk::Entry::builder().hexpand(true).build();
        model.add_css_class("nexora-entry");
        if let Ok(task) = config.task("ask") {
            if let Some(index) = provider_names.iter().position(|n| *n == task.provider) {
                provider.set_selected(index as u32);
            }
            model.set_text(&task.model);
        } else if let Some(first) = provider_names.first() {
            // Sensible default model per provider kind for a fresh config.
            let _ = first;
            model.set_text("claude-sonnet-5");
        }

        let api_key = gtk::Entry::builder()
            .placeholder_text("Paste API key (stored in config.toml, chmod 600)")
            .visibility(false)
            .hexpand(true)
            .build();
        api_key.add_css_class("nexora-entry");

        let hidden = gtk::Switch::new();
        hidden.set_active(config.general.hidden);
        hidden.set_halign(gtk::Align::Start);

        page.append(&field_row("Provider", &provider));
        page.append(&field_row("Model", &model));
        page.append(&field_row("API key", &api_key));
        let key_hint = gtk::Label::new(Some(
            "Leave blank to keep the current key or the api_key_env from config.toml.",
        ));
        key_hint.add_css_class("nexora-status");
        key_hint.set_xalign(0.0);
        key_hint.set_wrap(true);
        page.append(&key_hint);
        page.append(&field_row("Hidden (anti-capture)", &hidden));

        // Anti-capture reality for this compositor.
        let hidden_note = gtk::Label::new(Some(&hidden::status_report()));
        hidden_note.add_css_class("nexora-status");
        hidden_note.set_xalign(0.0);
        hidden_note.set_wrap(true);
        page.append(&hidden_note);

        page.append(&self.keybind_section());

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
        page.append(&footer);

        let scroll = gtk::ScrolledWindow::builder()
            .child(&page)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        self.stack.add_named(&scroll, Some("settings"));

        drop(config);
        SettingsWidgets {
            provider,
            provider_names,
            model,
            api_key,
            hidden,
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

        let hypr = "bind = SUPER, A, exec, nexora toggle\n\
                    bind = SUPER SHIFT, A, exec, nexora run explain-screen";
        let niri = "Mod+A { spawn \"nexora\" \"toggle\"; }\n\
                    Mod+Shift+A { spawn \"nexora\" \"run\" \"explain-screen\"; }";
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.append(&self.copy_button("Copy Hyprland binds", hypr));
        row.append(&self.copy_button("Copy niri binds", niri));
        section.append(&row);
        section
    }

    fn copy_button(self: &Rc<Self>, label: &str, payload: &'static str) -> gtk::Button {
        let button = gtk::Button::with_label(label);
        button.add_css_class("nexora-attach");
        let this = Rc::clone(self);
        button.connect_clicked(move |_| {
            WidgetExt::display(&this.window)
                .clipboard()
                .set_text(payload);
            this.set_settings_feedback("copied to clipboard");
        });
        button
    }

    fn save_settings(self: &Rc<Self>) {
        let settings = self.settings.borrow();
        let Some(widgets) = settings.as_ref() else {
            return;
        };
        let index = widgets.provider.selected() as usize;
        let Some(provider) = widgets.provider_names.get(index).cloned() else {
            self.set_settings_feedback("pick a provider first");
            return;
        };
        let model = widgets.model.text().trim().to_string();
        if model.is_empty() {
            self.set_settings_feedback("enter a model name");
            return;
        }
        let key = widgets.api_key.text().to_string();
        let update = SettingsUpdate {
            hidden: widgets.hidden.is_active(),
            hyprland_rule: self.config.borrow().general.hyprland_rule.clone(),
            task: "ask".to_string(),
            provider,
            model,
            api_key: (!key.is_empty()).then_some(key),
        };
        drop(settings);

        if let Err(err) = crate::config::apply_settings(&update) {
            self.set_settings_feedback(&format!("save failed: {err:#}"));
            return;
        }
        // Don't keep the secret in the widget after it is written.
        if let Some(widgets) = self.settings.borrow().as_ref() {
            widgets.api_key.set_text("");
        }
        self.reload_config();
        self.apply_hidden_change(update.hidden);
        self.set_settings_feedback("saved");
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
            hidden::apply(&self.config.borrow().general.hyprland_rule)
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

fn field_row(label: &str, control: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let label = gtk::Label::new(Some(label));
    label.set_xalign(0.0);
    label.set_width_chars(20);
    row.append(&label);
    row.append(control);
    row
}

fn install_tags(view: &gtk::TextView) {
    let buffer = view.buffer();
    let role = gtk::TextTag::builder()
        .name("role")
        .weight(700)
        .foreground("#b7c4ff")
        .build();
    let dim = gtk::TextTag::builder()
        .name("dim")
        .foreground("#8b93a7")
        .build();
    buffer.tag_table().add(&role);
    buffer.tag_table().add(&dim);
}

fn apply_badge_style(badge: &gtk::Label, state: &HiddenState) {
    badge.remove_css_class("hidden-active");
    badge.remove_css_class("hidden-off");
    match state {
        HiddenState::Active => badge.add_css_class("hidden-active"),
        HiddenState::Manual(detail) | HiddenState::Unsupported(detail) => {
            badge.add_css_class("hidden-off");
            badge.set_tooltip_text(Some(detail));
        }
    }
}

fn setup_layer_shell(window: &gtk::ApplicationWindow, config: &Config) {
    // layer-shell is a Wayland protocol; probing it on X11 trips a GTK assertion.
    let on_wayland = WidgetExt::display(window)
        .type_()
        .name()
        .contains("Wayland");
    let use_layer_shell = match config.general.layer_shell.as_str() {
        "off" => false,
        "on" => on_wayland,
        _ => on_wayland && gtk4_layer_shell::is_supported(),
    };
    if !use_layer_shell {
        return;
    }
    window.init_layer_shell();
    window.set_namespace(Some("nexora"));
    window.set_layer(Layer::Overlay);
    // Grab the keyboard while visible so a global keybind → type → Enter
    // round-trip never needs the mouse. No anchors set = centered.
    window.set_keyboard_mode(KeyboardMode::Exclusive);
}
