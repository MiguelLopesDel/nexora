//! "Window behavior" settings page.

use gtk4 as gtk;
use gtk4::prelude::*;

use super::widgets::{entry_with, field_row, spin, switch};
use crate::config::Config;

pub(super) struct GeneralSettingsWidgets {
    pub(super) hidden: gtk::Switch,
    pub(super) layer_shell: gtk::DropDown,
    pub(super) hyprland_rule: gtk::Entry,
    pub(super) width: gtk::SpinButton,
    pub(super) height: gtk::SpinButton,
}

pub(super) fn build_general_settings(config: &Config) -> GeneralSettingsWidgets {
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

pub(super) fn append_general_fields(page: &gtk::Box, general: &GeneralSettingsWidgets) {
    page.append(&field_row("Hidden from capture", &general.hidden));
    page.append(&field_row("Window mode", &general.layer_shell));
    page.append(&field_row("Hyprland rule", &general.hyprland_rule));
    page.append(&field_row("Window width", &general.width));
    page.append(&field_row("Window height", &general.height));
}
