//! nexdesk: a Rust RDP desktop client (winit + softbuffer) built on IronRDP.
mod app;
mod diag;
mod grab;
mod keymap;
mod shot;
mod tls;
mod ui;

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use ironrdp_client::config::{ClipboardType, ConfigBuilder, Destination};
use ironrdp_client::rdp::{RdpClient, RdpOutputEvent};
use ironrdp_pdu::rdp::capability_sets::MajorPlatformType;
use nexdesk_core::rdpfile::RdpFile;
use winit::event_loop::EventLoop;

use crate::app::{App, UserEvent};

const HELP: &str = "\
nexdesk - Rust RDP client

USAGE:
  nexdesk --host <HOST[:PORT]> -u <USER> [-d <DOMAIN>] [--width N --height N]
  nexdesk --rdp <FILE.rdp> [-u <USER>]

OPTIONS:
  --host <HOST[:PORT]>   server address (default port 3389)
  --rdp <FILE>           load address/user/domain/size from a .rdp file
  -u, --user <USER>      username
  -d, --domain <DOMAIN>  domain
  --width <N>            desktop width  (default 1920)
  --height <N>           desktop height (default 1080)
  --dynamic-resize       (experimental) resize the remote desktop with the window
  --fullscreen           start full screen
  --no-key-capture       do not send Super/Alt+Tab to the remote while full screen
  --no-clipboard         disable clipboard redirection (text, images, files)
  --no-drop-paste        after dropping files on the window, do not press Ctrl+V on the remote
  --tls <MODE>           server certificate checking: ask (default) | accept-new | strict | insecure
  --forget-host          delete the pinned certificate for this host, then continue
  --perf <lan|balanced|slow>  speed preset (default: from the .rdp file, else balanced). balanced turns off
                         wallpaper/menu animations/full-window drag; slow also turns off themes and uses 16-bit colour
  --native-frame         use the system title bar instead of NexDesk's own header bar
  --x11 / --wayland      force the window system (default: automatic). File drops from the
                         local desktop need --x11 on Wayland (the winit toolkit lacks Wayland DnD)
  -h, --help             show this help

The password is read from $NEXDESK_PASSWORD or prompted (never passed as an argument).
In full screen, move the mouse to the top edge to show the connection bar (pin, minimise, restore, close).
Hotkeys: Ctrl+Alt+End = Ctrl+Alt+Del on the remote; Ctrl+Alt+Break = toggle full screen. Logs: NEXDESK_LOG=debug
";

struct Args {
    host: Option<String>,
    rdp: Option<PathBuf>,
    user: Option<String>,
    domain: Option<String>,
    width: Option<u16>,
    height: Option<u16>,
    dynamic_resize: bool,
    fullscreen: bool,
    capture_keys: bool,
    clipboard: bool,
    drop_paste: bool,
    tls: tls::Policy,
    forget_host: bool,
    backend: Option<Backend>,
    speed: Option<nexdesk_core::profiles::Speed>,
    native_frame: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Backend {
    X11,
    Wayland,
}

fn parse_args() -> Result<Option<Args>> {
    let mut a = Args {
        host: None,
        rdp: None,
        user: None,
        domain: None,
        width: None,
        height: None,
        dynamic_resize: false,
        fullscreen: false,
        capture_keys: true,
        clipboard: true,
        drop_paste: true,
        tls: std::env::var("NEXDESK_TLS").ok().and_then(|v| tls::Policy::parse(&v)).unwrap_or(tls::Policy::Ask),
        forget_host: false,
        backend: None,
        speed: None,
        native_frame: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = |name: &str| it.next().with_context(|| format!("{name} needs a value"));
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(None);
            }
            "--host" => a.host = Some(val("--host")?),
            "--rdp" => a.rdp = Some(PathBuf::from(val("--rdp")?)),
            "-u" | "--user" => a.user = Some(val("--user")?),
            "-d" | "--domain" => a.domain = Some(val("--domain")?),
            "--width" => a.width = Some(val("--width")?.parse().context("--width")?),
            "--height" => a.height = Some(val("--height")?.parse().context("--height")?),
            "--dynamic-resize" => a.dynamic_resize = true,
            "--fullscreen" => a.fullscreen = true,
            "--no-key-capture" => a.capture_keys = false,
            "--no-clipboard" => a.clipboard = false,
            "--no-drop-paste" => a.drop_paste = false,
            "--forget-host" => a.forget_host = true,
            "--perf" => {
                let v = val("--perf")?;
                a.speed = Some(
                    nexdesk_core::profiles::Speed::parse(&v)
                        .with_context(|| format!("--perf: use lan, balanced or slow (got {v:?})"))?,
                );
            }
            "--native-frame" => a.native_frame = true,
            "--x11" => a.backend = Some(Backend::X11),
            "--wayland" => a.backend = Some(Backend::Wayland),
            "--tls" => {
                let v = val("--tls")?;
                a.tls = tls::Policy::parse(&v).with_context(|| format!("--tls: unknown mode {v:?}"))?;
            }
            other => bail!("unknown argument: {other}\n\n{HELP}"),
        }
    }
    Ok(Some(a))
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

