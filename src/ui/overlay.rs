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

use crate::config::{Config, MeetingConfig, ProviderKind, SettingsUpdate, VisionConfig};
use crate::conversation::{Conversation, Role};
use crate::hidden::{self, HiddenState};
use crate::meeting::{self, SessionEvent};
use crate::providers::{ChatRequest, StreamEvent, stream_chat};
use crate::vision;
use crate::whisper;
use crate::{runtime, screenshot};

/// Widgets of the settings panel, kept so Save can read them back.
struct SettingsWidgets {
    provider_names: Vec<String>,
    provider: ProviderSettingsWidgets,
    general: GeneralSettingsWidgets,
    meeting: MeetingSettingsWidgets,
    vision: VisionSettingsWidgets,
    feedback: gtk::Label,
}

struct ProviderSettingsWidgets {
    cards: gtk::FlowBox,
    kind: gtk::DropDown,
    base_url: gtk::Entry,
    api_key_env: gtk::Entry,
    api_key: gtk::Entry,
    clear_api_key: gtk::CheckButton,
    model: gtk::Entry,
    model_choices: gtk::DropDown,
    refresh_models: gtk::Button,
    model_status: gtk::Label,
    thinking: gtk::DropDown,
    reasoning_effort: gtk::DropDown,
    token_notice: gtk::Label,
}

struct GeneralSettingsWidgets {
    hidden: gtk::Switch,
    layer_shell: gtk::DropDown,
    hyprland_rule: gtk::Entry,
    width: gtk::SpinButton,
    height: gtk::SpinButton,
}

struct MeetingSettingsWidgets {
    audio_source: gtk::DropDown,
    audio_device: gtk::Entry,
    chunk_seconds: gtk::SpinButton,
    silence_threshold: gtk::SpinButton,
    transcription_backend: gtk::DropDown,
    whisper_catalog: gtk::DropDown,
    whisper_download: gtk::Button,
    whisper_remove: gtk::Button,
    whisper_progress: gtk::ProgressBar,
    whisper_status: gtk::Label,
    transcription_provider: gtk::DropDown,
    transcription_model: gtk::Entry,
    input_language: gtk::Entry,
    translate: gtk::Switch,
    target_language: gtk::Entry,
    suggestions: gtk::Switch,
    objection_handling: gtk::Switch,
    automatic_notes: gtk::Switch,
    screen_context: gtk::Switch,
    screen_interval: gtk::SpinButton,
    summary: gtk::Switch,
    save_session: gtk::Switch,
    analysis_task: gtk::Entry,
    profile: gtk::DropDown,
    profile_name: gtk::Entry,
    profile_system: gtk::TextView,
}

