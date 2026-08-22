//! Window chrome helpers: layer-shell setup and the anti-capture badge style.

use gtk4 as gtk;
use gtk4::prelude::*;
use gtk4_layer_shell::{KeyboardMode, Layer, LayerShell};

use crate::config::Config;
use crate::hidden::HiddenState;

pub(super) fn apply_badge_style(badge: &gtk::Label, state: &HiddenState) {
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

pub(super) fn setup_layer_shell(window: &gtk::ApplicationWindow, config: &Config) {
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
