# Safe Share on Hyprland and niri

Research snapshot: 2026-07-15. Upstream source observations are pinned to Hyprland
`d7fc7240`, xdg-desktop-portal-hyprland `0e832b50`, niri `0777769e`, and OBS Studio
`9d38a938` so that later upstream changes do not silently change the basis of this
decision.

## Question and required invariant

Safe Share needs a user-selectable full-monitor stream that shows the same live
desktop scene as the physical monitor, except that every Nexora-owned surface and
its visual effects are absent. Pixels covered by Nexora must be freshly recomposed
from the surfaces below, not replaced by black, transparency, a stale screenshot,
or guessed pixels.

The useful definition is:

> For every captured frame, render the selected output's scene at that frame's
> presentation state with the set of Nexora surface trees removed before
> composition.

This is stronger than “anti-capture.” It requires the compositor's scene graph and
the underlying client buffers. Once a compositor has flattened the scene into an
output texture, no portal, PipeWire node, or post-processing filter can recover the
live pixels hidden by an opaque overlay.

## Conclusion

There is **no unmodified, common Hyprland/niri mechanism today that provides this
invariant**.

- Hyprland's `noscreenshare` rule explicitly draws black rectangles. Its current
  monitor capture path first draws the already-composited mirror texture and then
  adds those black rectangles.
- niri's `block-out-from "screencast"` also explicitly replaces matched windows and
  layers with solid black buffers.
- XDG Desktop Portal selects and authorizes sources, then returns PipeWire streams.
  It neither owns the compositor scene graph nor specifies how an output is
  composited.
- A headless/virtual output is another desktop output, not a live clean clone of a
  physical output. Hyprland can create one, but has no native filtered scene-clone
  primitive; niri does not have built-in output mirroring. The portal's `VIRTUAL`
  type means “extend with new virtual monitor,” not “publish an arbitrary sanitized
  monitor feed.”

The recommended v1 is therefore a **version-pinned compositor capability**, with a
small adapter for each compositor, that adds a distinct “omit from capture” render
semantic. While Safe Share is explicitly enabled, the ordinary physical-monitor
portal source is rendered through that semantic. Existing Zoom, browser/WebRTC,
Discord/Electron, OBS, and recorder integrations can keep selecting a normal
`MONITOR`; no custom source type or virtual camera is required.

For a product that must honestly say “guaranteed on Hyprland and niri,” Nexora must
ship and detect compatible patched compositor builds (or get equivalent patches
upstream). A Hyprland plugin can prototype the render hook, but its upstream plugin
guidance warns that internal methods can change without notice and that function
hooks are the easiest mechanism to break. It is not a sufficient guarantee by
itself. niri's required change is in the compositor render path, so a matched niri
build is needed as well.

## What exists now

### XDG Desktop Portal and PipeWire

