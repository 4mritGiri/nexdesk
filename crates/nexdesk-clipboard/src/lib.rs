//! Linux clipboard + file transfer for the RDP `CLIPRDR` channel.
//!
//! IronRDP only ships a real clipboard backend for Windows; everywhere else it is a no-op stub,
//! which is why copy/paste (text *and* files) silently did nothing on Linux. This crate is the
//! missing backend. The local side is the X11 selection protocol, which also covers Wayland
//! desktops through XWayland (GNOME/Mutter and KDE/KWin keep both clipboards in sync).
mod convert;
mod engine;
mod files;
pub mod transport;
#[cfg(unix)]
mod x11;

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use ironrdp_cliprdr::backend::{CliprdrBackend, CliprdrBackendFactory, ClipboardMessage};
use ironrdp_cliprdr::pdu::{
    ClipboardFormat, ClipboardGeneralCapabilityFlags, FileContentsRequest, FileContentsResponse, FileDescriptor,
    FormatDataRequest, FormatDataResponse, LockDataId,
};

use crate::engine::{EngineHandle, EngineMsg};
use crate::transport::{Transport, TransportEvent};

/// Where clipboard messages for the RDP session go (typically `RdpInputEvent::Clipboard`).
pub type Sink = Arc<dyn Fn(ClipboardMessage) + Send + Sync>;

#[derive(Clone, Debug)]
pub struct Config {
    /// Parent directory for files pulled from the remote clipboard (one private subdirectory per process).
    pub staging_root: PathBuf,
    /// Remote file selections up to this many bytes are downloaded as soon as they are copied,
    /// so the paste in the file manager is instant. Larger ones start on first paste.
    pub prefetch_limit: u64,
}

impl Default for Config {
    fn default() -> Self {
        let cache = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .unwrap_or_else(std::env::temp_dir);
        Self { staging_root: cache.join("nexdesk").join("clipboard"), prefetch_limit: 64 * 1024 * 1024 }
    }
}

/// Entry point: create one per process, hand [`LinuxClipboard::backend_factory`] to the RDP client.
pub struct LinuxClipboard {
    sink: Sink,
    cfg: Config,
    current: Arc<Mutex<Option<Sender<EngineMsg>>>>,
}

/// Cheap clone used by the UI (e.g. to offer dropped files to the remote side).
#[derive(Clone)]
pub struct ClipboardHandle {
    current: Arc<Mutex<Option<Sender<EngineMsg>>>>,
}

impl ClipboardHandle {
    /// Offer local files to the remote machine (they appear as a pasteable file selection there).
    /// Returns false when no clipboard session is active.
    pub fn offer_files(&self, paths: Vec<PathBuf>) -> bool {
        match self.current.lock().ok().and_then(|g| g.clone()) {
            Some(tx) => tx.send(EngineMsg::OfferFiles(paths)).is_ok(),
            None => false,
        }
    }
}

impl LinuxClipboard {
    pub fn new(sink: Sink, cfg: Config) -> Self {
        Self { sink, cfg, current: Arc::new(Mutex::new(None)) }
    }

    pub fn handle(&self) -> ClipboardHandle {
        ClipboardHandle { current: self.current.clone() }
    }

    pub fn backend_factory(&self) -> Box<dyn CliprdrBackendFactory + Send> {
        Box::new(Factory { sink: self.sink.clone(), cfg: self.cfg.clone(), current: self.current.clone() })
    }
}

struct Factory {
    sink: Sink,
    cfg: Config,
    current: Arc<Mutex<Option<Sender<EngineMsg>>>>,
}

impl CliprdrBackendFactory for Factory {
    fn build_cliprdr_backend(&self) -> Box<dyn CliprdrBackend> {
        let backend = build_backend(self.cfg.clone(), self.sink.clone(), open_transport);
        if let Ok(mut g) = self.current.lock() {
            *g = Some(backend.handle.tx.clone());
        }
        Box::new(backend)
    }
}

/// Builds the local transport; `None` -> clipboard degrades to "unavailable" instead of failing the session.
fn open_transport(events: crate::transport::EventSink) -> Option<Box<dyn Transport>> {
    #[cfg(unix)]
    {
        match x11::X11Transport::spawn(events) {
            Ok(t) => {
                tracing::info!("clipboard: using the X11 selection protocol (native X11 or XWayland)");
                return Some(Box::new(t));
            }
            Err(e) => tracing::warn!("clipboard unavailable: {e}"),
        }
    }
    let _ = events;
    None
}

