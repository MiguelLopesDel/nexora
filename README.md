# Nexora

**A featherweight AI overlay assistant for Linux.** Press a key anywhere, ask anything — about your screen, a term you just read, an error you're staring at — and get an answer streamed on top of whatever you're doing. Wayland-first, works on X11.

Think "Jarvis on your desktop", except it's a single small native binary, fully open source, and you bring your own API key (or local model).

## Features

- **Global keybind overlay** — bind `nexora toggle` to any key in your compositor; a prompt window appears over your work, streams the answer, and hides on Esc. The first invocation stays resident, so it's instant.
- **Conversation history** — the overlay keeps the whole conversation, so follow-up questions have context. `Ctrl+N` starts a fresh chat. Conversations are saved to `~/.local/share/nexora/history/` and the last one is restored on launch. Screenshots are never written to history and never resent on follow-ups (they're the token-heavy part).
- **Explain my screen** — `nexora run explain-screen` grabs a screenshot through the XDG desktop portal and either sends it directly to a multimodal model or converts it to OCR text through a separate vision proxy. This lets text-only models such as DeepSeek use screen context.
- **Live meeting assistant** — click 🎙 to continuously transcribe microphone or system audio, translate speech, surface reply ideas and objection handling, build notes, optionally use periodic screenshots as context, and generate a final summary. Capture, transcription and coaching run independently, so a slow reasoning response does not pause incoming transcript updates.
- **Local Vision & OCR** — choose a curated Qwen3-VL, MiniCPM-V or Moondream model in Settings, download it through Ollama with progress, and use it only for private screen description/OCR before the main model request.
- **Configurable assistant profiles** — choose a built-in interview, sales, study, presentation, or programming coach, or create a named prompt profile in the settings panel.
- **Any provider** — Anthropic natively, plus every OpenAI-compatible API: OpenAI, OpenRouter, DeepSeek, Gemini (compat endpoint), Ollama / llama.cpp running locally. Refresh each provider's live `/models` catalog from the UI, choose a model, and configure thinking/reasoning controls.
- **In-app settings** — a left-hand menu separates general behavior, visual provider cards, meeting controls, Vision & OCR, profiles, and shortcuts/privacy. Selecting a provider reveals its protocol, endpoint, environment variable, API key, and default model. Secrets remain in `config.toml` with mode `0600`.
- **Hidden mode** — where the compositor supports it, the Nexora window is excluded from screen capture: invisible to recordings, streams and screen shares. See the support matrix below — Nexora tells you the truth about what your compositor can do instead of pretending.
- **Ridiculously light** — native GTK4 in Rust. One binary, no Electron, no web view, no background CPU. It runs on a potato.

## Support matrix

| Environment | Overlay | Screenshot | Hidden (anti-capture) |
|---|---|---|---|
| Hyprland | layer-shell | portal | **automatic** (`layerrule no_screen_share`) |
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

Privilege and timing don't help because the screen recorder never reads *your* process; it reads the compositor's (or X server's) output. That's deliberate: if any app could make itself invisible to a screen share, malware would abuse it. This is exactly why only compositors with a native opt-out (niri; Hyprland via a layer/window rule) can do it — and why the paid tools that hide on Windows/macOS rely on OS-level client APIs (`SetWindowDisplayAffinity`, `NSWindow.sharingType`) that simply have no equivalent on Linux.

On Hyprland the rule keyword and syntax changed across versions. Nexora tries current `no_screen_share` and legacy `noscreenshare` layer rules automatically. If hiding does not take effect, run `nexora hidden status`.

## Installing

### Dependencies

- GTK 4 ≥ 4.10, gtk4-layer-shell, `xdg-desktop-portal`, and `parec` (PulseAudio utilities; it also works with PipeWire's Pulse compatibility layer).
- Optional: [Ollama](https://ollama.com/) for downloadable local Vision & OCR models.

```bash
# Debian / Ubuntu
sudo apt install libgtk-4-dev libgtk4-layer-shell-dev pulseaudio-utils

# Fedora
sudo dnf install gtk4-devel gtk4-layer-shell-devel pulseaudio-utils

# Arch
sudo pacman -S gtk4 gtk4-layer-shell libpulse
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

### Live meetings

Open ⚙ and configure the **Live meeting assistant** section before pressing 🎙. Choose **System audio**, **Microphone**, mix both, or paste a Pulse/PipeWire source name under **Custom device**. Transcription requires an OpenAI-compatible provider whose API implements `/audio/transcriptions`; coaching and translation use the configured analysis task. The default two-second audio windows are uploaded continuously in a task separate from coaching. A configurable local silence gate avoids uploading quiet windows, and queued transcripts are combined so suggestions stay near the live conversation instead of replaying stale chunks.

Recording, translation, screen context, notes, summaries, and on-disk storage are independently configurable. Saved sessions go to `~/.local/share/nexora/sessions/`. Obtain consent before recording other people and review your provider's data-retention policy. Automatic screen context may capture sensitive information; it is off by default.

### Local Vision & OCR

Open **Settings → Vision & OCR**, choose **Vision/OCR proxy**, select the Ollama provider and a curated model, then press **Download selected model**. `qwen3-vl:4b` is the recommended balance; 2B is faster on modest hardware, while 8B improves small-text OCR. Nexora uses Ollama's local `/api/tags`, `/api/pull`, and `/api/delete` endpoints, and sends images through its OpenAI-compatible vision endpoint.

In proxy mode the local model receives the screenshot and returns only a compact description plus OCR. The main task model receives that text—not the image. Direct mode remains available for Claude, Gemini, OpenAI, local multimodal models, or any other provider that accepts images. The capture interval, provider, model, Ollama URL, and vision prompt are all configurable in the interface.

### Model capabilities

The provider layer is capability-aware rather than a single lowest-common-denominator prompt call. The settings panel can query each provider's live `/models` endpoint and configure thinking plus reasoning effort. DeepSeek defaults to V4 Flash with thinking enabled; its request adapter sends the provider-specific `thinking` and `reasoning_effort` fields. Anthropic maps the same controls to adaptive thinking and output effort.

Thinking is not token-neutral: internal reasoning is included in output usage even when it is hidden or summarized. The UI labels lower/higher-use effort levels, warns when a choice increases latency, and only shows DeepSeek's effective `high` and `max` levels (`low`/`medium` are mapped to `high` by that API). The meeting page also marks per-chunk translation, image context, coaching, and summary calls so expensive combinations are visible before a session starts.

Nexora deliberately does not run a general tool-using agent loop for every live transcript chunk: extra model/tool turns would make meeting suggestions slower and more expensive. Agentic tools should remain opt-in for tasks that benefit from external retrieval or multi-step work, while transcription, translation, and live coaching keep the direct low-latency path.

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

The Wayland overlay uses on-demand keyboard focus. While Nexora is open, click another window to type there without closing the assistant. `Esc` hides Nexora; press the compositor shortcut bound to `nexora toggle` to show it again. Hyprland processes its own window-management binds before applications, so those binds can still affect the focused normal window underneath a layer-shell overlay. If that is undesirable, choose **General → Window mode → Normal window** and restart Nexora.

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
- [x] Live audio transcription, translation, coaching, notes, and summaries
- [x] Local Vision/OCR proxy with curated Ollama downloads
- [ ] Local whisper.cpp transcription backend
- [ ] GlobalShortcuts portal support (keybinds without touching compositor config)
- [x] Opt-in periodic screen understanding with configurable capture interval
- [ ] Prebuilt packages (.deb / .rpm / AUR)
- [ ] Markdown rendering in responses

Contributions welcome — see [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Nexora is licensed under the [GNU AGPL-3.0](LICENSE): you can use, study, modify and redistribute it, but derivative works must stay open source under the same terms, including when offered as a network service — and attribution must be preserved.

Contributions are accepted under the Contributor License Agreement in [CONTRIBUTING.md](CONTRIBUTING.md), which allows the project owner to relicense future versions and to offer optional premium services alongside the open client.

The **Nexora name and logo** are not covered by the code license — see [TRADEMARK.md](TRADEMARK.md).
