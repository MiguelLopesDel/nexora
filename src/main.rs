use clap::{Parser, Subcommand};
use nexora::{app, config, hidden};

#[derive(Parser)]
#[command(
    name = "nexora",
    version,
    about = "Featherweight AI overlay assistant for Linux (Wayland-first)",
    long_about = "Nexora shows an on-demand AI prompt overlay on top of whatever you are doing.\n\
                  The first invocation stays resident; bind the subcommands to keys in your\n\
                  compositor (e.g. `nexora toggle`, `nexora run explain-screen`)."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Show the overlay (default when no command is given)
    Show,
    /// Show the overlay if hidden, hide it if visible
    Toggle,
    /// Fire a preset from the config (built-in: explain-screen)
    Run {
        /// Preset name under [presets] in config.toml
        preset: String,
    },
    /// Ask an arbitrary question through the resident overlay
    Ask {
        /// Question to send through the configured ask task
        prompt: String,
    },
    /// Control the live session from scripts and compositor keybinds
    Session {
        #[command(subcommand)]
        action: SessionAction,
    },
    /// Run the local OpenAI-compatible intermediary (web search, compaction)
    Relay,
    /// Quit the resident instance
    Quit,
    /// Anti-capture (screen-share hiding) helpers
    Hidden {
        #[command(subcommand)]
        action: HiddenAction,
    },
    /// Configuration helpers
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand)]
enum HiddenAction {
    /// Report whether this compositor can hide Nexora from screen capture
    Status,
}

#[derive(Subcommand)]
enum SessionAction {
    /// Start live capture and transcription
    Start,
    /// Stop capture and finalize the session
    Stop,
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Write a commented starter config to ~/.config/nexora/config.toml
    Init,
    /// Print the config file path
    Path,
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();

    // Commands that never need the GUI instance run locally and exit.
    match &cli.command {
        Some(Command::Hidden {
            action: HiddenAction::Status,
        }) => {
            println!("{}", hidden::status_report());
            return std::process::ExitCode::SUCCESS;
        }
        Some(Command::Relay) => {
            let result = config::Config::load()
                .and_then(|config| nexora::runtime().block_on(nexora::relay::serve(config)));
            return match result {
                Ok(()) => std::process::ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("nexora relay: {err:#}");
                    std::process::ExitCode::FAILURE
                }
            };
        }
        Some(Command::Config { action }) => {
            let result = match action {
                ConfigAction::Path => {
                    println!("{}", config::config_path().display());
                    Ok(())
                }
                ConfigAction::Init => config::init_config_file().map(|path| {
                    println!("wrote {}", path.display());
                    println!("edit it to add your API keys, then run `nexora toggle`");
                }),
            };
            return match result {
                Ok(()) => std::process::ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("nexora: {err:#}");
                    std::process::ExitCode::FAILURE
                }
            };
        }
        _ => {}
    }

    // Everything else goes through the (possibly already running) GTK app.
    let forwarded: Vec<&str> = match &cli.command {
        None | Some(Command::Show) => vec!["show"],
        Some(Command::Toggle) => vec!["toggle"],
        Some(Command::Quit) => vec!["quit"],
        Some(Command::Run { preset }) => vec!["run", preset],
        Some(Command::Ask { prompt }) => vec!["ask", prompt],
        Some(Command::Session {
            action: SessionAction::Start,
        }) => vec!["session", "start"],
        Some(Command::Session {
            action: SessionAction::Stop,
        }) => vec!["session", "stop"],
        Some(Command::Hidden { .. }) | Some(Command::Config { .. }) | Some(Command::Relay) => {
            unreachable!()
        }
    };
    std::process::ExitCode::from(app::run(&forwarded) as u8)
}
