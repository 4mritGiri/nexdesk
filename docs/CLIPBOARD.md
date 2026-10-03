# Fixing Windows -> Linux file paste

## What I found in the IronRDP source (ironrdp-client 0.1.0, `src/rdp.rs`)
`RdpClient::run` picks the clipboard backend at compile time:
- Windows: `WinClipboard` (full support, incl. files)
- everything else, including Linux: `StubClipboard` — **does nothing**.

So with the stock client, Linux gets *no* clipboard (not even text). The backend is chosen inside
`run()`, so it cannot be injected from outside.

## Plan
1. **Vendor `ironrdp-client`'s `rdp.rs` + `config.rs`** into a `nexdesk-engine` crate (or fork) and replace the
   non-Windows `StubClipboard::new()` branch with `LinuxClipboard::new(...).backend_factory()`.
2. Implement `ironrdp_cliprdr::backend::CliprdrBackend` for Linux (read the trait in the `ironrdp-cliprdr`
   crate for the exact callbacks). Responsibilities:
   - **Text/images:** map CF_UNICODETEXT/CF_DIB <-> X11/Wayland selections.
   - **Remote -> local files:** on `FileGroupDescriptorW`, parse entries into `clipfiles::FileEntry`;
     create `StagingDir` under `$XDG_RUNTIME_DIR/nexdesk/`; when the local app pastes, issue
     `FileContentsRequest` (size, then ranges), write via `StagingDir::prepare_entry`, then set the local
     clipboard to `uri_list(top_level_paths)` (`text/uri-list`) and `gnome_copied_files(...)`
     (`x-special/gnome-copied-files`).
   - **Local -> remote files:** advertise `FileGroupDescriptorW`, serve `FileContents` from disk.
3. Local clipboard access: `x11rb` (X11) and `wayland-client` with `ext-data-control`/`wlr-data-control` (Wayland).
   `arboard` is text/image only.
4. Optional later: FUSE (`fuser`) so large files stream on demand instead of being fully staged first.

## Check before blaming the client
Jump servers often block this by policy. On the Windows host check
`Computer Configuration > Administrative Templates > Windows Components > Remote Desktop Services >
Remote Desktop Session Host > Device and Resource Redirection` ("Do not allow Clipboard redirection",
"Do not allow drive redirection"). If it is disabled server-side, no client can paste files.

## Workaround today
Drive redirection (`rdpdr`): share a local folder and copy via `\\tsclient\<name>`. `ironrdp-client` has an
`rdpdr` feature and `RdpdrConfig`; it is not wired into the CLI yet.
