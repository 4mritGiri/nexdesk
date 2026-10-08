use std::ops::Range;

use crate::{
    navigation::{Navigation, Screen},
    state::ManagerState,
    theme::*,
};

use gpui::{
    div, fill, prelude::*, px, relative, rgba, size, App, Bounds, ClipboardItem, Context,
    CursorStyle, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler, FocusHandle,
    FontWeight, GlobalElementId, KeyBinding, LayoutId, PaintQuad, Pixels, Point, Render, ResizeEdge,
    MouseButton, Decorations, WindowBackgroundAppearance, WindowControlArea, WindowDecorations, ShapedLine, SharedString, Style, TextRun, UTF16Selection, Window, WindowBounds,
    WindowOptions,
};

use gpui_platform::application;
use nexdesk_core::{
    credentials::Secret,
    books::AddressBooks,
    settings::{Settings, Theme, TlsMode},
    vault::{KdfParams, Vault, VaultError},
    logs::{self, Kind as LogKind, Level as LogLevel},
    profiles::{file_stem_for, Profile, Speed},
};
use nexdesk_session::SessionState;

// ============================================================================
// Password input actions
// ============================================================================

gpui::actions!(
    nexdesk_text_input,
    [
        InputBackspace,
        InputDelete,
        InputLeft,
        InputRight,
        InputSelectLeft,
        InputSelectRight,
        InputSelectAll,
        InputHome,
        InputEnd,
        InputPaste,
        InputCopy,
        InputCut,
    ]
);

// ============================================================================
// Application
// ============================================================================

pub struct NexDeskApp {
    pub state: ManagerState,
    pub nav: Navigation,
    pub form: Profile,

    pub selected_profile: Option<usize>,

    pub password_dialog: bool,
    pub password_input: Entity<TextInput>,
    pub password_error: Option<String>,

    /// Open "New / Edit connection" dialog, if any.
    pub editor: Option<Editor>,
    /// Index awaiting a second click on Delete (confirmation).
    pub pending_delete: Option<usize>,

    /// Sidebar shows icons only.
    pub sidebar_collapsed: bool,
    /// Connections as tiles (true) or as a list (false).
    pub grid_view: bool,
    history: Vec<Screen>,
    hist_pos: usize,
    pub search_open: bool,
    pub search_input: Entity<TextInput>,

    // logs / devices / address books
    pub logs_open: bool,
    log_entries: Vec<logs::Entry>,
    conn_log: Vec<logs::Entry>,
    log_loaded: Option<std::time::Instant>,
    confirm_clear: bool,
    books: AddressBooks,
    book_sel: Option<usize>,
    book_input: Entity<TextInput>,

    pub prefs: Settings,
    menu_open: bool,
    info_dialog: Option<InfoDialog>,

    // password vault
    vault: Option<Vault>,
    vault_dialog: Option<VaultDlg>,
    vault_pw1: Entity<TextInput>,
    vault_pw2: Entity<TextInput>,
    vault_rec: Entity<TextInput>,
    vault_error: Option<String>,
    /// Recovery key to display once after creating the vault.
    vault_new_key: Option<String>,
    save_pw: bool,
    confirm_wipe: bool,
    /// Network scan for computers with RDP enabled (Devices page).
    scan: Option<nexdesk_core::discover::Scan>,

    // Remote Control (NexDesk peer protocol)
    remote_addr: Entity<TextInput>,
    remote_probe: Option<(String, std::sync::mpsc::Receiver<Result<String, String>>)>,
    /// Agent address and the fingerprint the user is asked to trust.
    remote_trust: Option<(String, String)>,
    remote_msg: String,
    agent: Option<nexdesk_peer::control::Agent>,
    agent_fp: Option<String>,
    relay_input: Entity<TextInput>,
    agent_id: Option<String>,
    relay_status: String,
    /// Relay used by the connection attempt in progress (ID connections only).
    pending_relay: Option<String>,
    agent_request: Option<String>,
    share_view_only: bool,
    share_clipboard: bool,
    /// Show the relay server and sharing options on the Remote Control page.
    remote_adv: bool,
    viewers: Vec<std::process::Child>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VaultDlg {
    Setup,
    Unlock,
    UseRecovery,
    ChangeMaster,
    ShowRecoveryKey,
}

/// What the Preferences page needs to know about the vault.
#[derive(Clone, Copy)]
struct VaultStatus {
    exists: bool,
    unlocked: bool,
    count: usize,
    confirm_wipe: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InfoDialog {
    Shortcuts,
    About,
}

/// State of the connection editor dialog.
pub struct Editor {
    /// Name of the profile being edited (`None` = creating a new one).
    original: Option<String>,
    name: Entity<TextInput>,
    host: Entity<TextInput>,
    user: Entity<TextInput>,
    domain: Entity<TextInput>,
    pre_cmd: Entity<TextInput>,
    post_cmd: Entity<TextInput>,
    width: Entity<TextInput>,
    height: Entity<TextInput>,
    clipboard: bool,
    fullscreen: bool,
    speed: Speed,
    error: Option<String>,
}

fn make_input(cx: &mut Context<NexDeskApp>, placeholder: &'static str, value: &str) -> Entity<TextInput> {
    let input = cx.new(|cx| TextInput::new(cx, false, placeholder));
    input.update(cx, |i, cx| i.set_value(value, cx));
    input
}

impl NexDeskApp {
    pub fn new(
        engine_path: std::path::PathBuf,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let password_input = cx.new(|cx| TextInput::new(cx, true, "Password"));
        let search_input = cx.new(|cx| TextInput::new(cx, false, "Search"));
        let book_input = cx.new(|cx| TextInput::new(cx, false, "New address book"));
        let vault_pw1 = cx.new(|cx| TextInput::new(cx, true, "Master password"));
        let vault_pw2 = cx.new(|cx| TextInput::new(cx, true, "Repeat master password"));
        let vault_rec = cx.new(|cx| TextInput::new(cx, false, "XXXX-XXXX-XXXX-..."));
        // Re-render the list while typing in the search box.
        cx.observe(&search_input, |_, _, cx| cx.notify()).detach();

        let prefs = Settings::load();
        crate::theme::set_theme(prefs.theme);
        let vault_exists = nexdesk_core::vault::default_path().map(|p| Vault::exists(&p)).unwrap_or(false);
        let mut me = Self {
            state: ManagerState::load(engine_path),
            nav: Navigation::default(),
            form: Profile::default(),
            selected_profile: None,
            password_dialog: false,
            password_input,
            password_error: None,
            editor: None,
            pending_delete: None,
            sidebar_collapsed: prefs.sidebar_collapsed,
            grid_view: prefs.grid_view,
            history: vec![Screen::Connections],
            hist_pos: 0,
            search_open: false,
            search_input,
            logs_open: true,
            log_entries: Vec::new(),
            conn_log: Vec::new(),
            log_loaded: None,
            confirm_clear: false,
            books: nexdesk_core::books::default_path()
                .map(|p| AddressBooks::load(&p))
                .unwrap_or_else(AddressBooks::in_memory),
            book_sel: None,
            book_input,
            prefs,
            menu_open: false,
            info_dialog: None,
            vault: None,
            vault_dialog: None,
            vault_pw1,
            vault_pw2,
            vault_rec,
            vault_error: None,
            vault_new_key: None,
            save_pw: false,
            confirm_wipe: false,
            scan: None,
            remote_addr: make_input(cx, "Address (192.168.1.20) or 9-digit ID", ""),
            remote_probe: None,
            remote_trust: None,
            remote_msg: String::new(),
            agent: None,
            agent_fp: None,
            relay_input: make_input(
                cx,
                "relay.example.com:21117",
                &nexdesk_peer::store::default_dir().and_then(|d| nexdesk_peer::store::load_relay(&d)).unwrap_or_default(),
            ),
            agent_id: None,
            relay_status: String::new(),
            pending_relay: None,
            agent_request: None,
            share_view_only: false,
            share_clipboard: true,
            remote_adv: false,
            viewers: Vec::new(),
        };
        if vault_exists {
            me.open_vault_dialog(VaultDlg::Unlock, _window, cx);
        }
        me
    }


    /// Change a preference, save it and apply it immediately.
    fn update_settings(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Settings)) {
        f(&mut self.prefs);
        crate::theme::set_theme(self.prefs.theme);
        if let Err(e) = self.prefs.save() {
            self.state.status = format!("Could not save preferences: {e}");
        }
        cx.notify();
    }

    // ------------------------------------------------------------------
    // Password vault
    // ------------------------------------------------------------------

    fn vault_file() -> Option<std::path::PathBuf> {
        nexdesk_core::vault::default_path()
    }

    fn vault_exists() -> bool {
        Self::vault_file().map(|p| Vault::exists(&p)).unwrap_or(false)
    }

    fn open_vault_dialog(&mut self, kind: VaultDlg, window: &mut Window, cx: &mut Context<Self>) {
        for i in [&self.vault_pw1, &self.vault_pw2, &self.vault_rec] {
            i.update(cx, |i, cx| i.reset(cx));
        }
        self.vault_error = None;
        self.vault_dialog = Some(kind);
        let first = match kind {
            VaultDlg::UseRecovery => self.vault_rec.read(cx).focus_handle.clone(),
            _ => self.vault_pw1.read(cx).focus_handle.clone(),
        };
        window.focus(&first, cx);
        cx.notify();
    }

    fn close_vault_dialog(&mut self, cx: &mut Context<Self>) {
        self.vault_dialog = None;
        self.vault_error = None;
        self.vault_new_key = None;
        for i in [&self.vault_pw1, &self.vault_pw2, &self.vault_rec] {
            i.update(cx, |i, cx| i.reset(cx));
        }
        cx.notify();
    }

    fn vault_err(&mut self, e: VaultError, cx: &mut Context<Self>) {
        self.vault_error = Some(e.to_string());
        cx.notify();
    }

