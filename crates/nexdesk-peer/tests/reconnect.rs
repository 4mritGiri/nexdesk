//! The viewer survives an agent restart: it reconnects to the same pinned identity by itself.
//! Run under xvfb-run; skipped without DISPLAY.
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Agent that answers "yes" to every consent request; returns its stdout lines through a channel.
fn start_agent(home: &std::path::Path, addr: &str) -> (Child, mpsc::Receiver<String>) {
    let mut agent = Command::new(env!("CARGO_BIN_EXE_nexdesk-agent"))
        .args([
            "--listen",
            addr,
            "--stdio-control",
            "--reconnect-grace",
            "0",
        ])
        .env("HOME", home)
        .env_remove("XDG_CONFIG_HOME")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = agent.stdin.take().unwrap();
    let out = agent.stdout.take().unwrap();
    let err = agent.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let tx2 = tx.clone();
    std::thread::spawn(move || {
        for l in BufReader::new(out).lines().map_while(Result::ok) {
            if l.starts_with("REQUEST ") {
                let _ = writeln!(stdin, "yes");
                let _ = stdin.flush();
            }
            let _ = tx.send(l);
        }
    });
    std::thread::spawn(move || {
        for l in BufReader::new(err).lines().map_while(Result::ok) {
            let _ = tx2.send(l);
        }
    });
    (agent, rx)
}

fn wait_for(rx: &mpsc::Receiver<String>, what: &str, secs: u64) -> String {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if let Ok(l) = rx.recv_timeout(Duration::from_millis(200)) {
            if l.contains(what) {
                return l;
            }
        }
    }
    panic!("never saw {what:?}");
}

#[test]
fn viewer_reconnects_after_the_agent_restarts() {
    if std::env::var_os("DISPLAY").is_none() {
        eprintln!("no DISPLAY, skipping");
        return;
    }
    let home = std::env::temp_dir().join(format!("nexdesk-rc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let addr = format!("127.0.0.1:{port}");

    let (mut agent, rx) = start_agent(&home, &addr);
    let fp = wait_for(&rx, "IDENTITY ", 10)
        .trim_start_matches("IDENTITY ")
        .to_string();
    std::thread::sleep(Duration::from_millis(300));
    let mut viewer = Command::new(env!("CARGO_BIN_EXE_nexdesk-peer-view"))
        .args([&addr, "--trust", &fp])
        .env("HOME", &home)
        .env_remove("XDG_CONFIG_HOME")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for(&rx, "REQUEST ", 10);
    std::thread::sleep(Duration::from_secs(1)); // session established, frames flowing

    // the agent dies (network drop / restart): the viewer must stay open
    let _ = agent.kill();
    let _ = agent.wait();
    std::thread::sleep(Duration::from_millis(1500));
    assert!(
        viewer.try_wait().unwrap().is_none(),
        "the viewer must not exit on a dropped connection"
    );

    // the agent comes back with the same identity: the viewer reconnects (and is asked again)
    let (mut agent2, rx2) = start_agent(&home, &addr);
    let fp2 = wait_for(&rx2, "IDENTITY ", 10)
        .trim_start_matches("IDENTITY ")
        .to_string();
    assert_eq!(fp, fp2, "the identity must survive a restart");
    wait_for(&rx2, "REQUEST ", 20);
    std::thread::sleep(Duration::from_secs(1));
    assert!(viewer.try_wait().unwrap().is_none());
    let _ = viewer.kill();
    let _ = viewer.wait();
    wait_for(&rx2, "session with", 10);
    let _ = agent2.kill();
    let _ = agent2.wait();
    let _ = std::fs::remove_dir_all(&home);
}
