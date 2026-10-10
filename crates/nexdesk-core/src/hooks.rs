//! Per-connection commands that run before connecting (e.g. bring a VPN up) and after the
//! session ends. They are plain programs with arguments, never passed through a shell, never
//! receive the password, and run with a time limit. Output goes to the Console log.
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

/// Split `prog arg "two words" 'x y' a\ b` into argv. No expansion of any kind.
pub fn split_args(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut have = false;
    let mut quote: Option<char> = None;
    let mut it = s.chars();
    while let Some(c) = it.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => match it.next() {
                Some(n @ ('"' | '\\')) => cur.push(n),
                Some(n) => {
                    cur.push('\\');
                    cur.push(n);
                }
                None => return Err("trailing backslash".into()),
            },
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                have = true;
            }
            (None, '\\') => match it.next() {
                Some(n) => {
                    cur.push(n);
                    have = true;
                }
                None => return Err("trailing backslash".into()),
            },
            (None, c) if c.is_whitespace() => {
                if have {
                    out.push(std::mem::take(&mut cur));
                    have = false;
                }
            }
            (None, c) => {
                cur.push(c);
                have = true;
            }
        }
    }
    if quote.is_some() {
        return Err("unclosed quote".into());
    }
    if have {
        out.push(cur);
    }
    Ok(out)
}

/// Check a command line typed by the user (empty = no hook).
pub fn validate(cmd: &str) -> Result<(), String> {
    if cmd.trim().is_empty() {
        return Ok(());
    }
    let argv = split_args(cmd)?;
    match argv.first() {
        None => Ok(()),
        Some(p) if p.contains('\0') => Err("invalid character".into()),
        Some(_) => Ok(()),
    }
}

/// Run `cmd`; `Ok(())` if it exited with status 0 in time. `env` is added to the environment.
pub fn run(cmd: &str, env: &[(&str, &str)], timeout: Duration) -> Result<(), String> {
    let argv = split_args(cmd)?;
    let Some((prog, args)) = argv.split_first() else {
        return Ok(());
    };
    let mut c = Command::new(prog);
    c.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove("NEXDESK_PASSWORD");
    for (k, v) in env {
        c.env(k, v);
    }
    let mut child = c.spawn().map_err(|e| format!("cannot start {prog}: {e}"))?;
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    // Drain on helper threads so a chatty child cannot block on a full pipe.
    let reader = |p: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut s = String::new();
            if let Some(mut p) = p {
                let mut buf = Vec::new();
                let _ = p.by_ref().take(16 * 1024).read_to_end(&mut buf);
                s = String::from_utf8_lossy(&buf).into_owned();
                let _ = std::io::copy(&mut p, &mut std::io::sink());
            }
            s
        })
    };
    let (t_out, t_err) = (
        reader(out.take().map(|p| Box::new(p) as Box<dyn Read + Send>)),
        reader(err.take().map(|p| Box::new(p) as Box<dyn Read + Send>)),
    );
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{prog} timed out after {}s", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(e) => return Err(format!("wait failed: {e}")),
        }
    };
    let (o, e) = (
        t_out.join().unwrap_or_default(),
        t_err.join().unwrap_or_default(),
    );
    for line in o.lines().chain(e.lines()).take(20) {
        crate::logs::console(crate::logs::Level::Info, "hook", line);
    }
    if status.success() {
        Ok(())
    } else {
        Err(format!("{prog} exited with {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitting() {
        assert_eq!(split_args("a b  c").unwrap(), ["a", "b", "c"]);
        assert_eq!(
            split_args(r#"nmcli con up "Work VPN""#).unwrap(),
            ["nmcli", "con", "up", "Work VPN"]
        );
        assert_eq!(
            split_args("x 'a b' c\\ d \"\"").unwrap(),
            ["x", "a b", "c d", ""]
        );
        assert_eq!(split_args(r#"echo "a\"b""#).unwrap(), ["echo", "a\"b"]);
        assert!(split_args("echo 'oops").is_err());
        assert!(split_args("echo \\").is_err());
        assert!(split_args("   ").unwrap().is_empty());
        // no shell: metacharacters stay literal
        assert_eq!(
            split_args("echo $HOME; rm -rf /").unwrap(),
            ["echo", "$HOME;", "rm", "-rf", "/"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn runs_without_a_shell_and_reports_failures() {
        run("true", &[], DEFAULT_TIMEOUT).unwrap();
        assert!(run("false", &[], DEFAULT_TIMEOUT).is_err());
        assert!(run("/nonexistent/prog", &[], DEFAULT_TIMEOUT).is_err());
        // env is passed, and the password variable never is
        std::env::set_var("NEXDESK_PASSWORD", "secret");
        run(
            "sh -c 'test -z \"$NEXDESK_PASSWORD\" && test \"$NEXDESK_HOST\" = h'",
            &[("NEXDESK_HOST", "h")],
            DEFAULT_TIMEOUT,
        )
        .unwrap();
        // shell metacharacters are not interpreted (the file must not be created)
        let marker = std::env::temp_dir().join(format!("nexdesk-hook-{}", std::process::id()));
        let _ = run(
            &format!("echo hi > {}", marker.display()),
            &[],
            DEFAULT_TIMEOUT,
        );
        assert!(!marker.exists());
    }

    #[cfg(unix)]
    #[test]
    fn timeout_kills() {
        let t = Instant::now();
        let e = run("sleep 30", &[], Duration::from_millis(200)).unwrap_err();
        assert!(e.contains("timed out"));
        assert!(t.elapsed() < Duration::from_secs(5));
    }
}