    fn vault_submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kind) = self.vault_dialog else { return };
        let pw1 = self.vault_pw1.read(cx).value().to_string();
        let pw2 = self.vault_pw2.read(cx).value().to_string();
        let rec = self.vault_rec.read(cx).value().to_string();
        let Some(path) = Self::vault_file() else {
            self.vault_error = Some("No config directory available on this system.".into());
            cx.notify();
            return;
        };
        match kind {
            VaultDlg::Setup => {
                if pw1 != pw2 {
                    self.vault_error = Some("The two passwords do not match.".into());
                    cx.notify();
                    return;
                }
                match Vault::create(&path, &pw1, KdfParams::DEFAULT) {
                    Ok((v, key)) => {
                        self.vault = Some(v);
                        self.vault_new_key = Some(key);
                        logs::alarm(LogLevel::Info, "Password vault created", "", "");
                        self.open_vault_dialog(VaultDlg::ShowRecoveryKey, window, cx);
                    }
                    Err(e) => self.vault_err(e, cx),
                }
            }
            VaultDlg::Unlock => match Vault::unlock(&path, &pw1) {
                Ok(v) => {
                    self.state.status = format!("Vault unlocked ({} saved password(s)).", v.len());
                    self.vault = Some(v);
                    self.close_vault_dialog(cx);
                }
                Err(e) => {
                    if matches!(e, VaultError::WrongKey) {
                        logs::alarm(LogLevel::Warn, "Wrong vault master password", "", "");
                    }
                    self.vault_err(e, cx)
                }
            },
            VaultDlg::UseRecovery => match Vault::unlock_with_recovery(&path, &rec) {
                Ok(v) => {
                    logs::alarm(LogLevel::Warn, "Vault unlocked with the recovery key", "", "choose a new master password");
                    self.vault = Some(v);
                    self.open_vault_dialog(VaultDlg::ChangeMaster, window, cx);
                }
                Err(e) => self.vault_err(e, cx),
            },
            VaultDlg::ChangeMaster => {
                if pw1 != pw2 {
                    self.vault_error = Some("The two passwords do not match.".into());
                    cx.notify();
                    return;
                }
                let res = self.vault.as_mut().map(|v| v.change_master(&pw1));
                match res {
                    Some(Ok(())) => {
                        self.state.status = "Master password changed.".into();
                        self.close_vault_dialog(cx);
                    }
                    Some(Err(e)) => self.vault_err(e, cx),
                    None => self.close_vault_dialog(cx),
                }
            }
            VaultDlg::ShowRecoveryKey => self.close_vault_dialog(cx),
        }
    }

    fn lock_vault(&mut self, cx: &mut Context<Self>) {
        self.vault = None;
        self.confirm_wipe = false;
        self.state.status = "Vault locked.".into();
        cx.notify();
    }

    fn wipe_vault_passwords(&mut self, cx: &mut Context<Self>) {
        if !self.confirm_wipe {
            self.confirm_wipe = true;
            cx.notify();
            return;
        }
        self.confirm_wipe = false;
        if let Some(v) = self.vault.as_mut() {
            match v.clear() {
                Ok(()) => self.state.status = "All saved passwords were deleted.".into(),
                Err(e) => self.state.status = e.to_string(),
            }
        }
        cx.notify();
    }

    fn forget_saved_password(&mut self, cx: &mut Context<Self>) {
        if let (Some(p), Some(v)) = (self.selected_profile_cloned(), self.vault.as_mut()) {
            let _ = v.remove(&p.name);
            self.state.status = format!("Saved password for \"{}\" removed.", p.name);
        }
        cx.notify();
    }

    fn vault_status(&self) -> VaultStatus {
        VaultStatus {
            exists: Self::vault_exists(),
            unlocked: self.vault.is_some(),
            count: self.vault.as_ref().map(|v| v.len()).unwrap_or(0),
            confirm_wipe: self.confirm_wipe,
        }
    }

    /// Start the session; on success optionally remember the password in the vault.
    fn launch(&mut self, profile: Profile, password: Secret, save: bool, cx: &mut Context<Self>) -> Result<u64, String> {
        let name = profile.name.clone();
        match self.state.sessions.start(profile, password.clone()) {
            Ok((id, _)) => {
                if save {
                    if let Some(v) = self.vault.as_mut() {
                        if let Err(e) = v.set(&name, password) {
                            self.state.status = format!("Connected, but the password could not be saved: {e}");
                        }
                    }
                }
                self.nav.screen = Screen::Sessions;
                cx.notify();
                Ok(id)
            }
            Err(e) => Err(e.to_string()),
        }
    }


    // ------------------------------------------------------------------
    // Remote Control (viewer and agent run as separate processes)
    // ------------------------------------------------------------------

    /// Collect results of background work; returns true while something is still pending.
    fn poll_remote(&mut self, cx: &mut Context<Self>) -> bool {
        use nexdesk_peer::control::AgentEvent;
        let mut changed = false;
        if let Some((addr, rx)) = self.remote_probe.as_ref() {
            match rx.try_recv() {
                Ok(Ok(fp)) => {
                    let addr = addr.clone();
                    let known = nexdesk_peer::store::default_dir()
                        .map(|d| nexdesk_core::knownhosts::KnownHosts::load(&d.join("known_agents")))
                        .unwrap_or_else(nexdesk_core::knownhosts::KnownHosts::in_memory);
                    let key = if self.pending_relay.is_some() {
                        nexdesk_core::knownhosts::host_key(&format!("id-{addr}"), nexdesk_peer::DEFAULT_PORT)
                    } else {
                        let (host, port) = addr.rsplit_once(':').map(|(h, p)| (h.to_string(), p.parse().unwrap_or(nexdesk_peer::DEFAULT_PORT))).unwrap_or((addr.clone(), nexdesk_peer::DEFAULT_PORT));
                        nexdesk_core::knownhosts::host_key(&host, port)
                    };
                    self.remote_probe = None;
                    if matches!(known.lookup(&key, &fp), nexdesk_core::knownhosts::Lookup::Match) {
                        let relay = self.pending_relay.clone();
                        self.start_viewer(&addr, relay.as_deref(), None);
                    } else {
                        // unknown or changed: the user must compare the fingerprint first
                        self.remote_trust = Some((addr, fp));
                    }
                    changed = true;
                }
                Ok(Err(e)) => {
                    self.remote_msg = format!("Could not reach the agent: {e}");
                    self.remote_probe = None;
                    changed = true;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(_) => {
                    self.remote_probe = None;
                    changed = true;
                }
            }
        }
        let mut exited = false;
        if let Some(a) = self.agent.as_ref() {
            while let Ok(ev) = a.events.try_recv() {
                changed = true;
                match ev {
                    AgentEvent::Identity(f) => self.agent_fp = Some(f),
                    AgentEvent::Id(i) => self.agent_id = Some(i),
                    AgentEvent::Relay(t) => self.relay_status = t,
                    AgentEvent::Request(f) => {
                        self.agent_request = Some(f);
                        self.nav.screen = Screen::Remote; // make sure the person sees the question
                    }
                    AgentEvent::Exited => exited = true,
                }
            }
        }
        if exited {
            self.agent = None;
            self.agent_request = None;
            self.agent_fp = None;
            self.agent_id = None;
            self.relay_status.clear();
            self.remote_msg = "The sharing agent stopped.".into();
        }
        self.viewers.retain_mut(|c| !matches!(c.try_wait(), Ok(Some(_))));
        if changed {
            cx.notify();
        }
        self.remote_probe.is_some() || self.agent.is_some()
    }

    fn start_viewer(&mut self, addr: &str, relay: Option<&str>, trust: Option<&str>) {
        match nexdesk_peer::control::launch_viewer(addr, relay, trust, true) {
            Ok(c) => {
                self.viewers.push(c);
                self.remote_msg = format!("Opened a remote control window for {addr}.");
                nexdesk_core::logs::console(LogLevel::Info, "remote", &format!("viewer started for {addr}{}", if relay.is_some() { " (via relay)" } else { "" }));
            }
            Err(e) => self.remote_msg = e,
        }
    }

    /// The relay server typed in the box, with the default port added; saved for next time.
    fn relay_setting(&mut self, cx: &mut Context<Self>) -> Result<Option<String>, String> {
        let raw = self.relay_input.read(cx).value().trim().to_string();
        let dir = nexdesk_peer::store::default_dir();
        if raw.is_empty() {
            if let Some(d) = &dir {
                let _ = nexdesk_peer::store::save_relay(d, "");
            }
            return Ok(None);
        }
        if !nexdesk_peer::control::valid_addr(&raw) {
            return Err("The relay server must look like relay.example.com or relay.example.com:21117.".into());
        }
        if let Some(d) = &dir {
            let _ = nexdesk_peer::store::save_relay(d, &raw);
        }
        Ok(Some(if raw.contains(':') { raw } else { format!("{raw}:{}", nexdesk_network::DEFAULT_RELAY_PORT) }))
    }

    fn remote_connect(&mut self, cx: &mut Context<Self>) {
        let raw = self.remote_addr.read(cx).value().trim().to_string();
        let compact: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
        let (addr, relay) = if nexdesk_network::proto::valid_id(&compact) {
            match self.relay_setting(cx) {
                Ok(Some(r)) => (compact, Some(r)),
                Ok(None) => {
                    self.remote_msg = "To connect by ID, enter the relay server in the box below first.".into();
                    return;
                }
                Err(e) => {
                    self.remote_msg = e;
                    return;
                }
            }
        } else if nexdesk_peer::control::valid_addr(&raw) {
            (nexdesk_peer::control::with_port(&raw), None)
        } else {
            self.remote_msg = "Enter a nine digit ID, or an address like 192.168.1.20 or pc.lan:21118.".into();
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let (a2, r2) = (addr.clone(), relay.clone());
        std::thread::spawn(move || {
            let _ = tx.send(nexdesk_peer::control::probe(&a2, r2.as_deref()));
        });
        self.pending_relay = relay;
        self.remote_probe = Some((addr, rx));
        self.remote_msg = "Contacting the agent...".into();
    }

    fn toggle_agent(&mut self, cx: &mut Context<Self>) {
        if let Some(mut a) = self.agent.take() {
            a.stop();
            self.agent_fp = None;
            self.agent_id = None;
            self.relay_status.clear();
            self.agent_request = None;
            self.remote_msg = "Sharing stopped.".into();
            nexdesk_core::logs::console(LogLevel::Info, "remote", "screen sharing stopped");
            return;
        }
        let listen = format!("0.0.0.0:{}", nexdesk_peer::DEFAULT_PORT);
        let relay = match self.relay_setting(cx) {
            Ok(r) => r,
            Err(e) => {
                self.remote_msg = e;
                return;
            }
        };
        match nexdesk_peer::control::Agent::start(&listen, relay.as_deref(), self.share_view_only, self.share_clipboard) {
            Ok(a) => {
                self.agent = Some(a);
                self.remote_msg = "Sharing started. Every viewer must be approved here first.".into();
                nexdesk_core::logs::console(LogLevel::Info, "remote", "screen sharing started (consent required per viewer)");
            }
            Err(e) => self.remote_msg = e,
        }
    }

    fn answer_request(&mut self, yes: bool) {
        if let Some(a) = self.agent.as_mut() {
            a.answer(yes);
        }
        self.agent_request = None;
    }

    fn refresh_logs(&mut self) {
        self.conn_log = logs::read_tail(LogKind::Connection, 2000);
        if let Screen::Logs(kind) = self.nav.screen {
            self.log_entries = if kind == LogKind::Connection {
                self.conn_log.iter().take(500).cloned().collect()
            } else {
                logs::read_tail(kind, 500)
            };
        }
        self.log_loaded = Some(std::time::Instant::now());
    }

    // ------------------------------------------------------------------
    // Navigation (back / forward like a file manager)
    // ------------------------------------------------------------------

    fn navigate(&mut self, screen: Screen, cx: &mut Context<Self>) {
        if self.nav.screen != screen {
            self.history.truncate(self.hist_pos + 1);
            self.history.push(screen);
            self.hist_pos = self.history.len() - 1;
            self.nav.screen = screen;
        }
        self.confirm_clear = false;
        self.refresh_logs();
        cx.notify();
    }

    fn go_back(&mut self, cx: &mut Context<Self>) {
        if self.hist_pos > 0 {
            self.hist_pos -= 1;
            self.nav.screen = self.history[self.hist_pos];
            cx.notify();
        }
    }

    fn go_forward(&mut self, cx: &mut Context<Self>) {
        if self.hist_pos + 1 < self.history.len() {
            self.hist_pos += 1;
            self.nav.screen = self.history[self.hist_pos];
            cx.notify();
        }
    }

    fn toggle_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_open = !self.search_open;
        if self.search_open {
            let fh = self.search_input.read(cx).focus_handle.clone();
            window.focus(&fh, cx);
        } else {
            self.search_input.update(cx, |i, cx| i.set_value("", cx));
        }
        cx.notify();
    }

    // ------------------------------------------------------------------
    // Connection editor (New / Edit / Duplicate / Delete)
    // ------------------------------------------------------------------

    fn selected_profile_cloned(&self) -> Option<Profile> {
        self.selected_profile
            .and_then(|i| self.state.profiles.get(i))
            .cloned()
    }

    fn open_editor_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_editor(None, window, cx);
        let speed = self.prefs.default_speed;
        if let Some(ed) = self.editor.as_mut() {
            ed.speed = speed;
        }
    }

    /// New connection with the host already filled in (from the network scan).
    fn add_discovered(&mut self, host: String, window: &mut Window, cx: &mut Context<Self>) {
        self.open_editor_new(window, cx);
        if let Some(ed) = self.editor.as_ref() {
            ed.host.update(cx, |i, cx| i.set_value(&host, cx));
        }
    }

    fn open_editor_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.selected_profile_cloned() {
            Some(p) => self.open_editor(Some(p), window, cx),
            None => {
                self.state.status = "Select a connection to edit.".into();
                cx.notify();
            }
        }
    }

    fn open_editor(&mut self, existing: Option<Profile>, window: &mut Window, cx: &mut Context<Self>) {
        self.pending_delete = None;
        let original = existing.as_ref().map(|p| p.name.clone());
        let p = existing.unwrap_or_default();

        let name = make_input(cx, "Display name (optional)", &p.name);
        let host = make_input(cx, "host or host:port", &p.host);
        let user = make_input(cx, "User name", &p.user);
        let domain = make_input(cx, "Domain (optional)", &p.domain);
        let pre_cmd = make_input(cx, "e.g. nmcli con up \"Work VPN\"  (optional)", &p.pre_command);
        let post_cmd = make_input(cx, "e.g. nmcli con down \"Work VPN\"  (optional)", &p.post_command);
        let width = make_input(cx, "1920", &p.width.to_string());
        let height = make_input(cx, "1080", &p.height.to_string());
        let first = host.read(cx).focus_handle.clone();

        self.editor = Some(Editor {
            original,
            name,
            host,
            user,
            domain,
            pre_cmd,
            post_cmd,
            width,
            height,
            clipboard: p.clipboard,
            fullscreen: p.fullscreen,
            speed: p.speed,
            error: None,
        });
        window.focus(&first, cx);
        cx.notify();
    }

    fn cancel_editor(&mut self, cx: &mut Context<Self>) {
        self.editor = None;
        cx.notify();
    }

    fn toggle_editor_flag(&mut self, flag: &str, cx: &mut Context<Self>) {
        if let Some(ed) = self.editor.as_mut() {
            match flag {
                "clipboard" => ed.clipboard = !ed.clipboard,
                "fullscreen" => ed.fullscreen = !ed.fullscreen,
                "speed-lan" => ed.speed = Speed::Lan,
                "speed-balanced" => ed.speed = Speed::Balanced,
                "speed-slow" => ed.speed = Speed::Slow,
                _ => {}
            }
        }
        cx.notify();
    }

    /// Read the dialog fields into a validated profile.
    fn build_profile(&self, cx: &App) -> Result<Profile, String> {
        let ed = self.editor.as_ref().ok_or("No editor open")?;
        let host = ed.host.read(cx).value().trim().to_string();
        let user = ed.user.read(cx).value().trim().to_string();
        let domain = ed.domain.read(cx).value().trim().to_string();
        let mut name = ed.name.read(cx).value().trim().to_string();
        if name.is_empty() {
            name = host.clone();
        }
        let width: u16 = ed
            .width
            .read(cx)
            .value()
            .trim()
            .parse()
            .map_err(|_| "Width must be a number".to_string())?;
        let height: u16 = ed
            .height
            .read(cx)
            .value()
            .trim()
            .parse()
            .map_err(|_| "Height must be a number".to_string())?;

        let profile = Profile {
            name,
            host,
            user,
            domain,
            width,
            height,
            clipboard: ed.clipboard,
            fullscreen: ed.fullscreen,
            speed: ed.speed,
            pre_command: ed.pre_cmd.read(cx).value().trim().to_string(),
            post_command: ed.post_cmd.read(cx).value().trim().to_string(),
        };
        profile.validate().map_err(str::to_string)?;
        if file_stem_for(&profile.name).is_none() {
            return Err("Connection name is not usable as a file name".into());
        }
        Ok(profile)
    }

    /// Persist `profile`; returns the refreshed list.
    fn persist_profile(&self, profile: &Profile) -> Result<Vec<Profile>, String> {
        let store = self
            .state
            .store
            .as_ref()
            .ok_or("No config directory available on this system")?;
        let original = self.editor.as_ref().and_then(|e| e.original.clone());
        let new_stem = file_stem_for(&profile.name);
        let clashes = store.list().iter().any(|p| {
            file_stem_for(&p.name) == new_stem && Some(&p.name) != original.as_ref()
        });
        if clashes {
            return Err("A connection with this name already exists".into());
        }
        store.save(profile).map_err(|e| format!("Save failed: {e}"))?;
        if let Some(old) = original {
            if file_stem_for(&old) != new_stem {
                let _ = store.delete(&old); // rename: drop the old file
            }
        }
        Ok(store.list())
    }

    fn save_editor(&mut self, cx: &mut Context<Self>) {
        let result = self
            .build_profile(cx)
            .and_then(|p| self.persist_profile(&p).map(|list| (p, list)));
        match result {
            Ok((profile, list)) => {
                if let Some(old) = self.editor.as_ref().and_then(|e| e.original.clone()) {
                    if old != profile.name {
                        self.books.rename_connection(&old, Some(&profile.name));
                        if let Some(v) = self.vault.as_mut() {
                            let _ = v.rename(&old, &profile.name);
                        }
                    }
                }
                self.selected_profile = list.iter().position(|p| p.name == profile.name);
                self.state.selected = self.selected_profile;
                self.state.profiles = list;
                self.state.status = format!("Saved \"{}\" (password is never saved)", profile.name);
                self.editor = None;
            }
            Err(message) => {
                if let Some(ed) = self.editor.as_mut() {
                    ed.error = Some(message);
                }
            }
        }
        cx.notify();
    }

    fn duplicate_selected(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut p) = self.selected_profile_cloned() else {
            self.state.status = "Select a connection to duplicate.".into();
            cx.notify();
            return;
        };
        p.name = format!("{} (copy)", p.name);
        let result = match self.state.store.as_ref() {
            Some(store) => store.save(&p).map(|_| store.list()).map_err(|e| e.to_string()),
            None => Err("No config directory available".to_string()),
        };
        match result {
            Ok(list) => {
                self.selected_profile = list.iter().position(|x| x.name == p.name);
                self.state.selected = self.selected_profile;
                self.state.profiles = list;
                self.state.status = format!("Created \"{}\"", p.name);
            }
            Err(e) => self.state.status = format!("Duplicate failed: {e}"),
        }
        cx.notify();
    }

    fn delete_selected(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.selected_profile else {
            self.state.status = "Select a connection to delete.".into();
            cx.notify();
            return;
        };
        let Some(profile) = self.state.profiles.get(index).cloned() else {
            return;
        };
        if self.pending_delete != Some(index) {
            self.pending_delete = Some(index);
            self.state.status = format!("Click Delete again to remove \"{}\".", profile.name);
            cx.notify();
            return;
        }
        self.pending_delete = None;
        let result = match self.state.store.as_ref() {
            Some(store) => store.delete(&profile.name).map(|_| store.list()).map_err(|e| e.to_string()),
            None => Err("No config directory available".to_string()),
        };
        match result {
            Ok(list) => {
                self.books.rename_connection(&profile.name, None);
                if let Some(v) = self.vault.as_mut() {
                    let _ = v.remove(&profile.name);
                }
                self.state.profiles = list;
                self.selected_profile = None;
                self.state.selected = None;
                self.state.status = format!("Deleted \"{}\"", profile.name);
            }
            Err(e) => self.state.status = format!("Delete failed: {e}"),
        }
        cx.notify();
    }

    fn open_password_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.selected_profile else {
            self.state.status = "Select a connection first.".into();
            cx.notify();
            return;
        };

        if index >= self.state.profiles.len() {
            self.selected_profile = None;
            self.state.selected = None;
            self.state.status = "The selected connection no longer exists.".into();
            cx.notify();
            return;
        }

        // A saved password in the unlocked vault skips the prompt.
        if let Some(saved) = self.vault.as_ref().and_then(|v| v.get(&self.state.profiles[index].name)) {
            let profile = self.state.profiles[index].clone();
            match self.launch(profile, saved, false, cx) {
                Ok(id) => self.state.status = format!("Session #{id} is connecting (saved password)..."),
                Err(e) => self.state.status = format!("Unable to start the RDP session: {e}"),
            }
            cx.notify();
            return;
        }

        self.password_input.update(cx, |input, cx| {
            input.reset(cx);
        });

        self.save_pw = self.vault.is_some();
        self.password_error = None;
        self.password_dialog = true;

        let focus_handle = self.password_input.read(cx).focus_handle.clone();

        window.focus(&focus_handle, cx);

        cx.notify();
    }

    fn cancel_password_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.password_input.update(cx, |input, cx| {
            input.reset(cx);
        });

        self.password_error = None;
        self.password_dialog = false;

        cx.notify();
    }

    fn connect_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.selected_profile else {
            self.state.status = "Select a connection first.".into();
            cx.notify();
            return;
        };

        let Some(profile) = self.state.profiles.get(index).cloned() else {
            self.state.status = "Selected connection was not found.".into();
            self.selected_profile = None;
            self.state.selected = None;
            cx.notify();
            return;
        };

        let password = self.password_input.read(cx).value();

        if password.is_empty() {
            self.password_error = Some("Password is required.".into());

            let focus_handle = self.password_input.read(cx).focus_handle.clone();

            window.focus(&focus_handle, cx);
            cx.notify();
            return;
        }

        let save = self.save_pw && self.vault.is_some();
        match self.launch(profile, Secret::new(password.to_string()), save, cx) {
            Ok(id) => {
                self.password_input.update(cx, |input, cx| {
                    input.reset(cx);
                });
                self.password_error = None;
                self.password_dialog = false;
                self.state.status = format!("Session #{id} is connecting...");
                cx.notify();
            }
            Err(error) => {
                self.password_error = Some(error);
                self.state.status = "Unable to start the RDP session.".into();
                let focus_handle = self.password_input.read(cx).focus_handle.clone();
                window.focus(&focus_handle, cx);
                cx.notify();
            }
        }
    }

    fn disconnect_session(&mut self, id: u64, cx: &mut Context<Self>) {
        match self.state.sessions.disconnect(id) {
            Ok(_) => {
                self.state.status = format!("Disconnect requested for session #{id}.");

                self.state.sessions.remove_finished();
            }

            Err(error) => {
                self.state.status = error.to_string();
            }
        }

        cx.notify();
    }
}

