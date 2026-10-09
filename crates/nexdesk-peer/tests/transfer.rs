//! File transfer and chat through a real agent session. Run under xvfb-run; skipped without DISPLAY.
use std::net::TcpListener;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use nexdesk_crypto::Identity;
use nexdesk_peer::host::{run, Policy};
use nexdesk_peer::xfer;
use nexdesk_peer::{client, Msg};

fn wait_for(mut f: impl FnMut() -> bool, secs: u64) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

struct Session {
    out: mpsc::Sender<Msg>,
    events: mpsc::Receiver<xfer::Event>,
    chat: mpsc::Receiver<String>,
}

/// Connect a viewer: one thread writes whatever is queued, one routes what the agent says.
fn connect(addr: &str, viewer: Identity, agent: nexdesk_crypto::IdentityPublic) -> Session {
    let pinned = agent.fingerprint();
    let (mut reader, mut writer, _) = client::connect(addr, viewer, |p| p.fingerprint() == pinned).unwrap();
    let (out, rx) = mpsc::channel::<Msg>();
    let (etx, events) = mpsc::channel();
    let (ctx, chat) = mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(m) = rx.recv() {
            if writer.send(&m).is_err() {
                break;
            }
        }
    });
    std::thread::spawn(move || {
        while let Ok(m) = reader.recv() {
            match m {
                Msg::FilesAnswer { accept, .. } => drop(etx.send(xfer::Event::Answer(accept))),
                Msg::FileAck { id, bytes } => drop(etx.send(xfer::Event::Ack { id, bytes })),
                Msg::FileAbort { reason, .. } => drop(etx.send(xfer::Event::Abort(reason))),
                Msg::Chat(t) => drop(ctx.send(t)),
                _ => {}
            }
        }
    });
    Session { out, events, chat }
}

#[test]
fn files_and_chat_between_viewer_and_agent() {
    if std::env::var_os("DISPLAY").is_none() {
        eprintln!("no DISPLAY, skipping");
        return;
    }
    let tmp = std::env::temp_dir().join(format!("nexdesk-transfer-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (src, dest) = (tmp.join("src"), tmp.join("dest"));
    std::fs::create_dir_all(src.join("folder/sub")).unwrap();
    std::fs::create_dir_all(&dest).unwrap();
    let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(src.join("folder/big.bin"), &big).unwrap();
    std::fs::write(src.join("folder/sub/note.txt"), b"hello").unwrap();
    std::fs::write(src.join("folder/empty"), b"").unwrap();

    let viewer = Identity::generate().unwrap();
    let vfp = viewer.public().fingerprint_string();
    let answer = Arc::new(AtomicBool::new(true));
    let asked = Arc::new(Mutex::new(Vec::<String>::new()));
    let said = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut policy = Policy::new(false, false, vec![vfp], None);
    policy.download_dir = Some(dest.clone());
    {
        let (answer, asked) = (answer.clone(), asked.clone());
        policy.files = Some(Arc::new(move |s: &str| {
            asked.lock().unwrap().push(s.to_string());
            answer.load(std::sync::atomic::Ordering::SeqCst)
        }));
        let said = said.clone();
        policy.on_chat = Some(Arc::new(move |t: &str| said.lock().unwrap().push(t.to_string())));
    }
    let policy = Arc::new(policy);
    let id = Identity::generate().unwrap();
    let agent_pub = id.public().clone();
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let p2 = policy.clone();
    std::thread::spawn(move || run(l, Arc::new(id), p2, |_| {}));
    let s = connect(&addr, viewer, agent_pub);

    // chat both ways
    assert!(wait_for(|| policy.send_chat("hi from the host \u{1F600}"), 5), "session never became ready");
    assert_eq!(s.chat.recv_timeout(Duration::from_secs(5)).unwrap(), "hi from the host \u{1F600}");
    s.out.send(Msg::Chat("hello host".into())).unwrap();
    assert!(wait_for(|| said.lock().unwrap().iter().any(|t| t == "hello host"), 5));

    // declined: nothing is written
    answer.store(false, std::sync::atomic::Ordering::SeqCst);
    let items = xfer::collect(&[src.join("folder")]).unwrap();
    assert_eq!(items.len(), 3);
    let cancel = AtomicBool::new(false);
    let r = xfer::send(1, &items, &s.out, &s.events, &|_, _| {}, &cancel);
    assert!(r.is_err(), "declined transfer must fail: {r:?}");
    assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 0, "declined files must not appear");

    // accepted: folder structure and contents arrive intact
    answer.store(true, std::sync::atomic::Ordering::SeqCst);
    let r = xfer::send(2, &items, &s.out, &s.events, &|_, _| {}, &cancel);
    assert_eq!(r, Ok(3));
    assert_eq!(std::fs::read(dest.join("folder/big.bin")).unwrap(), big);
    assert_eq!(std::fs::read(dest.join("folder/sub/note.txt")).unwrap(), b"hello");
    assert_eq!(std::fs::read(dest.join("folder/empty")).unwrap(), b"");
    let first = asked.lock().unwrap().last().unwrap().clone();
    assert!(first.starts_with("3|3000005|"), "{first}");

    // sending again never overwrites
    assert_eq!(xfer::send(3, &items, &s.out, &s.events, &|_, _| {}, &cancel), Ok(3));
    assert!(dest.join("folder/big (1).bin").exists());
    assert_eq!(std::fs::read(dest.join("folder/big.bin")).unwrap(), big);

    // a hostile path is refused by the agent even if a modified viewer sends it
    s.out.send(Msg::FilesOffer { batch: 9, count: 1, total: 1, first: "x".into() }).unwrap();
    assert!(matches!(s.events.recv_timeout(Duration::from_secs(5)).unwrap(), xfer::Event::Answer(true)));
    s.out.send(Msg::FileStart { batch: 9, id: 0, size: 1, path: "../../escape".into() }).unwrap();
    assert!(matches!(s.events.recv_timeout(Duration::from_secs(5)).unwrap(), xfer::Event::Abort(_)));
    assert!(!tmp.join("escape").exists() && !tmp.parent().unwrap().join("escape").exists());
    let leftovers = std::fs::read_dir(dest.join("folder")).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().contains("nexdesk-part")).count();
    assert_eq!(leftovers, 0);
    let _ = std::fs::remove_dir_all(&tmp);
}