struct VisionSettingsWidgets {
    mode: gtk::DropDown,
    provider: gtk::DropDown,
    model: gtk::Entry,
    catalog: gtk::DropDown,
    ollama_url: gtk::Entry,
    prompt: gtk::TextView,
    installed: gtk::DropDown,
    refresh: gtk::Button,
    download: gtk::Button,
    delete: gtk::Button,
    progress: gtk::ProgressBar,
    status: gtk::Label,
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
    explain: gtk::Button,
    meeting_button: gtk::ToggleButton,
    response: gtk::TextView,
    end_mark: gtk::TextMark,
    status: gtk::Label,
    conversation: RefCell<Conversation>,
    busy: Cell<bool>,
    meeting_stop: RefCell<Option<tokio::sync::watch::Sender<bool>>>,
    // Live transcript of the active meeting, so a typed question can use it
    // as context. Cleared when a session starts.
    meeting_transcript: RefCell<Vec<String>>,
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
            explain,
            meeting_button,
            response,
            end_mark,
            status,
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
                this.open_settings();
            } else {
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
        *self.conversation.borrow_mut() = Conversation::new();
        self.render_conversation();
        self.set_status("new conversation");
        self.entry.grab_focus();
    }

    fn start_meeting(self: &Rc<Self>) {
        if self.meeting_stop.borrow().is_some() {
            return;
        }
        let resolved = {
            let config = self.config.borrow();
            let settings = config.meeting.clone();
            let vision_settings = config.vision.clone();
            let transcription = if settings.transcription_backend == "local" {
                let model_path = crate::whisper::model_path(&settings.whisper_model);
                if model_path.exists() {
                    Ok(meeting::TranscriptionBackend::Local { model_path })
                } else {
                    Err(anyhow::anyhow!(
                        "local whisper model `{}` is not downloaded — open Settings → Live meeting to download it, or switch transcription to remote",
                        settings.whisper_model
                    ))
                }
            } else {
                config
                    .providers
                    .get(&settings.transcription_provider)
                    .cloned()
                    .map(|provider| meeting::TranscriptionBackend::Remote { provider })
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "transcription provider `{}` is not configured",
                            settings.transcription_provider
                        )
                    })
            };
            let analysis = config
                .task(&settings.analysis_task)
                .and_then(|task| Ok((task.clone(), config.provider_for(task)?.clone())));
            let profile = config.profile(&settings.profile);
            let vision_provider = if settings.screen_context && vision_settings.mode == "proxy" {
                config
                    .provider(&vision_settings.provider)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "vision provider `{}` is not configured",
                            vision_settings.provider
                        )
                    })
                    .map(Some)
            } else {
                Ok(None)
            };
            match (transcription, analysis, vision_provider, profile) {
                (Ok(transcription), Ok((task, provider)), Ok(vision_provider), Ok(profile)) => {
                    Ok((
                        settings,
                        transcription,
                        task,
                        provider,
                        vision_settings,
                        vision_provider,
                        profile,
                    ))
                }
                (Err(err), _, _, _)
                | (_, Err(err), _, _)
                | (_, _, Err(err), _)
                | (_, _, _, Err(err)) => Err(err),
            }
        };
        let (
            settings,
            transcription,
            task,
            analysis_provider,
            vision_settings,
            vision_provider,
            profile,
        ) = match resolved {
            Ok(value) => value,
            Err(err) => {
                self.show_system_line(&format!("meeting cannot start: {err:#}"));
                self.meeting_button.set_active(false);
                return;
            }
        };

        if settings.screen_context && *self.hidden_state.borrow() != HiddenState::Active {
            self.show_system_line(
                "warning: screen context is enabled, but this compositor cannot confirm that the overlay is excluded from capture",
            );
        }
        self.stack.set_visible_child_name("chat");
        self.gear.set_active(false);
        self.show_meeting_line("Session", "Live assistant started", "meeting");

        self.meeting_transcript.borrow_mut().clear();
        let (events_tx, events_rx) = async_channel::unbounded();
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(true);
        *self.meeting_stop.borrow_mut() = Some(stop_tx);
        runtime().spawn(meeting::run_session(
            settings,
            meeting::SessionServices {
                transcription,
                analysis_task: task,
                analysis_provider,
                vision_settings,
                vision_provider,
                profile,
            },
            events_tx,
            stop_rx,
        ));

        let this = Rc::clone(self);
        glib::spawn_future_local(async move {
            while let Ok(event) = events_rx.recv().await {
                let finished = matches!(event, SessionEvent::Finished(_));
                this.handle_meeting_event(event);
                if finished {
                    break;
                }
            }
        });
    }

    fn stop_meeting(&self) {
        if let Some(stop) = self.meeting_stop.borrow().as_ref() {
            let _ = stop.send(false);
            self.set_status("stopping meeting and preparing summary…");
        }
    }

    fn handle_meeting_event(&self, event: SessionEvent) {
        match event {
            SessionEvent::Status(text) => self.set_status(&text),
            SessionEvent::Transcript(text) => {
                // Keep a bounded rolling transcript so a typed question can use
                // it as context without growing without bound.
                let mut transcript = self.meeting_transcript.borrow_mut();
                transcript.push(text.clone());
                if transcript.len() > 400 {
                    let overflow = transcript.len() - 400;
                    transcript.drain(..overflow);
                }
                drop(transcript);
                self.show_meeting_line("Transcript", &text, "meeting")
            }
            SessionEvent::Translation(text) => {
                self.show_meeting_line("Translation", &text, "translation")
            }
            SessionEvent::Insight(text) => self.show_meeting_line("Live coach", &text, "insight"),
            SessionEvent::Summary(text) => {
                self.show_meeting_line("Session summary", &text, "summary")
            }
            SessionEvent::Error(text) => self.show_system_line(&format!("meeting: {text}")),
            SessionEvent::Finished(path) => {
                self.meeting_stop.borrow_mut().take();
                self.meeting_button.set_active(false);
                match path {
                    Some(path) => self.set_status(&format!("session saved to {}", path.display())),
                    None => self.set_status("meeting finished"),
                }
            }
        }
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

        let mut image = image;
        let mut screen_description = None;
        if image.is_some() {
            let vision_config = self.config.borrow().vision.clone();
            match vision_config.mode.as_str() {
                "off" => image = None,
                "proxy" => {
                    let vision_provider = self
                        .config
                        .borrow()
                        .provider(&vision_config.provider)
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "vision provider `{}` is not configured",
                                vision_config.provider
                            )
                        });
                    let vision_provider = match vision_provider {
                        Ok(provider) => provider,
                        Err(err) => {
                            self.set_status("vision proxy not configured");
                            self.show_system_line(&format!("{err:#}"));
                            return;
                        }
                    };
                    self.set_status("describing screen with vision/OCR…");
                    match vision::describe_screen(
                        &vision_provider,
                        &vision_config.model,
                        &vision_config.prompt,
                        image.take().expect("screen image was checked"),
                    )
                    .await
                    {
                        Ok(description) => screen_description = Some(description),
                        Err(err) => {
                            self.set_status("vision/OCR failed");
                            self.show_system_line(&format!("vision/OCR failed: {err:#}"));
                            return;
                        }
                    }
                }
                _ => {}
            }
        }

        self.conversation
            .borrow_mut()
            .push_user(prompt, image.is_some());
        self.render_conversation();
        self.begin_assistant_line();
        self.set_status(&format!("{} · {}", task.provider, task.model));

        let mut messages = self.conversation.borrow().api_messages();
        // When a meeting is live, let a typed question use what is being said
        // as context so the model understands the ongoing conversation.
        if self.meeting_stop.borrow().is_some()
            && let Some(context) = self.recent_meeting_transcript(6_000)
            && let Some((_, text)) = messages.last_mut()
        {
            text.push_str("\n\nLive meeting transcript (most recent speech, for context):\n");
            text.push_str(&context);
        }
        if let Some(description) = screen_description
            && let Some((_, text)) = messages.last_mut()
        {
            text.push_str("\n\nScreen context from vision/OCR:\n");
            text.push_str(&description);
        }
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

    /// The tail of the live transcript, capped at `max_chars`.
    fn recent_meeting_transcript(&self, max_chars: usize) -> Option<String> {
        let transcript = self.meeting_transcript.borrow();
        if transcript.is_empty() {
            return None;
        }
        let mut selected = Vec::new();
        let mut length = 0;
        for chunk in transcript.iter().rev() {
            if length + chunk.len() > max_chars && !selected.is_empty() {
                break;
            }
            length += chunk.len();
            selected.push(chunk.as_str());
        }
        selected.reverse();
        Some(selected.join("\n"))
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

    fn show_meeting_line(&self, label: &str, text: &str, tag: &str) {
        self.insert_tagged(&format!("\n{label}\n"), tag);
        let buffer = self.response.buffer();
        buffer.insert(&mut buffer.end_iter(), &format!("{text}\n"));
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
        provider_page.append(&section_heading("Selected provider"));
        append_provider_fields(&provider_page, &provider);
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

    fn build_provider_settings(
        self: &Rc<Self>,
        config: &Config,
        provider_names: &[String],
    ) -> ProviderSettingsWidgets {
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
        let model_status =
            note_label("Use Refresh to query this provider's current /models catalog.");
        let thinking = gtk::DropDown::from_strings(&[
            "Provider default",
            "Thinking enabled",
            "Thinking disabled",
        ]);
        let reasoning_effort = gtk::DropDown::from_strings(&["Provider default"]);
        let token_notice = note_label("");
        token_notice.add_css_class("token-notice");

        let widgets = ProviderSettingsWidgets {
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
        };

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
            populate_provider_fields(config, name, &widgets);
        }

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
        widgets
    }

    fn build_meeting_settings(
        &self,
        config: &Config,
        provider_names: &[String],
    ) -> MeetingSettingsWidgets {
        let settings = &config.meeting;
        let audio_source = gtk::DropDown::from_strings(&[
            "System audio",
            "Microphone",
            "System + microphone",
            "Custom device",
        ]);
        audio_source.set_selected(match settings.audio_source.as_str() {
            "microphone" => 1,
            "both" => 2,
            "custom" => 3,
            _ => 0,
        });
        let audio_device = entry_with(&settings.audio_device, "Pulse/PipeWire source name");
        let chunk_seconds = spin(1.0, 60.0, settings.chunk_seconds as f64);
        let silence_threshold = spin(0.0, 3000.0, settings.silence_threshold as f64);

        let transcription_backend = gtk::DropDown::from_strings(&[
            "Local (whisper.cpp, audio stays on this computer)",
            "Remote API (uploads audio to the provider)",
        ]);
        transcription_backend.set_selected(match settings.transcription_backend.as_str() {
            "remote" => 1,
            _ => 0,
        });
        let (whisper_catalog, whisper_download, whisper_remove, whisper_progress, whisper_status) =
            build_whisper_manager(&settings.whisper_model);

        let provider_strs: Vec<&str> = provider_names.iter().map(String::as_str).collect();
        let transcription_provider = gtk::DropDown::from_strings(&provider_strs);
        if let Some(index) = provider_names
            .iter()
            .position(|name| *name == settings.transcription_provider)
        {
            transcription_provider.set_selected(index as u32);
        }
        let transcription_model =
            entry_with(&settings.transcription_model, "gpt-4o-mini-transcribe");
        let input_language = entry_with(
            &settings.input_language,
            "Blank = auto-detect (e.g. pt, en)",
        );
        let translate = switch(settings.translate);
        let target_language = entry_with(&settings.target_language, "Portuguese (Brazil)");
        let suggestions = switch(settings.suggestions);
        let objection_handling = switch(settings.objection_handling);
        let automatic_notes = switch(settings.automatic_notes);
        let screen_context = switch(settings.screen_context);
        let screen_interval = spin(1.0, 100.0, settings.screen_interval_chunks as f64);
        let summary = switch(settings.summary);
        let save_session = switch(settings.save_session);
        let analysis_task = entry_with(&settings.analysis_task, "ask");

        let profile_names = config.profile_names();
        let profile_strs: Vec<&str> = profile_names.iter().map(String::as_str).collect();
        let profile = gtk::DropDown::from_strings(&profile_strs);
        if let Some(index) = profile_names
            .iter()
            .position(|name| *name == settings.profile)
        {
            profile.set_selected(index as u32);
        }
        let profile_system = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .height_request(80)
            .build();
        profile_system.add_css_class("nexora-response");
        if let Ok(selected) = config.profile(&settings.profile) {
            profile_system.buffer().set_text(&selected.system);
        }
        let profile_name = entry_with(&settings.profile, "New or existing profile name");
        let config_for_profiles = config.clone();
        let names_for_profiles = profile_names.clone();
        let profile_name_for_change = profile_name.clone();
        let profile_system_for_change = profile_system.clone();
        profile.connect_selected_notify(move |dropdown| {
            let Some(name) = names_for_profiles.get(dropdown.selected() as usize) else {
                return;
            };
            if let Ok(selected) = config_for_profiles.profile(name) {
                profile_name_for_change.set_text(name);
                profile_system_for_change
                    .buffer()
                    .set_text(&selected.system);
            }
        });

        MeetingSettingsWidgets {
            audio_source,
            audio_device,
            chunk_seconds,
            silence_threshold,
            transcription_backend,
            whisper_catalog,
            whisper_download,
            whisper_remove,
            whisper_progress,
            whisper_status,
            transcription_provider,
            transcription_model,
            input_language,
            translate,
            target_language,
            suggestions,
            objection_handling,
            automatic_notes,
            screen_context,
            screen_interval,
            summary,
            save_session,
            analysis_task,
            profile,
            profile_name,
            profile_system,
        }
    }

    fn build_vision_settings(
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
            if let Some(item) = dropdown_text(dropdown)
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

        VisionSettingsWidgets {
            mode,
            provider,
            model,
            catalog,
            ollama_url,
            prompt,
            installed,
            refresh,
            download,
            delete,
            progress,
            status,
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
        let Some(selected_provider) = widgets.provider.cards.selected_children().first().cloned()
        else {
            self.set_settings_feedback("pick a provider first");
            return;
        };
        let index = selected_provider.index() as usize;
        let Some(provider) = widgets.provider_names.get(index).cloned() else {
            self.set_settings_feedback("pick a provider first");
            return;
        };
        let model = widgets.provider.model.text().trim().to_string();
        if model.is_empty() {
            self.set_settings_feedback("enter a model name");
            return;
        }
        let key = widgets.provider.api_key.text().to_string();
        let meeting_widgets = &widgets.meeting;
        let profile_name = meeting_widgets.profile_name.text().trim().to_string();
        if profile_name.is_empty()
            || !profile_name
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "-_".contains(character))
        {
            self.set_settings_feedback("profile name may contain only letters, numbers, - and _");
            return;
        }
        let transcription_index = meeting_widgets.transcription_provider.selected() as usize;
        let Some(transcription_provider) = widgets.provider_names.get(transcription_index).cloned()
        else {
            self.set_settings_feedback("pick a transcription provider");
            return;
        };
        let profile_buffer = meeting_widgets.profile_system.buffer();
        let profile_system = profile_buffer
            .text(
                &profile_buffer.start_iter(),
                &profile_buffer.end_iter(),
                false,
            )
            .to_string();
        if meeting_widgets.transcription_model.text().trim().is_empty() {
            self.set_settings_feedback("enter a transcription model");
            return;
        }
        if meeting_widgets.analysis_task.text().trim().is_empty() {
            self.set_settings_feedback("enter an analysis task");
            return;
        }
        if meeting_widgets.translate.is_active()
            && meeting_widgets.target_language.text().trim().is_empty()
        {
            self.set_settings_feedback("enter a translation target language");
            return;
        }
        if meeting_widgets.audio_source.selected() == 3
            && meeting_widgets.audio_device.text().trim().is_empty()
        {
            self.set_settings_feedback("enter a custom audio device");
            return;
        }
        if profile_system.trim().is_empty() {
            self.set_settings_feedback("enter an assistant profile prompt");
            return;
        }
        let vision_widgets = &widgets.vision;
        let vision_provider_index = vision_widgets.provider.selected() as usize;
        let Some(vision_provider) = widgets.provider_names.get(vision_provider_index).cloned()
        else {
            self.set_settings_feedback("pick a vision provider");
            return;
        };
        let vision_prompt_buffer = vision_widgets.prompt.buffer();
        let vision_prompt = vision_prompt_buffer
            .text(
                &vision_prompt_buffer.start_iter(),
                &vision_prompt_buffer.end_iter(),
                false,
            )
            .to_string();
        if vision_widgets.model.text().trim().is_empty() || vision_prompt.trim().is_empty() {
            self.set_settings_feedback("enter a vision model and OCR prompt");
            return;
        }
        let vision = VisionConfig {
            mode: match vision_widgets.mode.selected() {
                1 => "proxy",
                2 => "off",
                _ => "direct",
            }
            .into(),
            provider: vision_provider,
            model: vision_widgets.model.text().trim().into(),
            prompt: vision_prompt,
            ollama_url: vision_widgets.ollama_url.text().trim().into(),
        };
        let meeting = MeetingConfig {
            audio_source: match meeting_widgets.audio_source.selected() {
                1 => "microphone",
                2 => "both",
                3 => "custom",
                _ => "system",
            }
            .into(),
            audio_device: meeting_widgets.audio_device.text().trim().into(),
            chunk_seconds: meeting_widgets.chunk_seconds.value_as_int() as u64,
            silence_threshold: meeting_widgets.silence_threshold.value_as_int() as u16,
            transcription_backend: if meeting_widgets.transcription_backend.selected() == 1 {
                "remote".into()
            } else {
                "local".into()
            },
            whisper_model: selected_whisper_model(&meeting_widgets.whisper_catalog)
                .unwrap_or_else(|| "base".into()),
            transcription_provider,
            transcription_model: meeting_widgets.transcription_model.text().trim().into(),
            input_language: meeting_widgets.input_language.text().trim().into(),
            translate: meeting_widgets.translate.is_active(),
            target_language: meeting_widgets.target_language.text().trim().into(),
            suggestions: meeting_widgets.suggestions.is_active(),
            objection_handling: meeting_widgets.objection_handling.is_active(),
            automatic_notes: meeting_widgets.automatic_notes.is_active(),
            screen_context: meeting_widgets.screen_context.is_active(),
            screen_interval_chunks: meeting_widgets.screen_interval.value_as_int() as u32,
            summary: meeting_widgets.summary.is_active(),
            save_session: meeting_widgets.save_session.is_active(),
            analysis_task: meeting_widgets.analysis_task.text().trim().into(),
            profile: profile_name.clone(),
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
            provider_thinking: match widgets.provider.thinking.selected() {
                1 => Some(true),
                2 => Some(false),
                _ => None,
            },
            provider_reasoning_effort: selected_effort(&widgets.provider.reasoning_effort),
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

fn field_row(label: &str, control: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let label = gtk::Label::new(Some(label));
    label.set_xalign(0.0);
    label.set_width_chars(20);
    row.append(&label);
    row.append(control);
    row
}

fn settings_page(title: &str) -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 10);
    page.set_margin_top(6);
    page.set_margin_bottom(10);
    page.set_margin_start(8);
    page.set_margin_end(8);
    page.append(&section_heading(title));
    page
}

fn settings_scroll(page: &gtk::Box) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .child(page)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build()
}

