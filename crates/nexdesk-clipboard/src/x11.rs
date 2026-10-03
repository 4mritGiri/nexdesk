//! X11 clipboard transport (also serves Wayland desktops through XWayland, which both
//! GNOME/Mutter and KDE/KWin bridge to the Wayland clipboard).
//!
//! One worker thread owns an X connection and a hidden window. Other threads talk to it
//! through a command queue plus a ClientMessage "wake-up" event, so it can sit in
//! `wait_for_event()` with no polling. Implements the ICCCM selection protocol including
//! INCR transfers in both directions (needed for big text and images).
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xfixes::{self, SelectionEventMask};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask,
    PropMode, Property, SelectionNotifyEvent, SelectionRequestEvent, Window, WindowClass, SELECTION_NOTIFY_EVENT,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_DEPTH_FROM_PARENT, CURRENT_TIME, NONE};

use crate::transport::{EventSink, Provider, Transport, TransportEvent};

const READ_TIMEOUT: Duration = Duration::from_secs(10);
const INCR_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_READ_BYTES: usize = 256 * 1024 * 1024;

enum Cmd {
    Read { token: u64, target: String },
    Own { targets: Vec<String>, provider: Arc<dyn Provider> },
    Disown,
    Query,
    Serve { req: SelectionRequestEvent, prop: Atom, data: Option<Vec<u8>> },
    Tick,
}

struct Shared {
    conn: RustConnection,
    win: Window,
    wake: Atom,
    cmds: Mutex<VecDeque<Cmd>>,
    stop: AtomicBool,
}

impl Shared {
    fn push(&self, c: Cmd) {
        if let Ok(mut q) = self.cmds.lock() {
            q.push_back(c);
        }
        self.wake();
    }
    fn wake(&self) {
        let ev = ClientMessageEvent::new(32, self.win, self.wake, [0u32; 5]);
        let _ = self.conn.send_event(false, self.win, EventMask::NO_EVENT, ev);
        let _ = self.conn.flush();
    }
}

pub struct X11Transport {
    shared: Arc<Shared>,
}

impl X11Transport {
    /// Connects to `$DISPLAY` and starts the worker. Fails (cleanly) when there is no X server.
    pub fn spawn(events: EventSink) -> Result<Self, String> {
        let (conn, screen_num) = RustConnection::connect(None).map_err(|e| format!("cannot connect to X11: {e}"))?;
        let root = conn.setup().roots[screen_num].root;
        let win = conn.generate_id().map_err(|e| e.to_string())?;
        conn.create_window(
            COPY_DEPTH_FROM_PARENT,
            win,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )
        .map_err(|e| e.to_string())?;
        let intern = |n: &str| -> Result<Atom, String> {
            Ok(conn.intern_atom(false, n.as_bytes()).map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?.atom)
        };
        let atoms = Atoms {
            clipboard: intern("CLIPBOARD")?,
            targets: intern("TARGETS")?,
            incr: intern("INCR")?,
            multiple: intern("MULTIPLE")?,
            timestamp: intern("TIMESTAMP")?,
            utf8: intern("UTF8_STRING")?,
            string: intern("STRING")?,
            text: intern("TEXT")?,
        };
        let wake = intern("NEXDESK_WAKE")?;
        xfixes::query_version(&conn, 5, 0)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| format!("XFixes unavailable: {e}"))?;
        xfixes::select_selection_input(&conn, win, atoms.clipboard, SelectionEventMask::SET_SELECTION_OWNER)
            .map_err(|e| e.to_string())?;
        conn.flush().map_err(|e| e.to_string())?;

        let shared = Arc::new(Shared { conn, win, wake, cmds: Mutex::new(VecDeque::new()), stop: AtomicBool::new(false) });
        let max_req = shared.conn.maximum_request_bytes();
        let incr_chunk = max_req.saturating_sub(1024).clamp(4096, 512 * 1024);

