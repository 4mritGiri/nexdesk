//! The manager's flow without a terminal: probe -> trust -> consent over stdin/stdout.
//! Run under xvfb-run (the viewer opens a window); skipped when there is no DISPLAY.
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn probe_trust_and_consent_without_a_terminal() {
    if std::env::var_os("DISPLAY").is_none() {
        eprintln!("no DISPLAY, skipping");
        return;
    }
    let home = std::env::temp_dir().join(format!("nexdesk-ctl-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let addr = format!("127.0.0.1:{port}");

    let mut agent = Command::new(env!("CARGO_BIN_EXE_nexdesk-agent"))
        .args(["--listen", &addr, "--stdio-control"])
        .env("HOME", &home)
        .env_remove("XDG_CONFIG_HOME")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = agent.stdin.take().unwrap();
    let mut lines = BufReader::new(agent.stdout.take().unwrap()).lines();
    let ident = lines.next().unwrap().unwrap();
    let fp = ident.strip_prefix("IDENTITY ").expect("identity line").to_string();
    std::thread::sleep(Duration::from_millis(300));

    let view = env!("CARGO_BIN_EXE_nexdesk-peer-view");
    // probe prints the same fingerprint
    let out = Command::new(view).args(["--probe", &addr]).env("HOME", &home).env_remove("XDG_CONFIG_HOME").stdin(Stdio::null()).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains(&format!("FINGERPRINT {fp}")), "{out:?}");

    // a wrong --trust does not pin: the viewer refuses without ever asking the agent
    let bad = Command::new(view).args([&addr, "--trust", "SHA256:AAAA"]).env("HOME", &home).env_remove("XDG_CONFIG_HOME").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap();
    assert!(!bad.success());

    // the right --trust pins and reaches the consent question
    let mut viewer = Command::new(view).args([&addr, "--trust", &fp]).env("HOME", &home).env_remove("XDG_CONFIG_HOME").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    let req = lines.next().unwrap().unwrap();
    assert!(req.starts_with("REQUEST SHA256:"), "{req}");
    writeln!(stdin, "no").unwrap();
    stdin.flush().unwrap();
    let t = Instant::now();
    loop {
        if viewer.try_wait().unwrap().is_some() {
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(10), "viewer must end after the agent said no");
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = agent.kill();
    let _ = std::fs::remove_dir_all(&home);
}
