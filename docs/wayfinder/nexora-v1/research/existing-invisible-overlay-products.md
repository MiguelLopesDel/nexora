# Existing “Invisible Overlay” Products on Linux Wayland

Research snapshot: 2026-07-15. Open-source observations are pinned to Cheating
Daddy `998a056d` and Specter AI `b3de47c5` so that later changes do not silently
alter the evidence.

## Executive conclusion

No product or client-side framework examined provides a verified Linux Wayland
mechanism that leaves an overlay visible on the physical Hyprland or niri output
while a full-monitor share contains freshly recomposed pixels from behind that
overlay.

The products divide into four groups:

1. Commercial products that advertise invisibility but ship only on Windows and/or
   macOS.
2. Cross-platform Electron or Tauri products that call a content-protection API
   which their own framework documents as unsupported on Linux.
3. Overlays that are absent only when the user shares one application window, a
   different monitor, or a different workspace. This is source selection, not
   full-monitor capture exclusion.
4. Hyprland and niri compositor rules that deliberately replace protected content
   with solid black rectangles.

On Windows 10 2004 and later, `WDA_EXCLUDEFROMCAPTURE` is the one documented
mainstream mechanism in this survey whose result is removal of the window from the
capture. Older Windows versions produce black instead. Electron's equivalent on
macOS is no longer reliable against ScreenCaptureKit. Neither API exists in
Electron or Tauri/TAO on Linux.

Therefore these products do not change the architectural conclusion in
[Safe Share on Hyprland and niri](safe-share-wayland.md): clean omission on the
same full-monitor stream requires a compositor-side capture render that removes
Nexora before composition.

## Outcome vocabulary

Marketing pages use “invisible,” “undetectable,” “transparent,” and “stealth” as
if they were interchangeable. They are not. This report classifies only outcomes
supported by product code or platform documentation:

| Outcome | What the remote viewer receives | Meets Nexora's clean full-monitor invariant? |
| --- | --- | --- |
| Clean omission / recomposition | The overlay is absent and the captured scene beneath it is rendered | **Yes**, if guaranteed for every frame and all Nexora surfaces/effects |
| Black redaction | A black rectangle covers the protected bounds | No |
| Transparent/empty source | Alpha or an empty stream, not the physical scene underneath the overlay | No |
| Application-window selection | Only the chosen application's surface is captured; unrelated overlay windows are outside the source | No for full-monitor sharing; useful fallback |
| Separate monitor/workspace | The overlay is placed outside the selected source | No for “same current monitor scene”; useful operational fallback |
| Visible/no-op | The full-monitor capture includes the overlay | No |
| Platform-only | A native mechanism may work on Windows or macOS, but the product has no Linux implementation | No on Hyprland/niri |

Opacity, click-through input, always-on-top state, hiding a Dock/taskbar icon, a
generic “tool” window, and process-name disguise are not capture-exclusion
evidence.

## Commercial product claims

The following are first-party product statements. They establish what the vendor
ships or claims, not an independent pixel-level verification.

| Product | First-party evidence | Verified mechanism or boundary | Result relevant to Linux Wayland |
| --- | --- | --- | --- |
| CoPilot Interview | The product calls Ghost Mode private during sharing, but its FAQ says Linux is not available and lists Windows 10/11 and macOS 12+ | The page describes a separate desktop window that stays outside a browser capture rectangle, without documenting full-monitor Linux composition | **Platform-only** and potentially **application-window selection**; no Linux build |
| MeetAssist | Its landing page says the desktop app is invisible to sharing and recording “On Windows and macOS”; it explicitly labels the Chrome extension visible on screen share | Desktop/native claim is limited to Windows/macOS; browser extension is not excluded | **Platform-only**; the extension is **visible** |
| PhantomDesk | The product page says its invisibility relies on Windows-specific Win32 APIs with no macOS or Linux equivalent | Windows `WDA_EXCLUDEFROMCAPTURE` | **Platform-only**; this is unusually precise and does not overclaim Linux |
| Korvat | The official page claims `Qt WindowType.Tool` makes the app invisible to sharing, OBS, and screenshots, and offers only a Windows download | Qt documents `Tool` as a tool-window type, not capture exclusion; the vendor supplies no Linux build | **Platform-only**; its stated Qt rationale is not evidence of capture exclusion |
| Cue | The project page advertises Linux AppImage/deb artifacts, but its technical description says `setContentProtection(true)` hides capture only “on Mac and Windows” | Electron content protection; no Linux alternative is described | Linux artifact may run, but full-monitor invisibility is **unsupported/no-op** |

