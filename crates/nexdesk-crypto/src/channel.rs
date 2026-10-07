//! Record layer: ChaCha20-Poly1305 with a counter nonce per direction.
//! The transport is an ordered byte stream (TCP/TLS-free), so the counter is implicit and never sent;
//! a replayed, dropped, reordered or modified record fails authentication.
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::{Error, MAX_RECORD};

/// Ratchet the key every this many records (forward secrecy within a long session).
const REKEY_EVERY: u64 = 1 << 20;

struct Direction {
    key: Zeroizing<[u8; 32]>,
    counter: u64,
}

impl Direction {
    fn new(key: [u8; 32]) -> Self {
        Self { key: Zeroizing::new(key), counter: 0 }
    }

    fn nonce(&self) -> Nonce {
        let mut n = [0u8; 12];
        n[4..].copy_from_slice(&self.counter.to_be_bytes());
        *Nonce::from_slice(&n)
    }

    fn advance(&mut self) -> Result<(), Error> {
        self.counter = self.counter.checked_add(1).ok_or(Error::Exhausted)?;
        if self.counter % REKEY_EVERY == 0 {
            let hk = Hkdf::<Sha256>::from_prk(&*self.key).map_err(|_| Error::Exhausted)?;
            let mut next = [0u8; 32];
            hk.expand(b"nexdesk rekey", &mut next).map_err(|_| Error::Exhausted)?;
            self.key = Zeroizing::new(next);
        }
        Ok(())
    }
}

/// An established, mutually authenticated session. One per connection; not `Clone` on purpose.
pub struct Session {
    send: Direction,
    recv: Direction,
    id: [u8; 32],
}

impl Session {
    pub(crate) fn new(send_key: [u8; 32], recv_key: [u8; 32], id: [u8; 32]) -> Self {
        Self { send: Direction::new(send_key), recv: Direction::new(recv_key), id }
    }

    /// Value both sides derive identically; bind higher-level authentication (e.g. a one-time code) to it.
    pub fn session_id(&self) -> [u8; 32] {
        self.id
    }

    /// Encrypt one record. Output = ciphertext || 16-byte tag.
    pub fn seal(&mut self, plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>, Error> {
        if plaintext.len() > MAX_RECORD {
            return Err(Error::TooLarge);
        }
        let c = ChaCha20Poly1305::new(Key::from_slice(&*self.send.key));
        let out = c
            .encrypt(&self.send.nonce(), Payload { msg: plaintext, aad })
            .map_err(|_| Error::Record)?;
        self.send.advance()?;
        Ok(out)
    }

    /// Decrypt the next record. On failure the session must be dropped (the counter is not advanced,
    /// but a peer that sends garbage is not worth keeping).
    pub fn open(&mut self, record: &[u8], aad: &[u8]) -> Result<Vec<u8>, Error> {
        if record.len() > MAX_RECORD + 16 {
            return Err(Error::TooLarge);
        }
        let c = ChaCha20Poly1305::new(Key::from_slice(&*self.recv.key));
        let out = c
            .decrypt(&self.recv.nonce(), Payload { msg: record, aad })
            .map_err(|_| Error::Record)?;
        self.recv.advance()?;
        Ok(out)
    }

    #[cfg(test)]
    pub(crate) fn set_counters(&mut self, send: u64, recv: u64) {
        self.send.counter = send;
        self.recv.counter = recv;
    }
}