fn note_label(text: &str) -> gtk::Label {
    let note = gtk::Label::new(Some(text));
    note.add_css_class("nexora-status");
    note.set_xalign(0.0);
    note.set_wrap(true);
    note
}

fn build_general_settings(config: &Config) -> GeneralSettingsWidgets {
    let hidden = switch(config.general.hidden);
    let layer_shell = gtk::DropDown::from_strings(&[
        "Automatic (recommended)",
        "Layer-shell overlay",
        "Normal window",
    ]);
    layer_shell.set_selected(match config.general.layer_shell.as_str() {
        "on" => 1,
        "off" => 2,
        _ => 0,
    });
    GeneralSettingsWidgets {
        hidden,
        layer_shell,
        hyprland_rule: entry_with(&config.general.hyprland_rule, "no_screen_share"),
        width: spin(480.0, 1600.0, config.general.width as f64),
        height: spin(320.0, 1200.0, config.general.height as f64),
    }
}

fn append_general_fields(page: &gtk::Box, general: &GeneralSettingsWidgets) {
    page.append(&field_row("Hidden from capture", &general.hidden));
    page.append(&field_row("Window mode", &general.layer_shell));
    page.append(&field_row("Hyprland rule", &general.hyprland_rule));
    page.append(&field_row("Window width", &general.width));
    page.append(&field_row("Window height", &general.height));
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

fn append_provider_fields(page: &gtk::Box, provider: &ProviderSettingsWidgets) {
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

fn section_heading(text: &str) -> gtk::Label {
    let heading = gtk::Label::new(Some(text));
    heading.add_css_class("nexora-title");
    heading.set_xalign(0.0);
    heading.set_margin_top(8);
    heading
}

fn entry_with(value: &str, placeholder: &str) -> gtk::Entry {
    let entry = gtk::Entry::builder()
        .text(value)
        .placeholder_text(placeholder)
        .hexpand(true)
        .build();
    entry.add_css_class("nexora-entry");
    entry
}

fn switch(active: bool) -> gtk::Switch {
    let control = gtk::Switch::new();
    control.set_active(active);
    control.set_halign(gtk::Align::Start);
    control
}

fn spin(min: f64, max: f64, value: f64) -> gtk::SpinButton {
    let control = gtk::SpinButton::with_range(min, max, 1.0);
    control.set_value(value);
    control
}

/// Catalog dropdown plus download/remove controls for local whisper models.
fn build_whisper_manager(
    selected_model: &str,
) -> (
    gtk::DropDown,
    gtk::Button,
    gtk::Button,
    gtk::ProgressBar,
    gtk::Label,
) {
    let labels: Vec<String> = whisper::PRESETS
        .iter()
        .map(|preset| {
            format!(
                "{} · {} · {} — {}",
                preset.id, preset.download, preset.size, preset.description
            )
        })
        .collect();
    let values: Vec<&str> = labels.iter().map(String::as_str).collect();
    let catalog = gtk::DropDown::from_strings(&values);
    if let Some(index) = whisper::PRESETS
        .iter()
        .position(|preset| preset.id == selected_model)
    {
        catalog.set_selected(index as u32);
    }
    let download = gtk::Button::with_label("Download selected model");
    download.add_css_class("nexora-attach");
    let remove = gtk::Button::with_label("Remove selected model");
    remove.add_css_class("nexora-attach");
    let progress = gtk::ProgressBar::new();
    progress.set_show_text(true);
    progress.set_visible(false);
    let status = note_label(&whisper_status_text());

    let catalog_for_download = catalog.clone();
    let status_for_download = status.clone();
    let progress_for_download = progress.clone();
    let download_button = download.clone();
    download.connect_clicked(move |_| {
        let Some(model) = selected_whisper_model(&catalog_for_download) else {
            return;
        };
        status_for_download.set_text(&format!("Downloading ggml-{model}.bin…"));
        progress_for_download.set_fraction(0.0);
        progress_for_download.set_text(Some("Starting…"));
        progress_for_download.set_visible(true);
        download_button.set_sensitive(false);
        let (progress_tx, progress_rx) = async_channel::unbounded();
        let (done_tx, done_rx) = async_channel::bounded(1);
        runtime().spawn(async move {
            let result = whisper::download_model(&model, progress_tx).await;
            let _ = done_tx.send(result).await;
        });
        let status = status_for_download.clone();
        let bar = progress_for_download.clone();
        let button = download_button.clone();
        glib::spawn_future_local(async move {
            loop {
                while let Ok(update) = progress_rx.try_recv() {
                    if let Some(total) = update.total.filter(|total| *total > 0) {
                        let fraction = update.completed as f64 / total as f64;
                        bar.set_fraction(fraction);
                        bar.set_text(Some(&format!(
                            "{} of {} · {:.0}%",
                            vision::format_bytes(update.completed),
                            vision::format_bytes(total),
                            fraction * 100.0
                        )));
                    } else {
                        bar.set_text(Some(&vision::format_bytes(update.completed)));
                    }
                }
                if let Ok(result) = done_rx.try_recv() {
                    button.set_sensitive(true);
                    match result {
                        Ok(()) => {
                            bar.set_fraction(1.0);
                            bar.set_text(Some("Complete"));
                            status.set_text(&whisper_status_text());
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

    let catalog_for_remove = catalog.clone();
    let status_for_remove = status.clone();
    remove.connect_clicked(move |_| {
        let Some(model) = selected_whisper_model(&catalog_for_remove) else {
            return;
        };
        match whisper::remove_model(&model) {
            Ok(()) => status_for_remove.set_text(&whisper_status_text()),
            Err(err) => status_for_remove.set_text(&format!("Remove failed: {err:#}")),
        }
    });

    (catalog, download, remove, progress, status)
}

fn selected_whisper_model(catalog: &gtk::DropDown) -> Option<String> {
    whisper::PRESETS
        .get(catalog.selected() as usize)
        .map(|preset| preset.id.to_string())
}

fn whisper_status_text() -> String {
    let installed = whisper::installed_models();
    if installed.is_empty() {
        "No local model downloaded yet. Downloads come from the official whisper.cpp repository and stay on this computer.".into()
    } else {
        let list: Vec<String> = installed
            .iter()
            .map(|(name, bytes)| format!("{name} ({})", vision::format_bytes(*bytes)))
            .collect();
        format!("Downloaded: {}", list.join(", "))
    }
}

fn append_meeting_fields(page: &gtk::Box, meeting: &MeetingSettingsWidgets) {
    page.append(&field_row("Audio source", &meeting.audio_source));
    page.append(&field_row("Custom audio device", &meeting.audio_device));
    page.append(&field_row(
        "Chunk seconds (lower = more requests)",
        &meeting.chunk_seconds,
    ));
    page.append(&field_row(
        "Silence gate (0 = disabled)",
        &meeting.silence_threshold,
    ));
    page.append(&section_heading("Transcription"));
    page.append(&field_row(
        "Transcription backend",
        &meeting.transcription_backend,
    ));
    page.append(&field_row("Local whisper model", &meeting.whisper_catalog));
    let whisper_actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    whisper_actions.append(&meeting.whisper_download);
    whisper_actions.append(&meeting.whisper_remove);
    page.append(&whisper_actions);
    page.append(&meeting.whisper_progress);
    page.append(&meeting.whisper_status);
    page.append(&note_label(
        "Local transcription runs on the CPU. `base` keeps up with 2-second chunks on most machines; try `small` or the quantized large-v3-turbo if your CPU is fast. Remote transcription uploads every audio chunk to the selected provider.",
    ));
    page.append(&field_row(
        "Remote transcription provider",
        &meeting.transcription_provider,
    ));
    page.append(&field_row(
        "Remote transcription model",
        &meeting.transcription_model,
    ));
    page.append(&field_row("Spoken language", &meeting.input_language));
    page.append(&field_row(
        "Live translation (+1 call/chunk)",
        &meeting.translate,
    ));
    page.append(&field_row("Target language", &meeting.target_language));
    page.append(&field_row(
        "Reply suggestions (shared AI call)",
        &meeting.suggestions,
    ));
    page.append(&field_row(
        "Objection handling (shared AI call)",
        &meeting.objection_handling,
    ));
    page.append(&field_row(
        "Automatic notes (shared AI call)",
        &meeting.automatic_notes,
    ));
    page.append(&field_row(
        "Screen context (image tokens)",
        &meeting.screen_context,
    ));
    page.append(&field_row(
        "Screen every N chunks",
        &meeting.screen_interval,
    ));
    page.append(&field_row("Final summary (+1 call)", &meeting.summary));
    page.append(&field_row("Save session", &meeting.save_session));
    page.append(&field_row("Analysis task", &meeting.analysis_task));
    page.append(&note_label(
        "Token guide: transcription always makes one request per non-empty audio chunk. Translation adds a second request. Suggestions, objections and notes share one coaching request; enabling any of them activates it. Screen context adds image input to that request. Longer chunks reduce request frequency but increase delay.",
    ));
}

fn append_vision_fields(page: &gtk::Box, vision: &VisionSettingsWidgets) {
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

fn append_profile_fields(page: &gtk::Box, meeting: &MeetingSettingsWidgets) {
    page.append(&field_row("Assistant profile", &meeting.profile));
    page.append(&field_row("Profile name", &meeting.profile_name));
    page.append(&field_row("Profile prompt", &meeting.profile_system));
}

fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn selected_provider_name(cards: &gtk::FlowBox, names: &[String]) -> Option<String> {
    let child = cards.selected_children().first().cloned()?;
    names.get(child.index() as usize).cloned()
}

fn dropdown_text(dropdown: &gtk::DropDown) -> Option<String> {
    dropdown
        .selected_item()?
        .downcast::<gtk::StringObject>()
        .ok()
        .map(|item| item.string().to_string())
}

fn selected_effort(dropdown: &gtk::DropDown) -> Option<String> {
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

fn set_effort_choices(
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

fn update_token_notice(
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

fn set_model_choices(dropdown: &gtk::DropDown, models: &[String], selected: &str) {
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
    let meeting = gtk::TextTag::builder()
        .name("meeting")
        .weight(700)
        .foreground("#8fd9a0")
        .build();
    let translation = gtk::TextTag::builder()
        .name("translation")
        .weight(700)
        .foreground("#8ed9e8")
        .build();
    let insight = gtk::TextTag::builder()
        .name("insight")
        .weight(700)
        .foreground("#d5b7ff")
        .build();
    let summary = gtk::TextTag::builder()
        .name("summary")
        .weight(700)
        .foreground("#ffd18e")
        .build();
    buffer.tag_table().add(&role);
    buffer.tag_table().add(&dim);
    buffer.tag_table().add(&meeting);
    buffer.tag_table().add(&translation);
    buffer.tag_table().add(&insight);
    buffer.tag_table().add(&summary);
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
    // Take keyboard focus only while the user interacts with the overlay.
    // Exclusive mode prevents using applications underneath and makes global
    // compositor binds feel inconsistent on Hyprland.
    window.set_keyboard_mode(KeyboardMode::OnDemand);
}
