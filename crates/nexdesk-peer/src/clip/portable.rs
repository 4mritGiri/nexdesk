//! Clipboard text sync on Windows and macOS: the system clipboard is polled twice a second.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::wire::{Msg, MAX_CLIP};

pub struct ClipSync {
    last: Arc<Mutex<Option<String>>>,
    set: Sender<String>,
    stop: Arc<AtomicBool>,
}

impl ClipSync {
    /// Watch the local clipboard; every new text is sent as `Msg::Clip` through `out`.
    pub fn start(out: Sender<Msg>) -> Result<Self, String> {
        let last: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let (set, set_rx) = channel::<String>();
        let (ready_tx, ready_rx) = channel::<Result<(), String>>();
        {
            let (last, stop) = (last.clone(), stop.clone());
            std::thread::Builder::new()
                .name("clip-sync".into())
                .spawn(move || {
                    let mut cb = match arboard::Clipboard::new() {
                        Ok(c) => {
                            let _ = ready_tx.send(Ok(()));
                            c
                        }
                        Err(e) => {
                            let _ = ready_tx.send(Err(e.to_string()));
                            return;
                        }
                    };
                    while !stop.load(Ordering::Relaxed) {
                        while let Ok(text) = set_rx.try_recv() {
                            let _ = cb.set_text(text);
                        }
                        if let Ok(text) = cb.get_text() {
                            if !text.is_empty() && text.len() <= MAX_CLIP {
                                let mut l = last.lock().unwrap_or_else(|e| e.into_inner());
                                if l.as_deref() != Some(text.as_str()) {
                                    *l = Some(text.clone());
                                    drop(l);
                                    if out.send(Msg::Clip(text)).is_err() {
                                        break;
                                    }
                                }
                            }
                        }
                        std::thread::sleep(Duration::from_millis(500));
                    }
                })
                .map_err(|e| e.to_string())?;
        }
        ready_rx.recv().map_err(|e| e.to_string())??;
        Ok(Self { last, set, stop })
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
        let _ = self.set.send(text);
    }
}

impl Drop for ClipSync {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