// ============================================================================
// Main render
// ============================================================================

impl Render for NexDeskApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.state.sessions.remove_finished();

        let profiles = self.state.profiles.clone();
        let sessions = self.state.sessions.list();

        let status: SharedString = self.state.status.clone().into();

        let filter = self.search_input.read(cx).value().trim().to_lowercase();
        let has_saved_pw = match (&self.vault, self.selected_profile.and_then(|i| self.state.profiles.get(i))) {
            (Some(v), Some(p)) => v.has(&p.name),
            _ => false,
        };
        if self.poll_remote(cx) {
            window.request_animation_frame(); // keep polling the helper processes
        }
        if self.scan.as_ref().map(|s| !s.is_done()).unwrap_or(false) {
            window.request_animation_frame(); // keep repainting while the scan runs
        }
        let content = match self.nav.screen {
            Screen::Connections => {
                connection_view(profiles, self.selected_profile, self.pending_delete, self.grid_view, filter, has_saved_pw, cx)
                    .into_any_element()
            }

            Screen::Sessions => session_view(sessions, cx).into_any_element(),

            Screen::Devices => devices_view(&self.state.profiles, &self.conn_log, &filter, self.scan.as_ref(), cx).into_any_element(),

            Screen::Remote => remote_view(
                self.remote_addr.clone(),
                &self.remote_msg,
                self.remote_probe.is_some(),
                self.agent.is_some(),
                self.agent_fp.clone(),
                self.relay_input.clone(),
                self.agent_id.clone(),
                self.relay_status.clone(),
                self.share_view_only,
                self.share_clipboard,
                self.viewers.len(),
                self.remote_adv,
                cx,
            )
            .into_any_element(),

            Screen::AddressBooks => books_view(
                &self.books,
                self.book_sel,
                &self.state.profiles,
                self.book_input.clone(),
                cx,
            )
            .into_any_element(),

            Screen::Logs(kind) => logs_view(kind, &self.log_entries, &filter, self.confirm_clear, cx).into_any_element(),

            Screen::Settings => settings_view(&self.prefs, self.vault_status(), cx).into_any_element(),
        };

        if matches!(self.nav.screen, Screen::Logs(_) | Screen::Devices)
            && self.log_loaded.map(|t| t.elapsed().as_secs() >= 3).unwrap_or(true)
        {
            self.refresh_logs();
        }
        let title = self.nav.screen.title();
        let hv = HeaderState {
            title,
            can_back: self.hist_pos > 0,
            can_forward: self.hist_pos + 1 < self.history.len(),
            sidebar_collapsed: self.sidebar_collapsed,
            grid_view: self.grid_view,
            search_open: self.search_open,
            search_input: self.search_input.clone(),
            on_connections: self.nav.screen == Screen::Connections,
            searchable: self.nav.screen.searchable(),
            menu_open: self.menu_open,
        };

        let client = matches!(window.window_decorations(), Decorations::Client { .. });
        let rounded = client && !window.is_maximized();

        let body = div()
            .flex_1()
            .flex()
            .min_h_0()
            .child(sidebar_view(self.nav.screen, self.sidebar_collapsed, self.logs_open, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id("content-scroll")
                            .flex_1()
                            .min_w_0()
                            .p_6()
                            .overflow_y_scroll()
                            .child(content),
                    )
                    .child(
                        div()
                            .h(px(30.))
                            .px_4()
                            .flex()
                            .items_center()
                            .text_xs()
                            .text_color(muted())
                            .child(status),
                    ),
            );

        let mut main = div()
            .size_full()
            .bg(window_bg())
            .text_color(text())
            .flex()
            .flex_col()
            .overflow_hidden()
            .when(rounded, |d| d.rounded(px(12.)).border_1().border_color(border()))
            .child(header_view(hv, client, cx))
            .child(body);

        if let Some(ed) = self.editor.as_ref() {
            main = main.child(editor_dialog(ed, cx));
        } else if self.password_dialog {
            main = main.child(password_dialog(
                self.selected_profile
                    .and_then(|i| self.state.profiles.get(i))
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "Connection".into()),
                self.password_input.clone(),
                self.password_error.clone(),
                if self.vault.is_some() { Some(self.save_pw) } else { None },
                Self::vault_exists(),
                cx,
            ));
        }

        if self.menu_open {
            main = main.child(menu_popover(cx));
        }
        if let Some(d) = self.info_dialog {
            main = main.child(info_dialog(d, cx));
        }
        if let Some((addr, fp)) = self.remote_trust.clone() {
            main = main.child(remote_dialog(
                "Trust this computer?",
                vec![
                    if nexdesk_network::proto::valid_id(&addr) { format!("First connection to the computer with ID {addr}.") } else { format!("First connection to {addr}.") },
                    "Its fingerprint is:".into(),
                    fp.clone(),
                    "Compare it with the fingerprint shown on that computer (Remote Control > Share this computer). Only continue if they are identical.".into(),
                ],
                "Trust and connect",
                "Cancel",
                cx.listener(move |this, _e, _w, cx| {
                    if let Some((a, f)) = this.remote_trust.take() {
                        let relay = this.pending_relay.clone();
                        this.start_viewer(&a, relay.as_deref(), Some(&f));
                    }
                    cx.notify();
                }),
                cx.listener(|this, _e, _w, cx| {
                    this.remote_trust = None;
                    this.remote_msg = "Cancelled. Nothing was connected.".into();
                    cx.notify();
                }),
            ));
        }
        if let Some(fp) = self.agent_request.clone() {
            main = main.child(remote_dialog(
                "Allow remote control?",
                vec![
                    "A computer wants to see this screen".to_string() + if self.share_view_only { " (view only)." } else { " and control it." },
                    "Its fingerprint:".into(),
                    fp,
                    "Only allow it if you expect this connection and the fingerprint matches what the other person tells you.".into(),
                ],
                "Allow",
                "Deny",
                cx.listener(|this, _e, _w, cx| {
                    this.answer_request(true);
                    cx.notify();
                }),
                cx.listener(|this, _e, _w, cx| {
                    this.answer_request(false);
                    cx.notify();
                }),
            ));
        }
        if let Some(k) = self.vault_dialog {
            main = main.child(vault_dialog_view(
                k,
                [self.vault_pw1.clone(), self.vault_pw2.clone(), self.vault_rec.clone()],
                self.vault_error.clone(),
                self.vault_new_key.clone(),
                Self::vault_exists(),
                cx,
            ));
        }

        // Client-side decorations need their own resize handles.
        div()
            .size_full()
            .relative()
            .child(main)
            .when(client && !window.is_maximized(), |d| d.children(resize_handles()))
    }
}

fn resize_handles() -> Vec<gpui::AnyElement> {
    use gpui::CursorStyle as C;
    use ResizeEdge as E;
    const T: f32 = 5.;
    const K: f32 = 12.;
    let mk = |id: &'static str, edge: E, cursor: C| {
        div()
            .id(id)
            .absolute()
            .cursor(cursor)
            .on_mouse_down(MouseButton::Left, move |_, window, _| {
                window.start_window_resize(edge)
            })
    };
    vec![
        mk("rz-t", E::Top, C::ResizeUpDown).top_0().left(px(K)).right(px(K)).h(px(T)).into_any_element(),
        mk("rz-b", E::Bottom, C::ResizeUpDown).bottom_0().left(px(K)).right(px(K)).h(px(T)).into_any_element(),
        mk("rz-l", E::Left, C::ResizeLeftRight).left_0().top(px(K)).bottom(px(K)).w(px(T)).into_any_element(),
        mk("rz-r", E::Right, C::ResizeLeftRight).right_0().top(px(K)).bottom(px(K)).w(px(T)).into_any_element(),
        mk("rz-tl", E::TopLeft, C::ResizeUpLeftDownRight).top_0().left_0().size(px(K)).into_any_element(),
        mk("rz-tr", E::TopRight, C::ResizeUpRightDownLeft).top_0().right_0().size(px(K)).into_any_element(),
        mk("rz-bl", E::BottomLeft, C::ResizeUpRightDownLeft).bottom_0().left_0().size(px(K)).into_any_element(),
        mk("rz-br", E::BottomRight, C::ResizeUpLeftDownRight).bottom_0().right_0().size(px(K)).into_any_element(),
    ]
}

// ============================================================================
// Header bar (window dots on the left, title in the middle)
// ============================================================================

fn dot(
    id: &'static str,
    color: gpui::Rgba,
    cx: &mut Context<NexDeskApp>,
    action: fn(&mut Window),
) -> impl IntoElement {
    div()
        .id(id)
        .size(px(14.))
        .rounded_full()
        .bg(color)
        .cursor_pointer()
        .opacity(0.92)
        .hover(|s| s.opacity(1.0))
        // Do not let the click start a window drag.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |_this, _e, window, _cx| action(window)))
}

/// The close dot of a popup: it belongs to the popup card (top-left, like a window title bar)
/// and only dismisses that popup. Popups have no minimise / full-screen dots.
fn popup_close(
    id: &'static str,
    cx: &mut Context<NexDeskApp>,
    on_close: impl Fn(&mut NexDeskApp, &mut Window, &mut Context<NexDeskApp>) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex_none()
        .size(px(14.))
        .rounded_full()
        .bg(dot_close())
        .cursor_pointer()
        .opacity(0.92)
        .hover(|s| s.opacity(1.0))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, _e, w, cx| on_close(this, w, cx)))
}

/// Title bar of a popup card: close dot, optional icon and title on a darker strip that runs edge to
/// edge (the card has 24 px padding, so the bar pulls itself out with negative margins).
fn popup_header(
    card_w: f32,
    dot: impl IntoElement,
    icon: Option<&'static str>,
    title: impl Into<SharedString>,
) -> impl IntoElement {
    div()
        .w(px(card_w - 2.))
        .h(px(48.))
        .flex_none()
        .mx(px(-24.))
        .mt(px(-24.))
        .mb(px(4.))
        .px_4()
        .flex()
        .items_center()
        .gap_3()
        .rounded_t(px(15.))
        .bg(sidebar())
        .border_b_1()
        .border_color(border())
        .child(dot)
        .when_some(icon, |d, name| d.child(ico(name, 18., accent_hover())))
        .child(div().text_lg().font_weight(FontWeight::BOLD).child(title.into()))
}

