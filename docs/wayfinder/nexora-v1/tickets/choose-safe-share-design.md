---
title: Choose the Safe Share v1 design
label: wayfinder:prototype
parent: ../map.md
status: paused
assignee: root
blocked_by:
  - Establish the Safe Share architecture available on Hyprland and niri
context: prototype/safe-share-virtual-workspace:docs/safe-share-virtual-workspace-prototype.md
prototype_commit: 020d477
---

## Paused (2026-07-16)

The user paused this track: hidden mode, anti-capture, and virtual
outputs/nested runtimes need deeper planning than live improvisation allows.
State at pause: nested runtime (cage window share) is the leading design;
pointer forwarding and no-hotplug behavior are proven; still unverified are
the marker actually transmitted via window share, keyboard typing through the
share, OBS/browser pickers, viewer-side teardown behavior, and the niri
equivalence run. Resume from prototype commit `020d477` and the
"Full-monitor recording" section below.

## Question

Given the verified compositor mechanisms, which Safe Share workflow and technical boundary should v1 adopt, and does a rough end-to-end prototype prove that Zoom, Google Meet, Discord, OBS, and standard portal recorders receive the clean scene while the user continues seeing and interacting with Nexora?

## Accepted direction

Adopt a **Safe Share virtual workspace** rather than modifying or patching the
host compositor. Applications intended for sharing run on a clean virtual output;
Nexora remains on the user's physical output and must never create a surface on
the shared output. Window-only sharing continues to use the meeting application's
normal window picker.

This deliberately changes the full-monitor workflow: the virtual output is an
independent clean workspace, not a filtered clone of the physical monitor. Nexora
must describe that distinction in the UI and must not claim that arbitrary
physical-monitor capture is protected.

## Prototype gate

Keep this ticket open until a one-command, throwaway proof demonstrates that:

- Hyprland and niri expose the clean output through their supported portal path;
- the user can see and interact with applications on that output from the physical
  display while Nexora remains usable;
- Zoom or browser WebRTC, Discord, and OBS can select the source;
- recordings contain the clean workspace with no Nexora surfaces or black
  redaction; and
- teardown and crash recovery do not strand outputs, applications, or portal
  sessions.

## Working copy

The `/tmp` worktree used for the first prototype round did not survive a reboot
and was pruned. The two prototype files are now materialized directly in the
main checkout — `src/bin/safe_share_prototype.rs` and
`docs/safe-share-virtual-workspace-prototype.md` — gitignored so they never mix
with tracked changes. The branch `prototype/safe-share-virtual-workspace`
remains the source of truth; re-materialize with
`git show prototype/safe-share-virtual-workspace:<path> > <path>` if the
copies are lost.

## Prototype result so far

The throwaway probe compiles and reports compositor, portal, tool, and output
state with one command. On Hyprland it can create a temporary named headless
output, place a full-screen marker on an assigned workspace, hold the output for
manual portal recording, and clean up afterward. Live execution in the user's
graphical session is still required to validate the picker and recorded pixels.

The same native approach does not pass on niri: niri has no unprivileged command
for creating a headless output. A cross-compositor design therefore still needs a
dedicated nested runtime or another independently supplied virtual display.
Local preview with forwarded pointer and keyboard input also remains unproven;
plain output mirroring is view-only and does not satisfy this gate.

## Live run results (Hyprland, 2026-07-16)

Confirmed on the user's real Hyprland session:

- The probe created and configured the `NEXORA-SAFE-SHARE` headless output.
- Discord's screen-share picker listed the output; the transmission showed the
  clean marker workspace with no Nexora surfaces and no black redaction.
- After teardown the output no longer appeared when starting a new share.

Not yet confirmed: OBS and browser WebRTC selection (only Discord was tested),
and local input forwarding.

**Fail-closed finding:** a Discord stream that was already live kept
transmitting after the output was removed; only new share attempts lost access
to the source. Removing the virtual output therefore does not terminate active
consumer capture sessions. Real teardown must end or verify consumer sessions
(portal/PipeWire side) rather than rely on output removal, and the next run
must record what a live viewer displays after removal (frozen frame, black, or
session end). A second observed behavior: windows left on the shared workspace
jump to the physical monitor when the output is removed, so product teardown
must migrate or close them deliberately.

Prototype commit `9844b76` adds an optional interaction probe: wayvnc bound to
the virtual output on loopback plus a terminal on the shared workspace, with a
VNC viewer opened automatically when available. Typing into that terminal
through the preview is the pending proof of pointer/keyboard forwarding.

## Second live run (Hyprland, 2026-07-16, wayvnc probe)

- **Pointer forwarding proven.** Through the wayvnc preview the user grabbed a
  window on the shared workspace and dragged it out — it landed on the real
  desktop. The preview showed the terminal and full interfaces. Keyboard
  forwarding still needs one explicit confirmation (type into the terminal via
  the preview).
- **Boundary leakage (design-critical).** With `auto` positioning the headless
  output was adjacent to the physical monitor, so the desktop became seamless:
  the cursor wandered onto the shared output, windows could be dragged across
  in both directions, and Nexora itself could be dragged onto the shared
  output — where the active anti-capture rule made it a **black rectangle** in
  captures, exactly the redaction Safe Share forbids (and a tell that something
  hidden exists). The v1 design must (a) never let a Nexora surface reach the
  shared output, (b) prevent accidental cursor/window migration, not merely
  advise against it. Prototype now places the output at `20000x0` with no
  shared edge, which removes both leak paths at the compositor level.