The ScreenCast portal advertises three source-type bits: existing `MONITOR`,
application `WINDOW`, and `VIRTUAL`, where `VIRTUAL` is specifically documented as
“Extend with new virtual monitor.” `SelectSources` accepts the requested bitmask,
and `Start` returns one or more PipeWire node identifiers plus metadata such as
`source_type`, monitor position and size, a stable stream ID, and (in interface
version 6) `pipewire-serial`. `OpenPipeWireRemote` exposes only the nodes authorized
for that portal session. See the official
[ScreenCast interface](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html#org.freedesktop.portal.ScreenCast).

This contract is transport and authorization, not composition. Backends are chosen
per portal interface through `portals.conf`; it does not aggregate an arbitrary
PipeWire producer into another backend's chooser. See the official
[`portals.conf` selection rules](https://flatpak.github.io/xdg-desktop-portal/docs/portals.conf.html).

The newer staging `ext-image-copy-capture-v1` protocol also leaves composition to
the compositor. Its output-source specification is helpful because it explicitly
says the capture shows the output content and that some elements may be omitted,
including overlays “marked as transparent to capturing.” It provides no request by
which an ordinary client can mark itself that way; the compositor must supply that
policy. See
[`ext_output_image_capture_source_manager_v1`](https://gitlab.freedesktop.org/wayland/wayland-protocols/-/blob/main/staging/ext-image-capture-source/ext-image-capture-source-v1.xml).
The older `wlr-screencopy` protocol merely asks the compositor to capture an output
or region and is now deprecated in favor of ext-image-copy-capture; it has no
surface-exclusion parameter. See the
[`zwlr_screencopy_manager_v1` protocol](https://gitlab.freedesktop.org/wlroots/wlr-protocols/-/blob/master/unstable/wlr-screencopy-unstable-v1.xml).

Consequences:

1. PipeWire can carry a clean frame but cannot create one from a flattened dirty
   frame.
2. A custom PipeWire node is not automatically a selectable portal monitor.
3. A custom portal backend could label and transport a Safe Share stream, but it
   would still need a compositor API that produces the clean render. Replacing the
   ScreenCast backend would also make that backend responsible for ordinary monitor
   and window sharing, persistence, cursor modes, and permission UI.

### Hyprland

Hyprland documents both window and layer `noscreenshare` as drawing black
rectangles. For windows, the rectangle is drawn even when another window is above
it. See the official
[window and layer rule table](https://wiki.hypr.land/0.52.0/Configuring/Window-Rules/#dynamic-effects).

The implementation matches the documentation. In
[`CScreenshareFrame::renderMonitor`](https://github.com/hyprwm/Hyprland/blob/d7fc7240f4efd0abac1c1f23f09b78b30b4e0782/src/managers/screenshare/ScreenshareFrame.cpp#L172-L279),
Hyprland obtains the monitor's mirror texture, draws that texture into the capture
target, and then enters a section explicitly labeled “render black boxes for
noscreenshare.” At that point the overlay has already been flattened into the
texture, so skipping the later rectangle cannot reveal the live background.

xdg-desktop-portal-hyprland currently implements monitor capture by repeatedly
issuing `zwlr_screencopy_manager_v1.capture_output`; window capture uses Hyprland's
toplevel-export protocol. See
[`SSession::startCopy`](https://github.com/hyprwm/xdg-desktop-portal-hyprland/blob/0e832b50ecc49d4bae01a29845c1b3fafc5c5c99/src/portals/Screencopy.cpp#L339-L374).
The picker currently parses screen, window, and region selections, but not a
workspace or a sanitized virtual source, even though the backend advertises the
`VIRTUAL` source bit. See
[`promptForScreencopySelection`](https://github.com/hyprwm/xdg-desktop-portal-hyprland/blob/0e832b50ecc49d4bae01a29845c1b3fafc5c5c99/src/shared/ScreencopyShared.cpp#L41-L131)
and the
[`AvailableSourceTypes` registration](https://github.com/hyprwm/xdg-desktop-portal-hyprland/blob/0e832b50ecc49d4bae01a29845c1b3fafc5c5c99/src/portals/Screencopy.cpp#L644-L665).
Advertising `VIRTUAL` is therefore not evidence of a usable Safe Share source.

Hyprland can create a fake headless output with `hyprctl output create headless`.
The official documentation describes it as an output for VNC/RDP/Sunshine use, not
as a clone or filtered view of another output. See
[`hyprctl output`](https://wiki.hypr.land/Configuring/Advanced-and-Cool/Using-hyprctl/#output).
An independent headless workspace does not satisfy “the entire current scene,” and
mirroring a final physical-output texture would preserve the Nexora pixels (or the
black redaction).

Hyprland does offer plugins, but its own
[plugin guidelines](https://wiki.hypr.land/Plugins/Development/Plugin-Guidelines/#usage-of-the-api)
say that internal methods may change without notice and function hooks should be a
last resort because they are easiest to break. A plugin is appropriate for proving
the render strategy, provided it is pinned to the exact Hyprland commit. A supported
v1 guarantee should use an upstream/core API or a packaged core patch instead of an
unbounded hook into `renderMonitor`.

### niri

niri's official screencasting documentation says monitor and window casting use
PipeWire and `xdg-desktop-portal-gnome`, while `wlr-screencopy` is an alternative.
Its documented block-out behavior is replacement by solid black rectangles. See
the official [Screencasting page](https://github.com/niri-wm/niri/wiki/Screencasting)
and
[`block-out-from`](https://github.com/niri-wm/niri/wiki/Configuration:-Window-Rules#block-out-from).

niri already has the render architecture closest to what Safe Share needs. It has
separate `Output`, `Screencast`, and `ScreenCapture` render targets; the PipeWire
path renders screencast streams separately during output redraw. See
[`RenderTarget`](https://github.com/niri-wm/niri/blob/0777769e719b7c9b7c980d4ea66288bfbb4da5b3/src/render_helpers/mod.rs#L87-L127)
and the
[PipeWire render call](https://github.com/niri-wm/niri/blob/0777769e719b7c9b7c980d4ea66288bfbb4da5b3/src/niri.rs#L4700-L4715).
However, matched windows currently emit a `SolidColorRenderElement`, and matched
layers emit their solid-color block-out buffer, rather than omitting the element.
See
[`Mapped::render_normal`](https://github.com/niri-wm/niri/blob/0777769e719b7c9b7c980d4ea66288bfbb4da5b3/src/window/mapped.rs#L644-L675)
and
[`MappedLayer::render`](https://github.com/niri-wm/niri/blob/0777769e719b7c9b7c980d4ea66288bfbb4da5b3/src/layer/mapped.rs#L195-L230).
Because the screencast render list is independent, a new omit semantic can skip the
surface tree, popups, shadow, blur, and other effects before flattening and naturally
expose elements already present below it.

niri implements the private `org.gnome.Mutter.ScreenCast` D-Bus interface consumed
by xdg-desktop-portal-gnome. Its `record_monitor` accepts only an existing enabled
output from niri's output map, then starts a PipeWire target for that output. See
[`Session::record_monitor`](https://github.com/niri-wm/niri/blob/0777769e719b7c9b7c980d4ea66288bfbb4da5b3/src/dbus/mutter_screen_cast.rs#L176-L224).
niri can also publish a special dynamic *window* cast target, but that target changes
between one window or one monitor and is not a second clean full-monitor source.

niri's official screencasting page states that it has no built-in output mirroring
and recommends `wl-mirror`, which mirrors into a window. That window is not a clean
copy of the host monitor and is not a monitor source. See
[Screen mirroring](https://github.com/niri-wm/niri/wiki/Screencasting#screen-mirroring).

## Alternatives considered

| Approach | Hyprland | niri | Meets the invariant? |
| --- | --- | --- | --- |
| Existing `noscreenshare` / `block-out-from` | Native | Native | **No.** Both deliberately draw black rectangles. |
| Post-process portal/PipeWire frames | Possible transport layer | Possible transport layer | **No.** Occluded live pixels are absent from the input frame. |
| Headless or XDP `VIRTUAL` output | Headless output exists | No built-in mirrored output | **No.** It extends the desktop; it does not clone the physical scene with a filter. |
| Mirror physical output to another output/window | Native output mirroring exists, but clones the composed image | Third-party window mirror only | **No.** A post-composition mirror contains Nexora or its redaction. |
| PipeWire virtual camera | Can carry a custom compositor's result | Same | **No as the primary design.** It appears as a camera/device where supported, not as a portal monitor, and meeting camera policy/quality differs from screen share. |
| Rebuild the desktop from individual window captures and IPC geometry | Hyprland IPC gives some geometry | niri IPC gives some geometry | **No.** It misses wallpaper/layers/popups, exact z-order, effects, transitions, and synchronized frames; portal selection is user-mediated. |
| Run apps in a nested compositor and share that window | Possible | Possible | **No for the stated requirement.** It changes the desktop/session model and is not the current host monitor scene. |
| Custom ScreenCast portal backend | Possible | Possible | **Insufficient alone.** It still needs a clean compositor render and must replace or reproduce normal backend behavior. |
| Compositor-side second composition excluding Nexora | Core patch or exact-version prototype plugin | Small core render-path patch | **Yes.** This is the only approach that owns the required scene data at the right time. |

## Recommended v1 architecture

### 1. Add an explicit compositor semantic, separate from redaction

Do not change the security meaning of existing black-out rules globally. Add a
separate rule/effect, conceptually `omit-from "capture"`, with these semantics:

- It applies only when Nexora Safe Share has been explicitly enabled by the user.
- It removes the complete matched surface tree and every compositor-generated
  visual derived from it: child surfaces, xdg popups, layer popups, shadows, blur,
  dimming, borders, animation snapshots, overview/Alt-Tab previews, and drag icons.
- It affects monitor and region capture paths used for Safe Share, never the
  physical output.
- If the compositor cannot establish the omit set or clean render target, it stops
  or rejects the cast. It must not fall back to leaking Nexora or to black boxes.

Keep `noscreenshare`/`block-out-from` available for password managers and other
sensitive content. Black is desirable there because omission would reveal whatever
is underneath and could give a false impression that the hidden application is not
open.

### 2. Render a capture-specific scene before flattening

For niri, extend its existing `RenderTarget::Screencast` render-list construction.
When an element belongs to Nexora, push no element for the surface or its effects.
Keep every non-Nexora element in its normal z-order. Apply the same policy to the
capture path selected for v1 (PipeWire portal at minimum; direct screen-copy paths
only if they are claimed as supported).

For Hyprland, do not base the clean result on `getMirrorTexture()` followed by a
patch rectangle. Build a capture render pass from the output scene and skip Nexora
elements before they are flattened. A second render pass is the least ambiguous v1
implementation. A later optimization may cache a clean framebuffer, but only if
damage, z-order, effects, and capture synchronization remain equivalent to the
second pass.

The adapters should force normal composition while a clean cast is active, avoiding
direct-scanout/overlay-plane shortcuts that bypass the filtered render graph. They
must also keep underlying occluded clients producing frame callbacks; otherwise the
revealed background can be valid but stale.

### 3. Reuse the ordinary portal monitor source

While Safe Share is active, choosing the physical monitor in the existing portal
picker yields the filtered render. The backend still reports `source_type=MONITOR`,
normal logical position/size, cursor mode, PipeWire node, and restoration metadata.
This deliberately avoids requiring applications to request `VIRTUAL`, recognize a
custom source kind, accept a virtual camera, or adopt a Nexora plugin.

This is also the only practical broad-compatibility approach. Current OBS source
code has a `VIRTUAL` enum value but its registered desktop, window, and unified
capture types request only `MONITOR`, `WINDOW`, or their union; its desktop source
sets `types=MONITOR` in `SelectSources`. See OBS's
[`screencast-portal.c`](https://github.com/obsproject/obs-studio/blob/9d38a938318aa32906e3727be30a972a3ecd0d94/plugins/linux-pipewire/screencast-portal.c#L26-L42)
and
[`select_source`](https://github.com/obsproject/obs-studio/blob/9d38a938318aa32906e3727be30a972a3ecd0d94/plugins/linux-pipewire/screencast-portal.c#L345-L383).

A branded “Nexora Safe Share — monitor name” picker entry can be a later portal UX
enhancement. It is not needed for the pixel guarantee and would require changes to
both portal stacks or a replacement backend.

### 4. Define and attest the guarantee at runtime

Nexora should show “Safe Share available” only after a compositor adapter handshake
confirms:

- exact compatible compositor/adapter version;
- clean output rendering and all-surface-tree omission support;
- portal backend and capture protocol actually in use;
- the stable Nexora layer namespace/app ID is matched;
- every currently mapped Nexora surface is in the compositor's omit set; and
- a self-test frame proves that a known Nexora test surface is absent while a known
  changing background is present.

If any check fails, the UI must say Safe Share is unavailable. It may offer the
existing black-redaction mode under a different label, but must not call it clean
recomposition. Safe Share should be opt-in per session and visibly indicate the
selected monitor and active cast.

Nexora's layer surfaces should be non-exclusive so they do not move or resize the
underlying desktop. Omitting an exclusive-zone panel could expose pixels but cannot
undo layout changes caused by reserving space, which would violate “the same scene
without Nexora.”

## Consumer compatibility and limitations

The guarantee follows the capture path, not an application name.

### Zoom

Zoom's current official documentation says a Linux Wayland session can share only
an entire desktop or whiteboard, not an individual application. That limitation is
compatible with this v1 because the supported source is deliberately a monitor. See
Zoom's
[screen-sharing requirements](https://support.zoom.com/hc/en/article?id=zm_kb&sysparm_article=KB0060596).
Safe Share works only when that desktop selection resolves to the patched portal and
compositor capture path. Zoom-specific sharing modes such as a second camera,
whiteboard, file/video share, or an Xorg session do not use the guaranteed monitor
path. Annotation on Wayland is also officially limited to sharing the entire desktop
on one display, which should be included in acceptance tests. See Zoom's
[annotation limitation](https://support.zoom.com/hc/en/article?id=zm_kb&sysparm_article=KB0067931).

### Google Meet and other browser WebRTC clients

Chromium/WebRTC's Wayland desktop capturer is explicitly a PipeWire capturer backed
by the screen-cast portal. See WebRTC's
[`BaseCapturerPipeWire`](https://webrtc.googlesource.com/src/+/e6ec81a89ca904f1816b76456426babc28a9d767/modules/desktop_capture/linux/wayland/base_capturer_pipewire.h#28).
Meet therefore benefits when the browser is running its Wayland/PipeWire portal
path and the user selects a monitor. Browser flags, builds without PipeWire, X11 or
XWayland fallback, enterprise policy, and browser-provided tab capture are outside
the guarantee. Firefox requires its own acceptance run even though niri documents
Firefox among supported portal/PipeWire consumers.

### Discord and other Electron clients

Electron applications can reach the PipeWire/XDG portal path through Chromium's
WebRTC capture stack, but shipped Electron versions and application-specific WebRTC
integrations vary. Electron's own Wayland issue notes that Discord and similar apps
have historically differed because of unsupported Electron versions or non-standard
WebRTC implementations. See
[Electron issue 30652](https://github.com/electron/electron/issues/30652).
Accordingly, Discord is supported only after Nexora observes the portal cast and the
version/launch mode passes the test matrix. XWayland capture, an old client that
never opens the portal, or a custom capture implementation is not covered.

### OBS Studio

OBS's Linux PipeWire desktop capture directly calls the ScreenCast portal, requests
`MONITOR`, and retains restore tokens, so it is a strong reference consumer for the
v1 path. See the pinned OBS source links above. OBS window capture is not covered by
the full-monitor guarantee. Neither are third-party OBS plugins, game capture,
camera capture, a nested compositor source, or capture protocols that bypass the
patched path.

### Other portal recorders

Any recorder that receives the selected monitor node through
`org.freedesktop.portal.ScreenCast` gets exactly the compositor's clean frames;
recording versus live transmission makes no difference. Persistence tokens restore
a source selection, not the Safe Share rendering policy, so the compositor handshake
must be revalidated on every session. Recorders using direct `wlr-screencopy`,
`ext-image-copy-capture`, vendor GPU capture, X11 root-window capture, or external
hardware are covered only if the corresponding compositor path implements and
attests the same omit semantic. A portal recorder opening a third-party screenshot
preview can also expose an unpatched screenshot path inside the cast; niri explicitly
warns about this interaction for its current `block-out-from "screencast"` rule.

## Main engineering risks

1. **Surface completeness.** Nexora may create layer surfaces, regular xdg
   toplevels, popups, tooltips, IME surfaces, drag icons, or animation snapshots.
   Matching only the primary layer namespace is insufficient.
2. **Stale background.** Clients fully occluded by Nexora may be throttled. The
   clean cast must keep needed surfaces live and damage the capture when either the
   omitted surface moves or any newly revealed surface updates.
3. **Effects leak.** Blur samples, shadows, focus rings, dimming, rounded-corner
   masks, overview previews, and captured transition snapshots can reveal the shape
   or contents of Nexora even when its primary texture is skipped.
4. **Direct scanout and hardware planes.** A capture path that copies a scanout
   buffer or physical plane after composition can bypass the clean scene. Active
   Safe Share should force the verified composition path.
5. **Performance.** niri already builds a separate screencast render. Hyprland will
   pay for a second full scene pass unless it gains a carefully damage-tracked clean
   framebuffer. Test 4K/HiDPI, multiple monitors, 60/120/144 Hz, integrated GPUs,
   NVIDIA, and multi-GPU laptops.
6. **Color and geometry.** Rotation, fractional scale, HDR/color management,
   dmabuf modifiers, cursor metadata, and monitor hotplug must preserve the portal
   stream's declared size/position and negotiated PipeWire format.
7. **Version drift.** Hyprland internals and plugins are not a stable cross-version
   seam; niri's private Mutter-compatible D-Bus and render internals also evolve.
   Package exact compatible versions and fail closed on mismatch.
8. **Capture-path ambiguity.** Application branding is not proof of portal use.
   The guarantee must be based on observed portal/compositor session state, with
   separate acceptance tests for Zoom, Chrome/Chromium Meet, Firefox Meet, Discord,
   and OBS.
9. **Semantics and user trust.** Clean omission is not a general privacy redaction
   feature: it reveals the live scene below. Keep its rule distinct, session-scoped,
   explicit, and limited to Nexora-owned surfaces.

## Acceptance criteria for the compositor adapters

- Put a continuously changing test window behind opaque, translucent, blurred, and
  moving Nexora surfaces; the captured monitor must show the live test window with
  no black, transparency, outline, stale pixels, or Nexora-derived effect.
- Repeat for every Nexora window/layer type and all popup descendants, including
  opening and closing transitions.
- Verify wallpaper, desktop layers, panels, notifications, regular and fullscreen
  windows, cursor modes, and non-Nexora overlays remain in correct z-order.
- Exercise monitor/region capture through the active portal backend, OBS PipeWire
  Desktop Capture, Chromium and Firefox WebRTC, Zoom Wayland desktop share, and the
  supported Discord build.
- Hotplug, rotate, scale, suspend/resume, change refresh rate, and restart PipeWire;
  the stream must recover cleanly or stop, never silently revert to leak/black.
- Attempt unsupported direct capture paths. Nexora must label them unsupported, or
  they must pass the same test before being added to the guarantee.
- Measure GPU time and dropped frames at the supported maximum resolutions and
  refresh rates on the declared hardware matrix.

## Decision

Build Safe Share v1 as compositor-side clean recomposition, not as a virtual output,
virtual camera, PipeWire filter, or portal-only feature. Upstream a distinct capture
omission semantic to both compositors where possible; until then, ship exact-version
patched Hyprland and niri packages, use a Hyprland plugin only as a pinned prototype,
and gate every claim on runtime attestation and a self-test. Reuse the normal portal
`MONITOR` source so existing consumers require no new source type. Treat every
capture path outside the attested compositor/portal route as unsupported.
