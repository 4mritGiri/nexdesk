//! Enterprise session boundary. The protocol implementation stays behind a
//! process boundary so a crashed RDP engine cannot take down the manager UI.

use nexdesk_core::{credentials::Secret, profiles::Profile};
use std::{
    collections::HashMap,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Created,
    Starting,
    Connecting,
    Connected,
    Reconnecting,
    Disconnecting,
    Disconnected,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisconnectReason {
    UserRequested,
    RemoteClosed,
    ProtocolError,
    ProcessExited,
}

#[derive(Debug, Clone)]
pub enum SessionEvent {
    StateChanged {
        id: u64,
        from: SessionState,
        to: SessionState,
    },
    Connected {
        id: u64,
    },
    Disconnected {
        id: u64,
        reason: DisconnectReason,
    },
    Failed {
        id: u64,
        message: String,
    },
    ReconnectScheduled {
        id: u64,
        attempt: u32,
        delay: Duration,
    },
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("session {0} was not found")]
    NotFound(u64),
    #[error("session {0} is already active")]
    AlreadyActive(u64),
    #[error("invalid profile: {0}")]
    InvalidProfile(&'static str),
    #[error("failed to start RDP engine: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("before-connect command failed: {0}")]
    Hook(String),
}

/// Run a profile hook with context in the environment (never the password).
fn run_hook(label: &str, cmd: &str, p: &Profile) -> Result<(), String> {
    let r = nexdesk_core::hooks::run(
        cmd,
        &[
            ("NEXDESK_HOST", p.host.trim()),
            ("NEXDESK_PROFILE", p.name.trim()),
            ("NEXDESK_USER", p.user.trim()),
        ],
        nexdesk_core::hooks::DEFAULT_TIMEOUT,
    );
    match &r {
        Ok(()) => nexdesk_core::logs::connection(
            nexdesk_core::logs::Level::Info,
            label,
            &p.name,
            &p.host,
            &p.user,
            "ok",
        ),
        Err(e) => nexdesk_core::logs::connection(
            nexdesk_core::logs::Level::Error,
            label,
            &p.name,
            &p.host,
            &p.user,
            e,
        ),
    }
    r
}

/// After-disconnect command, on its own thread so the UI never waits for it.
fn spawn_post_hook(p: &Profile) {
    if p.post_command.trim().is_empty() {
        return;
    }
    let p = p.clone();
    let _ = std::thread::Builder::new()
        .name("post-hook".into())
        .spawn(move || {
            let _ = run_hook("After-disconnect", &p.post_command, &p);
        });
}

#[derive(Debug, Default)]
pub struct SessionMetrics {
    started: std::sync::atomic::AtomicU64,
    connected: std::sync::atomic::AtomicU64,
    failed: std::sync::atomic::AtomicU64,
    reconnects: std::sync::atomic::AtomicU64,
}

impl SessionMetrics {
    pub fn started(&self) -> u64 {
        self.started.load(std::sync::atomic::Ordering::Relaxed)
    }
    pub fn connected(&self) -> u64 {
        self.connected.load(std::sync::atomic::Ordering::Relaxed)
    }
    pub fn failed(&self) -> u64 {
        self.failed.load(std::sync::atomic::Ordering::Relaxed)
    }
    pub fn reconnects(&self) -> u64 {
        self.reconnects.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[derive(Debug)]
pub struct Session {
    pub id: u64,
    pub profile: Profile,
    pub state: SessionState,
    pub created_at: Instant,
    pub last_error: Option<String>,
    started_wall: Option<Instant>,
    child: Option<Child>,
    password: Option<Secret>,
    engine_path: PathBuf,
}

impl Session {
    fn transition(&mut self, next: SessionState) -> SessionEvent {
        let from = self.state;
        self.state = next;
        SessionEvent::StateChanged {
            id: self.id,
            from,
            to: next,
        }
    }
}

#[derive(Clone)]
pub struct SessionManager {
    inner: Arc<Mutex<ManagerInner>>,
    pub metrics: Arc<SessionMetrics>,
}

struct ManagerInner {
    next_id: u64,
    sessions: HashMap<u64, Session>,
    engine_path: PathBuf,
}

impl SessionManager {
    pub fn new(engine_path: PathBuf) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ManagerInner {
                next_id: 1,
                sessions: HashMap::new(),
                engine_path,
            })),
            metrics: Arc::new(SessionMetrics::default()),
        }
    }

    pub fn list(&self) -> Vec<(u64, Profile, SessionState)> {
        let g = self.inner.lock().expect("session manager poisoned");
        g.sessions
            .values()
            .map(|s| (s.id, s.profile.clone(), s.state))
            .collect()
    }

    pub fn start(
        &self,
        profile: Profile,
        password: Secret,
    ) -> Result<(u64, Vec<SessionEvent>), SessionError> {
        profile.validate().map_err(SessionError::InvalidProfile)?;
        // Pre-connect command (e.g. bring a VPN up). Runs before anything is spawned; failure aborts.
        if !profile.pre_command.trim().is_empty() {
            if let Err(e) = run_hook("Before-connect", &profile.pre_command, &profile) {
                return Err(SessionError::Hook(e));
            }
        }
        let mut g = self.inner.lock().expect("session manager poisoned");
        let id = g.next_id;
        g.next_id += 1;

        let mut session = Session {
            id,
            profile: profile.clone(),
            state: SessionState::Created,
            created_at: Instant::now(),
            last_error: None,
            started_wall: None,
            child: None,
            password: Some(password),
            engine_path: g.engine_path.clone(),
        };
        let mut events = vec![session.transition(SessionState::Starting)];

        let mut cmd = Command::new(&session.engine_path);
        cmd.arg("--host")
            .arg(profile.host.trim())
            .arg("-u")
            .arg(profile.user.trim())
            .arg("--width")
            .arg(profile.width.to_string())
            .arg("--height")
            .arg(profile.height.to_string());
        if !profile.domain.trim().is_empty() {
            cmd.arg("-d").arg(profile.domain.trim());
        }
        cmd.arg("--perf").arg(profile.speed.cli());
        if !profile.clipboard {
            cmd.arg("--no-clipboard");
        }
        let prefs = nexdesk_core::settings::Settings::load();
        if profile.fullscreen || prefs.start_fullscreen {
            cmd.arg("--fullscreen");
        }
        cmd.arg("--tls").arg(prefs.tls.cli());
        if !prefs.key_capture {
            cmd.arg("--no-key-capture");
        }
        if !prefs.drop_paste {
            cmd.arg("--no-drop-paste");
        }
        if prefs.native_frame {
            cmd.arg("--native-frame");
        }
        // Environment handoff avoids command-line exposure. The manager never logs this value.
        if let Some(secret) = &session.password {
            cmd.env("NEXDESK_PASSWORD", secret.expose());
        }
        cmd.env("NEXDESK_PROFILE", profile.name.trim());
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        match cmd.spawn() {
            Ok(mut child) => {
                // Engine output becomes the Console log (and never blocks on a full pipe).
                if let Some(err) = child.stderr.take() {
                    let name = format!("session {id}");
                    std::thread::spawn(move || {
                        use std::io::BufRead;
                        for line in std::io::BufReader::new(err).lines().map_while(Result::ok) {
                            let level = if line.contains("ERROR") || line.contains("error") {
                                nexdesk_core::logs::Level::Error
                            } else if line.contains("WARN") || line.contains("warn") {
                                nexdesk_core::logs::Level::Warn
                            } else {
                                nexdesk_core::logs::Level::Info
                            };
                            nexdesk_core::logs::console(level, &name, line.trim_end());
                        }
                    });
                }
                nexdesk_core::logs::connection(
                    nexdesk_core::logs::Level::Info,
                    "Started",
                    &profile.name,
                    &profile.host,
                    &profile.user,
                    &format!("engine pid {}", child.id()),
                );
                session.started_wall = Some(Instant::now());
                session.child = Some(child);
                self.metrics
                    .started
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                events.push(session.transition(SessionState::Connecting));
                tracing::info!(session_id = id, host = %profile.host, "RDP engine started");
                g.sessions.insert(id, session);
                Ok((id, events))
            }
            Err(e) => {
                session.last_error = Some(e.to_string());
                nexdesk_core::logs::connection(
                    nexdesk_core::logs::Level::Error,
                    "Failed",
                    &profile.name,
                    &profile.host,
                    &profile.user,
                    &format!("cannot start engine: {e}"),
                );
                session.state = SessionState::Failed;
                self.metrics
                    .failed
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                events.push(SessionEvent::Failed {
                    id,
                    message: e.to_string(),
                });
                spawn_post_hook(&profile);
                Err(SessionError::Spawn(e))
            }
        }
    }

    pub fn disconnect(&self, id: u64) -> Result<SessionEvent, SessionError> {
        let mut g = self.inner.lock().expect("session manager poisoned");
        let s = g.sessions.remove(&id).ok_or(SessionError::NotFound(id))?;
        let mut s = s;
        if let Some(mut c) = s.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        nexdesk_core::logs::connection(
            nexdesk_core::logs::Level::Info,
            "Disconnected",
            &s.profile.name,
            &s.profile.host,
            &s.profile.user,
            &format!(
                "closed from manager after {}s",
                s.started_wall.map(|t| t.elapsed().as_secs()).unwrap_or(0)
            ),
        );
        spawn_post_hook(&s.profile);
        Ok(SessionEvent::Disconnected {
            id,
            reason: DisconnectReason::UserRequested,
        })
    }

    pub fn remove_finished(&self) {
        let mut g = self.inner.lock().expect("session manager poisoned");
        g.sessions.retain(|_, s| {
            if let Some(child) = s.child.as_mut() {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        s.state = SessionState::Disconnected;
                        let ok = status.success();
                        nexdesk_core::logs::connection(
                            if ok {
                                nexdesk_core::logs::Level::Info
                            } else {
                                nexdesk_core::logs::Level::Error
                            },
                            "Ended",
                            &s.profile.name,
                            &s.profile.host,
                            &s.profile.user,
                            &format!(
                                "engine {} after {}s",
                                status,
                                s.started_wall.map(|t| t.elapsed().as_secs()).unwrap_or(0)
                            ),
                        );
                        spawn_post_hook(&s.profile);
                        false
                    }
                    Ok(None) => true,
                    Err(_) => true,
                }
            } else {
                true
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_transition_is_typed() {
        let p = Profile::default();
        let mut s = Session {
            id: 1,
            profile: p,
            state: SessionState::Created,
            created_at: Instant::now(),
            last_error: None,
            started_wall: None,
            child: None,
            password: None,
            engine_path: PathBuf::from("nexdesk-rdp"),
        };
        let ev = s.transition(SessionState::Starting);
        assert!(matches!(
            ev,
            SessionEvent::StateChanged {
                from: SessionState::Created,
                to: SessionState::Starting,
                ..
            }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn engine_output_and_exit_reach_the_logs() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("nexdesk-sess-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("XDG_DATA_HOME", &dir);
        let engine = dir.join("fake-engine.sh");
        std::fs::write(
            &engine,
            "#!/bin/sh\necho 'ERROR boom happened' >&2\nexit 3\n",
        )
        .unwrap();
        std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o755)).unwrap();

        let m = SessionManager::new(engine);
        let p = Profile {
            name: "Bank".into(),
            host: "10.0.0.5".into(),
            user: "bob".into(),
            ..Profile::default()
        };
        m.start(p, Secret::new("pw".to_string())).unwrap();
        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(100));
            m.remove_finished();
            if m.list().is_empty() {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(300)); // stderr reader thread
        let conn = nexdesk_core::logs::read_tail(nexdesk_core::logs::Kind::Connection, 10);
        let events: Vec<&str> = conn.iter().map(|e| e.fields[0].as_str()).collect();
        assert!(
            events.contains(&"Started") && events.contains(&"Ended"),
            "{events:?}"
        );
        let con = nexdesk_core::logs::read_tail(nexdesk_core::logs::Kind::Console, 10);
        assert!(con
            .iter()
            .any(|e| e.fields[1].contains("boom") && e.level == nexdesk_core::logs::Level::Error));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
