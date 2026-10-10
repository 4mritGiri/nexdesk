//! Engine tests with a scripted fake desktop clipboard (no X server needed).
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ironrdp_cliprdr::backend::{ClipboardMessage, CliprdrBackend};
use ironrdp_cliprdr::pdu::{
    ClipboardFileAttributes, ClipboardFormat, ClipboardFormatId, ClipboardFormatName,
    ClipboardGeneralCapabilityFlags, FileContentsFlags, FileContentsRequest, FileContentsResponse,
    FileDescriptor, FormatDataRequest, FormatDataResponse,
};

use crate::transport::{EventSink, Provider, Transport, TransportEvent};
use crate::{build_backend, convert, Config, LinuxBackend, Sink};

enum Call {
    Read(u64, String),
    Own(Vec<String>, Arc<dyn Provider>),
    Disown,
    Query,
}

#[derive(Clone, Default)]
struct Calls(Arc<Mutex<Vec<Call>>>);

struct Mock(Calls);
impl Transport for Mock {
    fn read(&self, token: u64, target: &str) {
        self.0
             .0
            .lock()
            .unwrap()
            .push(Call::Read(token, target.into()));
    }
    fn own(&self, targets: Vec<String>, provider: Arc<dyn Provider>) {
        self.0 .0.lock().unwrap().push(Call::Own(targets, provider));
    }
    fn disown(&self) {
        self.0 .0.lock().unwrap().push(Call::Disown);
    }
    fn query(&self) {
        self.0 .0.lock().unwrap().push(Call::Query);
    }
    fn name(&self) -> &'static str {
        "mock"
    }
}

struct Rig {
    backend: LinuxBackend,
    calls: Calls,
    msgs: Arc<Mutex<Vec<ClipboardMessage>>>,
    events: EventSink,
    root: std::path::PathBuf,
}

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("nexdesk-clip-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn rig(tag: &str) -> Rig {
    let root = tmp(tag);
    let calls = Calls::default();
    let msgs: Arc<Mutex<Vec<ClipboardMessage>>> = Arc::default();
    let m2 = msgs.clone();
    let sink: Sink = Arc::new(move |m| m2.lock().unwrap().push(m));
    let ev_slot: Arc<Mutex<Option<EventSink>>> = Arc::default();
    let slot = ev_slot.clone();
    let c2 = calls.clone();
    let cfg = Config {
        staging_root: root.join("stage"),
        prefetch_limit: 1024 * 1024,
    };
    let backend = build_backend(cfg, sink, move |events| {
        *slot.lock().unwrap() = Some(events);
        Some(Box::new(Mock(c2)))
    });
    let events = ev_slot.lock().unwrap().take().unwrap();
    Rig {
        backend,
        calls,
        msgs,
        events,
        root,
    }
}

fn wait<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

impl Rig {
    fn wait_own(&self) -> (Vec<String>, Arc<dyn Provider>) {
        wait("transport.own", || {
            let mut g = self.calls.0.lock().unwrap();
            let i = g.iter().position(|c| matches!(c, Call::Own(..)))?;
            match g.remove(i) {
                Call::Own(t, p) => Some((t, p)),
                _ => None,
            }
        })
    }
    fn wait_read(&self) -> (u64, String) {
        wait("transport.read", || {
            let mut g = self.calls.0.lock().unwrap();
            let i = g.iter().position(|c| matches!(c, Call::Read(..)))?;
            match g.remove(i) {
                Call::Read(t, n) => Some((t, n)),
                _ => None,
            }
        })
    }
    fn take_msg<T>(&self, what: &str, mut f: impl FnMut(&ClipboardMessage) -> Option<T>) -> T {
        wait(what, || {
            let mut g = self.msgs.lock().unwrap();
            let i = g.iter().position(|m| f(m).is_some())?;
            let m = g.remove(i);
            f(&m)
        })
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn ask(p: &Arc<dyn Provider>, target: &str) -> std::sync::mpsc::Receiver<Option<Vec<u8>>> {
    let (tx, rx) = channel();
    p.request(
        target,
        Box::new(move |d| {
            let _ = tx.send(d);
        }),
    );
    rx
}

#[test]
fn remote_text_is_offered_lazily_and_cached() {
    let mut r = rig("rtext");
    r.backend
        .on_remote_copy(&[ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)]);
    let (targets, provider) = r.wait_own();
    assert!(
        targets.iter().any(|t| t == "UTF8_STRING") && !targets.iter().any(|t| t == "text/uri-list")
    );
    let rx = ask(&provider, "UTF8_STRING");
    // The paste request is only sent once somebody asks for the data.
    r.take_msg("initiate paste", |m| matches!(m, ClipboardMessage::SendInitiatePaste(f) if *f == ClipboardFormatId::CF_UNICODETEXT).then_some(()));
    r.backend
        .on_format_data_response(FormatDataResponse::new_data(convert::text_to_rdp(
            "héllo\nworld",
        )));
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap(),
        "héllo\nworld".as_bytes()
    );
    // Second request (other target) is answered from the cache without a new round trip.
    let rx = ask(&provider, "STRING");
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap(),
        b"h\xe9llo\nworld"
    );
    assert!(r.msgs.lock().unwrap().is_empty());
}

