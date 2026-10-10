//! `nexdesk-relay`: rendezvous and relay server. Carries only end-to-end encrypted bytes.
use std::net::TcpListener;

use nexdesk_network::relay::{Limits, Relay};

const HELP: &str = "nexdesk-relay - lets NexDesk viewers reach agents behind NAT by ID

USAGE: nexdesk-relay [--listen ADDR:PORT] [--max-connections N] [--per-ip N] [--key-file PATH]

  --listen ADDR:PORT   default 0.0.0.0:21117
  --max-connections N  open connections at the same time (default 2000)
  --per-ip N           open connections from one address (default 30)
  --key-file PATH      access key (at least 16 characters, first line of the file). With a key, only
                       agents and viewers that know it can use the relay; give them the same key in
                       NEXDESK_RELAY_KEY. The environment variable works here too. Without a key the
                       relay is open to anyone who can reach it.

The relay sees addresses, IDs and traffic volume, never screen contents, keystrokes or clipboard:
viewer and agent encrypt end to end. Put it behind a firewall rule that allows only its port.";

fn die(m: &str) -> ! {
    eprintln!("{m}");
    std::process::exit(2);
}

fn main() {
    let mut listen = format!("0.0.0.0:{}", nexdesk_network::DEFAULT_RELAY_PORT);
    let mut limits = Limits::default();
    let mut key: Option<Vec<u8>> = std::env::var("NEXDESK_RELAY_KEY")
        .ok()
        .map(|k| k.trim().as_bytes().to_vec())
        .filter(|k| !k.is_empty());
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut num = |name: &str| -> usize {
            args.next()
                .and_then(|v| v.parse().ok())
                .filter(|n| *n > 0)
                .unwrap_or_else(|| die(&format!("{name} needs a positive number")))
        };
        match a.as_str() {
            "--listen" => listen = args.next().unwrap_or_else(|| die("--listen needs a value")),
            "--max-connections" => limits.max_connections = num("--max-connections"),
            "--per-ip" => limits.per_ip = num("--per-ip"),
            "--key-file" => {
                let path = args
                    .next()
                    .unwrap_or_else(|| die("--key-file needs a path"));
                let text = std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| die(&format!("cannot read {path}: {e}")));
                key = Some(text.lines().next().unwrap_or("").trim().as_bytes().to_vec());
            }
            "-h" | "--help" => {
                println!("{HELP}");
                return;
            }
            other => die(&format!("unknown option {other}\n\n{HELP}")),
        }
    }
    if let Some(k) = &key {
        if k.len() < 16 {
            die("the access key must be at least 16 characters");
        }
    }
    let listener = TcpListener::bind(&listen)
        .unwrap_or_else(|e| die(&format!("cannot listen on {listen}: {e}")));
    eprintln!(
        "relay listening on {listen} ({})",
        if key.is_some() {
            "access key required"
        } else {
            "open: anyone who can reach it may use it"
        }
    );
    Relay::with_key(limits, key, |m| eprintln!("{m}")).serve(listener);
}
