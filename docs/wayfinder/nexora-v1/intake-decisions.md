# Accepted Product Constraints

These constraints were accepted while charting the Nexora v1 Wayfinder map. They are inputs to its open decisions, not implementation tasks.

## Platforms and discreet sharing

- Linux Wayland is the platform. Hyprland and niri are the official v1 compositors; KDE Plasma, GNOME, and COSMIC are later compatibility targets.
- Full-monitor sharing uses a user-selected **Nexora Safe Share** source. It contains the complete monitor scene—windows, panels, notifications, menus, and cursor—but recomposes the pixels behind and excludes every Nexora surface without leaving a black rectangle.
- Sharing a specific application uses the meeting application's normal window source; Nexora retains its own application identity and is naturally excluded.
- Capture guarantees apply only to supported compositor/portal paths.

## Live session pipeline

- Capture microphone and call audio separately. Attribute the local speaker and diarize remote participants; cross-session voice recognition is optional, local, encrypted, and opt-in.
- Local transcription is the default. Remote transcription is opt-in and clearly identifies audio transfer and cost. Original text and live translations coexist, and the target language can change without losing history.
- Suggestions can be triggered automatically by relevant conversation changes or manually by a configurable shortcut. Stale responses move to history rather than replacing current guidance.
- Suggestions and objection handling never act in another application without explicit user action.
- Incremental notes and summaries track decisions, actions, owners, questions, and next steps, then consolidate at session end.

## Screen context and AI orchestration

- OCR and vision are local by default. Raw-image transfer is opt-in and available only for a compatible model.
- Optional **Capture screen on submit** takes a fresh clean frame when Enter submits Nexora's focused input. A separate configurable global shortcut provides capture-and-ask outside that input.
- Model capability—not provider name—controls image, audio, reasoning, reasoning-effort, tool, streaming, and structured-output behavior. Text-only models such as the current DeepSeek API receive a local or separately configured vision model's description.
- A bounded task orchestrator independently configures transcription, translation, OCR/vision, live suggestions, objection handling, and summaries. Reasoning effort is task-specific and exposes latency/cost impact.
- Assistant profiles store the complete session configuration, not only prompts.

## Privacy, cost, and interaction

- Sessions are ephemeral by default. Optional saved sessions are locally encrypted, with separate retention for text, audio, and captures.
- Starting capture is manual by default; a profile may explicitly opt into automatic start. Hiding the interface does not stop an active session.
- Capture indicators appear only inside Nexora and are excluded from Safe Share.
- The resident overlay has pass-through and interactive states. A global shortcut enters interaction; Escape hides or returns to pass-through without terminating the process.
- Secrets use KWallet or Secret Service, with environment variables as a fallback, never plain `config.toml` values.
- Cost estimates distinguish text, image, audio, and reasoning. Profiles can enforce session/month warnings and hard limits. Cross-provider fallback runs only through an explicitly authorized chain.

## Local runtimes and UI

- Ollama, a dedicated Whisper runtime, and dedicated OCR are acceptable. The GTK interface manages compatible downloads, updates, storage, and removal.
- Compatible GPU acceleration is automatic by default, with force-CPU controls. NVIDIA, AMD, and Intel are official acceleration families, subject to a documented runtime matrix.
- Navigation uses a left sidebar. Provider cards use recognizable artwork and reveal provider-specific credentials, models, and configuration below the selected card.
- Model selection combines a maintained catalog, provider discovery, and custom IDs with conservative capabilities until verified.
