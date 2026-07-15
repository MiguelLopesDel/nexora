//! The prompt overlay window.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gtk4 as gtk;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4_layer_shell::{KeyboardMode, Layer, LayerShell};

use crate::config::Config;
use crate::hidden::HiddenState;
use crate::providers::{ChatRequest, StreamEvent, stream_chat};
use crate::{runtime, screenshot};

pub struct Overlay {
    pub window: gtk::ApplicationWindow,
    config: Config,
    hidden_state: HiddenState,
    entry: gtk::Entry,
    attach: gtk::ToggleButton,
    response: gtk::TextView,
    end_mark: gtk::TextMark,
    status: gtk::Label,
    busy: Cell<bool>,
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

        // Header: title + anti-capture badge.
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let title = gtk::Label::new(Some("Nexora"));
        title.add_css_class("nexora-title");
        title.set_hexpand(true);
        title.set_xalign(0.0);
        let badge = gtk::Label::new(Some(hidden_state.badge()));
        badge.add_css_class("nexora-badge");
        badge.add_css_class(match hidden_state {
            HiddenState::Active => "hidden-active",
            _ => "hidden-off",
        });
        match &hidden_state {
            HiddenState::Manual(detail) | HiddenState::Unsupported(detail) => {
                badge.set_tooltip_text(Some(detail));
            }
            HiddenState::Active => {}
        }
        header.append(&title);
        header.append(&badge);
        root.append(&header);

        // Streaming response area.
        let response = gtk::TextView::builder()
            .editable(false)
            .cursor_visible(false)
            .wrap_mode(gtk::WrapMode::WordChar)
            .build();
        response.add_css_class("nexora-response");
        let end_mark = response
            .buffer()
            .create_mark(None, &response.buffer().end_iter(), false);
        let scroll = gtk::ScrolledWindow::builder()
            .child(&response)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        root.append(&scroll);

        let status = gtk::Label::new(None);
        status.add_css_class("nexora-status");
        status.set_xalign(0.0);
        root.append(&status);

        // Input row: screenshot toggle + prompt entry.
        let input_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let attach = gtk::ToggleButton::builder()
            .label("📷")
            .tooltip_text("Attach a screenshot of your screen")
            .build();
        attach.add_css_class("nexora-attach");
        let entry = gtk::Entry::builder()
            .placeholder_text("Ask anything… (Enter to send, Esc to hide)")
            .hexpand(true)
            .build();
        entry.add_css_class("nexora-entry");
        input_row.append(&attach);
        input_row.append(&entry);
        root.append(&input_row);

        window.set_child(Some(&root));

        let overlay = Rc::new(Self {
            window,
            config,
            hidden_state,
            entry,
            attach,
            response,
            end_mark,
            status,
            busy: Cell::new(false),
        });

        // Enter sends the prompt from the entry.
        let this = Rc::clone(&overlay);
        overlay.entry.connect_activate(move |entry| {
            let prompt = entry.text().trim().to_string();
            if prompt.is_empty() {
                return;
            }
            entry.set_text("");
            this.ask(prompt, this.attach.is_active(), "ask".to_string());
        });

        // Esc hides the overlay.
        let keys = gtk::EventControllerKey::new();
        let this = Rc::clone(&overlay);
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                this.window.set_visible(false);
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        overlay.window.add_controller(keys);

        overlay
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

    /// Send a prompt using the given task, optionally attaching a screenshot.
    pub fn ask(self: &Rc<Self>, prompt: String, attach_screen: bool, task_name: String) {
        if self.busy.get() {
            self.status
                .set_text("still answering — wait for the current response");
            return;
        }
        self.busy.set(true);
        self.response.buffer().set_text("");
        self.set_status("thinking…");

        let this = Rc::clone(self);
        glib::spawn_future_local(async move {
            this.run_ask(prompt, attach_screen, task_name).await;
            this.busy.set(false);
        });
    }

    async fn run_ask(self: &Rc<Self>, prompt: String, attach_screen: bool, task_name: String) {
        let image = if attach_screen {
            self.set_status("capturing screen…");
            match self.capture_avoiding_self().await {
                Ok(png) => Some(png),
                Err(err) => {
                    self.show_error(&format!("screenshot failed: {err:#}"));
                    return;
                }
            }
        } else {
            None
        };
        self.present();

        let (task, provider) = match self
            .config
            .task(&task_name)
            .and_then(|task| Ok((task.clone(), self.config.provider_for(task)?.clone())))
        {
            Ok(pair) => pair,
            Err(err) => {
                self.show_error(&format!(
                    "{err:#}\n\nRun `nexora config init` to create a starter config."
                ));
                return;
            }
        };

        self.set_status(&format!("{} · {}", task.provider, task.model));
        let request = ChatRequest::from_task(&task, prompt, image);
        let (tx, rx) = async_channel::unbounded::<StreamEvent>();
        runtime().spawn(async move { stream_chat(&provider, request, tx).await });

        while let Ok(event) = rx.recv().await {
            match event {
                StreamEvent::Delta(text) => self.append_response(&text),
                StreamEvent::Done => break,
                StreamEvent::Error(message) => {
                    self.show_error(&message);
                    return;
                }
            }
        }
        self.set_status("");
    }

    /// Screenshot the screen without Nexora in it: when the compositor is not
    /// already hiding us from capture, momentarily hide the window.
    async fn capture_avoiding_self(&self) -> anyhow::Result<Vec<u8>> {
        let must_hide = self.window.is_visible() && self.hidden_state != HiddenState::Active;
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

    fn append_response(&self, text: &str) {
        let buffer = self.response.buffer();
        buffer.insert(&mut buffer.end_iter(), text);
        self.response.scroll_mark_onscreen(&self.end_mark);
    }

    fn show_error(&self, message: &str) {
        let buffer = self.response.buffer();
        buffer.insert(&mut buffer.end_iter(), message);
        self.set_status("error");
    }

    fn set_status(&self, text: &str) {
        self.status.set_text(text);
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
