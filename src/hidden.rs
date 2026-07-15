//! Anti-capture ("hidden") mode.
//!
//! There is no universal Wayland protocol to exclude a window from screen
//! capture, so support is per compositor:
//!   - Hyprland: applied automatically via `hyprctl keyword windowrule`.
//!   - niri: needs a static `window-rule` in config.kdl (we print it).
//!   - everything else (GNOME, KDE, COSMIC, X11): unsupported today.

use std::process::Command;

/// Wayland app-id / X11 class of the Nexora window (the GTK application id).
pub const APP_ID: &str = "dev.nexora.Nexora";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compositor {
    Hyprland,
    Niri,
    Gnome,
    Kde,
    Cosmic,
    OtherWayland,
    X11,
    Unknown,
}

impl Compositor {
    pub fn detect() -> Self {
        let env = |k: &str| std::env::var(k).unwrap_or_default();
        if !env("HYPRLAND_INSTANCE_SIGNATURE").is_empty() {
            return Self::Hyprland;
        }
        if !env("NIRI_SOCKET").is_empty() {
            return Self::Niri;
        }
        let desktop = env("XDG_CURRENT_DESKTOP").to_lowercase();
        let session = env("XDG_SESSION_TYPE").to_lowercase();
        let wayland = session == "wayland" || !env("WAYLAND_DISPLAY").is_empty();
        if desktop.contains("gnome") {
            Self::Gnome
        } else if desktop.contains("kde") {
            Self::Kde
        } else if desktop.contains("cosmic") {
            Self::Cosmic
        } else if wayland {
            Self::OtherWayland
        } else if session == "x11" || !env("DISPLAY").is_empty() {
            Self::X11
        } else {
            Self::Unknown
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Hyprland => "Hyprland",
            Self::Niri => "niri",
            Self::Gnome => "GNOME",
            Self::Kde => "KDE Plasma",
            Self::Cosmic => "COSMIC",
            Self::OtherWayland => "Wayland (other compositor)",
            Self::X11 => "X11",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HiddenState {
    /// Compositor confirmed the window is excluded from capture.
    Active,
    /// User action needed; the string is what to do.
    Manual(String),
    /// This compositor cannot hide windows from capture.
    Unsupported(String),
}

impl HiddenState {
    pub fn badge(&self) -> &'static str {
        match self {
            Self::Active => "hidden from capture",
            Self::Manual(_) => "hidden: manual setup",
            Self::Unsupported(_) => "visible to capture",
        }
    }
}

/// KDL rule the user must add to niri's config.kdl.
pub fn niri_rule() -> String {
    format!(
        "window-rule {{\n    match app-id=\"{APP_ID}\"\n    block-out-from \"screen-capture\"\n}}"
    )
}

/// Try to enable anti-capture. Call before the window is mapped so
/// compositor-side rules apply to it on first map.
pub fn apply() -> HiddenState {
    match Compositor::detect() {
        Compositor::Hyprland => apply_hyprland(),
        Compositor::Niri => HiddenState::Manual(format!(
            "niri supports hiding, but only via config. Add to config.kdl:\n{}",
            niri_rule()
        )),
        other => HiddenState::Unsupported(format!(
            "{} has no way to exclude a window from screen capture yet; \
             Nexora will be visible to recordings and screen shares.",
            other.name()
        )),
    }
}

fn apply_hyprland() -> HiddenState {
    // Hyprland >= 0.51 merged the old windowrulev2 syntax into windowrule;
    // try the merged form first, then the legacy keyword for older releases.
    let rule = format!("noscreenshare, class:^({})$", regex_escape(APP_ID));
    for keyword in ["windowrule", "windowrulev2"] {
        match Command::new("hyprctl")
            .args(["keyword", keyword, &rule])
            .output()
        {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                // hyprctl exits 0 even on config errors; it prints "ok" on success.
                if stdout.trim().eq_ignore_ascii_case("ok") {
                    return HiddenState::Active;
                }
            }
            Ok(_) => {}
            Err(err) => {
                return HiddenState::Manual(format!(
                    "could not run hyprctl ({err}); add this rule to hyprland.conf:\n\
                     windowrule = {rule}"
                ));
            }
        }
    }
    HiddenState::Manual(format!(
        "hyprctl rejected the rule (old Hyprland?); add to hyprland.conf:\nwindowrule = {rule}"
    ))
}

fn regex_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if "\\.+*?()|[]{}^$".contains(ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Human-readable report for `nexora hidden status`.
pub fn status_report() -> String {
    let compositor = Compositor::detect();
    let support = match compositor {
        Compositor::Hyprland => {
            "supported — applied automatically at startup (windowrule noscreenshare)"
        }
        Compositor::Niri => "supported — requires a window-rule in config.kdl (shown below)",
        Compositor::Gnome | Compositor::Kde | Compositor::Cosmic | Compositor::OtherWayland => {
            "unsupported — this compositor cannot exclude a window from capture yet"
        }
        Compositor::X11 => "unsupported — on X11 any client can read any window",
        Compositor::Unknown => "unknown — could not detect a graphical session",
    };
    let mut report = format!(
        "compositor: {}\nanti-capture: {}",
        compositor.name(),
        support
    );
    if compositor == Compositor::Niri {
        report.push_str("\n\n");
        report.push_str(&niri_rule());
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regex_escape_escapes_dots() {
        assert_eq!(regex_escape("dev.nexora.Nexora"), r"dev\.nexora\.Nexora");
    }

    #[test]
    fn niri_rule_mentions_app_id() {
        assert!(niri_rule().contains(APP_ID));
        assert!(niri_rule().contains("block-out-from"));
    }
}