/// Round-rect icon button used in the header (glyph from the system font).
fn hbtn(
    id: &'static str,
    glyph: &'static str,
    enabled: bool,
    active: bool,
    cx: &mut Context<NexDeskApp>,
    action: fn(&mut NexDeskApp, &mut Window, &mut Context<NexDeskApp>),
) -> impl IntoElement {
    div()
        .id(id)
        .w(px(34.))
        .h(px(30.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(8.))
        .bg(if active { panel_2() } else { panel() })
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .when(enabled, |d| {
            d.cursor_pointer()
                .hover(|s| s.bg(hover()))
                .on_click(cx.listener(move |this, _e, window, cx| action(this, window, cx)))
        })
        .child(ico(glyph, 18., if !enabled { dim() } else if active { accent_hover() } else { icon() }))
}

/// Vector icon from `assets/icons`, tinted with `color`.
fn ico(name: &str, size: f32, color: gpui::Rgba) -> impl IntoElement {
    gpui::svg()
        .path(SharedString::from(format!("icons/{name}.svg")))
        .size(px(size))
        .flex_none()
        .text_color(color)
}

fn rgb_dim() -> gpui::Rgba {
    dim()
}

struct HeaderState {
    title: &'static str,
    can_back: bool,
    can_forward: bool,
    sidebar_collapsed: bool,
    grid_view: bool,
    search_open: bool,
    search_input: Entity<TextInput>,
    on_connections: bool,
    searchable: bool,
    menu_open: bool,
}

fn header_view(h: HeaderState, client: bool, cx: &mut Context<NexDeskApp>) -> impl IntoElement {
    let centre = if h.search_open {
        div()
            .flex_1()
            .h(px(32.))
            .px_4()
            .rounded_full()
            .bg(input_bg())
            .border_1()
            .border_color(accent())
            .flex()
            .items_center()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().w_full().child(h.search_input.clone()))
            .into_any_element()
    } else {
        // path-bar pill: "NexDesk / Connections"
        div()
            .flex_1()
            .h(px(32.))
            .px_4()
            .rounded_full()
            .bg(panel_2())
            .flex()
            .items_center()
            .justify_center()
            .gap_2()
            .text_sm()
            .child(div().text_color(muted()).child("NexDesk"))
            .child(div().text_color(rgb_dim()).child("/"))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(h.title))
            .into_any_element()
    };

    div()
        .id("header")
        .h(px(52.))
        .w_full()
        .flex_none()
        .bg(panel())
        .flex()
        .items_center()
        .gap_3()
        .px_4()
        .window_control_area(WindowControlArea::Drag)
        .on_mouse_down(MouseButton::Left, |e, window, _| {
            if e.click_count == 2 {
                window.zoom_window();
            } else {
                window.start_window_move();
            }
        })
        // traffic lights
        .child(
            div().flex().items_center().gap(px(8.)).mr_1().when(client, |d| {
                d.child(dot("dot-close", dot_close(), cx, |w| w.remove_window()))
                    .child(dot("dot-min", dot_min(), cx, |w| w.minimize_window()))
                    .child(dot("dot-max", dot_max(), cx, |w| w.zoom_window()))
            }),
        )
        .child(hbtn("h-sidebar", "sidebar", true, !h.sidebar_collapsed, cx, |this, _w, cx| {
            let v = !this.sidebar_collapsed;
            this.sidebar_collapsed = v;
            this.update_settings(cx, |s| s.sidebar_collapsed = v);
        }))
        // back / forward
        .child(
            div()
                .flex()
                .rounded(px(9.))
                .bg(panel())
                .child(hbtn("h-back", "back", h.can_back, false, cx, |this, _w, cx| this.go_back(cx)))
                .child(hbtn("h-fwd", "forward", h.can_forward, false, cx, |this, _w, cx| this.go_forward(cx))),
        )
        .child(centre)
        .child(hbtn("h-search", "search", h.searchable, h.search_open, cx, |this, w, cx| {
            this.toggle_search(w, cx)
        }))
        .child(
            div()
                .flex()
                .rounded(px(9.))
                .bg(panel())
                .child(hbtn("h-grid", "grid", h.on_connections, h.on_connections && h.grid_view, cx, |this, _w, cx| {
                    this.grid_view = true;
                    this.update_settings(cx, |s| s.grid_view = true);
                }))
                .child(hbtn("h-list", "list", h.on_connections, h.on_connections && !h.grid_view, cx, |this, _w, cx| {
                    this.grid_view = false;
                    this.update_settings(cx, |s| s.grid_view = false);
                })),
        )
        .child(hbtn("h-menu", "menu", true, h.menu_open, cx, |this, _w, cx| {
            this.menu_open = !this.menu_open;
            cx.notify();
        }))
}

// ============================================================================
// Sidebar (collapsible)
// ============================================================================

fn sidebar_view(active: Screen, collapsed: bool, logs_open: bool, cx: &mut Context<NexDeskApp>) -> impl IntoElement {
    let in_logs = matches!(active, Screen::Logs(_));
    let sub = |icon: &'static str, kind: LogKind, cx: &mut Context<NexDeskApp>| {
        sidebar_button(icon, kind.label(), collapsed, active == Screen::Logs(kind), Screen::Logs(kind), !collapsed, cx)
    };
    div()
        .id("sidebar")
        .w(px(if collapsed { 60. } else { 220. }))
        .flex_none()
        .bg(sidebar())
        .p_2()
        .flex()
        .flex_col()
        .gap_1()
        .overflow_y_scroll()
        .child(sidebar_button("connections", "Connections", collapsed, active == Screen::Connections, Screen::Connections, false, cx))
        .child(sidebar_button("devices", "Devices", collapsed, active == Screen::Devices, Screen::Devices, false, cx))
        .child(sidebar_button("zap", "Remote Control", collapsed, active == Screen::Remote, Screen::Remote, false, cx))
        .child(sidebar_button("book", "Address Books", collapsed, active == Screen::AddressBooks, Screen::AddressBooks, false, cx))
        .child(sidebar_button("sessions", "Sessions", collapsed, active == Screen::Sessions, Screen::Sessions, false, cx))
        .child(div().h(px(1.)).my_2().mx_2().bg(border()))
        // Logs group: header expands/collapses the four sub-pages (icons only when the sidebar is collapsed)
        .when(!collapsed, |d| {
            d.child(
                div()
                    .id("logs-group")
                    .w_full()
                    .h(px(34.))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .rounded(px(8.))
                    .text_color(if in_logs { text() } else { icon() })
                    .cursor_pointer()
                    .hover(|s| s.bg(row_hover()))
                    .on_click(cx.listener(|this, _e, _w, cx| {
                        this.logs_open = !this.logs_open;
                        cx.notify();
                    }))
                    .child(div().w(px(22.)).flex().justify_center().child(ico("logs", 18., if in_logs { accent_hover() } else { icon() })))
                    .child(div().flex_1().text_sm().font_weight(FontWeight::SEMIBOLD).child("Logs"))
                    .child(ico(if logs_open { "chevron-down" } else { "chevron-right" }, 14., dim())),
            )
        })
        .when(collapsed || logs_open || in_logs, |d| {
            d.child(sub("log-connection", LogKind::Connection, cx))
                .child(sub("log-file", LogKind::File, cx))
                .child(sub("log-alarm", LogKind::Alarm, cx))
                .child(sub("log-console", LogKind::Console, cx))
        })
        .child(div().h(px(1.)).my_2().mx_2().bg(border()))
        .child(sidebar_button("settings", "Settings", collapsed, active == Screen::Settings, Screen::Settings, false, cx))
}

fn sidebar_button(
    icon: &'static str,
    label: &'static str,
    collapsed: bool,
    active: bool,
    screen: Screen,
    indent: bool,
    cx: &mut Context<NexDeskApp>,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("nav-{screen:?}")))
        .w_full()
        .h(px(38.))
        .px_3()
        .when(indent, |d| d.pl(px(28.)))
        .flex()
        .items_center()
        .when(collapsed, |d| d.justify_center())
        .gap_3()
        .rounded(px(8.))
        .bg(if active { panel_2() } else { sidebar() })
        .text_color(text())
        .cursor_pointer()
        .when(!active, |d| d.hover(|s| s.bg(row_hover())))
        .on_click(cx.listener(move |this, _event, _window, cx| this.navigate(screen, cx)))
        .child(div().w(px(22.)).flex().justify_center().child(ico(icon, 18., if active { accent_hover() } else { crate::theme::icon() })))
        .when(!collapsed, |d| d.child(div().text_sm().child(label)))
}


// ============================================================================
// Remote Control
// ============================================================================

/// A text box that takes focus when clicked anywhere inside it.
fn field(input: Entity<TextInput>, cx: &mut Context<NexDeskApp>) -> impl IntoElement {
    let handle = input.read(cx).focus_handle.clone();
    div()
        .flex_1()
        .h(px(36.))
        .px_3()
        .rounded(px(8.))
        .bg(panel_2())
        .flex()
        .items_center()
        .cursor(CursorStyle::IBeam)
        .on_mouse_down(MouseButton::Left, move |_e, window, cx| window.focus(&handle, cx))
        .child(div().w_full().child(input))
}

#[allow(clippy::too_many_arguments)]
fn remote_view(
    addr: Entity<TextInput>,
    msg: &str,
    probing: bool,
    sharing: bool,
    fingerprint: Option<String>,
    relay_input: Entity<TextInput>,
    agent_id: Option<String>,
    relay_status: String,
    view_only: bool,
    clipboard: bool,
    open_windows: usize,
    advanced: bool,
    cx: &mut Context<NexDeskApp>,
) -> impl IntoElement {
    let card = || div().flex_1().min_w(px(300.)).p_5().rounded(px(12.)).bg(panel()).flex().flex_col().gap_3();
    let title = |t: &'static str| div().text_base().font_weight(FontWeight::SEMIBOLD).child(t);
    let hint = |t: String| div().text_xs().text_color(muted()).child(t);
    let has_relay = !relay_input.read(cx).value().trim().is_empty();

    // Left: this computer (like "This Desk")
    let mut mine = card().child(title("This computer"));
    mine = match (&agent_id, sharing) {
        (Some(id), _) => {
            let spaced = format!("{} {} {}", &id[0..3], &id[3..6], &id[6..9]);
            mine.child(hint("Your ID - tell it to the person who will connect".into()))
                .child(div().text_3xl().font_weight(FontWeight::SEMIBOLD).child(spaced))
        }
        (None, true) => mine
            .child(hint("Sharing is on".into()))
            .child(div().text_sm().child(if has_relay { "Getting your ID..." } else { "No ID yet: add a relay server under Settings below, then turn sharing off and on." })),
        (None, false) => mine
            .child(hint("Sharing is off. Turn it on to get an ID.".into()))
            .child(div().text_3xl().font_weight(FontWeight::SEMIBOLD).text_color(muted()).child("--- --- ---")),
    };
    if sharing && !relay_status.is_empty() {
        mine = mine.child(hint(format!("Server: {relay_status}")));
    }
    if sharing {
        if let Some(fp) = fingerprint {
            mine = mine.child(hint(format!("Fingerprint (the other person checks it once): {fp}")));
        }
    }
    mine = mine
        .child(div().flex().child(toolbar_button_dyn(
            "share-toggle".into(),
            if sharing { "Turn sharing off" } else { "Turn sharing on" },
            true,
            cx.listener(|this, _e, _w, cx| {
                this.toggle_agent(cx);
                cx.notify();
            }),
        )))
        .child(hint("You are asked to allow every connection.".into()));

    // Right: connect to another computer (like "Remote Desk")
    let theirs = card()
        .child(title("Connect to another computer"))
        .child(hint("Enter its ID (9 digits) or its address".into()))
        .child(div().flex().child(field(addr, cx)))
        .child(div().flex().child(toolbar_button_dyn(
            "remote-connect".into(),
            if probing { "Connecting..." } else { "Connect" },
            !probing,
            cx.listener(|this, _e, _w, cx| {
                this.remote_connect(cx);
                cx.notify();
            }),
        )))
        .when(open_windows > 0, |d| d.child(hint(format!("{open_windows} remote window(s) open"))))
        .child(hint("The first time, you compare a fingerprint to be sure it is the right computer.".into()));

    // Bottom: settings, hidden by default
    let mut options = div().w_full().p_4().rounded(px(12.)).bg(panel()).flex().flex_col().gap_3().child(
        div().flex().items_center().justify_between().child(title("Settings")).child(toolbar_button_dyn(
            "remote-adv".into(),
            if advanced { "Hide" } else { "Show" },
            true,
            cx.listener(|this, _e, _w, cx| {
                this.remote_adv = !this.remote_adv;
                cx.notify();
            }),
        )),
    );
    if advanced {
        options = options
            .child(hint("Relay server: lets people reach you by ID over the internet. Leave empty to use only your local network. Set it before turning sharing on.".into()))
            .child(div().flex().child(field(relay_input, cx)))
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(toolbar_button_dyn(
                        "share-viewonly".into(),
                        if view_only { "Others can only watch" } else { "Others can control" },
                        !sharing,
                        cx.listener(|this, _e, _w, cx| {
                            this.share_view_only = !this.share_view_only;
                            cx.notify();
                        }),
                    ))
                    .child(toolbar_button_dyn(
                        "share-clip".into(),
                        if clipboard { "Clipboard shared" } else { "Clipboard private" },
                        !sharing,
                        cx.listener(|this, _e, _w, cx| {
                            this.share_clipboard = !this.share_clipboard;
                            cx.notify();
                        }),
                    )),
            )
            .child(hint("Works with Linux X11 / XWayland screens. Encryption is a prototype, not independently reviewed.".into()));
    }
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_3()
        .child(div().w_full().flex().flex_wrap().gap_3().child(mine).child(theirs))
        .when(!msg.is_empty(), |d| d.child(div().text_sm().child(msg.to_string())))
        .child(options)
}

/// A modal question with two buttons, drawn like the other popups.
fn remote_dialog(
    title: &'static str,
    lines: Vec<String>,
    yes: &'static str,
    no: &'static str,
    on_yes: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    on_no: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let mut body = div().flex().flex_col().gap_2();
    for l in lines {
        body = body.child(div().text_sm().child(l));
    }
    div()
        .absolute()
        .inset_0()
        .occlude()
        .bg(scrim())
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(520.))
                .p_6()
                .rounded(px(16.))
                .bg(popover())
                .border_1()
                .border_color(border())
                .shadow_lg()
                .flex()
                .flex_col()
                .gap_4()
                .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(title))
                .child(body)
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap_3()
                        .child(toolbar_button_dyn("rd-no".into(), no, true, on_no))
                        .child(toolbar_button_dyn("rd-yes".into(), yes, true, on_yes)),
                ),
        )
}

// ============================================================================
// Connections
// ============================================================================

