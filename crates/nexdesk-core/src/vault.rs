//! Encrypted password vault.
//!
//! * One random 256-bit *data key* encrypts the entries (XChaCha20-Poly1305, fresh nonce per save).
//! * The data key is stored twice, wrapped: once under a key derived from the **master password**
//!   (Argon2id, per-vault random salt) and once under a random **recovery key** that is shown to
//!   the user exactly once. Either unlocks the vault; neither is stored anywhere.
//! * Header fields are authenticated; any bit flip makes unlocking fail instead of returning garbage.
//! * The plaintext is length-prefixed and padded to a multiple of 512 bytes, so the file size does not
//!   reveal how many connections have saved passwords.
//!
//! File layout (`~/.config/nexdesk/vault`, mode 0600):
//! `"NXV1" | m_kib u32 | t u32 | p u32 | salt[16] | n1[24] | wrap1[48] | n2[24] | wrap2[48] | nb[24] | body`
use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::credentials::Secret;

const MAGIC: &[u8; 4] = b"NXV1";
const HEADER_FIXED: usize = 4 + 12 + 16; // magic + kdf params + salt
const WRAP_LEN: usize = 24 + 48;
const PAD: usize = 512;
pub const MIN_MASTER_LEN: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfParams {
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

impl KdfParams {
    /// 64 MiB, 3 passes: about 0.3 s on a laptop. Resists GPU guessing far better than PBKDF2.
    pub const DEFAULT: KdfParams = KdfParams { m_kib: 64 * 1024, t: 3, p: 1 };

    fn sane(&self) -> bool {
        (8..=1024 * 1024).contains(&self.m_kib) && (1..=12).contains(&self.t) && (1..=16).contains(&self.p)
    }
}

#[derive(Debug)]
pub enum VaultError {
    Missing,
    AlreadyExists,
    Corrupt,
    /// Wrong master password or recovery key (or the file was tampered with).
    WrongKey,
    Weak,
    Io(io::Error),
}

impl fmt::Display for VaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VaultError::Missing => write!(f, "no vault has been set up yet"),
            VaultError::AlreadyExists => write!(f, "a vault already exists"),
            VaultError::Corrupt => write!(f, "the vault file is damaged or not a NexDesk vault"),
            VaultError::WrongKey => write!(f, "wrong master password or recovery key"),
            VaultError::Weak => write!(f, "the master password must be at least {MIN_MASTER_LEN} characters"),
            VaultError::Io(e) => write!(f, "vault file error: {e}"),
        }
    }
}
impl std::error::Error for VaultError {}
impl From<io::Error> for VaultError {
    fn from(e: io::Error) -> Self {
        VaultError::Io(e)
    }
}

#[derive(Zeroize, ZeroizeOnDrop)]
struct Key([u8; 32]);

pub struct Vault {
    path: PathBuf,
    params: KdfParams,
    salt: [u8; 16],
    wrap1: [u8; WRAP_LEN],
    wrap2: [u8; WRAP_LEN],
    dk: Key,
    entries: BTreeMap<String, Secret>,
}

impl fmt::Debug for Vault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Vault({} entries, locked contents hidden)", self.entries.len())
    }
}

pub fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("nexdesk").join("vault"))
}

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::getrandom(&mut b).expect("operating system random number generator failed");
    b
}

fn derive(master: &str, salt: &[u8; 16], p: KdfParams) -> Result<Key, VaultError> {
    let params = Params::new(p.m_kib, p.t, p.p, Some(32)).map_err(|_| VaultError::Corrupt)?;
    let mut out = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(master.as_bytes(), salt, &mut out)
        .map_err(|_| VaultError::Corrupt)?;
    Ok(Key(out))
}

fn seal(key: &Key, aad: &[u8], plain: &[u8]) -> ([u8; 24], Vec<u8>) {
    let nonce: [u8; 24] = random();
    let c = XChaCha20Poly1305::new((&key.0).into());
    let ct = c
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: plain, aad })
        .expect("encryption cannot fail for in-memory buffers");
    (nonce, ct)
}

fn open(key: &Key, aad: &[u8], nonce: &[u8], ct: &[u8]) -> Result<Vec<u8>, VaultError> {
    let c = XChaCha20Poly1305::new((&key.0).into());
    c.decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad }).map_err(|_| VaultError::WrongKey)
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n")
}
fn unesc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('t') => o.push('\t'),
                Some('n') => o.push('\n'),
                Some(x) => o.push(x),
                None => o.push('\\'),
            }
        } else {
            o.push(c);
        }
    }
    o
}

