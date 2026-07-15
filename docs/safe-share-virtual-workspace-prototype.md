# Safe Share Virtual Workspace Prototype

This is a throwaway probe, not production Nexora code. It answers whether the
current Wayland session can create and expose a clean virtual workspace without
patching the compositor.

Run it from a graphical Hyprland or niri terminal:

```sh
cargo run --bin safe_share_prototype
```

The probe prints compositor, portal, tool, and output state before making any
change. On Hyprland it can create a temporary `NEXORA-SAFE-SHARE` headless output,
place a full-screen marker on it, and keep the output alive while the operator
selects it in OBS, a browser meeting, or Discord. Pressing Enter removes the marker
and output. A later run also removes a stale prototype output left by a crash.

On niri the probe currently stops after capability inspection because niri has no
unprivileged native command for creating a headless output. That negative result is
part of the prototype: a cross-compositor design needs a dedicated nested runtime
or another independently supplied virtual display.

## Pass criteria

- The virtual output is selectable through the normal portal picker.
- Its recording contains the marker but never Nexora or black redaction.
- A local preview forwards pointer and keyboard input to the virtual workspace.
- Cleanup removes the output, marker, and any preview/control session.

The current Hyprland probe tests creation, portal selection, recording, and
cleanup. It deliberately reports local input forwarding as unproven; output
mirroring alone is not interactive.