fn connection_view(
    profiles: Vec<Profile>,
    selected_profile: Option<usize>,
    pending_delete: Option<usize>,
    grid_view: bool,
    filter: String,
    has_saved_pw: bool,
    cx: &mut Context<NexDeskApp>,
) -> impl IntoElement {
    let mut grid = div().flex().gap_3();
    grid = if grid_view { grid.flex_wrap() } else { grid.flex_col().gap_1() };

    let total = profiles.len();
    let mut shown = 0;

    for (index, profile) in profiles.into_iter().enumerate() {
        // `index` stays the position in the full list so selection keeps working while filtering.
        if !filter.is_empty()
            && ![&profile.name, &profile.host, &profile.user]
                .iter()
                .any(|f| f.to_lowercase().contains(&filter))
        {
            continue;
        }
        shown += 1;
        let selected = selected_profile == Some(index);

        let on_click = cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
            if index < this.state.profiles.len() {
                this.selected_profile = Some(index);
                this.state.selected = Some(index);
                this.state.status = format!("Selected {}", this.state.profiles[index].name);
                if event.click_count() >= 2 {
                    this.open_password_dialog(window, cx);
                }
            }
            cx.notify();
        });

        if grid_view {
            // Little monitor icon drawn from divs (no icon-font dependency).
            let monitor = div()
                .flex()
                .flex_col()
                .items_center()
                .child(
                    div()
                        .w(px(64.))
                        .h(px(44.))
                        .rounded(px(7.))
                        .bg(rgb_blue_dark())
                        .border_2()
                        .border_color(accent_hover())
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(accent_hover())
                        .text_xs()
                        .child("RDP"),
                )
                .child(div().w(px(10.)).h(px(5.)).bg(accent_hover()))
                .child(div().w(px(30.)).h(px(3.)).rounded_full().bg(accent_hover()));

            grid = grid.child(
                div()
                    .id(SharedString::from(format!("profile-{index}")))
                    .w(px(172.))
                    .p_3()
                    .rounded(px(12.))
                    .bg(if selected { accent_soft() } else { bg() })
                    .when(!selected, |d| d.hover(|s| s.bg(row_hover())))
                    .cursor_pointer()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .on_click(on_click)
                    .child(div().h(px(64.)).flex().items_center().child(monitor))
                    .child(
                        div()
                            .max_w_full()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .overflow_hidden()
                            .child(profile.name.clone()),
                    )
                    .child(
                        div()
                            .max_w_full()
                            .text_xs()
                            .text_color(muted())
                            .overflow_hidden()
                            .child(format!("{} · {}", profile.host, profile.user)),
                    )
                    .child(div().text_xs().text_color(muted()).child(format!(
                        "{}×{} · {}",
                        profile.width,
                        profile.height,
                        profile.speed.label()
                    ))),
            );
        } else {
            grid = grid.child(
                div()
                    .id(SharedString::from(format!("profile-{index}")))
                    .w_full()
                    .h(px(52.))
                    .px_3()
                    .rounded(px(10.))
                    .bg(if selected { accent_soft() } else { bg() })
                    .when(!selected, |d| d.hover(|s| s.bg(row_hover())))
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap_3()
                    .on_click(on_click)
                    .child(
                        div()
                            .w(px(34.))
                            .h(px(24.))
                            .rounded(px(5.))
                            .bg(rgb_blue_dark())
                            .border_1()
                            .border_color(accent_hover()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .overflow_hidden()
                            .child(profile.name.clone()),
                    )
                    .child(div().w(px(220.)).text_sm().text_color(muted()).overflow_hidden().child(profile.host.clone()))
                    .child(div().w(px(120.)).text_sm().text_color(muted()).overflow_hidden().child(profile.user.clone()))
                    .child(div().w(px(100.)).text_xs().text_color(muted()).child(format!("{}×{}", profile.width, profile.height)))
                    .child(div().w(px(90.)).text_xs().text_color(muted()).child(profile.speed.label())),
            );
        }
    }

    if shown == 0 {
        grid = grid.child(
            div()
                .w_full()
                .p_8()
                .rounded(px(12.))
                .bg(panel())
                .text_color(muted())
                .flex()
                .justify_center()
                .child(if total == 0 {
                    "No connections yet. Press New to add one."
                } else {
                    "No connection matches your search."
                }),
        );
    }

    let has_selection = selected_profile.is_some();
    let confirming = pending_delete.is_some() && pending_delete == selected_profile;
    let delete_label = if confirming { "Confirm delete" } else { "Delete" };

    div()
        .flex()
        .flex_col()
        .gap_4()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(action_button("Connect", has_selection, cx))
                .child(toolbar_button("New", true, false, NexDeskApp::open_editor_new, cx))
                .child(toolbar_button("Edit", has_selection, false, NexDeskApp::open_editor_edit, cx))
                .child(toolbar_button("Duplicate", has_selection, false, NexDeskApp::duplicate_selected, cx))
                .when(has_saved_pw, |d| d.child(toolbar_button("Forget password", true, false, |this, _w, cx| this.forget_saved_password(cx), cx)))
                .child(toolbar_button(delete_label, has_selection, confirming, NexDeskApp::delete_selected, cx)),
        )
        .child(
            div()
                .text_sm()
                .text_color(muted())
                .child("Double-click a connection to open it. Passwords are never stored in .rdp files."),
        )
        .child(grid)
}

fn rgb_blue_dark() -> gpui::Rgba {
    blue_dark()
}

// ============================================================================
// Devices
// ============================================================================

fn devices_view(
    profiles: &[Profile],
    conn_log: &[logs::Entry],
    filter: &str,
    scan: Option<&nexdesk_core::discover::Scan>,
    cx: &mut Context<NexDeskApp>,
) -> impl IntoElement {
    let known = nexdesk_core::knownhosts::default_path()
        .map(|p| nexdesk_core::knownhosts::KnownHosts::load(&p))
        .unwrap_or_else(nexdesk_core::knownhosts::KnownHosts::in_memory);

    // One row per computer (several saved connections may point at the same host).
    let mut seen: Vec<String> = Vec::new();
    let mut list = div().w_full().flex().flex_col().gap_2();
    let mut shown = 0;
    for (index, p) in profiles.iter().enumerate() {
        let host_lc = p.host.trim().to_lowercase();
        if seen.contains(&host_lc) {
            continue;
        }
        seen.push(host_lc.clone());
        if !filter.is_empty() && ![&p.name, &p.host, &p.user].iter().any(|f| f.to_lowercase().contains(filter)) {
            continue;
        }
        shown += 1;
        let (name, port) = match p.host.trim().rsplit_once(':') {
            Some((h, port)) if port.parse::<u16>().is_ok() => (h.to_string(), port.parse::<u16>().unwrap_or(3389)),
            _ => (p.host.trim().to_string(), 3389),
        };
        let key = nexdesk_core::knownhosts::host_key(&name, port);
        let pinned = known.is_pinned(&key);
        let st = logs::device_stats(conn_log, p.host.trim());
        let last = if st.last_ts_ms == 0 {
            "Never connected".to_string()
        } else {
            format!("{} · {}", logs::format_time(st.last_ts_ms), st.last_event)
        };
        let key2 = key.clone();
        list = list.child(
            div()
                .id(SharedString::from(format!("dev-{index}")))
                .w_full()
                .h(px(64.))
                .px_4()
                .rounded(px(12.))
                .bg(panel())
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .flex_none()
                        .size(px(10.))
                        .rounded_full()
                        .bg(if st.last_event == "Failed" { danger() } else if st.sessions > 0 { dot_max() } else { rgb_dim() }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(p.host.clone()))
                        .child(div().text_xs().text_color(muted()).child(format!("{} · {}", p.name, p.user))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_xs()
                        .text_color(muted())
                        .child(div().child(last))
                        .child(div().child(format!("{} session(s)", st.sessions))),
                )
                .child(
                    div()
                        .flex_none()
                        .px_3()
                        .py_1()
                        .rounded_full()
                        .text_xs()
                        .bg(if pinned { accent_soft() } else { panel_2() })
                        .text_color(if pinned { text() } else { muted() })
                        .child(if pinned { "Pinned" } else { "Unverified" }),
                )
                .child(toolbar_button_dyn(
                    format!("dev-connect-{index}"),
                    "Connect",
                    true,
                    cx.listener(move |this, _e, window, cx| {
                        this.selected_profile = Some(index);
                        this.state.selected = Some(index);
                        this.open_password_dialog(window, cx);
                    }),
                ))
                .when(pinned, |d| {
                    d.child(toolbar_button_dyn(
                        format!("dev-forget-{index}"),
                        "Forget key",
                        true,
                        cx.listener(move |this, _e, _w, cx| {
                            if let Some(path) = nexdesk_core::knownhosts::default_path() {
                                let mut k = nexdesk_core::knownhosts::KnownHosts::load(&path);
                                let _ = k.forget(&key2);
                                logs::alarm(LogLevel::Info, "Pinned certificate forgotten", &key2, "removed by user from Devices");
                            }
                            this.state.status = "Pinned certificate removed. You will be asked again on the next connection.".into();
                            cx.notify();
                        }),
                    ))
                }),
        );
    }
    if shown == 0 {
        list = list.child(empty_card("No devices yet. Computers appear here once you save a connection."));
    }
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_3()
        .child(nearby_card(scan, profiles, cx))
        .child(div().text_sm().text_color(muted()).child("Computers you have connections to, with last activity and certificate status."))
        .child(list)
}

/// "Computers on this network with Remote Desktop enabled", found by probing port 3389.
fn nearby_card(
    scan: Option<&nexdesk_core::discover::Scan>,
    profiles: &[Profile],
    cx: &mut Context<NexDeskApp>,
) -> impl IntoElement {
    let running = scan.map(|s| !s.is_done()).unwrap_or(false);
    let status = match scan {
        None => "Look for computers on your network that accept Remote Desktop connections (port 3389).".to_string(),
        Some(s) if running => {
            let (a, b) = s.progress();
            format!("Scanning your network... {a}/{b}")
        }
        Some(s) if s.total == 0 => "No network connection found.".to_string(),
        Some(s) => format!("Scan finished: {} computer(s) with Remote Desktop found.", s.results().len()),
    };
    let mut card = div()
        .w_full()
        .p_4()
        .rounded(px(12.))
        .bg(panel())
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Nearby computers"))
                        .child(div().text_xs().text_color(muted()).child(status)),
                )
                .child(toolbar_button_dyn(
                    "scan-network".into(),
                    if running { "Scanning..." } else if scan.is_some() { "Scan again" } else { "Scan network" },
                    !running,
                    cx.listener(|this, _e, _w, cx| {
                        this.scan = Some(nexdesk_core::discover::Scan::start());
                        nexdesk_core::logs::console(LogLevel::Info, "discover", "network scan started (TCP 3389, local subnet)");
                        cx.notify();
                    }),
                )),
        );
    if let Some(s) = scan {
        for (i, f) in s.results().into_iter().enumerate() {
            let ip = f.ip.to_string();
            let saved = profiles.iter().position(|p| {
                let h = p.host.trim();
                h == ip || h.strip_suffix(":3389") == Some(ip.as_str()) || f.name.as_deref().map(|n| n.eq_ignore_ascii_case(h)).unwrap_or(false)
            });
            let title = f.name.clone().unwrap_or_else(|| ip.clone());
            let sub = match (&f.name, f.is_self) {
                (_, true) => format!("{ip} · this computer"),
                (Some(_), _) => ip.clone(),
                _ => "no name published".to_string(),
            };
            let host = ip.clone();
            card = card.child(
                div()
                    .id(SharedString::from(format!("nearby-{i}")))
                    .w_full()
                    .h(px(48.))
                    .px_3()
                    .rounded(px(8.))
                    .bg(panel_2())
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(div().flex_none().size(px(8.)).rounded_full().bg(dot_max()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .child(div().text_sm().child(title))
                            .child(div().text_xs().text_color(muted()).child(sub)),
                    )
                    .child(match saved {
                        Some(idx) => toolbar_button_dyn(
                            format!("nearby-connect-{i}"),
                            "Connect",
                            true,
                            cx.listener(move |this, _e, window, cx| {
                                this.selected_profile = Some(idx);
                                this.state.selected = Some(idx);
                                this.open_password_dialog(window, cx);
                            }),
                        )
                        .into_any_element(),
                        None => toolbar_button_dyn(
                            format!("nearby-add-{i}"),
                            "Add connection",
                            true,
                            cx.listener(move |this, _e, window, cx| this.add_discovered(host.clone(), window, cx)),
                        )
                        .into_any_element(),
                    }),
            );
        }
        if !running && s.total > 0 && s.results().is_empty() {
            card = card.child(div().text_xs().text_color(muted()).child(
                "Nothing found. The other computer must be on, awake and on this network, with Remote Desktop enabled (Windows Pro/Enterprise/Server, not Home) and port 3389 allowed in its firewall.",
            ));
        }
    }
    card
}

fn empty_card(msg: &'static str) -> impl IntoElement {
    div().w_full().p_8().rounded(px(12.)).bg(panel()).text_color(muted()).flex().justify_center().child(msg)
}

/// Small pill button with a runtime id and an arbitrary click handler.
fn toolbar_button_dyn(
    id: String,
    label: &'static str,
    enabled: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id))
        .flex_none()
        .px_3()
        .h(px(30.))
        .flex()
        .items_center()
        .rounded(px(8.))
        .text_sm()
        .bg(panel_2())
        .text_color(if enabled { text() } else { muted() })
        .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(hover())).on_click(on_click))
        .child(label)
}

// ============================================================================
// Address books
// ============================================================================

