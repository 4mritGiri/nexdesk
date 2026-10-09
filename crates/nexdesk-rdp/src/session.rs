//! Starts one RDP session: builds the IronRDP config, TLS policy and clipboard, then runs the
//! protocol engine on its own thread. Output events are tagged with the session id so a single
//! window can show several sessions as tabs.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use ironrdp_client::config::{ClipboardType, ConfigBuilder, Destination};
use ironrdp_client::rdp::{RdpClient, RdpInputEvent, RdpOutputEvent};
use ironrdp_pdu::rdp::capability_sets::MajorPlatformType;
use nexdesk_clipboard::ClipboardHandle;
use tokio::sync::mpsc::UnboundedSender;
use winit::event_loop::EventLoopProxy;

use crate::app::UserEvent;
use crate::ipc::Spec;
use crate::tls;

pub struct Started {
    pub input_tx: UnboundedSender<RdpInputEvent>,
    pub clip: Option<ClipboardHandle>,
    /// Only the visible tab forwards local clipboard changes to its server.
    pub clip_active: Arc<AtomicBool>,
}

fn platform() -> MajorPlatformType {
    if cfg!(windows) {
        MajorPlatformType::WINDOWS
    } else if cfg!(target_os = "macos") {
        MajorPlatformType::MACINTOSH
    } else {
        MajorPlatformType::UNIX
    }
}

pub fn start(id: u64, spec: &Spec, proxy: EventLoopProxy<UserEvent>) -> Result<Started> {
    use ironrdp_pdu::rdp::client_info::PerformanceFlags as P;
    use nexdesk_core::profiles::Speed;

    let mut builder = ConfigBuilder::new()
        .with_destination(Destination::new(spec.host.clone())?)
        .with_username(spec.user.clone())
        .with_password(spec.password.clone())
        .with_client_build(0)
        .with_client_dir("C:\\Windows\\System32\\mstscax.dll")
        .with_client_name("nexdesk")
        .with_platform(platform())
        .with_desktop_width(spec.width)
        .with_desktop_height(spec.height)
        .with_clipboard(if spec.clipboard { ClipboardType::Enable } else { ClipboardType::Disable });
    if let Some(d) = &spec.domain {
        builder = builder.with_domain(d.clone());
    }
    let balanced = P::DISABLE_WALLPAPER | P::DISABLE_FULLWINDOWDRAG | P::DISABLE_MENUANIMATIONS | P::ENABLE_FONT_SMOOTHING;
    let flags = match spec.speed {
        Speed::Lan => P::ENABLE_FONT_SMOOTHING | P::ENABLE_DESKTOP_COMPOSITION,
        Speed::Balanced => balanced,
        Speed::Slow => balanced | P::DISABLE_THEMING | P::DISABLE_CURSOR_SHADOW | P::DISABLE_CURSORSETTINGS,
    };
    builder = builder.with_performance_flags(flags);
    if spec.speed == Speed::Slow {
        builder = builder.with_color_depth(16);
    }
    let mut config = builder.build()?;

    // ---- TLS: pin / verify the server certificate (stock IronRDP accepts anything)
    nexdesk_core::logs::set_context_host(&spec.host);
    let known_path = nexdesk_core::knownhosts::default_path();
    let key = nexdesk_core::knownhosts::host_key(config.destination().name(), config.destination().port());
    let mut known = match &known_path {
        Some(p) => nexdesk_core::knownhosts::KnownHosts::load(p),
        None => nexdesk_core::knownhosts::KnownHosts::in_memory(),
    };
    if spec.forget_host {
        match known.forget(&key) {
            Ok(true) => eprintln!("forgot the pinned certificate for {key}"),
            Ok(false) => eprintln!("no pinned certificate for {key}"),
            Err(e) => eprintln!("cannot update known_hosts: {e}"),
        }
    }
    if spec.tls == tls::Policy::Insecure {
        nexdesk_core::logs::alarm(nexdesk_core::logs::Level::Warn, "Certificate verification disabled (--tls insecure)", &spec.host, "");
        eprintln!("WARNING: --tls insecure: the server certificate is NOT verified");
    } else {
        let prompt_proxy = proxy.clone();
        let prompt: tls::Prompt = Arc::new(move |info| {
            let (tx, rx) = std::sync::mpsc::channel();
            if prompt_proxy.send_event(UserEvent::CertPrompt(id, info, tx)).is_err() {
                return false;
            }
            rx.recv_timeout(std::time::Duration::from_secs(300)).unwrap_or(false)
        });
        let verifier = tls::PolicyVerifier::new(config.destination().name(), config.destination().port(), spec.tls, known, prompt);
        config.set_tls_verifier(verifier);
    }

    // Protocol engine runs on its own tokio runtime; the UI thread owns the window.
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<RdpOutputEvent>(64);
    let mut client = RdpClient::new(config, out_tx);
    let input_tx = client.input_sender();

    // ---- Clipboard: real Linux backend instead of IronRDP's no-op stub
    let clip_active = Arc::new(AtomicBool::new(true));
    let mut clip_handle = None;
    if spec.clipboard {
        let sink_tx = input_tx.clone();
        let gate = clip_active.clone();
        let clip = nexdesk_clipboard::LinuxClipboard::new(
            Arc::new(move |m| {
                if gate.load(Ordering::Relaxed) {
                    let _ = sink_tx.send(RdpInputEvent::Clipboard(m));
                }
            }),
            nexdesk_clipboard::Config::default(),
        );
        client.set_clipboard_factory(clip.backend_factory());
        clip_handle = Some(clip.handle());
    }

    std::thread::Builder::new().name(format!("rdp-engine-{id}")).spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio runtime");
        rt.block_on(async move {
            let forward = async {
                while let Some(ev) = out_rx.recv().await {
                    if proxy.send_event(UserEvent::Rdp(id, ev)).is_err() {
                        break;
                    }
                }
            };
            tokio::join!(client.run(), forward);
        });
    })?;

    Ok(Started { input_tx, clip: clip_handle, clip_active })
}
