//! THROWAWAY PROTOTYPE — do not ship as product code.
//!
//! This executable answers one question: can the current Wayland session expose
//! a clean virtual workspace that ordinary screen-share consumers can select?

use gtk4::prelude::*;
use std::env;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::time::Duration;

const OUTPUT: &str = "NEXORA-SAFE-SHARE";
const WORKSPACE: &str = "99";
const MARKER_ARG: &str = "--marker";

fn main() -> ExitCode {
    if env::args().any(|arg| arg == MARKER_ARG) {
        return run_marker();
    }

    println!("Nexora Safe Share virtual-workspace prototype");
    println!("THROWAWAY: this probes capabilities; it is not production code.\n");

    let state = State::inspect();
    state.print();

    if !io::stdin().is_terminal() || env::args().any(|arg| arg == "--report-only") {
        return ExitCode::SUCCESS;
    }

    match state.compositor.as_str() {
        "hyprland" => run_hyprland_probe(),
        "niri" => {
            println!("\nVERDICT: runtime gate not met on niri.");
            println!(
                "niri does not expose an unprivileged command for creating a headless output."
            );
            println!(
                "The next prototype must supply a compositor-independent output (for example,"
            );
            println!("a dedicated nested runtime) before portal and interaction tests can run.");
            ExitCode::from(2)
        }
        _ => {
            println!("\nVERDICT: run this inside a Hyprland or niri Wayland session.");
            ExitCode::from(2)
        }
    }
}

struct State {
    compositor: String,
    session_type: String,
    desktop: String,
    wayland_display: String,
    screen_cast_sources: String,
    remote_desktop_devices: String,
    compositor_outputs: String,
    tools: Vec<(&'static str, bool)>,
}

impl State {
    fn inspect() -> Self {
        let compositor = if env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
            "hyprland"
        } else if env::var_os("NIRI_SOCKET").is_some() {
            "niri"
        } else {
            "unknown"
        };

        let compositor_outputs = match compositor {
            "hyprland" => command_output("hyprctl", &["monitors", "all", "-j"]),
            "niri" => command_output("niri", &["msg", "outputs"]),
            _ => "unavailable".to_owned(),
        };

        Self {
            compositor: compositor.to_owned(),
            session_type: env_value("XDG_SESSION_TYPE"),
            desktop: env_value("XDG_CURRENT_DESKTOP"),
            wayland_display: env_value("WAYLAND_DISPLAY"),
            screen_cast_sources: portal_property(
                "org.freedesktop.portal.ScreenCast",
                "AvailableSourceTypes",
            ),
            remote_desktop_devices: portal_property(
                "org.freedesktop.portal.RemoteDesktop",
                "AvailableDeviceTypes",
            ),
            compositor_outputs,
            tools: [
                "gdbus",
                "hyprctl",
                "niri",
                "wl-mirror",
                "wayvnc",
                "vncviewer",
            ]
            .into_iter()
            .map(|tool| (tool, command_exists(tool)))
            .collect(),
        }
    }

    fn print(&self) {
        println!("STATE");
        println!("  compositor: {}", self.compositor);
        println!("  session_type: {}", self.session_type);
        println!("  desktop: {}", self.desktop);
        println!("  wayland_display: {}", self.wayland_display);
        println!("  portal_screen_cast_sources: {}", self.screen_cast_sources);
        println!(
            "  portal_remote_desktop_devices: {}",
            self.remote_desktop_devices
        );
        println!("  tools:");
        for (tool, installed) in &self.tools {
            println!("    {tool}: {}", if *installed { "yes" } else { "no" });
        }
        println!(
            "  compositor_outputs:\n{}",
            indent(&self.compositor_outputs, 4)
        );
    }
}