fn main() -> Result<()> {
    use tracing_subscriber::EnvFilter;
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("NEXDESK_LOG").unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();

    let Some(args) = parse_args()? else {
        return Ok(());
    };

    // .rdp file values act as defaults; explicit flags win.
    let file = match &args.rdp {
        Some(p) => {
            let text =
                std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?;
            RdpFile::parse(&text)
        }
        None => RdpFile::default(),
    };

    let host = args
        .host
        .clone()
        .or_else(|| file.full_address().map(str::to_owned))
        .context("no server given: use --host or --rdp")?;
    let user = args
        .user
        .clone()
        .or_else(|| file.username().map(str::to_owned))
        .context("no username given: use -u or put it in the .rdp file")?;
    let user_for_log = user.clone();
    nexdesk_core::logs::set_context_host(&host);
    let domain = args
        .domain
        .clone()
        .or_else(|| file.domain().map(str::to_owned));
    let file_speed: Option<String> = file.get_int("connection type").map(|v| match v {
        6 => "lan",
        1 | 2 | 3 => "slow",
        _ => "balanced",
    }.to_string());
    let (fw, fh) = file.desktop_size().unwrap_or((1920, 1080));
    let (width, height) = (args.width.unwrap_or(fw), args.height.unwrap_or(fh));

    let password = match std::env::var("NEXDESK_PASSWORD") {
        Ok(p) => p,
        Err(_) => rpassword::prompt_password(format!("Password for {user}@{host}: "))?,
    };

    let mut builder = ConfigBuilder::new()
        .with_destination(Destination::new(host.clone())?)
        .with_username(user)
        .with_password(password)
        .with_client_build(0)
        .with_client_dir("C:\\Windows\\System32\\mstscax.dll")
        .with_client_name("nexdesk")
        .with_platform(platform())
        .with_desktop_width(width)
        .with_desktop_height(height)
        .with_clipboard(if args.clipboard {
            ClipboardType::Enable
        } else {
            ClipboardType::Disable
        });
    if let Some(d) = domain {
        builder = builder.with_domain(d);
    }
    {
        use ironrdp_pdu::rdp::client_info::PerformanceFlags as P;
        use nexdesk_core::profiles::Speed;
        let speed = args.speed.unwrap_or_else(|| match &file_speed {
            Some(v) => Speed::parse(v).unwrap_or_default(),
            None => Speed::Balanced,
        });
        let balanced = P::DISABLE_WALLPAPER | P::DISABLE_FULLWINDOWDRAG | P::DISABLE_MENUANIMATIONS | P::ENABLE_FONT_SMOOTHING;
        let flags = match speed {
            Speed::Lan => P::ENABLE_FONT_SMOOTHING | P::ENABLE_DESKTOP_COMPOSITION,
            Speed::Balanced => balanced,
            Speed::Slow => balanced | P::DISABLE_THEMING | P::DISABLE_CURSOR_SHADOW | P::DISABLE_CURSORSETTINGS,
        };
        builder = builder.with_performance_flags(flags);
        if speed == Speed::Slow {
            builder = builder.with_color_depth(16);
        }
    }
    let mut config = builder.build()?;

    // ---- TLS: pin / verify the server certificate (stock IronRDP accepts anything)
    let known_path = nexdesk_core::knownhosts::default_path();
    let key = nexdesk_core::knownhosts::host_key(config.destination().name(), config.destination().port());
    let mut known = match &known_path {
        Some(p) => nexdesk_core::knownhosts::KnownHosts::load(p),
        None => nexdesk_core::knownhosts::KnownHosts::in_memory(),
    };
    if args.forget_host {
        match known.forget(&key) {
            Ok(true) => eprintln!("forgot the pinned certificate for {key}"),
            Ok(false) => eprintln!("no pinned certificate for {key}"),
            Err(e) => eprintln!("cannot update known_hosts: {e}"),
        }
    }

    let mut builder = EventLoop::<UserEvent>::with_user_event();
    #[cfg(target_os = "linux")]
    match args.backend {
        Some(Backend::X11) => {
            use winit::platform::x11::EventLoopBuilderExtX11;
            builder.with_x11();
        }
        Some(Backend::Wayland) => {
            use winit::platform::wayland::EventLoopBuilderExtWayland;
            builder.with_wayland();
        }
        None => {}
    }
    let event_loop = builder.build()?;
    let proxy = event_loop.create_proxy();

    if args.tls == tls::Policy::Insecure {
        nexdesk_core::logs::alarm(nexdesk_core::logs::Level::Warn, "Certificate verification disabled (--tls insecure)", &host, "");
    }
    if args.tls != tls::Policy::Insecure {
        let prompt_proxy = proxy.clone();
        let prompt: tls::Prompt = std::sync::Arc::new(move |info| {
            let (tx, rx) = std::sync::mpsc::channel();
            if prompt_proxy.send_event(UserEvent::CertPrompt(info, tx)).is_err() {
                return false;
            }
            rx.recv_timeout(std::time::Duration::from_secs(300)).unwrap_or(false)
        });
        let verifier = tls::PolicyVerifier::new(
            config.destination().name(),
            config.destination().port(),
            args.tls,
            known,
            prompt,
        );
        config.set_tls_verifier(verifier);
    } else {
        eprintln!("WARNING: --tls insecure: the server certificate is NOT verified");
    }

    // Protocol engine runs on its own tokio runtime; the UI thread owns the window.
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<RdpOutputEvent>(64);
    let mut client = RdpClient::new(config, out_tx);
    let input_tx = client.input_sender();

    // ---- Clipboard: real Linux backend instead of IronRDP's no-op stub
    let mut clip_handle = None;
    if args.clipboard {
        let sink_tx = input_tx.clone();
        let clip = nexdesk_clipboard::LinuxClipboard::new(
            std::sync::Arc::new(move |m| {
                let _ = sink_tx.send(ironrdp_client::rdp::RdpInputEvent::Clipboard(m));
            }),
            nexdesk_clipboard::Config::default(),
        );
        client.set_clipboard_factory(clip.backend_factory());
        clip_handle = Some(clip.handle());
    }

    let ui_proxy = proxy.clone();
    std::thread::Builder::new()
        .name("rdp-engine".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(async move {
                let forward = async {
                    while let Some(ev) = out_rx.recv().await {
                        if proxy.send_event(UserEvent::Rdp(ev)).is_err() {
                            break;
                        }
                    }
                };
                tokio::join!(client.run(), forward);
            });
        })?;

    let mut app = App::new(
        app::Options {
            title: format!("nexdesk - {host}"),
            host_label: host.clone(),
            initial_size: (u32::from(width), u32::from(height)),
            dynamic_resize: args.dynamic_resize,
            native_frame: args.native_frame,
            log_profile: std::env::var("NEXDESK_PROFILE").unwrap_or_else(|_| host.clone()),
            log_user: user_for_log,
            start_fullscreen: args.fullscreen || file.get_int("screen mode id") == Some(2),
            capture_keys: args.capture_keys,
            drop_paste: args.drop_paste,
            proxy: ui_proxy,
        },
        input_tx,
        clip_handle,
    );
    event_loop.run_app(&mut app)?;

    match app.failure {
        Some(msg) => bail!(msg),
        None => Ok(()),
    }
}
