# Safe Share Virtual Workspace Prototype

This is a throwaway probe, not production Nexora code. It answers whether the
current Wayland session can create and expose a clean virtual workspace without
patching the compositor.

Run it from a graphical Hyprland or niri terminal:

```sh
cargo run --bin safe_share_prototype
```

The probe prints compositor, portal, tool, and output state before making any
change, then offers two probes.

## Nested-runtime probe (recommended)

Repeated live runs showed that hotplugging any output into the host session
breaks quickshell-based desktop shells every time, regardless of geometry or
focus handling. The nested-runtime probe avoids the hotplug class entirely:
a nested compositor (`cage`, falling back to `gamescope`) runs the clean
marker — including a text field — and appears as a **normal window** on the
desktop. Meeting applications share it through their ordinary *window*
picker, the window itself is the local preview, and pointer/keyboard input is
native, with no VNC and no new output. This is also the only viable path on
niri. Its pass criteria: the window share transmits the marker with no Nexora
surfaces and no black redaction even when Nexora overlaps the window locally;
typing into the marker's text field works directly; closing the runtime ends
cleanly and the viewer-side behavior at teardown is recorded.

## Headless-output probe (Hyprland only)

On Hyprland the probe can create a temporary `NEXORA-SAFE-SHARE` headless output,
place a full-screen marker on it, and keep the output alive while the operator
selects it in OBS, a browser meeting, or Discord. Pressing Enter removes the marker
and output. A later run also removes a stale prototype output left by a crash.

On niri the probe currently stops after capability inspection because niri has no
unprivileged native command for creating a headless output. That negative result is
part of the prototype: a cross-compositor design needs a dedicated nested runtime
or another independently supplied virtual display.

If `wayvnc` is installed, the probe also offers a local interactive preview:
wayvnc attaches to the virtual output on loopback (`127.0.0.1:5999`, no auth,
throwaway), a terminal is launched on the shared workspace, and a VNC viewer
(`vncviewer` or `wlvncc`) opens automatically when available. Typing into that
terminal through the preview proves pointer and keyboard forwarding — plain
output mirroring never does. The preview is killed during teardown, before the
output is removed.

The headless output is placed past the existing layout with a small gap, so no
edge is shared with any physical monitor. A live run showed why: with `auto`
positioning the virtual output is adjacent, so the cursor wanders onto it,
windows can be dragged across in both directions, and Nexora itself can stray
onto the shared output — where an active anti-capture rule renders it as a
black rectangle in captures, exactly the redaction Safe Share must never show.
Compositor keybinds always act on the real session; they are not forwarded
into the preview.

Creating a monitor also steals focus and warps the cursor onto the invisible
output, which strands the user: workspace binds keep acting on the monitor
they cannot see. The probe records the focused monitor before creating the
output and focuses it back immediately after setup. Live runs additionally
saw a quickshell-based desktop shell disappear whenever the output appeared —
with both huge and minimal layout bounding boxes, so the hotplug event itself
is the trigger (upstream quickshell documents a Qt monitor-hotplug crash bug
and shells are recreated reactively when the screen list changes). The shell
did not return when the output was removed and had to be restarted manually.
Third-party shells reacting to output hotplug are a real compatibility risk
that the product design must test for explicitly; it also strengthens the
case for a nested-runtime design that never hotplugs an output into the host
session.

Note two teardown behaviors observed on Hyprland: windows left on the shared
workspace jump to the physical monitor when the output is removed, and a
consumer that is already streaming the output does not necessarily stop when
the output disappears. Teardown in a real design must close or verify consumer
sessions instead of relying on output removal alone.

## Pass criteria

- The virtual output is selectable through the normal portal picker.
- Its recording contains the marker but never Nexora or black redaction.
- A local preview forwards pointer and keyboard input to the virtual workspace.
- Cleanup removes the output, marker, and any preview/control session.

The current Hyprland probe tests creation, portal selection, recording, and
cleanup. It deliberately reports local input forwarding as unproven; output
mirroring alone is not interactive.