const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// 32 bytes -> `ABCD-EFGH-...` (52 characters in 13 groups).
pub fn encode_recovery_key(k: &[u8; 32]) -> String {
    let mut out = String::new();
    let (mut buf, mut bits) = (0u32, 0u32);
    for &b in k {
        buf = (buf << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(B32[((buf >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(B32[((buf << (5 - bits)) & 31) as usize] as char);
    }
    out.as_bytes().chunks(4).map(|c| std::str::from_utf8(c).unwrap_or("")).collect::<Vec<_>>().join("-")
}

pub fn decode_recovery_key(s: &str) -> Option<[u8; 32]> {
    let (mut buf, mut bits) = (0u32, 0u32);
    let mut out = Vec::with_capacity(32);
    for ch in s.chars().filter(|c| !matches!(c, '-' | ' ' | '\t' | '\n')) {
        let v = B32.iter().position(|&b| b as char == ch.to_ascii_uppercase())? as u32;
        buf = (buf << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    out.try_into().ok()
}

impl Vault {
    pub fn exists(path: &Path) -> bool {
        path.is_file()
    }

    /// Create a new vault. Returns it (unlocked) and the recovery key to show to the user once.
    pub fn create(path: &Path, master: &str, params: KdfParams) -> Result<(Vault, String), VaultError> {
        if master.chars().count() < MIN_MASTER_LEN {
            return Err(VaultError::Weak);
        }
        if path.exists() {
            return Err(VaultError::AlreadyExists);
        }
        let salt: [u8; 16] = random();
        let dk = Key(random());
        let rk = Key(random());
        let mut v = Vault {
            path: path.to_path_buf(),
            params,
            salt,
            wrap1: [0; WRAP_LEN],
            wrap2: [0; WRAP_LEN],
            dk,
            entries: BTreeMap::new(),
        };
        v.rewrap_master(master)?;
        let (n2, c2) = seal(&rk, b"recovery", &v.dk.0);
        v.wrap2[..24].copy_from_slice(&n2);
        v.wrap2[24..].copy_from_slice(&c2);
        v.save()?;
        let shown = encode_recovery_key(&rk.0);
        Ok((v, shown))
    }

    fn rewrap_master(&mut self, master: &str) -> Result<(), VaultError> {
        self.salt = random();
        let kek = derive(master, &self.salt, self.params)?;
        let (n1, c1) = seal(&kek, b"master", &self.dk.0);
        self.wrap1[..24].copy_from_slice(&n1);
        self.wrap1[24..].copy_from_slice(&c1);
        Ok(())
    }

    fn header_aad(&self) -> Vec<u8> {
        let mut h = Vec::with_capacity(HEADER_FIXED + 2 * WRAP_LEN);
        h.extend_from_slice(MAGIC);
        h.extend_from_slice(&self.params.m_kib.to_le_bytes());
        h.extend_from_slice(&self.params.t.to_le_bytes());
        h.extend_from_slice(&self.params.p.to_le_bytes());
        h.extend_from_slice(&self.salt);
        // both wrapped keys are authenticated too, so damage to the recovery copy is noticed immediately
        h.extend_from_slice(&self.wrap1);
        h.extend_from_slice(&self.wrap2);
        h
    }

    fn read_raw(path: &Path) -> Result<(KdfParams, [u8; 16], [u8; WRAP_LEN], [u8; WRAP_LEN], Vec<u8>), VaultError> {
        let data = match std::fs::read(path) {
            Ok(d) => d,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(VaultError::Missing),
            Err(e) => return Err(e.into()),
        };
        let min = HEADER_FIXED + 2 * WRAP_LEN + 24 + 16;
        if data.len() < min || &data[..4] != MAGIC {
            return Err(VaultError::Corrupt);
        }
        let u = |i: usize| u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
        let params = KdfParams { m_kib: u(4), t: u(8), p: u(12) };
        if !params.sane() {
            return Err(VaultError::Corrupt);
        }
        let salt: [u8; 16] = data[16..32].try_into().map_err(|_| VaultError::Corrupt)?;
        let w1: [u8; WRAP_LEN] = data[32..32 + WRAP_LEN].try_into().map_err(|_| VaultError::Corrupt)?;
        let w2: [u8; WRAP_LEN] = data[32 + WRAP_LEN..32 + 2 * WRAP_LEN].try_into().map_err(|_| VaultError::Corrupt)?;
        Ok((params, salt, w1, w2, data[32 + 2 * WRAP_LEN..].to_vec()))
    }

    fn finish_unlock(path: &Path, params: KdfParams, salt: [u8; 16], w1: [u8; WRAP_LEN], w2: [u8; WRAP_LEN], body: Vec<u8>, dk: Key) -> Result<Vault, VaultError> {
        let mut v = Vault { path: path.to_path_buf(), params, salt, wrap1: w1, wrap2: w2, dk, entries: BTreeMap::new() };
        let aad = v.header_aad();
        // authenticate the header: it is the AAD of the body
        if body.len() < 24 + 16 {
            return Err(VaultError::Corrupt);
        }
        let plain = open(&v.dk, &aad, &body[..24], &body[24..]).map_err(|_| VaultError::Corrupt)?;
        if plain.len() < 4 {
            return Err(VaultError::Corrupt);
        }
        let n = u32::from_le_bytes([plain[0], plain[1], plain[2], plain[3]]) as usize;
        let text = plain.get(4..4 + n).ok_or(VaultError::Corrupt)?;
        let text = std::str::from_utf8(text).map_err(|_| VaultError::Corrupt)?;
        for line in text.lines() {
            if let Some((k, val)) = line.split_once('\t') {
                v.entries.insert(unesc(k), Secret::new(unesc(val)));
            }
        }
        Ok(v)
    }

    pub fn unlock(path: &Path, master: &str) -> Result<Vault, VaultError> {
        let (params, salt, w1, w2, body) = Self::read_raw(path)?;
        let kek = derive(master, &salt, params)?;
        let dk = open(&kek, b"master", &w1[..24], &w1[24..])?;
        let dk = Key(dk.try_into().map_err(|_| VaultError::Corrupt)?);
        Self::finish_unlock(path, params, salt, w1, w2, body, dk)
    }

    pub fn unlock_with_recovery(path: &Path, recovery: &str) -> Result<Vault, VaultError> {
        let rk = Key(decode_recovery_key(recovery).ok_or(VaultError::WrongKey)?);
        let (params, salt, w1, w2, body) = Self::read_raw(path)?;
        let dk = open(&rk, b"recovery", &w2[..24], &w2[24..])?;
        let dk = Key(dk.try_into().map_err(|_| VaultError::Corrupt)?);
        Self::finish_unlock(path, params, salt, w1, w2, body, dk)
    }

    /// Set a new master password (the recovery key stays valid).
    pub fn change_master(&mut self, new_master: &str) -> Result<(), VaultError> {
        if new_master.chars().count() < MIN_MASTER_LEN {
            return Err(VaultError::Weak);
        }
        self.rewrap_master(new_master)?;
        self.save()
    }

    fn save(&self) -> Result<(), VaultError> {
        let mut text = String::new();
        for (k, v) in &self.entries {
            text.push_str(&format!("{}\t{}\n", esc(k), esc(v.expose())));
        }
        let mut plain = Vec::with_capacity(4 + text.len() + PAD);
        plain.extend_from_slice(&(text.len() as u32).to_le_bytes());
        plain.extend_from_slice(text.as_bytes());
        plain.resize(plain.len().div_ceil(PAD) * PAD, 0);
        let aad = self.header_aad();
        let (nb, body) = seal(&self.dk, &aad, &plain);
        plain.zeroize();
        text.zeroize();

        let mut out = aad;
        out.extend_from_slice(&nb);
        out.extend_from_slice(&body);

        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("tmp");
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(&out)?;
        f.sync_all()?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    // ---- entries (every change is written to disk immediately)

    pub fn get(&self, connection: &str) -> Option<Secret> {
        self.entries.get(connection).cloned()
    }

    pub fn has(&self, connection: &str) -> bool {
        self.entries.contains_key(connection)
    }

    pub fn set(&mut self, connection: &str, password: Secret) -> Result<(), VaultError> {
        self.entries.insert(connection.to_string(), password);
        self.save()
    }

    pub fn remove(&mut self, connection: &str) -> Result<(), VaultError> {
        if self.entries.remove(connection).is_some() {
            self.save()?;
        }
        Ok(())
    }

    pub fn rename(&mut self, old: &str, new: &str) -> Result<(), VaultError> {
        if let Some(v) = self.entries.remove(old) {
            self.entries.insert(new.to_string(), v);
            self.save()?;
        }
        Ok(())
    }

    pub fn clear(&mut self) -> Result<(), VaultError> {
        self.entries.clear();
        self.save()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAST: KdfParams = KdfParams { m_kib: 64, t: 1, p: 1 };

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nexdesk-vault-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("vault")
    }

    #[test]
    fn create_save_unlock_roundtrip() {
        let p = tmp("rt");
        let (mut v, rec) = Vault::create(&p, "correct horse battery", FAST).unwrap();
        v.set("Bank DB", Secret::new("pa\tss\nword\\")).unwrap();
        v.set("Jump", Secret::new("x")).unwrap();
        assert_eq!(rec.len(), 52 + 12); // 52 chars + 12 dashes
        drop(v);
        let v = Vault::unlock(&p, "correct horse battery").unwrap();
        assert_eq!(v.get("Bank DB").unwrap().expose(), "pa\tss\nword\\");
        assert_eq!(v.len(), 2);
        assert!(v.get("nope").is_none());
    }

    #[test]
    fn wrong_password_and_weak_password() {
        let p = tmp("wrong");
        assert!(matches!(Vault::create(&p, "short", FAST), Err(VaultError::Weak)));
        Vault::create(&p, "long enough pw", FAST).unwrap();
        assert!(matches!(Vault::unlock(&p, "long enough px"), Err(VaultError::WrongKey)));
        assert!(matches!(Vault::create(&p, "long enough pw", FAST), Err(VaultError::AlreadyExists)));
        assert!(matches!(Vault::unlock(&p.with_file_name("nothing"), "x"), Err(VaultError::Missing)));
    }

    #[test]
    fn recovery_key_unlocks_and_master_can_be_reset() {
        let p = tmp("rec");
        let (mut v, rec) = Vault::create(&p, "first master pw", FAST).unwrap();
        v.set("A", Secret::new("secret-a")).unwrap();
        drop(v);
        // lowercase and spaces instead of dashes are accepted
        let typed = rec.replace('-', " ").to_lowercase();
        let mut v = Vault::unlock_with_recovery(&p, &typed).unwrap();
        assert_eq!(v.get("A").unwrap().expose(), "secret-a");
        v.change_master("second master pw").unwrap();
        drop(v);
        assert!(Vault::unlock(&p, "first master pw").is_err());
        assert!(Vault::unlock(&p, "second master pw").is_ok());
        // the recovery key still works after the change
        assert!(Vault::unlock_with_recovery(&p, &rec).is_ok());
        assert!(Vault::unlock_with_recovery(&p, "AAAA-BBBB").is_err());
    }

    #[test]
    fn tampering_is_detected_everywhere() {
        let p = tmp("tamper");
        let (mut v, _) = Vault::create(&p, "tamper test pw", FAST).unwrap();
        v.set("A", Secret::new("s")).unwrap();
        drop(v);
        let good = std::fs::read(&p).unwrap();
        for i in (0..good.len()).step_by(7) {
            let mut bad = good.clone();
            bad[i] ^= 0x01;
            std::fs::write(&p, &bad).unwrap();
            assert!(Vault::unlock(&p, "tamper test pw").is_err(), "flip at byte {i} went unnoticed");
        }
        std::fs::write(&p, &good).unwrap();
        assert!(Vault::unlock(&p, "tamper test pw").is_ok());
    }

    #[test]
    fn file_hides_contents_and_size_and_is_private() {
        let p = tmp("hide");
        let (mut v, _) = Vault::create(&p, "hide test pw", FAST).unwrap();
        let empty_len = std::fs::metadata(&p).unwrap().len();
        v.set("Bank", Secret::new("hunter2-hunter2")).unwrap();
        let raw = std::fs::read(&p).unwrap();
        let hay = String::from_utf8_lossy(&raw);
        assert!(!hay.contains("hunter2") && !hay.contains("Bank"));
        assert_eq!(raw.len() as u64, empty_len, "padding should hide a small change in size");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn rename_remove_clear() {
        let p = tmp("ren");
        let (mut v, _) = Vault::create(&p, "rename test pw", FAST).unwrap();
        v.set("old", Secret::new("1")).unwrap();
        v.rename("old", "new").unwrap();
        v.set("x", Secret::new("2")).unwrap();
        v.remove("x").unwrap();
        let v2 = Vault::unlock(&p, "rename test pw").unwrap();
        assert!(v2.has("new") && !v2.has("old") && !v2.has("x"));
        let mut v2 = v2;
        v2.clear().unwrap();
        assert!(Vault::unlock(&p, "rename test pw").unwrap().is_empty());
    }

    #[test]
    fn hostile_header_params_are_rejected_not_allocated() {
        let p = tmp("dos");
        Vault::create(&p, "dos test pw 1", FAST).unwrap();
        let mut d = std::fs::read(&p).unwrap();
        d[4..8].copy_from_slice(&u32::MAX.to_le_bytes()); // 4 TiB of Argon2 memory
        std::fs::write(&p, d).unwrap();
        assert!(matches!(Vault::unlock(&p, "dos test pw 1"), Err(VaultError::Corrupt)));
    }

    #[test]
    fn recovery_encoding_roundtrip() {
        let k: [u8; 32] = random();
        assert_eq!(decode_recovery_key(&encode_recovery_key(&k)), Some(k));
        assert_eq!(decode_recovery_key("0000"), None);
    }
}