#[test]
fn stale_provider_is_refused_after_new_remote_copy() {
    let mut r = rig("stale");
    r.backend
        .on_remote_copy(&[ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)]);
    let (_, old) = r.wait_own();
    r.backend
        .on_remote_copy(&[ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)]);
    let _ = r.wait_own();
    assert_eq!(
        ask(&old, "UTF8_STRING")
            .recv_timeout(Duration::from_secs(5))
            .unwrap(),
        None
    );
}

#[test]
fn remote_clipboard_cleared_disowns() {
    let mut r = rig("clear");
    r.backend.on_remote_copy(&[]);
    wait("disown", || {
        r.calls
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|c| matches!(c, Call::Disown))
            .then_some(())
    });
}

#[test]
fn local_text_is_advertised_and_served() {
    let mut r = rig("ltext");
    (r.events)(TransportEvent::OwnerChanged(vec![
        "TARGETS".into(),
        "UTF8_STRING".into(),
        "text/plain".into(),
    ]));
    let fmts = r.take_msg("initiate copy", |m| match m {
        ClipboardMessage::SendInitiateCopy(f) if !f.is_empty() => {
            Some(f.iter().map(|x| x.id()).collect::<Vec<_>>())
        }
        _ => None,
    });
    assert_eq!(fmts, vec![ClipboardFormatId::CF_UNICODETEXT]);
    r.backend.on_format_data_request(FormatDataRequest {
        format: ClipboardFormatId::CF_UNICODETEXT,
    });
    let (token, target) = r.wait_read();
    assert_eq!(target, "UTF8_STRING");
    (r.events)(TransportEvent::ReadDone {
        token,
        result: Ok(b"line1\nline2".to_vec()),
    });
    let data = r.take_msg("format data", |m| match m {
        ClipboardMessage::SendFormatData(d) if !d.is_error() => Some(d.data().to_vec()),
        _ => None,
    });
    assert_eq!(convert::text_from_rdp(&data), "line1\nline2");
    assert_eq!(&data[data.len() - 2..], &[0, 0]);
}

#[test]
fn unknown_format_request_is_answered_with_error() {
    let mut r = rig("unk");
    (r.events)(TransportEvent::OwnerChanged(vec!["UTF8_STRING".into()]));
    r.backend.on_format_data_request(FormatDataRequest {
        format: ClipboardFormatId::new(0xC123),
    });
    r.take_msg("error response", |m| {
        matches!(m, ClipboardMessage::SendFormatData(d) if d.is_error()).then_some(())
    });
}

#[test]
fn local_image_is_converted_to_dib() {
    let mut r = rig("limg");
    let mut img = image::RgbaImage::new(2, 2);
    img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
    let mut png = std::io::Cursor::new(Vec::new());
    img.write_to(&mut png, image::ImageFormat::Png).unwrap();
    (r.events)(TransportEvent::OwnerChanged(vec!["image/png".into()]));
    r.take_msg("initiate copy dib", |m| match m {
        ClipboardMessage::SendInitiateCopy(f)
            if f.iter().any(|x| x.id() == ClipboardFormatId::CF_DIB) =>
        {
            Some(())
        }
        _ => None,
    });
    r.backend.on_format_data_request(FormatDataRequest {
        format: ClipboardFormatId::CF_DIB,
    });
    let (token, target) = r.wait_read();
    assert_eq!(target, "image/png");
    (r.events)(TransportEvent::ReadDone {
        token,
        result: Ok(png.into_inner()),
    });
    let data = r.take_msg("dib", |m| match m {
        ClipboardMessage::SendFormatData(d) if !d.is_error() => Some(d.data().to_vec()),
        _ => None,
    });
    assert_eq!(u32::from_le_bytes(data[0..4].try_into().unwrap()), 40);
}

fn file_list_format() -> ClipboardFormat {
    ClipboardFormat::new(ClipboardFormatId::new(0xC001)).with_name(ClipboardFormatName::FILE_LIST)
}