fn books_view(
    books: &AddressBooks,
    sel: Option<usize>,
    profiles: &[Profile],
    book_input: Entity<TextInput>,
    cx: &mut Context<NexDeskApp>,
) -> impl IntoElement {
    let mut left = div().flex().flex_col().gap_1().w(px(240.)).flex_none();
    for (i, b) in books.books.iter().enumerate() {
        let on = sel == Some(i);
        left = left.child(
            div()
                .id(SharedString::from(format!("book-{i}")))
                .h(px(38.))
                .px_3()
                .rounded(px(8.))
                .flex()
                .items_center()
                .justify_between()
                .bg(if on { accent_soft() } else { bg() })
                .when(!on, |d| d.hover(|s| s.bg(row_hover())))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _e, _w, cx| {
                    this.book_sel = Some(i);
                    cx.notify();
                }))
                .child(div().text_sm().overflow_hidden().child(b.name.clone()))
                .child(div().text_xs().text_color(muted()).child(b.members.len().to_string())),
        );
    }
    if books.books.is_empty() {
        left = left.child(div().text_sm().text_color(muted()).p_2().child("No address books yet."));
    }
    left = left.child(
        div()
            .mt_2()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().p_2().rounded(px(8.)).bg(input_bg()).child(book_input))
            .child(toolbar_button_dyn(
                "book-create".into(),
                "Create address book",
                true,
                cx.listener(|this, _e, _w, cx| {
                    let name = this.book_input.read(cx).value().to_string();
                    match this.books.create(&name) {
                        Ok(()) => {
                            this.book_sel = this.books.books.iter().position(|b| b.name.eq_ignore_ascii_case(name.trim()));
                            this.book_input.update(cx, |i, cx| i.set_value("", cx));
                            this.state.status = format!("Created address book \"{}\"", name.trim());
                        }
                        Err(e) => this.state.status = e.to_string(),
                    }
                    cx.notify();
                }),
            )),
    );

    let mut right = div().flex_1().flex().flex_col().gap_3();
    match sel.and_then(|i| books.books.get(i).map(|b| (i, b))) {
        None => {
            right = right.child(empty_card("Select or create an address book."));
        }
        Some((bi, book)) => {
            let book_name = book.name.clone();
            let bn_delete = book_name.clone();
            right = right.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_lg().font_weight(FontWeight::BOLD).child(book_name.clone()))
                    .child(toolbar_button_dyn(
                        "book-delete".into(),
                        "Delete book",
                        true,
                        cx.listener(move |this, _e, _w, cx| {
                            this.books.delete(&bn_delete);
                            this.book_sel = None;
                            cx.notify();
                        }),
                    )),
            );
            let mut rows = div().flex().flex_col().gap_1();
            let mut any = false;
            for m in &book.members {
                let Some(pi) = profiles.iter().position(|p| &p.name == m) else { continue };
                any = true;
                let p = &profiles[pi];
                let (b1, m1) = (book_name.clone(), m.clone());
                rows = rows.child(
                    div()
                        .h(px(48.))
                        .px_4()
                        .rounded(px(10.))
                        .bg(panel())
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(div().flex_1().text_sm().font_weight(FontWeight::SEMIBOLD).child(p.name.clone()))
                        .child(div().w(px(220.)).text_xs().text_color(muted()).child(format!("{} · {}", p.host, p.user)))
                        .child(toolbar_button_dyn(
                            format!("bk-connect-{bi}-{pi}"),
                            "Connect",
                            true,
                            cx.listener(move |this, _e, window, cx| {
                                this.selected_profile = Some(pi);
                                this.state.selected = Some(pi);
                                this.open_password_dialog(window, cx);
                            }),
                        ))
                        .child(toolbar_button_dyn(
                            format!("bk-remove-{bi}-{pi}"),
                            "Remove",
                            true,
                            cx.listener(move |this, _e, _w, cx| {
                                this.books.remove(&b1, &m1);
                                cx.notify();
                            }),
                        )),
                );
            }
            if !any {
                rows = rows.child(empty_card("This book is empty. Add connections below."));
            }
            right = right.child(rows);

            // connections that are not in this book yet
            let mut chips = div().flex().flex_wrap().gap_2();
            let mut addable = 0;
            for (pi, p) in profiles.iter().enumerate() {
                if book.members.iter().any(|m| m == &p.name) {
                    continue;
                }
                addable += 1;
                let (b2, n2) = (book_name.clone(), p.name.clone());
                chips = chips.child(
                    div()
                        .id(SharedString::from(format!("bk-add-{bi}-{pi}")))
                        .px_3()
                        .h(px(30.))
                        .flex()
                        .items_center()
                        .rounded_full()
                        .bg(panel_2())
                        .text_sm()
                        .cursor_pointer()
                        .hover(|s| s.bg(hover()))
                        .on_click(cx.listener(move |this, _e, _w, cx| {
                            let _ = this.books.add(&b2, &n2);
                            cx.notify();
                        }))
                        .child(format!("+ {}", p.name)),
                );
            }
            if addable > 0 {
                right = right
                    .child(div().mt_2().text_sm().text_color(muted()).child("Add a connection"))
                    .child(chips);
            }
        }
    }
    div().flex().gap_6().child(left).child(right)
}

// ============================================================================
// Logs
// ============================================================================

fn col_width(kind: LogKind, i: usize) -> Option<f32> {
    // None = takes the remaining width
    match (kind, i) {
        (LogKind::Connection, 0) => Some(110.),
        (LogKind::Connection, 1) => Some(170.),
        (LogKind::Connection, 2) => Some(190.),
        (LogKind::Connection, 3) => Some(110.),
        (LogKind::File, 0) => Some(240.),
        (LogKind::File, 1) => Some(60.),
        (LogKind::File, 2) => Some(80.),
        (LogKind::File, 3) => Some(220.),
        (LogKind::Alarm, 0) => Some(360.),
        (LogKind::Alarm, 1) => Some(200.),
        (LogKind::Console, 0) => Some(100.),
        _ => None,
    }
}

fn logs_view(
    kind: LogKind,
    entries: &[logs::Entry],
    filter: &str,
    confirm_clear: bool,
    cx: &mut Context<NexDeskApp>,
) -> impl IntoElement {
    let cols = kind.columns();
    let cell = |i: usize, content: String, color: gpui::Rgba| {
        let d = div().text_xs().text_color(color).overflow_hidden().px_1();
        match col_width(kind, i) {
            Some(w) => d.w(px(w)).flex_none().child(content),
            None => d.flex_1().child(content),
        }
    };

    let mut head = div().h(px(30.)).px_3().flex().items_center().text_xs().text_color(muted()).font_weight(FontWeight::SEMIBOLD);
    head = head.child(div().w(px(150.)).flex_none().px_1().child("Time"));
    for (i, c) in cols.iter().enumerate() {
        let d = div().px_1().overflow_hidden();
        head = head.child(match col_width(kind, i) {
            Some(w) => d.w(px(w)).flex_none().child(*c),
            None => d.flex_1().child(*c),
        });
    }

    let mut body = div().flex().flex_col().rounded(px(12.)).bg(panel()).overflow_hidden();
    body = body.child(head).child(div().h(px(1.)).bg(border()));
    let mut shown = 0;
    for e in entries {
        if !filter.is_empty() && !e.fields.iter().any(|f| f.to_lowercase().contains(filter)) {
            continue;
        }
        shown += 1;
        let (color, tint) = match e.level {
            LogLevel::Error => (danger(), gpui::rgba(0xff7b6318)),
            LogLevel::Warn => (warn(), gpui::rgba(0xf5c21112)),
            LogLevel::Info => (text(), gpui::rgba(0x00000000)),
        };
        let mut row = div()
            .min_h(px(28.))
            .px_3()
            .py_1()
            .flex()
            .items_start()
            .bg(tint)
            .hover(|s| s.bg(row_hover()))
            .child(cell_time(logs::format_time(e.ts_ms)));
        for i in 0..cols.len() {
            let v = e.fields.get(i).cloned().unwrap_or_default();
            let mut c = cell(i, v, if i == 0 { color } else { text() });
            if kind == LogKind::Console && i == 1 {
                c = c.font_family("Monospace");
            }
            row = row.child(c);
        }
        body = body.child(row);
    }
    if shown == 0 {
        body = body.child(div().p_8().flex().justify_center().text_sm().text_color(muted()).child(match kind {
            LogKind::Connection => "No connection events yet.",
            LogKind::File => "No file transfers yet. Copy files between this computer and a remote desktop.",
            LogKind::Alarm => "No alarms. Certificate and security events appear here.",
            LogKind::Console => "No console output yet.",
        }));
    }

    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(toolbar_button_dyn(
                    "logs-refresh".into(),
                    "Refresh",
                    true,
                    cx.listener(|this, _e, _w, cx| {
                        this.refresh_logs();
                        cx.notify();
                    }),
                ))
                .child(toolbar_button_dyn(
                    "logs-clear".into(),
                    if confirm_clear { "Confirm clear" } else { "Clear" },
                    true,
                    cx.listener(move |this, _e, _w, cx| {
                        if let Screen::Logs(k) = this.nav.screen {
                            if this.confirm_clear {
                                logs::clear(k);
                                this.confirm_clear = false;
                                this.state.status = format!("{} log cleared.", k.label());
                            } else {
                                this.confirm_clear = true;
                            }
                            this.refresh_logs();
                        }
                        cx.notify();
                    }),
                ))
                .child(div().ml_2().text_xs().text_color(muted()).child(format!("{shown} entries (newest first, last 500). Passwords and file contents are never logged."))),
        )
        .child(body)
}

fn cell_time(t: String) -> impl IntoElement {
    div().w(px(150.)).flex_none().px_1().text_xs().text_color(muted()).child(t)
}

// ============================================================================
// Sessions
// ============================================================================

fn session_view(
    sessions: Vec<(u64, Profile, SessionState)>,
    cx: &mut Context<NexDeskApp>,
) -> impl IntoElement {
    if sessions.is_empty() {
        return div().flex().flex_col().gap_2().child(
            div()
                .p_5()
                .rounded(px(12.))
                .bg(panel())
                .text_color(muted())
                .child("No active sessions."),
        );
    }

    let mut list = div().flex().flex_col().gap_2();

    for (id, profile, state) in sessions {
        let can_disconnect = matches!(
            state,
            SessionState::Created
                | SessionState::Starting
                | SessionState::Connecting
                | SessionState::Connected
                | SessionState::Reconnecting
        );

        let card = div()
            .p_4()
            .rounded(px(12.))
            .bg(panel())
            .flex()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .child(profile.name.clone()),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_sm()
                            .text_color(muted())
                            .child(format!("{}  •  {}", profile.host, profile.user)),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_xs()
                            .text_color(muted())
                            .child(format!("Session #{id}  •  {:?}", state)),
                    ),
            );

        if can_disconnect {
            list = list.child(
                card.child(
                    div()
                        .id(SharedString::from(format!("disconnect-{id}")))
                        .px_3()
                        .py_2()
                        .rounded(px(8.))
                        .bg(panel_2())
                        .hover(|s| s.bg(hover()))
                        .text_color(text())
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.disconnect_session(id, cx);
                        }))
                        .child("Disconnect"),
                ),
            );
        } else {
            list = list.child(card);
        }
    }

    list
}

// ============================================================================
// Settings
// ============================================================================

fn switch(id: &'static str, on: bool, cx: &mut Context<NexDeskApp>, f: fn(&mut Settings, bool)) -> impl IntoElement {
    div()
        .id(id)
        .flex_none()
        .w(px(44.))
        .h(px(24.))
        .rounded_full()
        .p(px(2.))
        .flex()
        .when(on, |d| d.justify_end())
        .bg(if on { accent() } else { hover() })
        .cursor_pointer()
        .on_click(cx.listener(move |this, _e, _w, cx| this.update_settings(cx, |s| f(s, !on))))
        .child(div().size(px(20.)).rounded_full().bg(gpui::rgb(0xffffff)))
}

fn seg_btn(id: String, label: &'static str, on: bool, cx: &mut Context<NexDeskApp>, f: impl Fn(&mut Settings) + 'static) -> impl IntoElement {
    div()
        .id(SharedString::from(id))
        .px_3()
        .h(px(30.))
        .flex()
        .items_center()
        .text_sm()
        .rounded(px(7.))
        .bg(if on { accent() } else { panel_2() })
        .text_color(if on { on_accent() } else { text() })
        .cursor_pointer()
        .when(!on, |d| d.hover(|s| s.bg(hover())))
        .on_click(cx.listener(move |this, _e, _w, cx| this.update_settings(cx, |s| f(s))))
        .child(label)
}

fn pref_row(title: &'static str, desc: &'static str, control: impl IntoElement) -> impl IntoElement {
    div()
        .w_full()
        .min_h(px(60.))
        .px_4()
        .py_2()
        .flex()
        .items_center()
        .gap_4()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(div().text_sm().child(title))
                .child(div().text_xs().text_color(muted()).child(desc)),
        )
        .child(control)
}

fn pref_group(title: &'static str, rows: Vec<gpui::AnyElement>) -> impl IntoElement {
    let mut card = div().w_full().rounded(px(12.)).bg(panel()).overflow_hidden().flex().flex_col();
    let n = rows.len();
    for (i, r) in rows.into_iter().enumerate() {
        card = card.child(r);
        if i + 1 < n {
            card = card.child(div().h(px(1.)).mx_4().bg(border()));
        }
    }
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_2()
        .child(div().px_1().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(muted()).child(title))
        .child(card)
}

fn settings_view(p: &Settings, vs: VaultStatus, cx: &mut Context<NexDeskApp>) -> impl IntoElement {
    let theme_ctl = div().flex().gap_1().children(Theme::ALL.map(|t| {
        seg_btn(format!("pref-theme-{}", t.label()), t.label(), p.theme == t, cx, move |s| s.theme = t).into_any_element()
    }));
    let speed_ctl = div().flex().gap_1().children(Speed::ALL.map(|v| {
        seg_btn(format!("pref-speed-{}", v.label()), v.label(), p.default_speed == v, cx, move |s| s.default_speed = v).into_any_element()
    }));
    let tls_ctl = div().flex().gap_1().children(TlsMode::ALL.map(|m| {
        seg_btn(format!("pref-tls-{}", m.label()), m.label(), p.tls == m, cx, move |s| s.tls = m).into_any_element()
    }));

    div()
        .w_full()
        .max_w(px(820.))
        .flex()
        .flex_col()
        .gap_5()
        .child(pref_group(
            "APPEARANCE",
            vec![
                pref_row("Theme", "Graphite is neutral dark, Midnight is darker, Light is for bright rooms.", theme_ctl).into_any_element(),
                pref_row("Start with the sidebar collapsed", "Icons only; toggle any time with the button in the header.", switch("pref-sidebar", p.sidebar_collapsed, cx, |s, v| s.sidebar_collapsed = v)).into_any_element(),
            ],
        ))
        .child(pref_group(
            "NEW CONNECTIONS",
            vec![pref_row("Default connection speed", "Slow network turns off wallpaper, themes and uses 16-bit colour.", speed_ctl).into_any_element()],
        ))
        .child(pref_group(
            "SESSION WINDOW",
            vec![
                pref_row("Open sessions in full screen", "Applies to every connection.", switch("pref-fs", p.start_fullscreen, cx, |s, v| s.start_fullscreen = v)).into_any_element(),
                pref_row("Send Super and Alt+Tab to the remote", "In full screen, shortcuts go to the remote computer instead of this desktop.", switch("pref-keys", p.key_capture, cx, |s, v| s.key_capture = v)).into_any_element(),
                pref_row("Paste automatically after dropping files", "Presses Ctrl+V on the remote after you drop files on the session window.", switch("pref-drop", p.drop_paste, cx, |s, v| s.drop_paste = v)).into_any_element(),
                pref_row("Use the system title bar", "Turn on if the custom header bar misbehaves with your window manager.", switch("pref-frame", p.native_frame, cx, |s, v| s.native_frame = v)).into_any_element(),
            ],
        ))
        .child(pref_group("PASSWORD VAULT", vault_rows(vs, cx)))
        .child(pref_group(
            "SECURITY",
            vec![
                pref_row("Server certificates", "Ask: confirm unknown and changed certificates. Accept new: pin silently, still refuse changes. Strict: only already-trusted servers.", tls_ctl).into_any_element(),
                pref_row("Passwords", "Never saved. They are passed to the session through a private environment variable.", div().text_xs().text_color(muted()).child("Always")).into_any_element(),
            ],
        ))
        .child(div().text_xs().text_color(muted()).child("Preferences are saved in ~/.config/nexdesk/settings and apply to the next session you start."))
}

