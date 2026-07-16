---
title: Nexora v1 product and architecture
label: wayfinder:map
tracker: local-markdown
status: open
---

## Destination

A decision-complete Nexora v1 product and architecture specification, ready to hand to implementation planning, covering the live meeting experience, configurable AI pipeline, privacy, cost controls, local models, and discreet Wayland screen sharing.

## Notes

- This map plans the product; implementation is outside this effort.
- Follow the repository guidelines supplied for `/app`, especially opt-in privacy/cost behavior and truthful compositor support claims.
- Treat [Accepted product constraints](intake-decisions.md) as fixed input from the charting conversation.
- Research must prefer compositor source, protocol specifications, official provider documentation, and first-party runtime documentation.
- Open child tickets live in [`tickets/`](tickets/). Local Markdown front matter records labels, ownership, and fallback dependency relationships.
- 2026-07-16: the Safe Share prototyping track is paused by user decision — hidden mode, anti-capture, and virtual outputs/nested runtimes need dedicated planning. See [the paused ticket](tickets/choose-safe-share-design.md) for the resume state; other feature tracks proceed.

## Decisions so far

<!-- Closed ticket decisions are indexed here, one linked gist per ticket. -->

- [Establish the provider capability and cost contract](tickets/provider-capability-contract.md) — Use provenance-aware, tri-state model capabilities plus normalized usage and versioned, meter-specific price schedules while preserving raw provider data.
- [Establish the local transcription, diarization, vision, and OCR stack](tickets/local-ai-runtime-stack.md) — Base the candidate stack on PipeWire, Silero VAD, whisper.cpp, asynchronous sherpa-onnx diarization, Ollama vision/translation, and dedicated OCR, subject to hardware-tier benchmarks.
- [Establish the Safe Share architecture available on Hyprland and niri](tickets/safe-share-wayland-architecture.md) — Clean pixels require compositor-side omission before flattening; existing exclusion rules, portals, PipeWire, and virtual outputs cannot reconstruct already occluded content.
- [Define the live session and privacy domain model](tickets/session-privacy-domain-model.md) — Use an explicit recoverable lifecycle with separate activation and disclosure grants, artifact-specific retention, isolated degradation, fail-closed sharing, and two-phase finalization.

## Not yet specified

- Packaging, updates, sandbox boundaries, and installation UX after the Safe Share and local-runtime constraints are known.
- The curated local model catalog and hardware-tier recommendations after runtime benchmarking candidates are known.
- Detailed provider adapters, model-refresh policy, and unknown-model capability probing after the capability survey.
- Accessibility, localization, onboarding, and failure-recovery details after the primary UI workflow is prototyped.
- Quantitative latency, quality, resource, privacy, and compatibility release gates after the architecture choices settle.
- Export formats and optional third-party note integrations.

## Out of scope

- Implementing the v1 or decomposing the specification into build tasks.
- Guaranteeing exclusion from physical cameras or capture tools that bypass the Wayland compositor and XDG portal.
- Official v1 discreet-mode support for KDE Plasma, GNOME, or COSMIC; these remain later compatibility targets.
- Automatically speaking, typing, sending messages, or taking actions in third-party applications.
- Modifying Zoom, Google Meet, Discord, or other meeting applications.