- **Compositor keybinds are never forwarded.** Workspace-switch binds acted on
  the real session even while the preview was focused, which users experience
  as confusing navigation. The product preview needs its own interaction model
  and must document that compositor binds stay with the real session.
- The VNC viewer's pointer grab also confused the user (mouse would not leave
  the window; compositor focus keys did). A product-quality preview cannot be
  a raw VNC client.

## Third live run (Hyprland, 2026-07-16, isolated output)

Creating the headless output **stranded the user**: focus and cursor warped to
the invisible output, so workspace binds kept switching workspaces on a
monitor they could not see and the cursor vanished until they explicitly
switched to a workspace bound to the physical monitor. Their quickshell-based
desktop shell also disappeared when the output appeared (recovered by
restarting it after `hyprctl output remove`).

Consequences recorded:

- The probe (commit `6a45192`) now records the focused monitor before creating
  the output and focuses it back right after setup, and computes the isolated
  position from the real layout (right edge + small gap) instead of a huge
  fixed offset, keeping the layout bounding box small for shell components.
- **Design-critical:** output hotplug is a session-wide event. The v1 Safe
  Share must guarantee focus/cursor never land on the shared output, and must
  be tested against popular shells (waybar, quickshell-based shells, etc.),
  which may spawn surfaces on the new output, crash, or lose state when it
  appears and disappears.

## Fourth live run (Hyprland, 2026-07-16, focus fix confirmed)

Focus restoration worked: focus returned to the physical workspace instantly.
But the shell **still disappeared** with the minimal bounding box, leaving a
bare compositor until it was restarted manually. The geometry hypothesis is
falsified: the hotplug event itself breaks the shell. Upstream quickshell's
changelog records a workaround for a Qt bug that crashed on monitor plug and
unplug, and quickshell-based shells recreate their windows reactively when
the screen list changes; several sibling shells report similar breakage when
monitors are enabled or disabled.

Consequences:

- Shell compatibility on output hotplug (appear **and** disappear) must be an
  explicit Safe Share release gate, with a tested shell matrix and a
  documented recovery path. An updated quickshell may already include the
  hotplug fix; the user's shell version predates verification.
- This materially strengthens the **nested-runtime option**: a dedicated
  nested Wayland runtime supplies the clean scene without ever hotplugging an
  output into the host session, avoiding the shell-breakage class entirely —
  and it is already the only viable path on niri. Evaluate it as the primary
  cross-compositor design, with the native headless output as a
  Hyprland-specific fallback.

## Fifth live run and pivot to the nested runtime

A fifth run reproduced the shell breakage identically (focus stayed on the
physical monitor as designed, but the shell vanished again and had to be
restarted). Conclusion: with the user's current shell, **the headless-output
path breaks the desktop session on every activation** and cannot be the
primary design.

Prototype commit `1a174fb` therefore adds a **nested-runtime probe**, offered
first on both Hyprland and niri: a nested compositor (`cage`, falling back to
`gamescope`) runs the clean marker — now including a text field — and shows
up as a normal window on the desktop. Sharing uses the meeting application's
ordinary *window* picker; the window itself is the local preview; pointer and
keyboard input are native (no VNC, no grabs); no host output is created, so
the shell-breakage and focus-theft classes disappear, along with the
black-redaction risk (window shares capture only that window's buffer).

Workflow consequence to validate and then record in the map: with a nested
runtime, "Safe Share" is selected as a **window source**, not a monitor
source. Remaining gates for this path: window-picker selection in Discord,
OBS, and browser WebRTC; clean transmitted pixels with Nexora dragged over
the nested window; typing into the marker's text field; teardown behavior on
the viewer side when the shared window closes; and multi-application
management inside the nested scene (cage is a kiosk compositor — a product
design needs window management or one nested runtime per shared app).

## Full-monitor recording of the physical screen

Raised during live testing: with the nested-runtime design, what happens if a
recorder captures the **entire physical monitor** instead of the Safe Share
window? Answer, consistent with the accepted constraints ("capture guarantees
apply only to supported compositor/portal paths"):

- No client can clean an arbitrary physical-monitor capture on Wayland; the
  compositor composites the frame. This was equally true of the virtual-output
  variant — the physical monitor was never a protected source in any design.
  Safe Share **replaces** the full-monitor workflow: the thing you share is
  the clean scene (window source, or the Hyprland-only virtual output).
- Defense-in-depth when the physical monitor is captured anyway:
  - On Hyprland/niri the existing anti-capture rule keeps Nexora's content
    out of the frame, at the cost of possibly revealing a black region —
    content protection, not existence protection.
  - Hyprland's IPC emits a `screencast` event when capture starts/stops; the
    product can **auto-hide the overlay while a monitor capture is active**
    (sessions already continue while hidden per the domain model). Evaluate
    an equivalent signal on niri; without one, the honest
    "visible to capture" badge remains the fallback on other compositors.

**Marker bug found by the first cage run:** the marker never launched in any
earlier run — `gtk4::Application::run()` re-parses argv and aborted on the
unknown `--marker` option (silently when spawned by hyprctl, visibly inside
cage). Fixed in `020d477` by running the GTK app with an empty argument
list. Earlier "clean workspace" recordings therefore showed an empty
workspace rather than the marker; the pixel checks should be repeated with
the marker actually visible.
