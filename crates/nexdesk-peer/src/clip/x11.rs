//! Text clipboard sync over X11 (also works on Wayland desktops through XWayland).
//! Reuses the selection transport of `nexdesk-clipboard`. Clipboard contents are never logged.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nexdesk_clipboard::transport::{Provider, Reply, Transport, TransportEvent};
use nexdesk_clipboard::X11Transport;

use crate::wire::{Msg, MAX_CLIP};

const READ_TOKEN: u64 = 1;
const TEXT_TARGETS: [&str; 3] = ["UTF8_STRING", "text/plain;charset=utf-8", "text/plain"];

struct Text(Vec<u8>);
impl Provider for Text {
    fn request(&self, target: &str, reply: Reply) {
        reply(TEXT_TARGETS.contains(&target).then(|| self.0.clone()));
    }
}

pub struct ClipSync {
    transport: Arc<X11Transport>,
    last: Arc<Mutex<Option<String>>>,
    stop: Arc<AtomicBool>,
}

impl ClipSync {
    /// Watch the local clipboard; every new text is sent as `Msg::Clip` through `out`.
    pub fn start(out: Sender<Msg>) -> Result<Self, String> {
        let (ev_tx, ev_rx) = channel::<TransportEvent>();
        let ev_tx = Mutex::new(ev_tx);
        let transport = Arc::new(X11Transport::spawn(Arc::new(move |e| {
            let _ = ev_tx.lock().map(|t| t.send(e));
        }))?);
        let last: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        {
            let (transport, last, stop) = (transport.clone(), last.clone(), stop.clone());
            std::thread::Builder::new()
                .name("clip-sync".into())
                .spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        match ev_rx.recv_timeout(Duration::from_millis(500)) {
                            Ok(TransportEvent::OwnerChanged(t)) => {
                                if t.iter().any(|x| x == "UTF8_STRING") {
                                    transport.read(READ_TOKEN, "UTF8_STRING");
                                }
                            }
                            Ok(TransportEvent::ReadDone {
                                token: READ_TOKEN,
                                result: Ok(bytes),
                            }) => {
                                if bytes.is_empty() || bytes.len() > MAX_CLIP {
                                    continue;
                                }
                                let Ok(text) = String::from_utf8(bytes) else {
                                    continue;
                                };
                                let mut l = last.lock().unwrap_or_else(|e| e.into_inner());
                                if l.as_deref() != Some(text.as_str()) {
                                    *l = Some(text.clone());
                                    drop(l);
                                    if out.send(Msg::Clip(text)).is_err() {
                                        break;
                                    }
                                }
                            }
                            Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                            Err(_) => break,
                        }
                    }
                })
                .map_err(|e| e.to_string())?;
        }
        transport.query();
        Ok(Self {
            transport,
            last,
            stop,
        })
    }

    /// The other side copied `text`: make it the local clipboard.
    pub fn set_remote(&self, text: String) {
        if text.is_empty() || text.len() > MAX_CLIP {
            return;
        }
        {
            let mut l = self.last.lock().unwrap_or_else(|e| e.into_inner());
            if l.as_deref() == Some(text.as_str()) {
                return;
            }
            *l = Some(text.clone());
        }
        self.transport.own(
            TEXT_TARGETS.iter().map(|s| s.to_string()).collect(),
            Arc::new(Text(text.into_bytes())),
        );
    }
}

impl Drop for ClipSync {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
