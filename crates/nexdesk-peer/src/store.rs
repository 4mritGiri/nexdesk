//! Where the peer protocol keeps its identity and pinned peers.
//! Prototype: the identity seeds are a plain 0600 file; they belong in the encrypted vault later.
use std::io::Write;
use std::path::{Path, PathBuf};

use nexdesk_crypto::Identity;

use crate::PeerError;

/// `~/.config/nexdesk/peer` (honours `XDG_CONFIG_HOME`).
pub fn default_dir() -> Option<PathBuf> {
    let base = crate::paths::config_base()?;
    Some(base.join("nexdesk").join("peer"))
}

/// Load `dir/<name>` or create a fresh identity there (directory 0700, file 0600).
pub fn load_or_create_identity(dir: &Path, name: &str) -> Result<Identity, PeerError> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let path = dir.join(name);
    match std::fs::read(&path) {
        Ok(b) => {
            Identity::from_seed_bytes(&b).map_err(|_| PeerError::Proto("identity file is damaged"))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let id = Identity::generate()?;
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts.open(&path)?;
            f.write_all(&*id.to_seeds())?;
            f.sync_all()?;
            Ok(id)
        }
        Err(e) => Err(e.into()),
    }
}

/// The agent's relay ID (nine digits), created once and kept in `dir/agent-id`.
pub fn load_or_create_id(dir: &Path) -> Result<String, PeerError> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join("agent-id");
    if let Ok(s) = std::fs::read_to_string(&path) {
        let s = s.trim().to_string();
        if nexdesk_network::proto::valid_id(&s) {
            return Ok(s);
        }
    }
    let id = nexdesk_network::proto::random_id()
        .map_err(|_| PeerError::Proto("no random numbers available"))?;
    std::fs::write(&path, format!("{id}\n"))?;
    Ok(id)
}

/// Replace the saved relay ID with a new random one and return it. The identity key (fingerprint) is unchanged.
pub fn new_id(dir: &Path) -> Result<String, PeerError> {
    std::fs::create_dir_all(dir)?;
    let id = nexdesk_network::proto::random_id()
        .map_err(|_| PeerError::Proto("no random numbers available"))?;
    std::fs::write(dir.join("agent-id"), format!("{id}\n"))?;
    Ok(id)
}

/// The relay server this computer uses, one line in `dir/relay` (`host:port`), if configured.
pub fn load_relay(dir: &Path) -> Option<String> {
    let s = std::fs::read_to_string(dir.join("relay")).ok()?;
    let s = s.trim();
    (!s.is_empty() && crate::control::valid_addr(s)).then(|| s.to_string())
}

pub fn save_relay(dir: &Path, relay: &str) -> Result<(), PeerError> {
    if !relay.is_empty() && !crate::control::valid_addr(relay) {
        return Err(PeerError::Proto("not a valid relay address"));
    }
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("relay"), format!("{relay}\n"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_created_once_and_private() {
        let dir = std::env::temp_dir().join(format!("nd-peer-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = load_or_create_identity(&dir, "id").unwrap();
        let b = load_or_create_identity(&dir, "id").unwrap();
        assert_eq!(a.public(), b.public());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(dir.join("id"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        std::fs::write(dir.join("bad"), b"short").unwrap();
        assert!(load_or_create_identity(&dir, "bad").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
