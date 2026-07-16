//! GTK application: single instance, command forwarding, window lifecycle.
//!
//! The first `nexora …` invocation becomes the resident primary instance;
//! later invocations (from compositor keybinds) forward their command to it
//! over D-Bus and exit. That is what makes global keybinds work on every
//! Wayland compositor and X11 without any portal support.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4 as gtk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;

use crate::config::Config;
use crate::hidden::{self, HiddenState};
use crate::ui::{self, Overlay};

thread_local! {
    static OVERLAY: RefCell<Option<Rc<Overlay>>> = const { RefCell::new(None) };
}

pub fn run(forwarded: &[&str]) -> i32 {
    let app = gtk::Application::builder()
        .application_id(hidden::APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    app.connect_startup(|app| {
        // Install the desktop entry so xdg-desktop-portal attributes screenshot
        // and other portal requests to "Nexora" instead of the launching
        // terminal. Best-effort and idempotent.
        if let Err(err) = install_desktop_entry() {
            eprintln!("nexora: could not install desktop entry: {err:#}");
        }

        let provider = gtk::CssProvider::new();
        provider.load_from_data(ui::STYLE);
        if let Some(display) = gtk4::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        // Stay resident after the window hides so the next keybind is instant.
        std::mem::forget(app.hold());
    });

    app.connect_command_line(|app, cmdline| {
        let args: Vec<String> = cmdline
            .arguments()
            .into_iter()
            .skip(1)
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        glib::ExitCode::from(handle_command(app, &args))
    });

    // GApplication forwards this argv to the primary instance if one exists.
    let mut argv = vec!["nexora"];
    argv.extend_from_slice(forwarded);
    app.run_with_args(&argv).into()
}

fn handle_command(app: &gtk::Application, args: &[&str]) -> u8 {
    match args {
        [] | ["show"] => overlay(app).present(),
        ["toggle"] => overlay(app).toggle(),
        ["quit"] => app.quit(),
        ["run", preset_name] => {
            let overlay = overlay(app);
            match Config::load().unwrap_or_default().preset(preset_name) {
                Ok(preset) => {
                    overlay.present();
                    overlay.ask(preset.prompt, preset.attach_screen, preset.task);
                }
                Err(err) => {
                    eprintln!("nexora: {err:#}");
                    return 1;
                }
            }
        }
        other => {
            eprintln!("nexora: unknown command {other:?}");
            return 2;
        }
    }
    0
}

/// Write `dev.nexora.Nexora.desktop` into the user's applications directory if
/// it is missing or out of date. The portal reads this file to name and
/// icon the app in permission dialogs.
fn install_desktop_entry() -> std::io::Result<()> {
    const ENTRY: &str = include_str!("../assets/dev.nexora.Nexora.desktop");
    let Some(dir) = dirs::data_dir() else {
        return Ok(());
    };
    let apps = dir.join("applications");
    std::fs::create_dir_all(&apps)?;
    let path = apps.join(concat!("dev.nexora.Nexora", ".desktop"));
    if std::fs::read_to_string(&path).is_ok_and(|current| current == ENTRY) {
        return Ok(());
    }
    std::fs::write(&path, ENTRY)
}

/// Get or lazily build the single overlay window (primary instance only).
fn overlay(app: &gtk::Application) -> Rc<Overlay> {
    OVERLAY.with_borrow_mut(|slot| {
        if let Some(existing) = slot {
            return Rc::clone(existing);
        }
        let config = match Config::load() {
            Ok(config) => config,
            Err(err) => {
                eprintln!("nexora: {err:#}; using defaults");
                Config::default()
            }
        };
        // Apply before the window maps so compositor rules match it on first map.
        let hidden_state = if config.general.hidden {
            let state = hidden::apply(
                &config.general.hyprland_rule,
                config.general.layer_shell != "off",
            );
            match &state {
                HiddenState::Active => {}
                HiddenState::Manual(detail) | HiddenState::Unsupported(detail) => {
                    eprintln!("nexora: anti-capture: {detail}");
                }
            }
            state
        } else {
            HiddenState::Unsupported("disabled in config (general.hidden = false)".into())
        };

        let built = Overlay::new(app, config, hidden_state);
        // Hide instead of destroying on close so state survives toggles.
        let window = built.window.clone();
        window.connect_close_request(|window| {
            window.set_visible(false);
            glib::Propagation::Stop
        });
        *slot = Some(Rc::clone(&built));
        built
    })
}
