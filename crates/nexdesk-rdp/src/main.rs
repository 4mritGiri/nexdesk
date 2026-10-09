//! nexdesk: a Rust RDP desktop client (winit + softbuffer) built on IronRDP.
mod app;
mod diag;
mod grab;
mod ipc;
mod keymap;
mod session;
mod shot;
mod tls;
mod ui;

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
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
  --new-window           open this connection in its own window instead of a tab of the open window
  --native-frame         use the system title bar instead of NexDesk's own header bar
  --x11 / --wayland      force the window system (default: automatic). File drops from the
                         local desktop need --x11 on Wayland (the winit toolkit lacks Wayland DnD)
  -h, --help             show this help

The password is read from $NEXDESK_PASSWORD or prompted (never passed as an argument).
In full screen, move the mouse to the top edge to show the connection bar (pin, minimise, restore, close).
Hotkeys: Ctrl+Alt+End = Ctrl+Alt+Del on the remote; Ctrl+Alt+Break = toggle full screen;
         Ctrl+Alt+PageDown / PageUp = next / previous session tab.
Several connections share one window as tabs (use --new-window to open a separate one). Logs: NEXDESK_LOG=debug
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
    new_window: bool,
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
        new_window: false,
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
            "--new-window" => a.new_window = true,
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

    use nexdesk_core::profiles::Speed;
    let speed = args.speed.unwrap_or_else(|| match &file_speed {
        Some(v) => Speed::parse(v).unwrap_or_default(),
        None => Speed::Balanced,
    });
    let spec = ipc::Spec {
        host: host.clone(),
        user,
        domain,
        password,
        width,
        height,
        dynamic_resize: args.dynamic_resize,
        fullscreen: args.fullscreen || file.get_int("screen mode id") == Some(2),
        capture_keys: args.capture_keys,
        clipboard: args.clipboard,
        drop_paste: args.drop_paste,
        tls: args.tls,
        speed,
        forget_host: args.forget_host,
        profile: std::env::var("NEXDESK_PROFILE").ok().filter(|p| !p.trim().is_empty()).unwrap_or_else(|| host.clone()),
    };

    // A viewer window is already open: add this connection to it as a tab and wait until it closes.
    if !args.new_window {
        if let Some(code) = ipc::forward(&spec) {
            std::process::exit(code);
        }
    }
    let listener = if args.new_window { None } else { ipc::bind() };

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
    #[cfg(unix)]
    if let Some(l) = listener {
        ipc::serve(l, proxy.clone());
    }
    #[cfg(not(unix))]
    let _ = listener;

    let mut app = App::new(app::Options { native_frame: args.native_frame, proxy }, spec);
    event_loop.run_app(&mut app)?;

    // Leave without running destructors: the window system, clipboard threads and the
    // protocol runtime are torn down in an unsafe order otherwise (this used to end in SIGSEGV).
    ipc::cleanup();
    let code = match app.failure.take() {
        Some(msg) => {
            eprintln!("Error: {msg}");
            1
        }
        None => 0,
    };
    std::mem::forget(app);
    std::process::exit(code);
}
