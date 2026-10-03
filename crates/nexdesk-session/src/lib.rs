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
        let mut g = self.inner.lock().expect("session manager poisoned");
        let id = g.next_id;
        g.next_id += 1;

        let mut session = Session {
            id,
            profile: profile.clone(),
            state: SessionState::Created,
            created_at: Instant::now(),
            last_error: None,
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
        if !profile.clipboard {
            cmd.arg("--no-clipboard");
        }
        if profile.fullscreen {
            cmd.arg("--fullscreen");
        }
        // Environment handoff avoids command-line exposure. The manager never logs this value.
        if let Some(secret) = &session.password {
            cmd.env("NEXDESK_PASSWORD", secret.expose());
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        match cmd.spawn() {
            Ok(child) => {
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
                session.state = SessionState::Failed;
                self.metrics
                    .failed
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                events.push(SessionEvent::Failed {
                    id,
                    message: e.to_string(),
                });
                Err(SessionError::Spawn(e))
            }
        }
    }

    pub fn disconnect(&self, id: u64) -> Result<SessionEvent, SessionError> {
        let mut g = self.inner.lock().expect("session manager poisoned");
        let s = g.sessions.get_mut(&id).ok_or(SessionError::NotFound(id))?;
        s.child.take().map(|mut c| {
            let _ = c.kill();
        });
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
                    Ok(Some(_)) => {
                        s.state = SessionState::Disconnected;
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
}
