# NexDesk vs Remmina: honest status

Legend: **Done** = written (not all compiled/tested yet) · **Next** = planned, order below · **Gap** = not started

| Area | Remmina | NexDesk |
|---|---|---|
| Saved connections (.rdp compatible) | yes | **Done**: list, New, Edit, Duplicate, Delete |
| Full screen + `Ctrl+Alt+Break` toggle | yes | **Done** |
| Super / Alt+Tab to remote (X11 + Wayland) | partial | **Done**, needs real-world testing |
| Server cursor shapes | yes | **Done** |
| Passwords never on disk, zeroized in memory | keyring option | **Done** (no keyring yet) |
| Process isolation per session (crash-safe) | no | **Done** |
| Clipboard text/images (Linux) | yes | **Done**: X11 + Wayland (via XWayland), needs real-server testing (docs/CLIPBOARD.md) |
| Clipboard **files** both directions, folders | partial | **Done**: staged download + streamed upload |
| Drag & drop files onto the session | yes | **Done** on X11/XWayland (`--x11`); native Wayland blocked by winit |
| Full-screen connection bar (min/restore/close/pin) | yes | **Done** |
| TLS certificate verification / known hosts | yes | **Done**: system trust store, else TOFU pinning + changed-cert warning, handshake signatures verified |
| OS keyring (Secret Service) credentials | yes | **Next**: `CredentialStore` trait is ready |
| Drive redirection | yes | **Next** |
| Auto-reconnect | yes | **Next**: lifecycle states exist |
| RD Gateway | yes | **Next**: depends on IronRDP gateway support |
| Dynamic resize / multi-monitor | yes | **Gap** (`--dynamic-resize` flag is experimental) |
| Audio, printers, smartcard, USB | yes | **Gap** |
| Tabs / groups / search / quick connect | yes | **Gap** |
| Kerberos / SSO | yes | **Gap** |
| Admin policy file, audit log, signed packages | no | **Gap**: the enterprise differentiators |

## Suggested order
1. TLS verification + known-hosts store
2. Linux clipboard backend (text, then files)
3. Keyring credentials
4. Drive redirection, auto-reconnect
5. Groups, search, quick connect, tabs
6. Policy file (disable clipboard/drives fleet-wide), audit logging, .deb/Flatpak packaging