fn run_hyprland_probe() -> ExitCode {
    if !command_exists("hyprctl") {
        eprintln!("hyprctl is unavailable.");
        return ExitCode::from(2);
    }

    if command_output("hyprctl", &["monitors", "all", "-j"]).contains(OUTPUT) {
        println!("\nRemoving a stale prototype output from an interrupted run...");
        let _ = run("hyprctl", &["output", "remove", OUTPUT]);
    }

    if !confirm("Create a temporary 1920x1080 headless output now? [y/N] ") {
        println!("No changes made.");
        return ExitCode::SUCCESS;
    }

    if !run("hyprctl", &["output", "create", "headless", OUTPUT]) {
        eprintln!("VERDICT: Hyprland rejected headless-output creation.");
        return ExitCode::from(2);
    }

    let configured = run(
        "hyprctl",
        &[
            "keyword",
            "monitor",
            &format!("{OUTPUT},1920x1080@60,auto,1"),
        ],
    );
    let marker_started = start_hyprland_marker();
    std::thread::sleep(Duration::from_millis(750));
    let workspace_moved = run(
        "hyprctl",
        &["dispatch", "moveworkspacetomonitor", WORKSPACE, OUTPUT],
    );

    println!("\nSTATE AFTER CREATE");
    println!("  output_created: yes");
    println!("  output_configured: {configured}");
    println!("  workspace_assigned: {workspace_moved}");
    println!("  clean_marker_started: {marker_started}");
    println!(
        "  outputs:\n{}",
        indent(&command_output("hyprctl", &["monitors", "all", "-j"]), 4)
    );

    println!("\nLIVE CHECK");
    println!("  1. Open OBS, a browser meeting, or Discord screen sharing.");
    println!("  2. Select the monitor named {OUTPUT}.");
    println!("  3. Confirm the colored marker is present and Nexora is absent.");
    println!("  4. Record several seconds while moving Nexora on the physical monitor.");
    println!("  5. Return here and press Enter to tear the prototype down.");
    println!("\nInteraction is NOT proven merely by viewing the headless output. A VNC or");
    println!("portal RemoteDesktop preview must forward pointer and keyboard input.");

    let mut ignored = String::new();
    let _ = io::stdin().read_line(&mut ignored);
    stop_marker();
    let removed = run("hyprctl", &["output", "remove", OUTPUT]);

    println!("\nFINAL STATE");
    println!("  marker_stopped: yes");
    println!("  output_removed: {removed}");
    println!("  verdict: capture selection can be judged from the recording;");
    println!("           local interaction remains a separate required gate.");
    ExitCode::SUCCESS
}

fn start_hyprland_marker() -> bool {
    let Ok(exe) = env::current_exe() else {
        return false;
    };
    let command = format!(
        "[workspace {WORKSPACE} silent] {} {MARKER_ARG}",
        shell_quote(&exe)
    );
    run("hyprctl", &["dispatch", "exec", &command])
}

fn run_marker() -> ExitCode {
    let app = gtk4::Application::builder()
        .application_id("dev.nexora.SafeSharePrototype")
        .build();
    app.connect_activate(|app| {
        let label = gtk4::Label::new(Some(
            "NEXORA SAFE SHARE\nCLEAN VIRTUAL WORKSPACE\n\nThe Nexora overlay must not appear here.",
        ));
        label.add_css_class("title-1");
        let window = gtk4::ApplicationWindow::builder()
            .application(app)
            .title("Nexora Safe Share Prototype Marker")
            .default_width(1200)
            .default_height(700)
            .child(&label)
            .build();
        window.fullscreen();
        window.present();
    });

    let pid_path = marker_pid_path();
    let _ = std::fs::write(&pid_path, std::process::id().to_string());
    let status = app.run();
    let _ = std::fs::remove_file(pid_path);
    status.into()
}

fn stop_marker() {
    let path = marker_pid_path();
    if let Ok(pid) = std::fs::read_to_string(&path) {
        let _ = Command::new("kill").arg(pid.trim()).status();
    }
    let _ = std::fs::remove_file(path);
}

fn marker_pid_path() -> PathBuf {
    env::temp_dir().join("nexora-safe-share-prototype-marker.pid")
}

fn portal_property(interface: &str, property: &str) -> String {
    if !command_exists("gdbus") {
        return "gdbus unavailable".to_owned();
    }
    command_output(
        "gdbus",
        &[
            "call",
            "--session",
            "--dest",
            "org.freedesktop.portal.Desktop",
            "--object-path",
            "/org/freedesktop/portal/desktop",
            "--method",
            "org.freedesktop.DBus.Properties.Get",
            interface,
            property,
        ],
    )
}

fn env_value(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| "unset".to_owned())
}

fn command_exists(command: &str) -> bool {
    Command::new("sh")
        .args(["-c", "command -v -- \"$1\" >/dev/null 2>&1", "sh", command])
        .status()
        .is_ok_and(|status| status.success())
}

fn command_output(program: &str, args: &[&str]) -> String {
    match Command::new(program).args(args).output() {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            format!("error ({}): {}", output.status, stderr.trim())
        }
        Err(error) => format!("unavailable: {error}"),
    }
}

fn run(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .status()
        .is_ok_and(|status| status.success())
}

fn confirm(prompt: &str) -> bool {
    print!("\n{prompt}");
    let _ = io::stdout().flush();
    let mut answer = String::new();
    io::stdin().read_line(&mut answer).is_ok()
        && matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

fn indent(value: &str, spaces: usize) -> String {
    let prefix = " ".repeat(spaces);
    value
        .lines()
        .map(|line| format!("{prefix}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn shell_quote(path: &std::path::Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}
