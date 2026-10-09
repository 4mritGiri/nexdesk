//! `nexdesk-agent`: share this X11 screen with a viewer that you approve.
use std::io::{BufRead, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use std::sync::atomic::AtomicBool;

use nexdesk_peer::host::{handle_stream, run_shared, Log, Policy};
use nexdesk_peer::store;

const HELP: &str = "nexdesk-agent - share this screen (X11) with an authenticated viewer

USAGE: nexdesk-agent [--listen ADDR:PORT] [--allow SHA256:...]... [--view-only] [--no-clipboard] [--no-prompt] [--stdio-control] [--relay HOST:PORT [--id N]]

  --listen ADDR:PORT  where to listen (default 127.0.0.1:21118; use 0.0.0.0:21118 for the LAN)
  --allow FINGERPRINT accept this viewer without asking (repeatable)
  --view-only         the viewer sees the screen but cannot type or click
  --no-clipboard      do not share the text clipboard
  --reconnect-grace S a viewer approved in the last S seconds may reconnect without a new question
                      (default 60; 0 = always ask). Its identity is still verified by the handshake.
  --stdio-control     ask for consent through stdin/stdout (used by the NexDesk manager):
                      prints 'REQUEST <fingerprint>', waits for a line 'yes' or 'no'
  --relay HOST:PORT   also register at this relay so viewers can reach this computer by ID from
                      anywhere (no port forwarding). The relay never sees screen or keys.
  --id N              use this nine digit ID instead of the saved one
  --no-prompt         never ask; only --allow fingerprints get in
Every other viewer is shown on this terminal and must be approved with y.
The agent's own fingerprint is printed at start: give it to the viewer's user to compare.";

fn main() {
    let mut listen = format!("127.0.0.1:{}", nexdesk_peer::DEFAULT_PORT);
    let mut allow = Vec::new();
    let (mut view_only, mut prompt, mut clipboard, mut stdio) = (false, true, true, false);
    let mut grace = 60u64;
    let (mut relay, mut forced_id) = (None::<String>, None::<String>);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--listen" => listen = args.next().unwrap_or_else(|| die("--listen needs a value")),
            "--allow" => allow.push(args.next().unwrap_or_else(|| die("--allow needs a fingerprint"))),
            "--view-only" => view_only = true,
            "--no-prompt" => prompt = false,
            "--no-clipboard" => clipboard = false,
            "--relay" => relay = Some(args.next().unwrap_or_else(|| die("--relay needs HOST:PORT"))),
            "--id" => forced_id = Some(args.next().unwrap_or_else(|| die("--id needs nine digits"))),
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

    // With --stdio-control one thread owns stdin and routes each line, so a connection question,
    // a file question and chat can never steal each other's answers.
    let (conn_tx, conn_rx) = mpsc::channel::<bool>();
    let (files_tx, files_rx) = mpsc::channel::<bool>();
    let conn_rx = Mutex::new(conn_rx);
    let files_rx = Mutex::new(files_rx);
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
                    return conn_rx.lock().ok().and_then(|r| r.recv().ok()).unwrap_or(false);
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
    if prompt && !view_only {
        policy.files = Some(Arc::new(move |summary: &str| {
            let mut it = summary.splitn(3, '|');
            let count = it.next().unwrap_or("?");
            let bytes: u64 = it.next().and_then(|b| b.parse().ok()).unwrap_or(0);
            let first: String = it.next().unwrap_or("").chars().filter(|c| !c.is_control()).take(120).collect();
            if stdio {
                println!("FILES {count} {bytes} {first}");
                let _ = std::io::stdout().flush();
                return files_rx.lock().ok().and_then(|r| r.recv().ok()).unwrap_or(false);
            }
            eprint!("\nThe viewer wants to send {count} file(s), {bytes} bytes, starting with '{first}'.\nSave them to your Downloads/NexDesk folder? [y/N] ");
            let _ = std::io::stderr().flush();
            let mut line = String::new();
            let _ = std::io::stdin().lock().read_line(&mut line);
            line.trim().eq_ignore_ascii_case("y")
        }));
    }
    policy.on_chat = Some(Arc::new(move |t: &str| {
        let t: String = t.chars().filter(|c| !c.is_control()).collect();
        if stdio {
            println!("CHAT {t}");
            let _ = std::io::stdout().flush();
        } else {
            eprintln!("chat: {t}");
        }
    }));
    let listener = TcpListener::bind(&listen).unwrap_or_else(|e| die(&format!("cannot listen on {listen}: {e}")));
    eprintln!("listening on {listen}{}", if view_only { " (view only)" } else { "" });
    let (id, policy) = (Arc::new(id), Arc::new(policy));
    if stdio {
        let policy = policy.clone();
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines().map_while(Result::ok) {
                let line = line.trim_end_matches('\r');
                if let Some(t) = line.strip_prefix("chat ") {
                    policy.send_chat(t);
                } else if let Some(a) = line.strip_prefix("files ") {
                    let _ = files_tx.send(a.trim().eq_ignore_ascii_case("yes"));
                } else {
                    let _ = conn_tx.send(line.trim().eq_ignore_ascii_case("yes"));
                }
            }
            // the manager went away: refuse anything still waiting
            let _ = conn_tx.send(false);
            let _ = files_tx.send(false);
        });
    }
    let busy = Arc::new(AtomicBool::new(false));
    let log: Log = Arc::new(|m: &str| eprintln!("{m}"));
    // keep the registration alive for as long as the agent runs
    let _registration = relay.map(|relay| {
        let my_id = match forced_id {
            Some(i) if nexdesk_network::proto::valid_id(&i) => i,
            Some(_) => die("--id must be nine digits"),
            None => store::load_or_create_id(&dir).unwrap_or_else(|e| die(&e.to_string())),
        };
        if !nexdesk_peer::control::valid_addr(&relay) {
            die("--relay needs a host or host:port");
        }
        let relay = if relay.contains(':') { relay } else { format!("{relay}:{}", nexdesk_network::DEFAULT_RELAY_PORT) };
        eprintln!("relay: {relay}, this computer's ID: {my_id}");
        if stdio {
            println!("ID {my_id}");
            let _ = std::io::stdout().flush();
        }
        let (id2, policy2, busy2, log2) = (id.clone(), policy.clone(), busy.clone(), log.clone());
        nexdesk_network::client::register(
            relay,
            my_id,
            move |s| handle_stream(s, &id2, &policy2, &busy2, &log2),
            move |st| {
                eprintln!("relay status: {st}");
                if stdio {
                    println!("RELAY {}", st.replace(['\n', '\r'], " "));
                    let _ = std::io::stdout().flush();
                }
            },
        )
        .unwrap_or_else(|e| die(&e.to_string()))
    });
    run_shared(listener, id, policy, log, busy);
}

fn die(m: &str) -> ! {
    eprintln!("{m}");
    std::process::exit(2);
}