Primary product sources:

- [CoPilot Interview landing page and FAQ](https://copilotinterview.com/) — Windows
  and macOS downloads, “Linux is not yet available,” and the vendor's description
  of Ghost Mode.
- [MeetAssist](https://meetassist.io/) — desktop claim limited to Windows/macOS and
  a warning that the Chrome extension is visible.
- [PhantomDesk](https://phantomdesk.dev/) — Windows-only Win32 mechanism and explicit
  statement that there is no macOS/Linux equivalent.
- [Korvat](https://korvatlabs.com/) — Windows download and the `Qt WindowType.Tool`
  claim.
- [Cue](https://cue.valthrax.com/) — claimed Linux packages, but capture protection
  explicitly scoped to Mac and Windows.

This pattern also appears across other meeting/interview copilots: a broad
“invisible on screen share” headline is usually paired with only Windows/macOS
downloads. A platform list is stronger evidence than the headline when assessing
Hyprland or niri.

## Open-source implementations

Source code provides a more useful test than screenshots or marketing copy. The
following projects have Linux-related claims or builds and expose their window
creation code.

### Cheating Daddy: unconditional Electron call

Cheating Daddy describes its Linux support as “kinda, dont use, just for testing
rn.” Its window setup unconditionally calls `setContentProtection(true)`, with no
Linux compositor integration or alternative exclusion path.

- [Pinned platform note](https://github.com/sohzm/cheating-daddy/blob/998a056d0270bb2097c9e10acc85ed5335965df2/README.md#L20-L27)
- [Pinned window setup](https://github.com/sohzm/cheating-daddy/blob/998a056d0270bb2097c9e10acc85ed5335965df2/src/utils/window.js#L31-L49)

Electron documents this API only for macOS and Windows. Therefore the Linux call
does not establish protection.

**Linux outcome:** **visible/no-op** for full-monitor capture.

### Specter AI: explicit Windows implementation and Linux disclaimer

Specter AI is the clearest negative control. Its overlay source comments state that
Linux has no reliable capture-exclusion API, and its README calls Wayland exclusion
unreliable. On Windows it directly invokes `SetWindowDisplayAffinity` with
`WDA_EXCLUDEFROMCAPTURE`.

The project also records an important failure mode: Electron content protection on
its transparent/layered Windows window produced a **black rectangle** in captures,
so the project uses native FFI and prefers visibility over falling back to black.

- [Pinned platform implementation summary](https://github.com/umairinayat/Specter-AI/blob/b3de47c562b34a20bf5aa76215e5ce174ab82696/src/main/overlay-window.ts#L1-L10)
- [Pinned black-rectangle warning and Windows-only guard](https://github.com/umairinayat/Specter-AI/blob/b3de47c562b34a20bf5aa76215e5ce174ab82696/src/main/capture-protection.ts#L1-L15)
- [Pinned Linux limitation](https://github.com/umairinayat/Specter-AI/blob/b3de47c562b34a20bf5aa76215e5ce174ab82696/README.md#L219-L231)

**Linux outcome:** **visible/no-op**; no claimed clean Wayland path. **Windows
fallback outcome:** black with the problematic layered-window path, clean removal
claimed only after the native Windows call succeeds.

### What these implementations demonstrate

The reviewed projects use different product language but converge on the same code
boundary:

| Implementation | Linux artifact? | Actual exclusion primitive | Linux full-monitor result |
| --- | --- | --- | --- |
| Cheating Daddy | Experimental | Electron `setContentProtection` | Unsupported; visible/no-op |
| Specter AI | Yes | Windows FFI only | Explicitly no reliable Linux API |

None contains a Hyprland or niri capture-render integration. A Linux installer is
not evidence that the Windows/macOS stealth mechanism was ported.

## Framework and protocol audit

### Electron

Electron labels `BrowserWindow.setContentProtection` as **macOS Windows**, not
Linux. Its documentation gives the observable results:

- Windows 10 2004+: Electron calls `SetWindowDisplayAffinity` with
  `WDA_EXCLUDEFROMCAPTURE`, and the window is removed from capture entirely.
- Older Windows: the behavior falls back to `WDA_MONITOR`, producing a black
  captured window.
- macOS: Electron sets `NSWindow.sharingType` to `NSWindowSharingNone`, but warns
  that newer apps using ScreenCaptureKit can capture the window anyway.

See the official
[`BrowserWindow.setContentProtection`](https://www.electronjs.org/docs/latest/api/browser-window#winsetcontentprotectionenable-macos-windows)
documentation.

**Linux outcome:** no API. Always-on-top, transparency, click-through input, and
`setVisibleOnAllWorkspaces` affect presentation/input only.

### Tauri and TAO

Tauri exposes `content_protected`, but the windowing layer implementing it is TAO.
TAO explicitly documents `with_content_protection` as unsupported on Linux. See
the official
[`tao::window::WindowBuilder`](https://docs.rs/tao/latest/tao/window/struct.WindowBuilder.html#method.with_content_protection)
documentation and its
[source annotation](https://docs.rs/tao/latest/src/tao/window.rs.html#573-581).

**Linux outcome:** no-op/unsupported, not a compositor request.

### Qt

Qt documents `Qt::Tool` as a small tool window, `WindowStaysOnTopHint` as a stacking
hint, and `WindowTransparentForInput` as passing input events through the window.
It does not define any of them as capture exclusion. See the official
[`Qt::WindowType` descriptions](https://doc.qt.io/qt-6/qt.html#WindowType-enum).

This directly weakens claims such as Korvat's that `Qt WindowType.Tool` alone is
“completely invisible” to OBS, screenshots, and screen sharing. At most, a tool
window may be omitted when a meeting application captures a different selected
window. It remains part of a compositor's full-output scene.

**Linux outcome:** no capture-exclusion API established. **Possible observed
outcome:** application-window selection only.

### GTK4 and layer-shell

GTK4 creates ordinary Wayland surfaces. `gtk4-layer-shell` lets an application put
a surface in the background, bottom, top, or overlay layer and configure anchors,
exclusive zones, margins, monitor, namespace, and keyboard interactivity. The
underlying `wlr-layer-shell` protocol says `set_layer` changes the layer in which a
surface is rendered; it defines no capture-exclusion request.

- [`gtk4-layer-shell` project and API purpose](https://github.com/wmww/gtk4-layer-shell)
- [Upstream `wlr-layer-shell` protocol](https://gitlab.freedesktop.org/wlroots/wlr-protocols/-/blob/master/unstable/wlr-layer-shell-unstable-v1.xml)
- [GTK4 Wayland backend APIs](https://docs.gtk.org/gdk4/wayland.html)

Putting Nexora in the overlay layer makes it reliably topmost on compatible
compositors. It does not make it absent from monitor capture.

**Linux outcome:** visible in an ordinary monitor stream unless the compositor
applies its own capture policy.

## Linux compositor techniques

### Hyprland `noscreenshare`: black, not background

Hyprland's official window-rule documentation says `noscreenshare` hides windows
and layer surfaces by drawing black rectangles in their place. For windows, the
rectangle is drawn even if other windows are above it. See
[`noscreenshare`](https://wiki.hypr.land/0.52.0/Configuring/Window-Rules/#dynamic-effects).

**Outcome:** **black redaction**. It does not re-render the scene without the
protected surface.

### niri `block-out-from`: black, not background

niri's official rule documentation says `block-out-from "screencast"` replaces
matched windows with solid black rectangles; the same policy can apply to
layer-shell notifications. Its warning also distinguishes portal screencasts from
third-party screenshot tools. See
[`block-out-from`](https://github.com/YaLTeR/niri/wiki/Configuration:-Window-Rules#block-out-from).

**Outcome:** **black redaction**.

### Hardware overlay planes and direct scanout

DRM/KMS has primary, cursor, and overlay planes, and some hardware can scan a plane
out without first flattening it into the primary framebuffer. That fact is not a
client API for “visible locally, excluded remotely”:

- The Linux kernel documents planes as KMS resources configured by DRM userspace,
  with hardware-specific format, scaling, placement, and sharing restrictions.
- Wayland clients submit buffers to the compositor; the compositor decides whether
  to use those buffers as textures, direct scanout, subsurfaces, or hardware planes.
- niri exposes `enable-overlay-planes` only as an experimental debug option and
  says it may cause frame drops. This demonstrates compositor ownership rather than
  a per-client guarantee.

See the kernel's
[KMS plane model](https://docs.kernel.org/gpu/drm-kms.html#standard-plane-properties),
Wayland's
[compositor/direct-scanout description](https://wayland.freedesktop.org/docs/book/Compositors.html),
and niri's
[`enable-overlay-planes` debug option](https://github.com/YaLTeR/niri/wiki/Configuration:-Debug-Options#enable-overlay-planes).

Even if a particular capture implementation accidentally omits a hardware plane,
that behavior can change when composition, scaling, transforms, effects, GPU
driver state, or capture starts. A normal GTK/Electron/Qt Wayland client cannot
reserve such a plane or make exclusion fail closed.

**Outcome:** not a supported product primitive and not a guarantee.

### Separate output, virtual monitor, or workspace

These are useful source-separation patterns, not clean copies of the physical
monitor:

- The ScreenCast portal defines `VIRTUAL` as extending the desktop with a new
  virtual monitor, not as a filtered clone. See the official
  [ScreenCast source types](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html#org.freedesktop.portal.ScreenCast.AvailableSourceTypes).
- Hyprland can create a headless output, but the official command describes a new
  fake output for VNC/RDP/Sunshine use. See
  [`hyprctl output create headless`](https://wiki.hypr.land/Configuring/Advanced-and-Cool/Using-hyprctl/#output).
- niri's dynamic cast target can switch between one selected window or one monitor;
  when cleared it is an empty transparent stream. It is not a second filtered copy
  of a currently visible output. See niri's official
  [Screencasting page](https://github.com/YaLTeR/niri/wiki/Screencasting#dynamic-screencast-target).

An overlay can be placed on an unshared monitor/workspace, but then the user is not
seeing it over the exact scene being shared. Mirroring an already-composed output
also mirrors the overlay or its black redaction.

**Outcome:** **separate source** or **transparent/empty source**, not recomposition.

### Portal backends and PipeWire

The XDG ScreenCast portal authorizes `MONITOR`, `WINDOW`, and `VIRTUAL` sources and
returns PipeWire node IDs. Portal backends are selected per interface via
`portals.conf`. Neither contract lets an ordinary overlay ask the compositor to
remove one surface from a monitor render.

- [ScreenCast portal interface](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html)
- [`portals.conf` backend selection](https://flatpak.github.io/xdg-desktop-portal/docs/portals.conf.html)

A custom backend can transport a clean PipeWire stream only after some compositor
API has produced the clean frames. It cannot recover occluded live pixels from a
flattened monitor frame.

## What is genuinely usable without compositor changes

For an unmodified Hyprland or niri installation, the defensible fallbacks are:

1. Ask the user to share one application window. A separate Nexora toplevel or
   layer surface is then outside the selected source. This must be labeled
   **window-share isolation**, not invisible full-screen mode.
2. Put Nexora on an unshared physical/headless output. This must be labeled
   **separate-display mode** and must not claim to overlay the shared scene.
3. Use the compositor's black-out rule for privacy when black redaction is
   acceptable. This must preview the actual black result.
4. Hide Nexora before capture. This is deterministic but removes the live on-screen
   assistance the feature is meant to preserve.

None is equivalent to “the user sees Nexora over the physical monitor while remote
viewers see the same monitor with live background pixels restored.”

## Product implications for Nexora

- Do not present Electron/Tauri `contentProtection`, Qt `Tool`, transparency,
  click-through, layer-shell overlay placement, or hardware overlay planes as a
  Linux Safe Share implementation.
- Treat all third-party “invisible” claims as unverified until there is a Linux
  build, a named compositor/capture path, source code or protocol evidence, and a
  frame-level result showing the live background rather than black/transparency.
- Detect Hyprland/niri capabilities at runtime. If only their stock redaction rule
  is available, say “black redaction,” not “invisible.”
- Keep application-window sharing and a separate monitor as explicit fallbacks.
- Preserve delayed assistant responses in conversation history if desired, but do
  not let persistence imply that a late response can safely be shown over an
  active full-monitor share. Display policy and history retention are separate.
- For the clean invariant, continue with the version-pinned compositor capture
  adapters proposed in [Safe Share on Hyprland and niri](safe-share-wayland.md).

## Evidence standard for future candidates

A new product should count as a Linux precedent only if it supplies all of the
following:

1. A Linux Wayland build tested on a named Hyprland/niri version.
2. The exact capture source: portal monitor, portal window, direct screencopy, OBS
   source, or recorder API.
3. The exact exclusion mechanism in protocol or source code.
4. A test with an animated surface behind an opaque overlay, proving that captured
   pixels are current rather than black, transparent, stale, or inferred.
5. Coverage for child surfaces, popups, shadows, blur, animations, cursor, and
   damage updates.
6. Fail-closed behavior when the compositor/backend version is incompatible.

No surveyed product met this standard on Linux Wayland.