fn vault_rows(vs: VaultStatus, cx: &mut Context<NexDeskApp>) -> Vec<gpui::AnyElement> {
    let btn = |id: &str, label: &'static str, cx: &mut Context<NexDeskApp>, f: fn(&mut NexDeskApp, &mut Window, &mut Context<NexDeskApp>)| {
        toolbar_button_dyn(id.to_string(), label, true, cx.listener(move |this, _e, w, cx| f(this, w, cx))).into_any_element()
    };
    let mut rows: Vec<gpui::AnyElement> = Vec::new();
    if !vs.exists {
        rows.push(
            pref_row(
                "Save passwords securely",
                "Protect saved passwords with one master password (Argon2id + XChaCha20-Poly1305). Without a vault, passwords are asked each time.",
                btn("vault-setup", "Set up vault", cx, |this, w, cx| this.open_vault_dialog(VaultDlg::Setup, w, cx)),
            )
            .into_any_element(),
        );
    } else if !vs.unlocked {
        rows.push(
            pref_row(
                "Vault is locked",
                "Unlock it to use and save passwords.",
                div()
                    .flex()
                    .gap_2()
                    .child(btn("vault-unlock", "Unlock", cx, |this, w, cx| this.open_vault_dialog(VaultDlg::Unlock, w, cx)))
                    .child(btn("vault-recover", "Use recovery key", cx, |this, w, cx| this.open_vault_dialog(VaultDlg::UseRecovery, w, cx))),
            )
            .into_any_element(),
        );
    } else {
        rows.push(
            pref_row(
                "Vault is unlocked",
                if vs.count == 1 { "1 saved password. Connections with a saved password connect without asking." } else { "Connections with a saved password connect without asking." },
                div()
                    .flex()
                    .gap_2()
                    .child(btn("vault-lock", "Lock now", cx, |this, _w, cx| this.lock_vault(cx))),
            )
            .into_any_element(),
        );
        rows.push(
            pref_row(
                "Master password",
                "Your recovery key keeps working after you change it.",
                btn("vault-change", "Change…", cx, |this, w, cx| this.open_vault_dialog(VaultDlg::ChangeMaster, w, cx)),
            )
            .into_any_element(),
        );
        rows.push(
            pref_row(
                "Saved passwords",
                "Remove every password from the vault (the vault itself stays).",
                btn(
                    "vault-wipe",
                    if vs.confirm_wipe { "Click again to confirm" } else { "Delete all" },
                    cx,
                    |this, _w, cx| this.wipe_vault_passwords(cx),
                ),
            )
            .into_any_element(),
        );
    }
    rows
}

fn vault_dialog_view(
    kind: VaultDlg,
    inputs: [Entity<TextInput>; 3],
    error: Option<String>,
    new_key: Option<String>,
    vault_exists: bool,
    cx: &mut Context<NexDeskApp>,
) -> impl IntoElement {
    let [pw1, pw2, rec] = inputs;
    let field = |input: Entity<TextInput>| {
        div().p_3().rounded(px(8.)).bg(input_bg()).border_1().border_color(border()).line_height(px(24.)).child(input)
    };
    let (title, blurb): (&str, &str) = match kind {
        VaultDlg::Setup => ("Set up the password vault", "Choose a master password (at least 8 characters). Saved connection passwords are encrypted with it. NexDesk cannot recover it for you; you will get a recovery key next."),
        VaultDlg::Unlock => ("Unlock the password vault", "Enter your master password to use saved passwords."),
        VaultDlg::UseRecovery => ("Unlock with the recovery key", "Type the recovery key you saved when the vault was created. You will then choose a new master password."),
        VaultDlg::ChangeMaster => ("Choose a new master password", "At least 8 characters. The recovery key stays the same."),
        VaultDlg::ShowRecoveryKey => ("Save your recovery key", "This is the ONLY way to open the vault if you forget the master password. Store it somewhere safe and offline (password manager, printed copy). It is shown only now."),
    };
    let mut body = div().flex().flex_col().gap_3();
    match kind {
        VaultDlg::Setup | VaultDlg::ChangeMaster => {
            body = body.child(field(pw1)).child(field(pw2));
        }
        VaultDlg::Unlock => body = body.child(field(pw1)),
        VaultDlg::UseRecovery => body = body.child(field(rec)),
        VaultDlg::ShowRecoveryKey => {
            let key = new_key.clone().unwrap_or_default();
            let copy = key.clone();
            body = body
                .child(
                    div()
                        .p_4()
                        .rounded(px(10.))
                        .bg(input_bg())
                        .border_1()
                        .border_color(warn())
                        .font_family("Monospace")
                        .text_sm()
                        .child(key),
                )
                .child(toolbar_button_dyn(
                    "vault-copy-key".into(),
                    "Copy to clipboard",
                    true,
                    cx.listener(move |this, _e, _w, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                        this.state.status = "Recovery key copied. Paste it somewhere safe, then clear your clipboard.".into();
                        cx.notify();
                    }),
                ));
        }
    }
    let primary = match kind {
        VaultDlg::Setup => "Create vault",
        VaultDlg::Unlock => "Unlock",
        VaultDlg::UseRecovery => "Unlock",
        VaultDlg::ChangeMaster => "Change password",
        VaultDlg::ShowRecoveryKey => "I have saved the key",
    };
    div()
        .absolute()
        .inset_0()
        .occlude() // nothing behind the popup may react to the mouse
        .bg(scrim())
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(520.))
                .p_6()
                .rounded(px(16.))
                .bg(popover())
                .border_1()
                .border_color(border())
                .shadow_lg()
                .flex()
                .flex_col()
                .gap_4()
                .child(popup_header(
                    520.,
                    div().when(kind != VaultDlg::ShowRecoveryKey, |d| {
                        // The recovery key must be acknowledged, so that popup has no close dot.
                        d.child(popup_close("pc-vault", cx, move |this, w, cx| {
                            if kind == VaultDlg::UseRecovery && vault_exists {
                                this.open_vault_dialog(VaultDlg::Unlock, w, cx)
                            } else {
                                this.close_vault_dialog(cx)
                            }
                        }))
                    }),
                    Some("shield"),
                    title,
                ))
                .child(div().text_sm().text_color(muted()).child(blurb))
                .child(body)
                .when_some(error, |d, e| d.child(div().text_sm().text_color(danger()).child(e)))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div().flex().gap_2().when(kind == VaultDlg::Unlock, |d| {
                                d.child(toolbar_button_dyn(
                                    "vault-use-recovery".into(),
                                    "Use recovery key",
                                    true,
                                    cx.listener(|this, _e, w, cx| this.open_vault_dialog(VaultDlg::UseRecovery, w, cx)),
                                ))
                            }),
                        )
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .when(kind != VaultDlg::ShowRecoveryKey, |d| {
                                    d.child(toolbar_button_dyn(
                                        "vault-cancel".into(),
                                        if kind == VaultDlg::Unlock { "Skip" } else { "Cancel" },
                                        true,
                                        cx.listener(move |this, _e, w, cx| {
                                            if kind == VaultDlg::UseRecovery && vault_exists {
                                                this.open_vault_dialog(VaultDlg::Unlock, w, cx)
                                            } else {
                                                this.close_vault_dialog(cx)
                                            }
                                        }),
                                    ))
                                })
                                .child(
                                    div()
                                        .id("vault-primary")
                                        .px_5()
                                        .h(px(34.))
                                        .flex()
                                        .items_center()
                                        .rounded(px(8.))
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .bg(accent())
                                        .text_color(on_accent())
                                        .cursor_pointer()
                                        .hover(|s| s.bg(accent_hover()))
                                        .on_click(cx.listener(|this, _e, w, cx| this.vault_submit(w, cx)))
                                        .child(primary),
                                ),
                        ),
                ),
        )
}

// ---- header "⋯" menu and its dialogs

fn menu_item(id: &'static str, label: &'static str, shortcut: &'static str, cx: &mut Context<NexDeskApp>, f: fn(&mut NexDeskApp, &mut Context<NexDeskApp>)) -> impl IntoElement {
    div()
        .id(id)
        .h(px(34.))
        .px_3()
        .rounded(px(8.))
        .flex()
        .items_center()
        .justify_between()
        .text_sm()
        .cursor_pointer()
        .hover(|s| s.bg(row_hover()))
        .on_click(cx.listener(move |this, _e, _w, cx| {
            this.menu_open = false;
            f(this, cx);
            cx.notify();
        }))
        .child(label)
        .child(div().text_xs().text_color(muted()).child(shortcut))
}

fn menu_popover(cx: &mut Context<NexDeskApp>) -> impl IntoElement {
    div()
        .id("menu-backdrop")
        .absolute()
        .inset_0()
        .occlude()
        .on_click(cx.listener(|this, _e, _w, cx| {
            this.menu_open = false;
            cx.notify();
        }))
        .child(
            div()
                .absolute()
                .top(px(54.))
                .right(px(12.))
                .w(px(250.))
                .p_1()
                .rounded(px(12.))
                .bg(popover())
                .border_1()
                .border_color(border())
                .shadow_lg()
                .flex()
                .flex_col()
                .child(menu_item("m-prefs", "Preferences", "", cx, |this, cx| this.navigate(Screen::Settings, cx)))
                .child(menu_item("m-keys", "Keyboard Shortcuts", "", cx, |this, _| this.info_dialog = Some(InfoDialog::Shortcuts)))
                .child(div().h(px(1.)).my_1().mx_2().bg(border()))
                .child(menu_item("m-about", "About NexDesk", "", cx, |this, _| this.info_dialog = Some(InfoDialog::About))),
        )
}

fn info_dialog(d: InfoDialog, cx: &mut Context<NexDeskApp>) -> impl IntoElement {
    let (title, rows): (&str, Vec<(&str, &str)>) = match d {
        InfoDialog::Shortcuts => (
            "Keyboard Shortcuts",
            vec![
                ("Ctrl+Alt+Break", "Toggle full screen in a session"),
                ("Ctrl+Alt+End", "Send Ctrl+Alt+Del to the remote computer"),
                ("Mouse to the top edge", "Show the toolbar in full screen"),
                ("Super / Alt+Tab", "Go to the remote computer while in full screen"),
                ("Double-click a connection", "Connect"),
                ("Ctrl+C / Ctrl+V", "Copy and paste text, images and files both ways"),
            ],
        ),
        InfoDialog::About => (
            "About NexDesk",
            vec![
                ("Version", env!("CARGO_PKG_VERSION")),
                ("Protocol engine", "IronRDP (Rust)"),
                ("Interface", "GPUI"),
                ("Data", "~/.config/nexdesk and ~/.local/share/nexdesk"),
            ],
        ),
    };
    let mut body = div().flex().flex_col().gap_2();
    for (k, v) in rows {
        body = body.child(
            div()
                .flex()
                .gap_4()
                .child(div().w(px(180.)).flex_none().text_sm().font_weight(FontWeight::SEMIBOLD).child(k))
                .child(div().flex_1().text_sm().text_color(muted()).child(v)),
        );
    }
    div()
        .absolute()
        .inset_0()
        .occlude() // nothing behind the popup may react to the mouse
        .bg(scrim())
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(560.))
                .p_6()
                .rounded(px(16.))
                .bg(popover())
                .border_1()
                .border_color(border())
                .shadow_lg()
                .flex()
                .flex_col()
                .gap_4()
                .child(popup_header(
                    560.,
                    popup_close("pc-info", cx, |this, _w, cx| {
                        this.info_dialog = None;
                        cx.notify();
                    }),
                    None,
                    title,
                ))
                .child(body)
                .child(
                    div().flex().justify_end().child(toolbar_button_dyn(
                        "info-close".into(),
                        "Close",
                        true,
                        cx.listener(|this, _e, _w, cx| {
                            this.info_dialog = None;
                            cx.notify();
                        }),
                    )),
                ),
        )
}

// ============================================================================
// Password dialog
// ============================================================================

fn password_dialog(
    profile_name: String,
    password_input: Entity<TextInput>,
    error: Option<String>,
    save_choice: Option<bool>,
    vault_exists: bool,
    cx: &mut Context<NexDeskApp>,
) -> impl IntoElement {
    let password_empty = password_input.read(cx).value().is_empty();

    div()
        .absolute()
        .inset_0()
        .occlude() // nothing behind the popup may react to the mouse
        .bg(scrim())
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(440.))
                .p_6()
                .rounded(px(16.))
                .bg(panel())
                .border_1()
                .border_color(border())
                .shadow_lg()
                .flex()
                .flex_col()
                .gap_4()
                .child(popup_header(
                    440.,
                    popup_close("pc-password", cx, |this, w, cx| this.cancel_password_dialog(w, cx)),
                    None,
                    "Connect",
                ))
                .child(
                    div()
                        .text_sm()
                        .text_color(muted())
                        .child(format!("Enter the password for {profile_name}.")),
                )
                .child(
                    div()
                        .p_3()
                        .rounded(px(8.))
                        .border_2()
                        .border_color(accent())
                        .bg(input_bg())
                        .line_height(px(24.))
                        .child(password_input),
                )
                .when_some(error, |element, error| {
                    element.child(div().text_sm().text_color(danger()).child(error))
                })
                .child(match save_choice {
                    Some(on) => div()
                        .id("save-pw")
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _e, _w, cx| {
                            this.save_pw = !this.save_pw;
                            cx.notify();
                        }))
                        .child(
                            div()
                                .size(px(18.))
                                .rounded(px(5.))
                                .border_1()
                                .border_color(if on { accent() } else { hover() })
                                .bg(if on { accent() } else { input_bg() })
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(if on { ico("check", 12., on_accent()).into_any_element() } else { div().into_any_element() }),
                        )
                        .child(div().text_sm().child("Save this password in the vault (encrypted)"))
                        .into_any_element(),
                    None => div()
                        .text_xs()
                        .text_color(muted())
                        .child(if vault_exists {
                            "The password vault is locked. Unlock it in Preferences to save passwords."
                        } else {
                            "Tip: set up the password vault in Preferences to save passwords securely."
                        })
                        .into_any_element(),
                })
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(dialog_button("Cancel", false, cx))
                        .child(dialog_button("Connect", !password_empty, cx)),
                ),
        )
}

// ============================================================================
// Connection editor dialog
// ============================================================================

fn field_row(label: &'static str, input: Entity<TextInput>) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_3()
        .child(div().w(px(120.)).text_sm().text_color(muted()).child(label))
        .child(
            div()
                .flex_1()
                .p_2()
                .rounded(px(8.))
                .bg(input_bg())
                .child(input),
        )
}

fn checkbox_row(
    id: &'static str,
    label: &'static str,
    checked: bool,
    cx: &mut Context<NexDeskApp>,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .on_click(cx.listener(move |this, _event, _window, cx| {
            this.toggle_editor_flag(id, cx);
        }))
        .child(
            div()
                .w(px(18.))
                .h(px(18.))
                .rounded(px(5.))
                .border_1()
                .border_color(if checked { accent() } else { hover() })
                .bg(if checked { accent() } else { input_bg() })
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .text_color(on_accent())
                .child(if checked { "✓" } else { "" }),
        )
        .child(div().text_sm().child(label))
}