/// A transport that never has anything (used when no display could be opened).
struct NullTransport;
impl Transport for NullTransport {
    fn read(&self, _: u64, _: &str) {}
    fn own(&self, _: Vec<String>, _: Arc<dyn transport::Provider>) {}
    fn disown(&self) {}
    fn query(&self) {}
    fn name(&self) -> &'static str {
        "none"
    }
}

pub(crate) fn build_backend(
    cfg: Config,
    sink: Sink,
    open: impl FnOnce(crate::transport::EventSink) -> Option<Box<dyn Transport>>,
) -> LinuxBackend {
    // The engine has to exist before the transport (the transport reports into it), so the
    // channel is created first and the transport is attached while spawning.
    let (tx, rx) = std::sync::mpsc::channel::<EngineMsg>();
    let events_tx = tx.clone();
    let events: crate::transport::EventSink = Arc::new(move |ev: TransportEvent| {
        let _ = events_tx.send(EngineMsg::Transport(ev));
    });
    let transport = open(events).unwrap_or_else(|| Box::new(NullTransport));
    let tmp_dir = cfg.staging_root.to_string_lossy().into_owned();
    let handle = engine::spawn_with(cfg, sink, transport, tx, rx);
    LinuxBackend { handle, tmp_dir }
}

#[derive(Debug)]
pub(crate) struct LinuxBackend {
    pub(crate) handle: EngineHandle,
    tmp_dir: String,
}

impl std::fmt::Debug for EngineHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EngineHandle")
    }
}

ironrdp_core::impl_as_any!(LinuxBackend);

impl Drop for LinuxBackend {
    fn drop(&mut self) {
        let _ = self.handle.tx.send(EngineMsg::Shutdown);
    }
}

impl LinuxBackend {
    fn send(&self, m: EngineMsg) {
        let _ = self.handle.tx.send(m);
    }
}

impl CliprdrBackend for LinuxBackend {
    fn temporary_directory(&self) -> &str {
        &self.tmp_dir
    }

    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        ClipboardGeneralCapabilityFlags::USE_LONG_FORMAT_NAMES
            | ClipboardGeneralCapabilityFlags::STREAM_FILECLIP_ENABLED
            | ClipboardGeneralCapabilityFlags::FILECLIP_NO_FILE_PATHS
            | ClipboardGeneralCapabilityFlags::CAN_LOCK_CLIPDATA
            | ClipboardGeneralCapabilityFlags::HUGE_FILE_SUPPORT_ENABLED
    }

    fn on_ready(&mut self) {
        self.send(EngineMsg::Ready);
    }

    fn on_request_format_list(&mut self) {
        self.send(EngineMsg::RequestFormatList);
    }

    fn on_process_negotiated_capabilities(&mut self, capabilities: ClipboardGeneralCapabilityFlags) {
        self.send(EngineMsg::Caps(capabilities));
    }

    fn on_remote_copy(&mut self, available_formats: &[ClipboardFormat]) {
        self.send(EngineMsg::RemoteCopy(available_formats.to_vec()));
    }

    fn on_format_data_request(&mut self, request: FormatDataRequest) {
        self.send(EngineMsg::FormatDataRequest(request.format));
    }

    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        self.send(EngineMsg::FormatDataResponse { error: response.is_error(), data: response.data().to_vec() });
    }

    fn on_file_contents_request(&mut self, request: FileContentsRequest) {
        self.send(EngineMsg::FileContentsRequest(request));
    }

    fn on_file_contents_response(&mut self, response: FileContentsResponse<'_>) {
        let result = if response.is_error() { Err(()) } else { Ok(response.data().to_vec()) };
        self.handle.router.deliver(response.stream_id(), result);
    }

    fn on_lock(&mut self, data_id: LockDataId) {
        self.send(EngineMsg::Lock(data_id.0));
    }

    fn on_unlock(&mut self, data_id: LockDataId) {
        self.send(EngineMsg::Unlock(data_id.0));
    }

    fn on_remote_file_list(&mut self, files: &[FileDescriptor], clip_data_id: Option<u32>) {
        self.send(EngineMsg::RemoteFileList { files: files.to_vec(), lock: clip_data_id });
    }
}

#[cfg(test)]
mod tests;