#[test]
fn remote_files_are_downloaded_and_offered_as_uris() {
    let mut r = rig("rfiles");
    r.backend.on_remote_copy(&[file_list_format()]);
    let (targets, provider) = r.wait_own();
    assert!(
        targets.iter().any(|t| t == "text/uri-list")
            && targets.iter().any(|t| t == "x-special/gnome-copied-files")
    );
    // The (tiny) file list is requested immediately.
    r.take_msg("paste file list", |m| {
        matches!(m, ClipboardMessage::SendInitiatePaste(f) if f.value() == 0xC001).then_some(())
    });

    let rx = ask(&provider, "text/uri-list");
    let files = vec![
        FileDescriptor::new("docs").with_attributes(ClipboardFileAttributes::DIRECTORY),
        FileDescriptor::new("a b.txt")
            .with_relative_path("docs")
            .with_file_size(11),
        FileDescriptor::new("big.bin").with_file_size(600_000),
    ];
    r.backend.on_remote_file_list(&files, Some(3));

    // Play the Windows side: answer RANGE requests from synthetic contents.
    let content = |idx: i32| -> Vec<u8> {
        match idx {
            1 => b"hello world".to_vec(),
            2 => (0..600_000u32).map(|i| (i % 251) as u8).collect(),
            _ => vec![],
        }
    };
    let msgs = r.msgs.clone();
    let end = Instant::now() + Duration::from_secs(20);
    let uris = loop {
        if let Ok(v) = rx.try_recv() {
            break v;
        }
        assert!(Instant::now() < end, "download did not finish");
        let reqs: Vec<FileContentsRequest> = {
            let mut g = msgs.lock().unwrap();
            let mut out = Vec::new();
            g.retain(|m| match m {
                ClipboardMessage::SendFileContentsRequest(q) => {
                    out.push(q.clone());
                    false
                }
                _ => true,
            });
            out
        };
        for q in reqs {
            assert_eq!(
                q.data_id,
                Some(3),
                "requests must carry the clipboard lock id"
            );
            assert!(q.flags.contains(FileContentsFlags::RANGE));
            let c = content(q.index);
            let start = (q.position as usize).min(c.len());
            let stop = (start + q.requested_size as usize).min(c.len());
            r.backend
                .on_file_contents_response(FileContentsResponse::new_data_response(
                    q.stream_id,
                    c[start..stop].to_vec(),
                ));
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let uris = String::from_utf8(uris.expect("uri list")).unwrap();
    // Only top-level entries are published (the file manager recurses into folders).
    assert_eq!(uris.lines().count(), 2, "{uris}");
    assert!(uris.contains("/docs\r\n") && uris.contains("/big.bin\r\n"));
    let docs = r
        .root
        .join("stage")
        .join(format!("p{}", std::process::id()))
        .join("g1")
        .join("docs");
    assert_eq!(std::fs::read(docs.join("a b.txt")).unwrap(), b"hello world");
    assert_eq!(
        std::fs::read(docs.parent().unwrap().join("big.bin")).unwrap(),
        content(2)
    );
    // The GNOME flavour is answered from the same download.
    let g = ask(&provider, "x-special/gnome-copied-files")
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert!(String::from_utf8(g).unwrap().starts_with("copy\nfile://"));
}

#[test]
fn hostile_remote_paths_cannot_escape_staging() {
    let mut r = rig("evil");
    r.backend.on_remote_copy(&[file_list_format()]);
    let (_, provider) = r.wait_own();
    let rx = ask(&provider, "text/uri-list");
    let files = vec![
        FileDescriptor::new("..\\..\\..\\pwned.txt").with_file_size(1),
        FileDescriptor::new("ok.txt").with_file_size(1),
    ];
    r.backend.on_remote_file_list(&files, None);
    let end = Instant::now() + Duration::from_secs(20);
    let result = loop {
        if let Ok(v) = rx.try_recv() {
            break v;
        }
        assert!(Instant::now() < end);
        let reqs: Vec<FileContentsRequest> = {
            let mut g = r.msgs.lock().unwrap();
            let mut out = Vec::new();
            g.retain(|m| match m {
                ClipboardMessage::SendFileContentsRequest(q) => {
                    out.push(q.clone());
                    false
                }
                _ => true,
            });
            out
        };
        for q in reqs {
            r.backend
                .on_file_contents_response(FileContentsResponse::new_data_response(
                    q.stream_id,
                    vec![b'x'],
                ));
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let _ = result;
    assert!(!r.root.join("pwned.txt").exists());
    assert!(!r.root.parent().unwrap().join("pwned.txt").exists());
}

#[test]
fn local_files_are_advertised_and_streamed() {
    let mut r = rig("lfiles");
    r.backend.on_process_negotiated_capabilities(
        ClipboardGeneralCapabilityFlags::STREAM_FILECLIP_ENABLED
            | ClipboardGeneralCapabilityFlags::CAN_LOCK_CLIPDATA,
    );
    std::fs::create_dir_all(r.root.join("src/inner")).unwrap();
    std::fs::write(r.root.join("src/one.txt"), b"abcdefghij").unwrap();
    std::fs::write(r.root.join("src/inner/two.txt"), b"xyz").unwrap();
    let uri = format!(
        "copy\nfile://{}",
        crate::convert::percent_decode("")
            .len()
            .to_string()
            .replace('0', "")
            + &r.root.join("src").to_string_lossy()
    );
    (r.events)(TransportEvent::OwnerChanged(vec![
        "x-special/gnome-copied-files".into(),
        "text/uri-list".into(),
    ]));
    let (token, target) = r.wait_read();
    assert_eq!(target, "x-special/gnome-copied-files");
    (r.events)(TransportEvent::ReadDone {
        token,
        result: Ok(uri.into_bytes()),
    });
    let descs = r.take_msg("file copy", |m| match m {
        ClipboardMessage::SendInitiateFileCopy(d) => Some(d.clone()),
        _ => None,
    });
    let names: Vec<_> = descs
        .iter()
        .map(|d| (d.name.as_str(), d.relative_path.as_deref()))
        .collect();
    assert_eq!(
        names,
        vec![
            ("src", None),
            ("inner", Some("src")),
            ("two.txt", Some("src\\inner")),
            ("one.txt", Some("src"))
        ]
    );
    // Server pulls "one.txt" (index 3).
    r.backend.on_lock(ironrdp_cliprdr::pdu::LockDataId(5));
    r.backend.on_file_contents_request(FileContentsRequest {
        stream_id: 77,
        index: 3,
        flags: FileContentsFlags::RANGE,
        position: 2,
        requested_size: 5,
        data_id: Some(5),
    });
    let data = r.take_msg("file data", |m| match m {
        ClipboardMessage::SendFileContentsResponse(d) if d.stream_id() == 77 => {
            Some(d.data().to_vec())
        }
        _ => None,
    });
    assert_eq!(data, b"cdefg");
}

#[test]
fn dropped_files_are_offered_through_the_handle() {
    let mut r = rig("drop");
    r.backend.on_process_negotiated_capabilities(
        ClipboardGeneralCapabilityFlags::STREAM_FILECLIP_ENABLED,
    );
    std::fs::write(r.root.join("dropped.txt"), b"hi").unwrap();
    r.backend
        .handle
        .tx
        .send(crate::engine::EngineMsg::OfferFiles(vec![r
            .root
            .join("dropped.txt")]))
        .unwrap();
    let descs = r.take_msg("file copy", |m| match m {
        ClipboardMessage::SendInitiateFileCopy(d) => Some(d.clone()),
        _ => None,
    });
    assert_eq!(descs.len(), 1);
    assert_eq!(descs[0].name, "dropped.txt");
}

#[test]
fn file_offer_without_server_support_falls_back_gracefully() {
    let mut r = rig("nofc");
    // STREAM_FILECLIP not negotiated: the server cannot do file transfer.
    std::fs::write(r.root.join("f.txt"), b"hi").unwrap();
    r.backend
        .handle
        .tx
        .send(crate::engine::EngineMsg::OfferFiles(vec![r
            .root
            .join("f.txt")]))
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(!r
        .msgs
        .lock()
        .unwrap()
        .iter()
        .any(|m| matches!(m, ClipboardMessage::SendInitiateFileCopy(_))));
}

#[test]
fn initial_empty_format_list_encodes_with_the_real_cliprdr_channel() {
    // The CLIPRDR layer needs one FormatList to leave its Initialization state; make sure the
    // empty list our backend sends at start-up is accepted and encodes.
    use ironrdp_cliprdr::{Client, Cliprdr};
    let (sink, _msgs): (Sink, Arc<Mutex<Vec<ClipboardMessage>>>) = {
        let m: Arc<Mutex<Vec<ClipboardMessage>>> = Arc::default();
        let m2 = m.clone();
        (Arc::new(move |x| m2.lock().unwrap().push(x)), m)
    };
    let backend = build_backend(
        Config {
            staging_root: tmp("init").join("s"),
            prefetch_limit: 0,
        },
        sink,
        |_| Some(Box::new(Mock(Calls::default()))),
    );
    let mut ch: Cliprdr<Client> = Cliprdr::new(Box::new(backend));
    assert!(ch.initiate_copy(&[]).is_ok());
}
