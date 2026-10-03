# Viewer behaviour (nexdesk-rdp)

| Action | How |
|---|---|
| Toggle full screen | **Ctrl+Alt+Break** (same as mstsc) |
| Start full screen | `--fullscreen`, or `screen mode id:i:2` in the profile/.rdp file |
| Ctrl+Alt+Del on the remote | **Ctrl+Alt+End** |
| Keep Super / Alt+Tab local | `--no-key-capture` |

## Keyboard capture
While the window is **full screen and focused**, Super, Alt+Tab, Alt+F4 etc. go to the remote machine
(mstsc's default "only when using the full screen"). Capture is released when the window loses focus
or leaves full screen.

* X11: XInput2 `XIGrabDevice` on the master keyboard.
* Wayland: `zwp_keyboard_shortcuts_inhibit_v1`. GNOME shows a notice; **Super+Escape** is GNOME's own
  "restore shortcuts" escape hatch. Compositors without that protocol: capture is skipped (a warning is logged,
  run with `NEXDESK_LOG=warn`).

## Cursor
Server cursor shapes (`PointerBitmap`) are converted from premultiplied to straight alpha and shown as a
native custom cursor. Previously they were ignored and a server "hide" request left the cursor invisible.
