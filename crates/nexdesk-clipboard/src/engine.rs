//! The RDP-facing half of the clipboard: a single actor thread that translates between
//! CLIPRDR callbacks/messages and a local [`Transport`] (the desktop clipboard).
//!
//! Remote -> local:  text and images are fetched lazily when a local app pastes; files are
//!                   downloaded into a private staging directory and then offered as
//!                   `text/uri-list` / `x-special/gnome-copied-files` (Linux has no virtual files).
//! Local -> remote:  text/images are read from the local owner when the server asks;
//!                   files are advertised as a CLIPRDR file list and streamed from disk on demand.
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ironrdp_cliprdr::backend::ClipboardMessage;
use ironrdp_cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardFormatName, ClipboardGeneralCapabilityFlags,
    FileContentsFlags, FileContentsRequest, FileDescriptor, FormatDataResponse,
};
use nexdesk_core::clipfiles::{gnome_copied_files, uri_list, FileEntry, StagingDir};

use crate::convert;
use crate::files::{spawn_file_server, FileJob, LocalTable};
use crate::transport::{Provider, Reply, Transport, TransportEvent};
use crate::{Config, Sink};

const CHUNK: u64 = 256 * 1024;
const CHUNK_TIMEOUT: Duration = Duration::from_secs(60);
const TEXT_WAIT: Duration = Duration::from_secs(20);
const FILE_WAIT: Duration = Duration::from_secs(30 * 60);

pub(crate) enum EngineMsg {
    Caps(ClipboardGeneralCapabilityFlags),
    RequestFormatList,
    Ready,
    RemoteCopy(Vec<ClipboardFormat>),
    FormatDataRequest(ClipboardFormatId),
    FormatDataResponse {
        error: bool,
        data: Vec<u8>,
    },
    RemoteFileList {
        files: Vec<FileDescriptor>,
        lock: Option<u32>,
    },
    FileContentsRequest(FileContentsRequest),
    Lock(u32),
    Unlock(u32),
    Transport(TransportEvent),
    LocalRequest {
        gen: u64,
        target: String,
        reply: Reply,
    },
    OfferFiles(Vec<PathBuf>),
    DownloadDone {
        gen: u64,
        result: Result<Vec<PathBuf>, String>,
    },
    Shutdown,
}

// ------------------------------------------------------------------ stream router

/// Routes `FileContentsResponse`s (arriving on the CLIPRDR callback thread) to the
/// downloader thread that is waiting for them.
#[derive(Default)]
pub(crate) struct StreamRouter {
    next: AtomicU32,
    waiting: Mutex<HashMap<u32, Sender<Result<Vec<u8>, ()>>>>,
}

impl StreamRouter {
    pub(crate) fn deliver(&self, stream_id: u32, result: Result<Vec<u8>, ()>) {
        let tx = self
            .waiting
            .lock()
            .ok()
            .and_then(|mut m| m.remove(&stream_id));
        if let Some(tx) = tx {
            let _ = tx.send(result);
        }
    }

    fn request(
        &self,
        sink: &Sink,
        mut req: FileContentsRequest,
        timeout: Duration,
    ) -> Result<Vec<u8>, String> {
        let id = self
            .next
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
            .max(1);
        req.stream_id = id;
        let (tx, rx) = channel();
        self.waiting.lock().map_err(|_| "poisoned")?.insert(id, tx);
        sink(ClipboardMessage::SendFileContentsRequest(req));
        match rx.recv_timeout(timeout) {
            Ok(Ok(d)) => Ok(d),
            Ok(Err(())) => Err("the remote side refused to send the file data".into()),
            Err(_) => {
                if let Ok(mut m) = self.waiting.lock() {
                    m.remove(&id);
                }
                Err("timed out waiting for file data".into())
            }
        }
    }

    fn fail_all(&self) {
        if let Ok(mut m) = self.waiting.lock() {
            for (_, tx) in m.drain() {
                let _ = tx.send(Err(()));
            }
        }
    }
}

// ------------------------------------------------------------------ provider

struct EngineProvider {
    tx: Sender<EngineMsg>,
    gen: u64,
}

impl Provider for EngineProvider {
    fn request(&self, target: &str, reply: Reply) {
        if let Err(e) = self.tx.send(EngineMsg::LocalRequest {
            gen: self.gen,
            target: target.to_owned(),
            reply,
        }) {
            if let EngineMsg::LocalRequest { reply, .. } = e.0 {
                reply(None);
            }
        }
    }
}

