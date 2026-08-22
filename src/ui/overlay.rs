//! The overlay window: a chat view with conversation history and a settings
//! panel, switched by a header toggle.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;

use crate::config::Config;
use crate::conversation::{Conversation, Role};
use crate::hidden::HiddenState;

use super::conversation_view::install_tags;
use super::settings::SettingsWidgets;
use super::window::{apply_badge_style, setup_layer_shell};

pub struct Overlay {
    pub window: gtk::ApplicationWindow,
    pub(super) config: RefCell<Config>,
    pub(super) hidden_state: RefCell<HiddenState>,
    pub(super) badge: gtk::Label,
    pub(super) stack: gtk::Stack,
    pub(super) gear: gtk::ToggleButton,
    pub(super) live_button: gtk::ToggleButton,
    // Chat view.
    pub(super) entry: gtk::Entry,
    pub(super) attach: gtk::ToggleButton,
    explain: gtk::Button,
    pub(super) meeting_button: gtk::ToggleButton,
    pub(super) response: gtk::TextView,
    pub(super) end_mark: gtk::TextMark,
    pub(super) live_response: gtk::TextView,
    pub(super) live_end_mark: gtk::TextMark,
    pub(super) status: gtk::Label,
    pub(super) live_status: gtk::Label,
    pub(super) conversation: RefCell<Conversation>,
    pub(super) busy: Cell<bool>,
    pub(super) meeting_stop: RefCell<Option<tokio::sync::watch::Sender<bool>>>,
    // Rolling transcript of the current or most recent meeting, so typed
    // questions can use it as context. A new session replaces it; a new chat
    // clears it after the session has finished.
    pub(super) meeting_transcript: RefCell<Vec<String>>,
    // Settings view.
    pub(super) settings: RefCell<Option<SettingsWidgets>>,
}

struct HeaderWidgets {
    root: gtk::Box,
    badge: gtk::Label,
    gear: gtk::ToggleButton,
    live_button: gtk::ToggleButton,
}

struct ChatPageWidgets {
    root: gtk::Box,
    response: gtk::TextView,
    end_mark: gtk::TextMark,
    status: gtk::Label,
    entry: gtk::Entry,
    attach: gtk::ToggleButton,
    explain: gtk::Button,
    meeting_button: gtk::ToggleButton,
}

struct LivePageWidgets {
    root: gtk::Box,
    response: gtk::TextView,
    end_mark: gtk::TextMark,
    status: gtk::Label,
}

fn build_window(app: &gtk::Application, config: &Config) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Nexora")
        .default_width(config.general.width)
        .default_height(config.general.height)
        .decorated(false)
        .resizable(false)
        .build();
    window.add_css_class("nexora");
    setup_layer_shell(&window, config);
    window
}

fn build_header(hidden_state: &HiddenState) -> HeaderWidgets {
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let title = gtk::Label::new(Some("Nexora"));
    title.add_css_class("nexora-title");
    title.set_hexpand(true);
    title.set_xalign(0.0);
    let badge = gtk::Label::new(Some(hidden_state.badge()));
    badge.add_css_class("nexora-badge");
    apply_badge_style(&badge, hidden_state);
    let gear = gtk::ToggleButton::builder()
        .label("⚙")
        .tooltip_text("Settings")
        .build();
    gear.add_css_class("nexora-attach");
    let live_button = gtk::ToggleButton::builder()
        .label("Live")
        .tooltip_text("Open live transcript, translation, and coaching")
        .build();
    live_button.add_css_class("nexora-attach");
    header.append(&title);
    header.append(&badge);
    header.append(&live_button);
    header.append(&gear);
    HeaderWidgets {
        root: header,
        badge,
        gear,
        live_button,
    }
}

fn build_chat_page() -> ChatPageWidgets {
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
        .tooltip_text("Attach a screenshot of your screen to your next question")
        .build();
    attach.add_css_class("nexora-attach");
    let explain = gtk::Button::builder()
        .label("🖥")
        .tooltip_text("Explain what is on my screen right now")
        .build();
    explain.add_css_class("nexora-attach");
    let meeting_button = gtk::ToggleButton::builder()
        .label("🎙")
        .tooltip_text("Start live meeting assistant")
        .build();
    meeting_button.add_css_class("nexora-attach");
    let entry = gtk::Entry::builder()
        .placeholder_text("Ask anything… (Enter to send · Ctrl+N new chat · Esc hide)")
        .hexpand(true)
        .build();
    entry.add_css_class("nexora-entry");
    input_row.append(&meeting_button);
    input_row.append(&explain);
    input_row.append(&attach);
    input_row.append(&entry);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.append(&scroll);
    root.append(&status);
    root.append(&input_row);

    ChatPageWidgets {
        root,
        response,
        end_mark,
        status,
        entry,
        attach,
        explain,
        meeting_button,
    }
}

