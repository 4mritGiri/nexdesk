//! `nexdesk-agent`: share this X11 screen with a viewer that you approve.
use std::io::{BufRead, Write};
use std::net::TcpListener;
use std::sync::Arc;

use nexdesk_peer::host::{run, Policy};
use nexdesk_peer::store;

const HELP: &str = "nexdesk-agent - share this screen (X11) with an authenticated viewer

USAGE: nexdesk-agent [--listen ADDR:PORT] [--allow SHA256:...]... [--view-only] [--no-prompt]

  --listen ADDR:PORT  where to listen (default 127.0.0.1:21118; use 0.0.0.0:21118 for the LAN)
  --allow FINGERPRINT accept this viewer without asking (repeatable)
  --view-only         the viewer sees the screen but cannot type or click
  --no-prompt         never ask; only --allow fingerprints get in
Every other viewer is shown on this terminal and must be approved with y.
The agent's own fingerprint is printed at start: give it to the viewer's user to compare.";

fn main() {
    let mut listen = format!("127.0.0.1:{}", nexdesk_peer::DEFAULT_PORT);
    let mut allow = Vec::new();
    let (mut view_only, mut prompt) = (false, true);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--listen" => listen = args.next().unwrap_or_else(|| die("--listen needs a value")),
            "--allow" => allow.push(args.next().unwrap_or_else(|| die("--allow needs a fingerprint"))),
            "--view-only" => view_only = true,
            "--no-prompt" => prompt = false,
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

    let policy = Policy {
        view_only,
        allow,
        prompt: prompt.then(|| {
            Box::new(|peer: &nexdesk_crypto::IdentityPublic| {
                eprint!("\nA viewer wants to connect.\n  fingerprint: {}\nAccept? [y/N] ", peer.fingerprint_string());
                let _ = std::io::stderr().flush();
                let mut line = String::new();
                let _ = std::io::stdin().lock().read_line(&mut line);
                line.trim().eq_ignore_ascii_case("y")
            }) as Box<dyn Fn(&_) -> bool + Send + Sync>
        }),
    };
    let listener = TcpListener::bind(&listen).unwrap_or_else(|e| die(&format!("cannot listen on {listen}: {e}")));
    eprintln!("listening on {listen}{}", if view_only { " (view only)" } else { "" });
    run(listener, Arc::new(id), Arc::new(policy), |m| eprintln!("{m}"));
}

fn die(m: &str) -> ! {
    eprintln!("{m}");
    std::process::exit(2);
}