// ------------------------------------------------------------------ state

enum Slot<T> {
    Idle,
    Pending,
    Ready(T),
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PasteKind {
    Text { unicode: bool },
    Image,
    FileList,
}

struct PasteJob {
    gen: u64,
    kind: PasteKind,
    format: ClipboardFormatId,
    started: Instant,
}

enum FilesState {
    Idle,
    Listing,
    Listed {
        files: Vec<FileDescriptor>,
        lock: Option<u32>,
    },
    Downloading,
    Done(Vec<PathBuf>),
    Failed,
}

struct Waiter {
    target: String,
    reply: Reply,
    deadline: Instant,
}

enum ReadPurpose {
    /// Reading the file URI list of a local copy so we can advertise it.
    FileList,
    /// Answering a server `FormatDataRequest`.
    FormatData {
        format: ClipboardFormatId,
        target: String,
    },
}

#[derive(Default, Clone)]
struct RemoteFormats {
    file_list: Option<ClipboardFormatId>,
    text: Option<(ClipboardFormatId, bool)>,
    image: Option<ClipboardFormatId>,
}

const TEXT_TARGETS: &[&str] = &[
    "UTF8_STRING",
    "text/plain;charset=utf-8",
    "text/plain",
    "STRING",
    "TEXT",
];
const FILE_TARGETS: &[&str] = &[
    "x-special/gnome-copied-files",
    "text/uri-list",
    "application/x-kde-cutselection",
    "UTF8_STRING",
    "text/plain;charset=utf-8",
];

struct Engine {
    cfg: Config,
    sink: Sink,
    tx: Sender<EngineMsg>,
    transport: Box<dyn Transport>,
    router: Arc<StreamRouter>,
    file_server: Sender<FileJob>,
    caps: ClipboardGeneralCapabilityFlags,
    ready: bool,

    // local -> remote
    next_token: u64,
    reads: HashMap<u64, ReadPurpose>,
    local_targets: Vec<String>,
    local_files: Option<Arc<LocalTable>>,
    locks: HashMap<u32, Arc<LocalTable>>,