/// Live session output is deliberately separate from manual chat. It can be
/// inspected on demand without turning transcript fragments or coaching
/// suggestions into conversation turns.
fn build_live_page() -> LivePageWidgets {
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
    let hint = gtk::Label::new(Some(
        "Live activity is context for the assistant, not part of your chat history.",
    ));
    hint.add_css_class("nexora-status");
    hint.set_xalign(0.0);
    let status = gtk::Label::new(None);
    status.add_css_class("nexora-status");
    status.set_xalign(0.0);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.append(&scroll);
    root.append(&status);
    root.append(&hint);

    LivePageWidgets {
        root,
        response,
        end_mark,
        status,
    }
}

impl Overlay {
    pub fn new(app: &gtk::Application, config: Config, hidden_state: HiddenState) -> Rc<Self> {
        let window = build_window(app, &config);
        let header = build_header(&hidden_state);
        let chat = build_chat_page();
        let live = build_live_page();

        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.set_margin_top(14);
        root.set_margin_bottom(14);
        root.set_margin_start(16);
        root.set_margin_end(16);
        root.append(&header.root);

        let stack = gtk::Stack::builder()
            .vexpand(true)
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        stack.add_named(&chat.root, Some("chat"));
        stack.add_named(&live.root, Some("live"));
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
            badge: header.badge,
            stack,
            gear: header.gear,
            live_button: header.live_button,
            entry: chat.entry,
            attach: chat.attach,
            explain: chat.explain,
            meeting_button: chat.meeting_button,
            response: chat.response,
            end_mark: chat.end_mark,
            live_response: live.response,
            live_end_mark: live.end_mark,
            status: chat.status,
            live_status: live.status,
            conversation: RefCell::new(conversation),
            busy: Cell::new(false),
            meeting_stop: RefCell::new(None),
            meeting_transcript: RefCell::new(Vec::new()),
            settings: RefCell::new(None),
        });

        overlay.render_conversation();
        overlay.set_status("Esc hides · use your global `nexora toggle` shortcut to reopen");
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

        // Explain-screen button: capture the screen and ask about it, no CLI.
        let this = Rc::clone(self);
        self.explain.connect_clicked(move |_| {
            let prompt = Config::load()
                .ok()
                .and_then(|config| config.preset("explain-screen").ok())
                .map(|preset| preset.prompt)
                .unwrap_or_else(|| {
                    "Explain what is on my screen. Focus on unusual terms, errors, and anything I would want clarified.".to_string()
                });
            this.ask(prompt, true, "explain-screen".to_string());
        });

        // Gear toggles the settings page.
        let this = Rc::clone(self);
        self.gear.connect_toggled(move |gear| {
            if gear.is_active() {
                this.live_button.set_active(false);
                this.open_settings();
            } else {
                this.stack.set_visible_child_name("chat");
                this.entry.grab_focus();
            }
        });

        let this = Rc::clone(self);
        self.live_button.connect_toggled(move |button| {
            if button.is_active() {
                this.gear.set_active(false);
                button.set_label("Live");
                this.stack.set_visible_child_name("live");
            } else if this.stack.visible_child_name().as_deref() == Some("live") {
                this.stack.set_visible_child_name("chat");
                this.entry.grab_focus();
            }
        });

        let this = Rc::clone(self);
        self.meeting_button.connect_toggled(move |button| {
            if button.is_active() {
                button.set_label("■");
                button.set_tooltip_text(Some("Stop and summarize meeting"));
                this.start_meeting();
            } else {
                button.set_label("🎙");
                button.set_tooltip_text(Some("Start live meeting assistant"));
                this.stop_meeting();
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
        // Keep the active session attached to a fresh chat, but do not leak a
        // finished session's transcript into an unrelated conversation.
        if self.meeting_stop.borrow().is_none() {
            self.meeting_transcript.borrow_mut().clear();
        }
        *self.conversation.borrow_mut() = Conversation::new();
        self.render_conversation();
        self.set_status("new conversation");
        self.entry.grab_focus();
    }
}
