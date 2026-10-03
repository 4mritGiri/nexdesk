//! nexdesk: a Rust RDP desktop client (winit + softbuffer) built on IronRDP.
mod app;
mod keymap;

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
  --no-clipboard         disable clipboard redirection
  -h, --help             show this help

The password is read from $NEXDESK_PASSWORD or prompted (never passed as an argument).
Hotkey: Ctrl+Alt+End sends Ctrl+Alt+Del to the remote. Logs: NEXDESK_LOG=debug
";

struct Args {
    host: Option<String>,
    rdp: Option<PathBuf>,
    user: Option<String>,
    domain: Option<String>,
    width: Option<u16>,
    height: Option<u16>,
    dynamic_resize: bool,
    clipboard: bool,
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
        clipboard: true,
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
            "--no-clipboard" => a.clipboard = false,
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
        .with_env_filter(EnvFilter::try_from_env("NEXDESK_LOG").unwrap_or_else(|_| EnvFilter::new("warn")))
        .init();

    let Some(args) = parse_args()? else { return Ok(()) };

    // .rdp file values act as defaults; explicit flags win.
    let file = match &args.rdp {
        Some(p) => {
            let text = std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?;
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
    let domain = args.domain.clone().or_else(|| file.domain().map(str::to_owned));
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
        .with_clipboard(if args.clipboard { ClipboardType::Enable } else { ClipboardType::Disable });
    if let Some(d) = domain {
        builder = builder.with_domain(d);
    }
    let config = builder.build()?;

    // Protocol engine runs on its own tokio runtime; the UI thread owns the window.
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<RdpOutputEvent>(64);
    let client = RdpClient::new(config, out_tx);
    let input_tx = client.input_sender();

    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();

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
        format!("nexdesk - {host}"),
        (u32::from(width), u32::from(height)),
        args.dynamic_resize,
        input_tx,
    );
    event_loop.run_app(&mut app)?;

    match app.failure {
        Some(msg) => bail!(msg),
        None => Ok(()),
    }
}