    // remote -> local
    gen: u64,
    remote: RemoteFormats,
    paste_queue: VecDeque<PasteJob>,
    inflight: Option<PasteJob>,
    text: Slot<Vec<u8>>,
    image: Slot<Vec<u8>>,
    files: FilesState,
    waiters: Vec<Waiter>,
    cancel: Arc<AtomicBool>,
    staging_base: PathBuf,
    staging: VecDeque<StagingDir>,
}

pub(crate) struct EngineHandle {
    pub tx: Sender<EngineMsg>,
    pub router: Arc<StreamRouter>,
}

pub(crate) fn spawn_with(
    cfg: Config,
    sink: Sink,
    transport: Box<dyn Transport>,
    tx: Sender<EngineMsg>,
    rx: Receiver<EngineMsg>,
) -> EngineHandle {
    let router = Arc::new(StreamRouter::default());
    let staging_base = cfg.staging_root.join(format!("p{}", std::process::id()));
    sweep_stale(&cfg.staging_root);
    let eng = Engine {
        file_server: spawn_file_server(sink.clone()),
        cfg,
        sink,
        tx: tx.clone(),
        transport,
        router: router.clone(),
        caps: ClipboardGeneralCapabilityFlags::empty(),
        ready: false,
        next_token: 0,
        reads: HashMap::new(),
        local_targets: Vec::new(),
        local_files: None,
        locks: HashMap::new(),
        gen: 0,
        remote: RemoteFormats::default(),
        paste_queue: VecDeque::new(),
        inflight: None,
        text: Slot::Idle,
        image: Slot::Idle,
        files: FilesState::Idle,
        waiters: Vec::new(),
        cancel: Arc::new(AtomicBool::new(false)),
        staging_base,
        staging: VecDeque::new(),
    };
    std::thread::Builder::new()
        .name("nexdesk-clip-engine".into())
        .spawn(move || eng.run(rx))
        .expect("spawn clipboard engine");
    EngineHandle { tx, router }
}

/// Remove staging directories left behind by crashed sessions (their pid is gone).
fn sweep_stale(root: &std::path::Path) {
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if let Some(pid) = name.strip_prefix('p').and_then(|p| p.parse::<u32>().ok()) {
            if pid != std::process::id() && !std::path::Path::new(&format!("/proc/{pid}")).exists()
            {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
}

fn is_text_target(t: &str) -> bool {
    TEXT_TARGETS.contains(&t)
}

impl Engine {
    fn run(mut self, rx: Receiver<EngineMsg>) {
        loop {
            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(EngineMsg::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(m) => self.handle(m),
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.housekeeping();
        }
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.router.fail_all();
        for w in self.waiters.drain(..) {
            (w.reply)(None);
        }
        self.transport.disown();
        while let Some(s) = self.staging.pop_front() {
            let _ = s.cleanup();
        }
        let _ = std::fs::remove_dir_all(&self.staging_base);
    }

    fn send(&self, m: ClipboardMessage) {
        (self.sink)(m);
    }

    fn handle(&mut self, msg: EngineMsg) {
        match msg {
            EngineMsg::Caps(c) => self.caps = c,
            // Start-up handshake: the CLIPRDR layer needs one FormatList to finish initialising.
            EngineMsg::RequestFormatList => {
                self.send(ClipboardMessage::SendInitiateCopy(Vec::new()))
            }
            EngineMsg::Ready => {
                self.ready = true;
                // Pick up whatever is already on the local clipboard.
                self.transport.query();
            }
            EngineMsg::RemoteCopy(f) => self.on_remote_copy(&f),
            EngineMsg::FormatDataRequest(fmt) => self.on_format_data_request(fmt),
            EngineMsg::FormatDataResponse { error, data } => {
                self.on_format_data_response(error, data)
            }
            EngineMsg::RemoteFileList { files, lock } => self.on_remote_file_list(files, lock),
            EngineMsg::FileContentsRequest(req) => self.on_file_contents_request(req),
            EngineMsg::Lock(id) => {
                if let Some(t) = &self.local_files {
                    if self.locks.len() < 64 {
                        self.locks.insert(id, t.clone());
                    }
                }
            }
            EngineMsg::Unlock(id) => {
                self.locks.remove(&id);
            }
            EngineMsg::Transport(ev) => self.on_transport(ev),
            EngineMsg::LocalRequest { gen, target, reply } => {
                self.on_local_request(gen, target, reply)
            }
            EngineMsg::OfferFiles(paths) => {
                let table = LocalTable::build(&paths);
                if table.is_empty() {
                    tracing::warn!("nothing to offer: no usable files in the dropped selection");
                } else {
                    nexdesk_core::logs::file(
                        nexdesk_core::logs::Level::Info,
                        "Local -> remote",
                        paths.len(),
                        0,
                        "Offered",
                        "",
                    );
                    self.offer_table(table);
                }
            }
            EngineMsg::DownloadDone { gen, result } => {
                if gen == self.gen {
                    self.files = match result {
                        Ok(paths) => {
                            nexdesk_core::logs::file(
                                nexdesk_core::logs::Level::Info,
                                "Remote -> local",
                                paths.len(),
                                0,
                                "Downloaded",
                                "",
                            );
                            FilesState::Done(paths)
                        }
                        Err(e) => {
                            tracing::warn!("file download failed: {e}");
                            nexdesk_core::logs::file(
                                nexdesk_core::logs::Level::Error,
                                "Remote -> local",
                                0,
                                0,
                                &format!("Failed: {e}"),
                                "",
                            );
                            FilesState::Failed
                        }
                    };
                    self.flush_waiters();
                }
            }
            EngineMsg::Shutdown => {}
        }
    }

    // ============================================================ local -> remote

    fn on_transport(&mut self, ev: TransportEvent) {
        match ev {
            TransportEvent::OwnerChanged(targets) => {
                self.local_targets = targets.clone();
                if targets.is_empty() {
                    return;
                }
                let uri = ["x-special/gnome-copied-files", "text/uri-list"]
                    .into_iter()
                    .find(|u| targets.iter().any(|t| t == u));
                match uri {
                    Some(u) => {
                        let token = self.token();
                        self.reads.insert(token, ReadPurpose::FileList);
                        self.transport.read(token, u);
                    }
                    None => self.advertise_basic(),
                }
            }
            TransportEvent::ReadDone { token, result } => {
                let Some(purpose) = self.reads.remove(&token) else {
                    return;
                };
                match purpose {
                    ReadPurpose::FileList => {
                        let paths = result
                            .ok()
                            .map(|d| convert::parse_uri_list(&String::from_utf8_lossy(&d)));
                        let table = paths.map(|p| LocalTable::build(&p));
                        match table {
                            Some(t) if !t.is_empty() => self.offer_table(t),
                            _ => self.advertise_basic(),
                        }
                    }
                    ReadPurpose::FormatData { format, target } => {
                        let resp = match result {
                            Ok(bytes) if format == ClipboardFormatId::CF_UNICODETEXT => {
                                let s = if target == "STRING" {
                                    bytes.iter().map(|&b| b as char).collect::<String>()
                                } else {
                                    String::from_utf8_lossy(&bytes).into_owned()
                                };
                                FormatDataResponse::new_data(convert::text_to_rdp(&s))
                            }
                            Ok(bytes) if format == ClipboardFormatId::CF_DIB => {
                                match convert::png_to_dib(&bytes) {
                                    Ok(d) => FormatDataResponse::new_data(d),
                                    Err(e) => {
                                        tracing::warn!("image conversion failed: {e}");
                                        FormatDataResponse::new_error()
                                    }
                                }
                            }
                            _ => FormatDataResponse::new_error(),
                        };
                        self.send(ClipboardMessage::SendFormatData(resp));
                    }
                }
            }
        }
    }

    fn token(&mut self) -> u64 {
        self.next_token += 1;
        self.next_token
    }

    /// Advertise text and/or image formats of the current local clipboard.
    fn advertise_basic(&mut self) {
        let mut formats = Vec::new();
        if self.local_targets.iter().any(|t| is_text_target(t)) {
            formats.push(ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT));
        }
        if self.local_targets.iter().any(|t| t == "image/png") {
            formats.push(ClipboardFormat::new(ClipboardFormatId::CF_DIB));
        }
        if !formats.is_empty() {
            self.local_files = None;
            self.send(ClipboardMessage::SendInitiateCopy(formats));
        }
    }

    fn offer_table(&mut self, table: LocalTable) {
        if !self
            .caps
            .contains(ClipboardGeneralCapabilityFlags::STREAM_FILECLIP_ENABLED)
        {
            tracing::warn!(
                "the server did not enable file transfer over the clipboard (check the RDS policy)"
            );
            self.advertise_basic();
            return;
        }
        let descriptors = table.descriptors.clone();
        self.local_files = Some(Arc::new(table));
        self.send(ClipboardMessage::SendInitiateFileCopy(descriptors));
    }

    fn on_format_data_request(&mut self, format: ClipboardFormatId) {
        let target = if format == ClipboardFormatId::CF_UNICODETEXT {
            TEXT_TARGETS
                .iter()
                .find(|t| self.local_targets.iter().any(|l| l == *t))
                .map(|t| (*t).to_owned())
        } else if format == ClipboardFormatId::CF_DIB {
            self.local_targets
                .iter()
                .find(|t| *t == "image/png")
                .cloned()
        } else {
            None
        };
        match target {
            Some(target) => {
                let token = self.token();
                self.transport.read(token, &target);
                self.reads
                    .insert(token, ReadPurpose::FormatData { format, target });
            }
            None => self.send(ClipboardMessage::SendFormatData(
                FormatDataResponse::new_error(),
            )),
        }
    }

    fn on_file_contents_request(&mut self, req: FileContentsRequest) {
        let table = req
            .data_id
            .and_then(|id| self.locks.get(&id).cloned())
            .or_else(|| self.local_files.clone());
        match table {
            Some(table) => {
                let _ = self.file_server.send(FileJob {
                    request: req,
                    table,
                });
            }
            None => self.send(ClipboardMessage::SendFileContentsResponse(
                ironrdp_cliprdr::pdu::FileContentsResponse::new_error(req.stream_id),
            )),
        }
    }

    // ============================================================ remote -> local

    fn on_remote_copy(&mut self, formats: &[ClipboardFormat]) {
        // New remote clipboard content invalidates everything fetched for the old one.
        self.gen += 1;
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.router.fail_all();
        for w in self.waiters.drain(..) {
            (w.reply)(None);
        }
        self.paste_queue.clear();
        self.inflight = None;
        self.text = Slot::Idle;
        self.image = Slot::Idle;
        self.files = FilesState::Idle;

        let mut r = RemoteFormats::default();
        for f in formats {
            let is_files = f
                .name()
                .map(|n| n.value() == ClipboardFormatName::FILE_LIST.value())
                .unwrap_or(false);
            if is_files {
                r.file_list = Some(f.id());
            } else if f.id() == ClipboardFormatId::CF_UNICODETEXT {
                r.text = Some((f.id(), true));
            } else if (f.id() == ClipboardFormatId::CF_TEXT
                || f.id() == ClipboardFormatId::CF_OEMTEXT)
                && !matches!(r.text, Some((_, true)))
            {
                r.text = Some((f.id(), false));
            } else if f.id() == ClipboardFormatId::CF_DIB
                || (f.id() == ClipboardFormatId::CF_DIBV5 && r.image.is_none())
            {
                r.image = Some(f.id());
            }
        }
        self.remote = r.clone();

        let mut targets: Vec<String> = Vec::new();
        if r.file_list.is_some() {
            targets.extend(FILE_TARGETS.iter().map(|s| (*s).to_owned()));
            // Fetch the (small) file list right away so downloads of small selections can prefetch.
            self.files = FilesState::Listing;
            self.queue_paste(PasteKind::FileList, r.file_list.expect("checked"));
        } else {
            if r.text.is_some() {
                targets.extend(TEXT_TARGETS.iter().map(|s| (*s).to_owned()));
            }
            if r.image.is_some() {
                targets.push("image/png".into());
            }
        }
        if targets.is_empty() {
            self.transport.disown();
        } else {
            let provider = Arc::new(EngineProvider {
                tx: self.tx.clone(),
                gen: self.gen,
            });
            self.transport.own(targets, provider);
        }
    }

    fn queue_paste(&mut self, kind: PasteKind, format: ClipboardFormatId) {
        self.paste_queue.push_back(PasteJob {
            gen: self.gen,
            kind,
            format,
            started: Instant::now(),
        });
        self.pump_pastes();
    }

    /// CLIPRDR only tracks one outstanding paste request, so they are strictly serialised.
    fn pump_pastes(&mut self) {
        while self.inflight.is_none() {
            let Some(mut job) = self.paste_queue.pop_front() else {
                return;
            };
            if job.gen != self.gen {
                continue;
            }
            job.started = Instant::now();
            let fmt = job.format;
            self.inflight = Some(job);
            self.send(ClipboardMessage::SendInitiatePaste(fmt));
        }
    }

    fn on_format_data_response(&mut self, error: bool, data: Vec<u8>) {
        let Some(job) = self.inflight.take() else {
            return;
        };
        if job.gen == self.gen {
            match job.kind {
                PasteKind::Text { unicode } => {
                    self.text = if error {
                        Slot::Failed
                    } else if unicode {
                        Slot::Ready(convert::text_from_rdp(&data).into_bytes())
                    } else {
                        let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
                        let s: String = data[..end].iter().map(|&b| b as char).collect();
                        Slot::Ready(s.replace("\r\n", "\n").into_bytes())
                    };
                }
                PasteKind::Image => {
                    self.image = match (error, convert::dib_to_png(&data)) {
                        (false, Ok(png)) => Slot::Ready(png),
                        (_, Err(e)) => {
                            tracing::warn!("cannot convert remote image: {e}");
                            Slot::Failed
                        }
                        _ => Slot::Failed,
                    };
                }
                PasteKind::FileList => {
                    // Only reached on error: a good file list arrives via on_remote_file_list.
                    self.files = FilesState::Failed;
                }
            }
            self.flush_waiters();
        }
        self.pump_pastes();
    }

    fn on_remote_file_list(&mut self, files: Vec<FileDescriptor>, lock: Option<u32>) {
        let job = self.inflight.take();
        if matches!(&job, Some(j) if j.gen == self.gen && j.kind == PasteKind::FileList) {
            let total: u64 = files.iter().map(|f| f.file_size.unwrap_or(0)).sum();
            nexdesk_core::logs::file(
                nexdesk_core::logs::Level::Info,
                "Remote -> local (copied on remote)",
                files.len(),
                total,
                "Offered",
                "",
            );
            self.files = FilesState::Listed { files, lock };
            if total <= self.cfg.prefetch_limit {
                self.start_download();
            }
            self.flush_waiters();
        }
        self.pump_pastes();
    }

    fn start_download(&mut self) {
        let FilesState::Listed { files, lock } =
            std::mem::replace(&mut self.files, FilesState::Downloading)
        else {
            return;
        };
        let id = format!("g{}", self.gen);
        let staging = match StagingDir::create(&self.staging_base, &id) {
            Ok(s) => s,
            Err(e) => {
                self.files = {
                    tracing::warn!("cannot create staging directory: {e}");
                    FilesState::Failed
                };
                return;
            }
        };
        // Keep the previous generation around (a file manager may still be copying from it).
        self.staging.push_back(staging);
        while self.staging.len() > 2 {
            if let Some(old) = self.staging.pop_front() {
                let _ = old.cleanup();
            }
        }
        let ctx = DownloadCtx {
            gen: self.gen,
            files,
            lock,
            base: self.staging_base.clone(),
            id,
            sink: self.sink.clone(),
            router: self.router.clone(),
            cancel: self.cancel.clone(),
            tx: self.tx.clone(),
        };
        std::thread::Builder::new()
            .name("nexdesk-clip-download".into())
            .spawn(move || ctx.run())
            .ok();
    }

    fn on_local_request(&mut self, gen: u64, target: String, reply: Reply) {
        if gen != self.gen {
            return reply(None);
        }
        let wait = if matches!(self.remote.file_list, Some(_)) {
            FILE_WAIT
        } else {
            TEXT_WAIT
        };
        self.waiters.push(Waiter {
            target,
            reply,
            deadline: Instant::now() + wait,
        });
        self.flush_waiters();
    }

    /// Try to satisfy every waiting local paste; kick off whatever fetch is still missing.
    fn flush_waiters(&mut self) {
        let waiters = std::mem::take(&mut self.waiters);
        for w in waiters {
            match self.resolve(&w.target) {
                Resolve::Ready(data) => (w.reply)(Some(data)),
                Resolve::Failed => (w.reply)(None),
                Resolve::Pending => self.waiters.push(w),
            }
        }
    }

    fn resolve(&mut self, target: &str) -> Resolve {
        if self.remote.file_list.is_some() {
            return self.resolve_files(target);
        }
        if target == "image/png" {
            let Some(fmt) = self.remote.image else {
                return Resolve::Failed;
            };
            return match &self.image {
                Slot::Ready(d) => Resolve::Ready(d.clone()),
                Slot::Failed => Resolve::Failed,
                Slot::Pending => Resolve::Pending,
                Slot::Idle => {
                    self.image = Slot::Pending;
                    self.queue_paste(PasteKind::Image, fmt);
                    Resolve::Pending
                }
            };
        }
        if is_text_target(target) {
            let Some((fmt, unicode)) = self.remote.text else {
                return Resolve::Failed;
            };
            return match &self.text {
                Slot::Ready(d) => {
                    if target == "STRING" {
                        let s = String::from_utf8_lossy(d);
                        Resolve::Ready(
                            s.chars()
                                .map(|c| {
                                    if (c as u32) < 256 {
                                        c as u32 as u8
                                    } else {
                                        b'?'
                                    }
                                })
                                .collect(),
                        )
                    } else {
                        Resolve::Ready(d.clone())
                    }
                }
                Slot::Failed => Resolve::Failed,
                Slot::Pending => Resolve::Pending,
                Slot::Idle => {
                    self.text = Slot::Pending;
                    self.queue_paste(PasteKind::Text { unicode }, fmt);
                    Resolve::Pending
                }
            };
        }
        Resolve::Failed
    }

    fn resolve_files(&mut self, target: &str) -> Resolve {
        if !FILE_TARGETS.contains(&target) {
            return Resolve::Failed;
        }
        if matches!(self.files, FilesState::Listed { .. }) {
            // Too big to prefetch: download on first demand.
            self.start_download();
        }
        match &self.files {
            FilesState::Done(paths) => Resolve::Ready(match target {
                "x-special/gnome-copied-files" => gnome_copied_files(paths, false).into_bytes(),
                "text/uri-list" => uri_list(paths).into_bytes(),
                "application/x-kde-cutselection" => b"0".to_vec(),
                _ => paths
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("\n")
                    .into_bytes(),
            }),
            FilesState::Failed => Resolve::Failed,
            _ => Resolve::Pending,
        }
    }

    fn housekeeping(&mut self) {
        let now = Instant::now();
        if self
            .inflight
            .as_ref()
            .map(|j| now.duration_since(j.started) > TEXT_WAIT)
            .unwrap_or(false)
        {
            tracing::warn!("remote clipboard did not answer in time");
            let job = self.inflight.take().expect("checked");
            if job.gen == self.gen {
                match job.kind {
                    PasteKind::Text { .. } => self.text = Slot::Failed,
                    PasteKind::Image => self.image = Slot::Failed,
                    PasteKind::FileList => self.files = FilesState::Failed,
                }
                self.flush_waiters();
            }
            self.pump_pastes();
        }
        let (expired, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.waiters)
            .into_iter()
            .partition(|w| w.deadline < now);
        self.waiters = keep;
        for w in expired {
            (w.reply)(None);
        }
    }
}

enum Resolve {
    Ready(Vec<u8>),
    Failed,
    Pending,
}

// ------------------------------------------------------------------ downloader

struct DownloadCtx {
    gen: u64,
    files: Vec<FileDescriptor>,
    lock: Option<u32>,
    base: PathBuf,
    id: String,
    sink: Sink,
    router: Arc<StreamRouter>,
    cancel: Arc<AtomicBool>,
    tx: Sender<EngineMsg>,
}

impl DownloadCtx {
    fn run(self) {
        let result = self.download();
        let _ = self.tx.send(EngineMsg::DownloadDone {
            gen: self.gen,
            result,
        });
    }

    fn download(&self) -> Result<Vec<PathBuf>, String> {
        use ironrdp_cliprdr::pdu::ClipboardFileAttributes;
        use std::io::Write;

        // Same directory the engine created (create() is idempotent); never cleaned up from here.
        let staging = StagingDir::create(&self.base, &self.id).map_err(|e| e.to_string())?;
        let mut entries: Vec<FileEntry> = Vec::new();
        for (i, d) in self.files.iter().enumerate() {
            if self.cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            let rel = match d.relative_path.as_deref().filter(|p| !p.is_empty()) {
                Some(dir) => format!("{dir}\\{}", d.name),
                None => d.name.clone(),
            };
            let is_dir = d
                .attributes
                .map(|a| a.contains(ClipboardFileAttributes::DIRECTORY))
                .unwrap_or(false);
            let mut entry = FileEntry {
                rel_path: rel,
                size: d.file_size.unwrap_or(0),
                is_dir,
            };
            let path = match staging.prepare_entry(&entry) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!("skipping remote file {:?}: {e}", d.name);
                    continue;
                }
            };
            if is_dir {
                entries.push(entry);
                continue;
            }
            let size = match d.file_size {
                Some(s) => s,
                None => {
                    let resp = self.router.request(
                        &self.sink,
                        FileContentsRequest {
                            stream_id: 0,
                            index: i as i32,
                            flags: FileContentsFlags::SIZE,
                            position: 0,
                            requested_size: 8,
                            data_id: self.lock,
                        },
                        CHUNK_TIMEOUT,
                    )?;
                    let arr: [u8; 8] = resp
                        .get(..8)
                        .and_then(|b| b.try_into().ok())
                        .ok_or("bad size response")?;
                    u64::from_le_bytes(arr)
                }
            };
            entry.size = size;
            let mut out =
                std::fs::File::create(&path).map_err(|e| format!("cannot create {path:?}: {e}"))?;
            let mut pos = 0u64;
            while pos < size {
                if self.cancel.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                let want = (size - pos).min(CHUNK) as u32;
                let data = self.router.request(
                    &self.sink,
                    FileContentsRequest {
                        stream_id: 0,
                        index: i as i32,
                        flags: FileContentsFlags::RANGE,
                        position: pos,
                        requested_size: want,
                        data_id: self.lock,
                    },
                    CHUNK_TIMEOUT,
                )?;
                if data.is_empty() {
                    return Err(format!("unexpected end of data in {:?}", d.name));
                }
                let take = data.len().min(want as usize);
                out.write_all(&data[..take])
                    .map_err(|e| format!("write failed: {e}"))?;
                pos += take as u64;
            }
            entries.push(entry);
        }
        Ok(staging.top_level_paths(&entries))
    }
}