        let worker = Worker {
            shared: shared.clone(),
            atoms,
            events,
            names: HashMap::new(),
            by_name: HashMap::new(),
            reads: HashMap::new(),
            next_prop: 0,
            probe_seq: 0,
            owned: None,
            incr_out: HashMap::new(),
            incr_chunk,
        };
        std::thread::Builder::new()
            .name("nexdesk-clip-x11".into())
            .spawn(move || worker.run())
            .map_err(|e| e.to_string())?;

        // Ticker: expires stuck transfers without needing a poll loop in the worker.
        let t = shared.clone();
        std::thread::Builder::new()
            .name("nexdesk-clip-tick".into())
            .spawn(move || {
                while !t.stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(500));
                    t.push(Cmd::Tick);
                }
            })
            .map_err(|e| e.to_string())?;

        Ok(Self { shared })
    }
}

impl Drop for X11Transport {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.push(Cmd::Disown);
        self.shared.wake();
    }
}

impl Transport for X11Transport {
    fn read(&self, token: u64, target: &str) {
        self.shared.push(Cmd::Read { token, target: target.to_owned() });
    }
    fn own(&self, targets: Vec<String>, provider: Arc<dyn Provider>) {
        self.shared.push(Cmd::Own { targets, provider });
    }
    fn disown(&self) {
        self.shared.push(Cmd::Disown);
    }
    fn query(&self) {
        self.shared.push(Cmd::Query);
    }
    fn name(&self) -> &'static str {
        "x11"
    }
}

#[derive(Clone, Copy)]
struct Atoms {
    clipboard: Atom,
    targets: Atom,
    incr: Atom,
    multiple: Atom,
    timestamp: Atom,
    utf8: Atom,
    string: Atom,
    text: Atom,
}

enum ReadKind {
    Targets { seq: u64 },
    Data { token: u64 },
}

struct ReadState {
    kind: ReadKind,
    target: Atom,
    deadline: Instant,
    incr: Option<Vec<u8>>,
    waiting_conversion: bool,
}

struct Owned {
    targets: Vec<(String, Atom)>,
    provider: Arc<dyn Provider>,
}

struct IncrOut {
    data: Vec<u8>,
    offset: usize,
    type_: Atom,
    deadline: Instant,
}

struct Worker {
    shared: Arc<Shared>,
    atoms: Atoms,
    events: EventSink,
    names: HashMap<Atom, String>,
    by_name: HashMap<String, Atom>,
    reads: HashMap<Atom, ReadState>,
    next_prop: u32,
    probe_seq: u64,
    owned: Option<Owned>,
    incr_out: HashMap<(Window, Atom), IncrOut>,
    incr_chunk: usize,
}

impl Worker {
    fn conn(&self) -> &RustConnection {
        &self.shared.conn
    }

    fn atom(&mut self, name: &str) -> Option<Atom> {
        if let Some(a) = self.by_name.get(name) {
            return Some(*a);
        }
        let a = self.conn().intern_atom(false, name.as_bytes()).ok()?.reply().ok()?.atom;
        self.by_name.insert(name.to_owned(), a);
        self.names.insert(a, name.to_owned());
        Some(a)
    }

    fn atom_name(&mut self, a: Atom) -> Option<String> {
        if let Some(n) = self.names.get(&a) {
            return Some(n.clone());
        }
        let n = String::from_utf8(self.conn().get_atom_name(a).ok()?.reply().ok()?.name).ok()?;
        self.names.insert(a, n.clone());
        self.by_name.insert(n.clone(), a);
        Some(n)
    }

    fn run(mut self) {
        loop {
            let ev = match self.shared.conn.wait_for_event() {
                Ok(e) => e,
                Err(e) => {
                    tracing::debug!("x11 clipboard connection closed: {e}");
                    return;
                }
            };
            self.handle_event(ev);
            // Drain anything else that is already queued before blocking again.
            while let Ok(Some(ev)) = self.shared.conn.poll_for_event() {
                self.handle_event(ev);
            }
            if self.shared.stop.load(Ordering::Relaxed) {
                self.disown();
                let _ = self.shared.conn.flush();
                return;
            }
            let _ = self.shared.conn.flush();
        }
    }