/// Segmented control: LAN / Balanced / Slow network.
fn speed_row(current: Speed, cx: &mut Context<NexDeskApp>) -> impl IntoElement {
    let seg = |id: &'static str, label: &'static str, which: Speed, cx: &mut Context<NexDeskApp>| {
        let on = current == which;
        div()
            .id(id)
            .px_4()
            .h(px(30.))
            .flex()
            .items_center()
            .text_sm()
            .rounded(px(7.))
            .bg(if on { accent() } else { panel_2() })
            .text_color(if on { on_accent() } else { text() })
            .cursor_pointer()
            .when(!on, |d| d.hover(|s| s.bg(hover())))
            .on_click(cx.listener(move |this, _e, _w, cx| this.toggle_editor_flag(id, cx)))
            .child(label)
    };
    div()
        .flex()
        .items_center()
        .gap_3()
        .child(div().w(px(120.)).text_sm().text_color(muted()).child("Connection speed"))
        .child(
            div()
                .flex()
                .gap_1()
                .child(seg("speed-lan", "LAN", Speed::Lan, cx))
                .child(seg("speed-balanced", "Balanced", Speed::Balanced, cx))
                .child(seg("speed-slow", "Slow network", Speed::Slow, cx)),
        )
}

fn editor_dialog(ed: &Editor, cx: &mut Context<NexDeskApp>) -> impl IntoElement {
    let title = if ed.original.is_some() {
        "Edit connection"
    } else {
        "New connection"
    };

    div()
        .absolute()
        .inset_0()
        .occlude() // nothing behind the popup may react to the mouse
        .bg(scrim())
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(580.))
                .p_6()
                .rounded(px(16.))
                .bg(panel())
                .border_1()
                .border_color(border())
                .shadow_lg()
                .flex()
                .flex_col()
                .gap_3()
                .child(popup_header(
                    580.,
                    popup_close("pc-editor", cx, |this, _w, cx| this.cancel_editor(cx)),
                    None,
                    title,
                ))
                .child(field_row("Computer", ed.host.clone()))
                .child(field_row("User name", ed.user.clone()))
                .child(field_row("Domain", ed.domain.clone()))
                .child(field_row("Display name", ed.name.clone()))
                .child(field_row("Before connect", ed.pre_cmd.clone()))
                .child(field_row("After disconnect", ed.post_cmd.clone()))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(div().w(px(120.)).text_sm().text_color(muted()).child("Desktop size"))
                        .child(
                            div()
                                .w(px(110.))
                                .p_2()
                                .rounded(px(8.))
                                .bg(input_bg())
                                .child(ed.width.clone()),
                        )
                        .child(div().text_color(muted()).child("×"))
                        .child(
                            div()
                                .w(px(110.))
                                .p_2()
                                .rounded(px(8.))
                                .bg(input_bg())
                                .child(ed.height.clone()),
                        ),
                )
                .child(speed_row(ed.speed, cx))
                .child(
                    div()
                        .mt_1()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(checkbox_row("clipboard", "Redirect clipboard", ed.clipboard, cx))
                        .child(checkbox_row("fullscreen", "Start in full screen", ed.fullscreen, cx)),
                )
                .when_some(ed.error.clone(), |element, error| {
                    element.child(div().text_sm().text_color(danger()).child(error))
                })
                .child(
                    div()
                        .mt_2()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            div()
                                .id("editor-cancel")
                                .px_4()
                                .py_2()
                                .rounded(px(8.))
                                .bg(panel_2())
                                .hover(|s| s.bg(hover()))
                                .text_color(text())
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _event, _window, cx| {
                                    this.cancel_editor(cx);
                                }))
                                .child("Cancel"),
                        )
                        .child(
                            div()
                                .id("editor-save")
                                .px_4()
                                .py_2()
                                .rounded(px(8.))
                                .bg(accent())
                                .hover(|s| s.bg(accent_hover()))
                                .text_color(on_accent())
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _event, _window, cx| {
                                    this.save_editor(cx);
                                }))
                                .child("Save"),
                        ),
                ),
        )
}

// ============================================================================
// Buttons
// ============================================================================

fn toolbar_button(
    label: &'static str,
    enabled: bool,
    destructive: bool,
    action: fn(&mut NexDeskApp, &mut Window, &mut Context<NexDeskApp>),
    cx: &mut Context<NexDeskApp>,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("toolbar-{label}")))
        .px_4()
        .h(px(34.))
        .flex()
        .items_center()
        .rounded(px(8.))
        .text_sm()
        .bg(panel_2())
        .text_color(if !enabled {
            muted()
        } else if destructive {
            danger()
        } else {
            text()
        })
        .when(enabled, |element| {
            element
                .cursor_pointer()
                .hover(|s| s.bg(hover()))
                .on_click(cx.listener(move |this, _event, window, cx| {
                    action(this, window, cx);
                }))
        })
        .child(label)
}

fn action_button(
    label: &'static str,
    enabled: bool,
    cx: &mut Context<NexDeskApp>,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("action-{label}")))
        .px_5()
        .h(px(34.))
        .flex()
        .items_center()
        .rounded(px(8.))
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .bg(if enabled { accent() } else { panel_2() })
        .text_color(if enabled { on_accent() } else { muted() })
        .when(enabled, |element| {
            element
                .cursor_pointer()
                .hover(|s| s.bg(accent_hover()))
                .on_click(cx.listener(|this, _event, window, cx| {
                    this.open_password_dialog(window, cx);
                }))
        })
        .child(label)
}

fn dialog_button(
    label: &'static str,
    primary: bool,
    cx: &mut Context<NexDeskApp>,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("dialog-{label}")))
        .px_5()
        .h(px(36.))
        .flex()
        .items_center()
        .rounded(px(8.))
        .bg(if primary { accent() } else { panel_2() })
        .text_color(if primary { on_accent() } else { text() })
        .cursor_pointer()
        .hover(|s| s.bg(if primary { accent_hover() } else { hover() }))
        .when(primary || label == "Cancel", |element| {
            element.on_click(cx.listener(move |this, _event, window, cx| match label {
                "Cancel" => {
                    this.cancel_password_dialog(window, cx);
                }

                "Connect" => {
                    this.connect_selected(window, cx);
                }

                _ => {}
            }))
        })
        .child(label)
}

// ============================================================================
// Password input state
// ============================================================================

/// Single-line text input. `masked` renders bullets (password fields).
pub struct TextInput {
    pub focus_handle: FocusHandle,

    masked: bool,
    placeholder: SharedString,

    content: String,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,

    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
}

impl TextInput {
    pub fn new(cx: &mut Context<Self>, masked: bool, placeholder: &'static str) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            masked,
            placeholder: SharedString::from(placeholder),
            content: String::new(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
        }
    }

    pub fn value(&self) -> String {
        self.content.clone()
    }

    pub fn set_value(&mut self, value: &str, cx: &mut Context<Self>) {
        self.content = value.replace(['\n', '\r'], " ");
        let end = self.content.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.content.clear();
        self.selected_range = 0..0;
        self.selection_reversed = false;
        self.marked_range = None;
        self.last_layout = None;
        self.last_bounds = None;

        cx.notify();
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content[..offset]
            .char_indices()
            .next_back()
            .map(|(index, _)| index)
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content[offset..]
            .char_indices()
            .nth(1)
            .map(|(index, _)| offset + index)
            .unwrap_or(self.content.len())
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = offset.min(self.content.len());

        self.selected_range = offset..offset;
        self.selection_reversed = false;

        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = offset.min(self.content.len());
        let cursor = self.cursor_offset();

        if offset < cursor {
            self.selected_range = offset..cursor;
            self.selection_reversed = true;
        } else {
            self.selected_range = cursor..offset;
            self.selection_reversed = false;
        }

        cx.notify();
    }

    fn left(&mut self, _action: &InputLeft, _window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx);
        }
    }

    fn right(&mut self, _action: &InputRight, _window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.end, cx);
        }
    }

    fn select_left(
        &mut self,
        _action: &InputSelectLeft,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(
        &mut self,
        _action: &InputSelectRight,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(
        &mut self,
        _action: &InputSelectAll,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;

        cx.notify();
    }

    fn home(&mut self, _action: &InputHome, _window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _action: &InputEnd, _window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn backspace(
        &mut self,
        _action: &InputBackspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            let cursor = self.cursor_offset();

            if cursor == 0 {
                window.play_system_bell();
                return;
            }

            let previous = self.previous_boundary(cursor);
            self.selected_range = previous..cursor;
        }

        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _action: &InputDelete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let cursor = self.cursor_offset();

            if cursor >= self.content.len() {
                window.play_system_bell();
                return;
            }

            let next = self.next_boundary(cursor);
            self.selected_range = cursor..next;
        }

        self.replace_text_in_range(None, "", window, cx);
    }

    fn paste(&mut self, _action: &InputPaste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text.replace('\n', ""), window, cx);
        }
    }

    fn copy(&mut self, _action: &InputCopy, _window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            return;
        }

        let text = self.content[self.selected_range.clone()].to_string();

        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    fn cut(&mut self, _action: &InputCut, window: &mut Window, cx: &mut Context<Self>) {
        self.copy(&InputCopy, window, cx);

        self.replace_text_in_range(None, "", window, cx);
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;

        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }

            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }

        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;

        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }

            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }

        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }
}

// ============================================================================
// GPUI text input handler
// ============================================================================

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);

        actual_range.replace(self.range_to_utf16(&range));

        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());

        self.content = format!(
            "{}{}{}",
            &self.content[..range.start],
            new_text,
            &self.content[range.end..],
        );

        let end = range.start + new_text.len();

        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;

        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());

        self.content = format!(
            "{}{}{}",
            &self.content[..range.start],
            new_text,
            &self.content[range.end..],
        );

        if new_text.is_empty() {
            self.marked_range = None;
        } else {
            self.marked_range = Some(range.start..range.start + new_text.len());
        }

        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .map(|range| range.start + range.start..range.end + range.start)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());

        self.selection_reversed = false;

        let _ = window;

        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;

        let range = self.range_from_utf16(&range_utf16);

        Some(Bounds::from_corners(
            Point::new(
                bounds.left() + last_layout.x_for_index(range.start),
                bounds.top(),
            ),
            Point::new(
                bounds.left() + last_layout.x_for_index(range.end),
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;

        let last_layout = self.last_layout.as_ref()?;

        let utf8_index = last_layout.index_for_x(point.x - line_point.x)?;

        Some(self.offset_to_utf16(utf8_index))
    }
}

// ============================================================================
// Password input element
// ============================================================================

struct TextInputElement {
    input: Entity<TextInput>,
}

struct TextInputPrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextInputElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextInputElement {
    type RequestLayoutState = ();
    type PrepaintState = TextInputPrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();

        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();

        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);

        let content = input.content.clone();

        let masked = input.masked;
        let display_text = if content.is_empty() {
            input.placeholder.clone()
        } else if masked {
            SharedString::from("•".repeat(content.chars().count()))
        } else {
            SharedString::from(content.clone())
        };
        // Byte offset in `content` -> byte offset in the displayed string. Bullets are 3 bytes
        // each, so masked fields need the char-count mapping (otherwise the caret drifts left).
        let to_display = |offset: usize| -> usize {
            let offset = offset.min(content.len());
            if masked && !content.is_empty() {
                content[..offset].chars().count() * '•'.len_utf8()
            } else if content.is_empty() {
                0
            } else {
                offset
            }
        };

        let selected_range = input.selected_range.clone();

        let cursor = input.cursor_offset();

        let style = window.text_style();

        let text_color = if content.is_empty() {
            muted().into()
        } else {
            text().into()
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };

        let font_size = style.font_size.to_pixels(window.rem_size());

        let line = window
            .text_system()
            .shape_line(display_text, font_size, &[run], None);

        let cursor_pos = line.x_for_index(to_display(cursor));

        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        Point::new(bounds.left() + cursor_pos, bounds.top()),
                        size(px(2.), bounds.bottom() - bounds.top()),
                    ),
                    accent(),
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        Point::new(
                            bounds.left() + line.x_for_index(to_display(selected_range.start)),
                            bounds.top(),
                        ),
                        Point::new(
                            bounds.left() + line.x_for_index(to_display(selected_range.end)),
                            bounds.bottom(),
                        ),
                    ),
                    rgba(0x335aa9ff),
                )),
                None,
            )
        };

        TextInputPrepaintState {
            line: Some(line),
            cursor,
            selection,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();

        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );

        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }

        let line = prepaint.line.take().unwrap();

        line.paint(
            bounds.origin,
            window.line_height(),
            gpui::TextAlign::Left,
            None,
            window,
            cx,
        )
        .unwrap();

        if focus_handle.is_focused(window) {
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        }

        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

// ============================================================================
// Render TextInput
// ============================================================================

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .key_context("NexDeskTextInput")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, window, cx| {
                    window.focus(&this.focus_handle, cx);
                }),
            )
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .line_height(px(24.))
            .text_size(px(15.))
            .child(
                div()
                    .h(px(24.))
                    .w_full()
                    .child(TextInputElement { input: cx.entity() }),
            )
    }
}

// ============================================================================
// Run
// ============================================================================

pub fn run(engine_path: std::path::PathBuf) {
    application().with_assets(crate::assets::Assets).run(move |cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("backspace", InputBackspace, Some("NexDeskTextInput")),
            KeyBinding::new("delete", InputDelete, Some("NexDeskTextInput")),
            KeyBinding::new("left", InputLeft, Some("NexDeskTextInput")),
            KeyBinding::new("right", InputRight, Some("NexDeskTextInput")),
            KeyBinding::new(
                "shift-left",
                InputSelectLeft,
                Some("NexDeskTextInput"),
            ),
            KeyBinding::new(
                "shift-right",
                InputSelectRight,
                Some("NexDeskTextInput"),
            ),
            KeyBinding::new("cmd-a", InputSelectAll, Some("NexDeskTextInput")),
            KeyBinding::new("ctrl-a", InputSelectAll, Some("NexDeskTextInput")),
            KeyBinding::new("cmd-v", InputPaste, Some("NexDeskTextInput")),
            KeyBinding::new("ctrl-v", InputPaste, Some("NexDeskTextInput")),
            KeyBinding::new("cmd-c", InputCopy, Some("NexDeskTextInput")),
            KeyBinding::new("ctrl-c", InputCopy, Some("NexDeskTextInput")),
            KeyBinding::new("cmd-x", InputCut, Some("NexDeskTextInput")),
            KeyBinding::new("ctrl-x", InputCut, Some("NexDeskTextInput")),
            KeyBinding::new("home", InputHome, Some("NexDeskTextInput")),
            KeyBinding::new("end", InputEnd, Some("NexDeskTextInput")),
        ]);

        let bounds = Bounds::centered(None, size(px(1120.), px(720.)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: None,
                window_decorations: Some(WindowDecorations::Client),
                window_background: WindowBackgroundAppearance::Transparent,
                window_min_size: Some(size(px(760.), px(480.))),
                app_id: Some("nexdesk".into()),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| NexDeskApp::new(engine_path.clone(), window, cx)),
        )
        .expect("open NexDesk window");

        cx.activate(true);
    });
}
