//! Small widget builders shared across every settings section.

use gtk4 as gtk;
use gtk4::prelude::*;

pub(super) fn field_row(label: &str, control: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let label = gtk::Label::new(Some(label));
    label.set_xalign(0.0);
    label.set_width_chars(20);
    row.append(&label);
    row.append(control);
    row
}

pub(super) fn settings_page(title: &str) -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 10);
    page.set_margin_top(6);
    page.set_margin_bottom(10);
    page.set_margin_start(8);
    page.set_margin_end(8);
    page.append(&section_heading(title));
    page
}

pub(super) fn settings_scroll(page: &gtk::Box) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .child(page)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build()
}

pub(super) fn note_label(text: &str) -> gtk::Label {
    let note = gtk::Label::new(Some(text));
    note.add_css_class("nexora-status");
    note.set_xalign(0.0);
    note.set_wrap(true);
    note
}

pub(super) fn section_heading(text: &str) -> gtk::Label {
    let heading = gtk::Label::new(Some(text));
    heading.add_css_class("nexora-title");
    heading.set_xalign(0.0);
    heading.set_margin_top(8);
    heading
}

pub(super) fn entry_with(value: &str, placeholder: &str) -> gtk::Entry {
    let entry = gtk::Entry::builder()
        .text(value)
        .placeholder_text(placeholder)
        .hexpand(true)
        .build();
    entry.add_css_class("nexora-entry");
    entry
}

pub(super) fn switch(active: bool) -> gtk::Switch {
    let control = gtk::Switch::new();
    control.set_active(active);
    control.set_halign(gtk::Align::Start);
    control
}

pub(super) fn spin(min: f64, max: f64, value: f64) -> gtk::SpinButton {
    let control = gtk::SpinButton::with_range(min, max, 1.0);
    control.set_value(value);
    control
}