    fn handle_event(&mut self, ev: Event) {
        match ev {
            Event::ClientMessage(m) if m.type_ == self.shared.wake => self.drain_cmds(),
            Event::XfixesSelectionNotify(e) if e.selection == self.atoms.clipboard => {
                if e.owner == self.shared.win {
                    return; // our own SetSelectionOwner
                }
                self.probe_owner(e.owner);
            }
            Event::SelectionNotify(e) => self.on_selection_notify(e.target, e.property),
            Event::SelectionRequest(e) => self.on_selection_request(e),
            Event::SelectionClear(e) if e.selection == self.atoms.clipboard => {
                self.owned = None;
                self.incr_out.clear();
            }
            Event::PropertyNotify(e) => match e.state {
                Property::NEW_VALUE => self.on_incr_in(e.atom),
                Property::DELETE => self.on_incr_out(e.window, e.atom),
                _ => {}
            },
            Event::Error(e) => tracing::trace!("x11 clipboard error event: {e:?}"),
            _ => {}
        }
    }

    fn drain_cmds(&mut self) {
        loop {
            let cmd = match self.shared.cmds.lock() {
                Ok(mut q) => q.pop_front(),
                Err(_) => None,
            };
            let Some(cmd) = cmd else { break };
            match cmd {
                Cmd::Read { token, target } => self.start_read(ReadKind::Data { token }, &target, true),
                Cmd::Own { targets, provider } => self.own(targets, provider),
                Cmd::Disown => self.disown(),
                Cmd::Query => {
                    if let Ok(r) = self.conn().get_selection_owner(self.atoms.clipboard).map(|c| c.reply()) {
                        if let Ok(r) = r {
                            if r.owner != self.shared.win {
                                self.probe_owner(r.owner);
                            }
                        }
                    }
                }
                Cmd::Serve { req, prop, data } => self.serve(req, prop, data),
                Cmd::Tick => self.expire(),
            }
        }
    }

    // ------------------------------------------------------------ reading

    fn probe_owner(&mut self, owner: Window) {
        self.probe_seq += 1;
        if owner == NONE {
            (self.events)(TransportEvent::OwnerChanged(Vec::new()));
            return;
        }
        let seq = self.probe_seq;
        self.start_read(ReadKind::Targets { seq }, "TARGETS", false);
    }

    fn start_read(&mut self, kind: ReadKind, target_name: &str, is_data: bool) {
        let fail = |this: &mut Self, kind: ReadKind, why: &str| match kind {
            ReadKind::Data { token } => (this.events)(TransportEvent::ReadDone { token, result: Err(why.to_owned()) }),
            ReadKind::Targets { .. } => (this.events)(TransportEvent::OwnerChanged(Vec::new())),
        };
        let Some(target) = self.atom(target_name) else {
            return fail(self, kind, "cannot intern target");
        };
        self.next_prop = self.next_prop.wrapping_add(1);
        let Some(prop) = self.atom(&format!("NEXDESK_READ_{}", self.next_prop % 64)) else {
            return fail(self, kind, "cannot intern property");
        };
        if self.reads.contains_key(&prop) {
            return fail(self, kind, "too many concurrent reads");
        }
        let _ = is_data;
        let ok = self
            .conn()
            .convert_selection(self.shared.win, self.atoms.clipboard, target, prop, CURRENT_TIME)
            .is_ok();
        if !ok {
            return fail(self, kind, "convert_selection failed");
        }
        self.reads.insert(
            prop,
            ReadState { kind, target, deadline: Instant::now() + READ_TIMEOUT, incr: None, waiting_conversion: true },
        );
    }

