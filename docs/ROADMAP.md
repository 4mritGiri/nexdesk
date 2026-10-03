# Roadmap
1. Compile `nexdesk-viewer` on a current toolchain; fix any API drift. Verify against a Windows host.
2. Linux clipboard backend (docs/CLIPBOARD.md): text, then files Windows->Linux, then Linux->Windows.
3. TLS certificate verification + known-hosts store (needed for enterprise use).
4. Render server cursor shapes (`RdpOutputEvent::PointerBitmap`).
5. Drive redirection (`rdpdr`), audio (`sound` feature), RD Gateway (`gateway` feature), Kerberos/smartcard.
6. Move to `wgpu` for HiDPI/multi-monitor; hardware H.264 (RDPEGFX) when IronRDP supports it.
7. Manager UI (GTK4 + libadwaita, or GPUI after the spike in the chat): connection list, credential
   storage via the OS keyring, `.rdp` import. Keep it in a separate `nexdesk-app` crate; core stays UI-free.
