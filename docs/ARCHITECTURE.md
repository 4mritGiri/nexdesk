# NexDesk enterprise architecture

```text
                 ┌─────────────────────────────┐
                 │          nexdesk             │
                 │        GPUI desktop         │
                 └──────────────┬──────────────┘
                                │
                                ▼
                 ┌─────────────────────────────┐
                 │        nexdesk-ui           │
                 │ navigation / theme / state  │
                 └──────────────┬──────────────┘
                                │
                                ▼
                 ┌─────────────────────────────┐
                 │      nexdesk-session        │
                 │ lifecycle / manager /       │
                 │ events / metrics / reconnect│
                 └──────────────┬──────────────┘
                                │ process boundary
                                ▼
                 ┌─────────────────────────────┐
                 │        nexdesk-rdp          │
                 │ IronRDP / input / output    │
                 └──────────────┬──────────────┘
                                │
                                ▼
                         Windows RDP host

nexdesk-core is shared by the layers above and remains UI/protocol agnostic.
nexdesk-renderer is the framebuffer/display boundary for future GPUI/GPU rendering.
```

## Session lifecycle

`Created → Starting → Connecting → Connected → Reconnecting → Connected` or `Disconnected/Failed`.

The manager owns lifecycle state; the RDP engine does not mutate UI state directly.

## Reconnect

The first implementation exposes typed reconnect events and metrics but does not silently reconnect an interactive desktop yet. That behavior should be enabled only after the protocol engine exposes a safe reconnect/credential retry contract.

## Credentials

Profiles contain connection metadata only. Passwords enter the session boundary as `Secret`. A production keyring implementation should implement `CredentialStore`; the UI should never receive plaintext from the persistence layer.

## Renderer

The framebuffer API is independent of softbuffer/winit. This leaves room for a GPUI/wgpu presentation path without coupling IronRDP decoding to the UI.

## Logging

Use `tracing` fields such as `session_id`, `host`, and `state`. Never emit password, token, clipboard contents, or file contents.