    fn on_selection_notify(&mut self, target: Atom, property: Atom) {
        if property == NONE {
            // Conversion refused: fail the oldest read waiting on this target.
            let key = self
                .reads
                .iter()
                .filter(|(_, r)| r.waiting_conversion && r.target == target)
                .min_by_key(|(_, r)| r.deadline)
                .map(|(p, _)| *p);
            if let Some(p) = key {
                self.finish_read(p, Err("owner refused the conversion".into()));
            }
            return;
        }
        let Some(rs) = self.reads.get_mut(&property) else { return };
        rs.waiting_conversion = false;
        let reply = self.shared.conn.get_property(true, self.shared.win, property, AtomEnum::ANY, 0, u32::MAX / 4);
        let Ok(Ok(reply)) = reply.map(|c| c.reply()) else {
            return self.finish_read(property, Err("cannot read property".into()));
        };
        if reply.type_ == self.atoms.incr {
            if let Some(rs) = self.reads.get_mut(&property) {
                rs.incr = Some(Vec::new());
                rs.deadline = Instant::now() + INCR_TIMEOUT;
            }
            return; // deleting the property (done above) tells the owner to start sending
        }
        self.finish_read(property, Ok((reply.value, reply.type_)));
    }

    fn on_incr_in(&mut self, prop: Atom) {
        let Some(rs) = self.reads.get_mut(&prop) else { return };
        if rs.incr.is_none() {
            return;
        }
        let reply = self.shared.conn.get_property(true, self.shared.win, prop, AtomEnum::ANY, 0, u32::MAX / 4);
        let Ok(Ok(reply)) = reply.map(|c| c.reply()) else {
            return self.finish_read(prop, Err("cannot read INCR chunk".into()));
        };
        let rs = self.reads.get_mut(&prop).expect("checked above");
        if reply.value.is_empty() {
            let data = rs.incr.take().unwrap_or_default();
            return self.finish_read(prop, Ok((data, reply.type_)));
        }
        let buf = rs.incr.as_mut().expect("incr buffer");
        if buf.len() + reply.value.len() > MAX_READ_BYTES {
            return self.finish_read(prop, Err("clipboard data too large".into()));
        }
        buf.extend_from_slice(&reply.value);
        rs.deadline = Instant::now() + INCR_TIMEOUT;
    }

    fn finish_read(&mut self, prop: Atom, result: Result<(Vec<u8>, Atom), String>) {
        let Some(rs) = self.reads.remove(&prop) else { return };
        match rs.kind {
            ReadKind::Data { token } => (self.events)(TransportEvent::ReadDone { token, result: result.map(|(d, _)| d) }),
            ReadKind::Targets { seq } => {
                if seq != self.probe_seq {
                    return; // a newer owner change superseded this probe
                }
                let mut names = Vec::new();
                if let Ok((data, _)) = result {
                    for c in data.chunks_exact(4) {
                        let a = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                        if let Some(n) = self.atom_name(a) {
                            names.push(n);
                        }
                    }
                }
                (self.events)(TransportEvent::OwnerChanged(names));
            }
        }
    }

    fn expire(&mut self) {
        let now = Instant::now();
        let dead: Vec<Atom> = self.reads.iter().filter(|(_, r)| r.deadline < now).map(|(p, _)| *p).collect();
        for p in dead {
            self.finish_read(p, Err("timed out waiting for the clipboard owner".into()));
        }
        self.incr_out.retain(|_, s| s.deadline >= now);
    }

    // ------------------------------------------------------------ owning

    fn own(&mut self, targets: Vec<String>, provider: Arc<dyn Provider>) {
        let mut v = Vec::new();
        for t in targets {
            if let Some(a) = self.atom(&t) {
                v.push((t, a));
            }
        }
        self.incr_out.clear();
        self.owned = Some(Owned { targets: v, provider });
        let _ = self.conn().set_selection_owner(self.shared.win, self.atoms.clipboard, CURRENT_TIME);
        let _ = self.conn().flush();
    }

