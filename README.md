# Nexora

**A featherweight AI overlay assistant for Linux.** Press a key anywhere, ask anything — about your screen, a term you just read, an error you're staring at — and get an answer streamed on top of whatever you're doing. Wayland-first, works on X11.

Think "Jarvis on your desktop", except it's a single small native binary, fully open source, and you bring your own API key (or local model).

## Features

- **Global keybind overlay** — bind `nexora toggle` to any key in your compositor; a prompt window appears over your work, streams the answer, and hides on Esc. The first invocation stays resident, so it's instant.
- **Conversation history** — the overlay keeps the whole conversation, so follow-up questions have context. `Ctrl+N` starts a fresh chat. Conversations are saved to `~/.local/share/nexora/history/` and the last one is restored on launch. Screenshots are never written to history and never resent on follow-ups (they're the token-heavy part).
- **Explain my screen** — `nexora run explain-screen` grabs a screenshot through the XDG desktop portal, sends it to a vision model, and explains what you're looking at. Define your own presets (translate screen, summarize, whatever) in the config and bind each to its own key.
- **Any provider** — Anthropic natively, plus every OpenAI-compatible API: OpenAI, OpenRouter, DeepSeek, Gemini (compat endpoint), Ollama / llama.cpp running locally. Pick a different provider+model per task.
- **In-app settings** — a ⚙ panel to choose the provider, paste an API key (stored in `config.toml`, `chmod 600`), set the model, toggle hidden mode, and copy ready-made keybind snippets. Power users can still hand-edit the TOML; the panel preserves your comments and other keys.
- **Hidden mode** — where the compositor supports it, the Nexora window is excluded from screen capture: invisible to recordings, streams and screen shares. See the support matrix below — Nexora tells you the truth about what your compositor can do instead of pretending.
- **Ridiculously light** — native GTK4 in Rust. One binary, no Electron, no web view, no background CPU. It runs on a potato.

## Support matrix

| Environment | Overlay | Screenshot | Hidden (anti-capture) |
|---|---|---|---|
| Hyprland | layer-shell | portal | **automatic** (`windowrule noscreenshare`) |
| niri | layer-shell | portal | **supported** (one rule in config.kdl, shown by `nexora hidden status`) |
| KDE Plasma (Wayland) | layer-shell | portal | not available in KWin yet |
| GNOME (Wayland) | regular window | portal | not available in Mutter yet |
| COSMIC | layer-shell | portal | not available yet |
| X11 (any WM) | regular window | portal | impossible by design (any X client can read any window) |

There is no universal Wayland protocol for excluding a window from capture; Nexora uses each compositor's native mechanism where one exists and shows a clear "visible to capture" badge where it doesn't.

### Why can't it just hide everywhere (with root, or by starting early)?

It can't, and this is architectural — not a permission Nexora can grab:

- **X11**: any connected client can read the whole root window (`XGetImage`), which *already contains* your window's pixels composited by the X server. There's no per-window "don't capture me" in the X protocol. Running as root or starting before the session changes nothing — a recorder reads the server's framebuffer, not Nexora's process.
- **GNOME (Mutter) / KDE (KWin) / most Wayland**: capture goes *through the compositor* (PipeWire + portal). The compositor composites every window into the frame it hands the recorder. To leave a window out, **the compositor** must support it and choose to. A client — even root, even launched at boot — can't override the compositor's own compositing, because there's no protocol for it. You'd have to patch the compositor itself.

Privilege and timing don't help because the screen recorder never reads *your* process; it reads the compositor's (or X server's) output. That's deliberate: if any app could make itself invisible to a screen share, malware would abuse it. This is exactly why only compositors with a native opt-out (niri; Hyprland via a windowrule) can do it — and why the paid tools that hide on Windows/macOS rely on OS-level client APIs (`SetWindowDisplayAffinity`, `NSWindow.sharingType`) that simply have no equivalent on Linux.

On Hyprland the rule keyword has changed across versions; if hiding doesn't take effect, run `nexora hidden status` and set `hyprland_rule` in `config.toml` to the keyword your version uses.

## Installing

### Dependencies

- GTK 4 ≥ 4.10, gtk4-layer-shell, and `xdg-desktop-portal` with a backend for your desktop (you almost certainly already have it).

```bash
# Debian / Ubuntu
sudo apt install libgtk-4-dev libgtk4-layer-shell-dev

# Fedora
sudo dnf install gtk4-devel gtk4-layer-shell-devel

# Arch
sudo pacman -S gtk4 gtk4-layer-shell
```

### Build

```bash
git clone https://github.com/MiguelLopesDel/nexora
cd nexora
cargo build --release
install -Dm755 target/release/nexora ~/.local/bin/nexora
```

## Quick start

```bash
nexora config init          # writes ~/.config/nexora/config.toml
$EDITOR ~/.config/nexora/config.toml   # pick providers, models, presets
export ANTHROPIC_API_KEY=...           # or whichever provider you chose
nexora toggle               # first run stays resident; press Esc to hide
```

See [config.example.toml](config.example.toml) for the full commented configuration: providers, per-task models, and presets.

### Bind the keys

Nexora's CLI forwards commands to the resident instance, so global shortcuts work on **every** compositor — just bind shell commands:

**Hyprland** (`hyprland.conf`):
```conf
bind = SUPER, A, exec, nexora toggle
bind = SUPER SHIFT, A, exec, nexora run explain-screen
```

**niri** (`config.kdl`):
```kdl
binds {
    Mod+A { spawn "nexora" "toggle"; }
    Mod+Shift+A { spawn "nexora" "run" "explain-screen"; }
}
```

**GNOME**: Settings → Keyboard → Custom Shortcuts → command `nexora toggle`.

**KDE Plasma**: System Settings → Shortcuts → Add Command → `nexora toggle`.

### Hidden mode

```bash
nexora hidden status   # what can your compositor do?
```

On Hyprland the anti-capture rule is applied automatically at startup. On niri, add the printed `window-rule` to your `config.kdl`. Everywhere else the overlay shows a "visible to capture" badge so you're never surprised. When you attach a screenshot and the window *can't* be hidden, Nexora briefly hides itself so it doesn't appear in its own capture.

## CLI

```
nexora                 show the overlay (also starts the resident instance)
nexora toggle          show/hide the overlay
nexora run <preset>    fire a preset (built-in: explain-screen)
nexora hidden status   anti-capture support report for this compositor
nexora config init     write a starter config
nexora config path     print the config path
nexora quit            stop the resident instance
```

## Roadmap

- [x] Conversation history in the overlay (follow-up questions)
- [x] In-app settings panel (provider, API key, model, hidden toggle)
- [ ] Audio transcription (mic → text via API or local whisper.cpp)
- [ ] GlobalShortcuts portal support (keybinds without touching compositor config)
- [ ] Watch mode — periodic screen understanding with cost controls (this burns tokens; it will be opt-in and heavily rate-limited)
- [ ] Prebuilt packages (.deb / .rpm / AUR)
- [ ] Markdown rendering in responses

Contributions welcome — see [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Nexora is licensed under the [GNU AGPL-3.0](LICENSE): you can use, study, modify and redistribute it, but derivative works must stay open source under the same terms, including when offered as a network service — and attribution must be preserved.

Contributions are accepted under the Contributor License Agreement in [CONTRIBUTING.md](CONTRIBUTING.md), which allows the project owner to relicense future versions and to offer optional premium services alongside the open client.

The **Nexora name and logo** are not covered by the code license — see [TRADEMARK.md](TRADEMARK.md).
