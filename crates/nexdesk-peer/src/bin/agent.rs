//! `nexdesk-agent`: share this X11 screen with a viewer that you approve.
use std::io::{BufRead, Write};
use std::net::TcpListener;
use std::sync::Arc;

use nexdesk_peer::host::{run, Policy};
use nexdesk_peer::store;

const HELP: &str = "nexdesk-agent - share this screen (X11) with an authenticated viewer

USAGE: nexdesk-agent [--listen ADDR:PORT] [--allow SHA256:...]... [--view-only] [--no-clipboard] [--no-prompt] [--stdio-control]

  --listen ADDR:PORT  where to listen (default 127.0.0.1:21118; use 0.0.0.0:21118 for the LAN)
  --allow FINGERPRINT accept this viewer without asking (repeatable)
  --view-only         the viewer sees the screen but cannot type or click
  --no-clipboard      do not share the text clipboard
  --reconnect-grace S a viewer approved in the last S seconds may reconnect without a new question
                      (default 60; 0 = always ask). Its identity is still verified by the handshake.
  --stdio-control     ask for consent through stdin/stdout (used by the NexDesk manager):
                      prints 'REQUEST <fingerprint>', waits for a line 'yes' or 'no'
  --no-prompt         never ask; only --allow fingerprints get in
Every other viewer is shown on this terminal and must be approved with y.
The agent's own fingerprint is printed at start: give it to the viewer's user to compare.";

fn main() {
    let mut listen = format!("127.0.0.1:{}", nexdesk_peer::DEFAULT_PORT);
    let mut allow = Vec::new();
    let (mut view_only, mut prompt, mut clipboard, mut stdio) = (false, true, true, false);
    let mut grace = 60u64;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--listen" => listen = args.next().unwrap_or_else(|| die("--listen needs a value")),
            "--allow" => allow.push(args.next().unwrap_or_else(|| die("--allow needs a fingerprint"))),
            "--view-only" => view_only = true,
            "--no-prompt" => prompt = false,
            "--no-clipboard" => clipboard = false,
            "--reconnect-grace" => {
                grace = args.next().and_then(|v| v.parse().ok()).unwrap_or_else(|| die("--reconnect-grace needs seconds"))
            }
            "--stdio-control" => stdio = true,
            "-h" | "--help" => {
                println!("{HELP}");
                return;
            }
            other => die(&format!("unknown option {other}\n\n{HELP}")),
        }
    }
    let dir = store::default_dir().unwrap_or_else(|| die("cannot find the config directory (HOME not set)"));
    let id = store::load_or_create_identity(&dir, "agent-identity").unwrap_or_else(|e| die(&e.to_string()));
    eprintln!("agent identity: {}", id.public().fingerprint_string());
    if stdio {
        println!("IDENTITY {}", id.public().fingerprint_string());
        let _ = std::io::stdout().flush();
    }

    let mut policy = Policy::new(
        view_only,
        clipboard,
        allow,
        prompt.then(|| {
            Box::new(move |peer: &nexdesk_crypto::IdentityPublic| {
                if stdio {
                    // one line out, one line in; the fingerprint is plain ASCII
                    println!("REQUEST {}", peer.fingerprint_string());
                    let _ = std::io::stdout().flush();
                    let mut line = String::new();
                    let _ = std::io::stdin().lock().read_line(&mut line);
                    return line.trim().eq_ignore_ascii_case("yes");
                }
                eprint!("\nA viewer wants to connect.\n  fingerprint: {}\nAccept? [y/N] ", peer.fingerprint_string());
                let _ = std::io::stderr().flush();
                let mut line = String::new();
                let _ = std::io::stdin().lock().read_line(&mut line);
                line.trim().eq_ignore_ascii_case("y")
            }) as Box<dyn Fn(&_) -> bool + Send + Sync>
        }),
    );
    policy.reconnect_grace = std::time::Duration::from_secs(grace);
    let listener = TcpListener::bind(&listen).unwrap_or_else(|e| die(&format!("cannot listen on {listen}: {e}")));
    eprintln!("listening on {listen}{}", if view_only { " (view only)" } else { "" });
    run(listener, Arc::new(id), Arc::new(policy), |m| eprintln!("{m}"));
}

fn die(m: &str) -> ! {
    eprintln!("{m}");
    std::process::exit(2);
}