    fn disown(&mut self) {
        if self.owned.take().is_some() {
            if let Ok(Ok(r)) = self.conn().get_selection_owner(self.atoms.clipboard).map(|c| c.reply()) {
                if r.owner == self.shared.win {
                    let _ = self.conn().set_selection_owner(NONE, self.atoms.clipboard, CURRENT_TIME);
                }
            }
        }
        self.incr_out.clear();
    }

    fn notify(&self, req: &SelectionRequestEvent, property: Atom) {
        let ev = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: req.time,
            requestor: req.requestor,
            selection: req.selection,
            target: req.target,
            property,
        };
        let _ = self.conn().send_event(false, req.requestor, EventMask::NO_EVENT, ev);
        let _ = self.conn().flush();
    }

    fn on_selection_request(&mut self, req: SelectionRequestEvent) {
        let prop = if req.property == NONE { req.target } else { req.property };
        let Some(owned) = self.owned.as_ref() else {
            return self.notify(&req, NONE);
        };
        if req.selection != self.atoms.clipboard || req.requestor == self.shared.win {
            return self.notify(&req, NONE);
        }
        if req.target == self.atoms.targets {
            let mut list: Vec<u32> = vec![self.atoms.targets, self.atoms.timestamp];
            list.extend(owned.targets.iter().map(|(_, a)| *a));
            let ok = self
                .conn()
                .change_property32(PropMode::REPLACE, req.requestor, prop, AtomEnum::ATOM, &list)
                .is_ok();
            return self.notify(&req, if ok { prop } else { NONE });
        }
        if req.target == self.atoms.timestamp || req.target == self.atoms.multiple {
            return self.notify(&req, NONE);
        }
        let Some((name, _)) = owned.targets.iter().find(|(_, a)| *a == req.target).cloned() else {
            return self.notify(&req, NONE);
        };
        let provider = owned.provider.clone();
        let shared = self.shared.clone();
        provider.request(
            &name,
            Box::new(move |data| {
                shared.push(Cmd::Serve { req, prop, data });
            }),
        );
    }

    fn serve(&mut self, req: SelectionRequestEvent, prop: Atom, data: Option<Vec<u8>>) {
        // The clipboard may have been replaced while the provider was working.
        let still_ours = self.owned.as_ref().map(|o| o.targets.iter().any(|(_, a)| *a == req.target)).unwrap_or(false);
        let Some(data) = data.filter(|_| still_ours) else {
            return self.notify(&req, NONE);
        };
        // Data type: text targets use their natural type, MIME targets use their own atom.
        let type_ = if req.target == self.atoms.text { self.atoms.utf8 } else { req.target };
        if data.len() > self.incr_chunk {
            let _ = self.conn().change_window_attributes(
                req.requestor,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            );
            let len = u32::try_from(data.len()).unwrap_or(u32::MAX);
            let ok = self
                .conn()
                .change_property32(PropMode::REPLACE, req.requestor, prop, self.atoms.incr, &[len])
                .is_ok();
            if !ok {
                return self.notify(&req, NONE);
            }
            self.incr_out.insert(
                (req.requestor, prop),
                IncrOut { data, offset: 0, type_, deadline: Instant::now() + INCR_TIMEOUT },
            );
            return self.notify(&req, prop);
        }
        let ok = self.conn().change_property8(PropMode::REPLACE, req.requestor, prop, type_, &data).is_ok();
        self.notify(&req, if ok { prop } else { NONE });
    }

    fn on_incr_out(&mut self, window: Window, prop: Atom) {
        let Some(s) = self.incr_out.get_mut(&(window, prop)) else { return };
        let end = (s.offset + self.incr_chunk).min(s.data.len());
        let chunk = s.data[s.offset..end].to_vec(); // empty chunk terminates the transfer
        s.offset = end;
        s.deadline = Instant::now() + INCR_TIMEOUT;
        let type_ = s.type_;
        let done = chunk.is_empty();
        let _ = self.shared.conn.change_property8(PropMode::REPLACE, window, prop, type_, &chunk);
        let _ = self.shared.conn.flush();
        if done {
            self.incr_out.remove(&(window, prop));
        }
    }
}

#[allow(dead_code)]
fn _atoms_used(a: &Atoms) -> (Atom, Atom) {
    (a.string, a.utf8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{channel, Receiver};

    struct Fixed(Vec<u8>);
    impl Provider for Fixed {
        fn request(&self, target: &str, reply: crate::transport::Reply) {
            if target == "NOPE" {
                reply(None)
            } else {
                reply(Some(self.0.clone()))
            }
        }
    }

    fn spawn() -> Option<(X11Transport, Receiver<TransportEvent>)> {
        std::env::var_os("DISPLAY")?;
        let (tx, rx) = channel();
        let tx = Mutex::new(tx);
        let t = X11Transport::spawn(Arc::new(move |e| {
            let _ = tx.lock().unwrap().send(e);
        }))
        .ok()?;
        Some((t, rx))
    }

    fn next_owner(rx: &Receiver<TransportEvent>) -> Vec<String> {
        loop {
            match rx.recv_timeout(Duration::from_secs(5)).expect("no OwnerChanged") {
                TransportEvent::OwnerChanged(t) if !t.is_empty() => return t,
                _ => {}
            }
        }
    }

    fn read(t: &X11Transport, rx: &Receiver<TransportEvent>, target: &str) -> Result<Vec<u8>, String> {
        t.read(42, target);
        loop {
            if let TransportEvent::ReadDone { token: 42, result } = rx.recv_timeout(Duration::from_secs(15)).expect("no ReadDone") {
                return result;
            }
        }
    }

    #[test]
    fn small_text_between_two_clients() {
        let (Some((a, _arx)), Some((b, brx))) = (spawn(), spawn()) else {
            eprintln!("skipped: no X server (run under xvfb-run)");
            return;
        };
        a.own(vec!["UTF8_STRING".into(), "text/uri-list".into(), "NOPE".into()], Arc::new(Fixed("héllo".as_bytes().to_vec())));
        let targets = next_owner(&brx);
        assert!(targets.iter().any(|t| t == "UTF8_STRING") && targets.iter().any(|t| t == "text/uri-list"), "{targets:?}");
        assert_eq!(read(&b, &brx, "UTF8_STRING").unwrap(), "héllo".as_bytes());
        assert!(read(&b, &brx, "NOPE").is_err(), "provider refusal must surface as an error");
        assert!(read(&b, &brx, "image/png").is_err(), "unoffered target must fail");
    }

    #[test]
    fn large_payload_uses_incr_both_ways() {
        let (Some((a, _arx)), Some((b, brx))) = (spawn(), spawn()) else {
            eprintln!("skipped: no X server (run under xvfb-run)");
            return;
        };
        let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 253) as u8).collect();
        a.own(vec!["image/png".into()], Arc::new(Fixed(big.clone())));
        let _ = next_owner(&brx);
        let got = read(&b, &brx, "image/png").unwrap();
        assert_eq!(got.len(), big.len());
        assert!(got == big, "INCR transfer corrupted the data");
    }

    #[test]
    fn ownership_handover_and_disown() {
        let (Some((a, arx)), Some((b, brx))) = (spawn(), spawn()) else {
            eprintln!("skipped: no X server (run under xvfb-run)");
            return;
        };
        a.own(vec!["UTF8_STRING".into()], Arc::new(Fixed(b"from-a".to_vec())));
        let _ = next_owner(&brx);
        b.own(vec!["UTF8_STRING".into()], Arc::new(Fixed(b"from-b".to_vec())));
        // a is told that someone else owns the clipboard now and can read b's data
        let _ = next_owner(&arx);
        assert_eq!(read(&a, &arx, "UTF8_STRING").unwrap(), b"from-b");
        b.disown();
        std::thread::sleep(Duration::from_millis(200));
        assert!(read(&a, &arx, "UTF8_STRING").is_err());
    }
}
