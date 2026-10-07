//! Three-message mutually authenticated hybrid handshake.
//!
//! ```text
//! I -> R  msg1: "NDH1" | x25519_eph_I | mlkem768_ek_I | nonce_I
//! R -> I  msg2: "NDH2" | x25519_eph_R | mlkem768_ct   | nonce_R | AEAD_hsR( identity_R | sig_R(th1) )
//! I -> R  msg3: "NDH3" | AEAD_hsI( identity_I | sig_I(th2) )
//! ```
//! * `th1` hashes msg1 and the clear part of msg2; `th2` additionally covers R's encrypted identity;
//!   the application keys are derived from `th3`, which covers everything.
//! * The key schedule input is `x25519_secret || mlkem_secret`: an attacker must break both.
//! * Signatures are dual (Ed25519 and ML-DSA-65) and domain-separated per role.
//! * Identities travel encrypted, so passive observers learn nothing about who connects.
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use ml_kem::kem::{Decapsulate, DecapsulationKey, Encapsulate, EncapsulationKey};
use ml_kem::{Ciphertext, Encoded, EncodedSizeUser, KemCore, MlKem768, MlKem768Params};
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use x25519_dalek::{EphemeralSecret, PublicKey};
use zeroize::Zeroizing;

use crate::channel::Session;
use crate::identity::{Identity, IdentityPublic, PUBLIC_LEN, SIG_LEN};
use crate::Error;

const M1: &[u8; 4] = b"NDH1";
const M2: &[u8; 4] = b"NDH2";
const M3: &[u8; 4] = b"NDH3";
const EK_LEN: usize = 1184;
const CT_LEN: usize = 1088;
const NONCE: usize = 32;
const AUTH_LEN: usize = PUBLIC_LEN + SIG_LEN;
const TAG: usize = 16;
const MSG1_LEN: usize = 4 + 32 + EK_LEN + NONCE;
const MSG2_CLEAR: usize = 4 + 32 + CT_LEN + NONCE;
const MSG2_LEN: usize = MSG2_CLEAR + AUTH_LEN + TAG;
const MSG3_LEN: usize = 4 + AUTH_LEN + TAG;
const LABEL_R: &[u8] = b"nexdesk-v1 responder";
const LABEL_I: &[u8] = b"nexdesk-v1 initiator";

type Ek = EncapsulationKey<MlKem768Params>;
type Dk = DecapsulationKey<MlKem768Params>;

fn hash(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"nexdesk-v1 transcript");
    for p in parts {
        h.update((p.len() as u32).to_be_bytes());
        h.update(p);
    }
    h.finalize().into()
}

fn rand32() -> Result<[u8; 32], Error> {
    let mut b = [0u8; 32];
    OsRng.try_fill_bytes(&mut b).map_err(|_| Error::Rng)?;
    Ok(b)
}

fn expand(prk: &[u8], info: &[u8]) -> Result<[u8; 32], Error> {
    let hk = Hkdf::<Sha256>::from_prk(prk).map_err(|_| Error::Malformed)?;
    let mut out = [0u8; 32];
    hk.expand(info, &mut out).map_err(|_| Error::Malformed)?;
    Ok(out)
}

fn extract(salt: &[u8], ikm: &[u8]) -> Zeroizing<[u8; 32]> {
    let (prk, _) = Hkdf::<Sha256>::extract(Some(salt), ikm);
    Zeroizing::new(prk.into())
}

fn aead_seal(key: &[u8; 32], aad: &[u8], pt: &[u8]) -> Result<Vec<u8>, Error> {
    ChaCha20Poly1305::new(Key::from_slice(key))
        .encrypt(Nonce::from_slice(&[0u8; 12]), Payload { msg: pt, aad })
        .map_err(|_| Error::Auth)
}

fn aead_open(key: &[u8; 32], aad: &[u8], ct: &[u8]) -> Result<Vec<u8>, Error> {
    ChaCha20Poly1305::new(Key::from_slice(key))
        .decrypt(Nonce::from_slice(&[0u8; 12]), Payload { msg: ct, aad })
        .map_err(|_| Error::Auth)
}

fn auth_blob(id: &Identity, label: &[u8], th: &[u8; 32]) -> Result<Vec<u8>, Error> {
    let mut v = id.public().encode();
    v.extend_from_slice(&id.sign(label, th)?);
    Ok(v)
}

fn check_auth(blob: &[u8], label: &[u8], th: &[u8; 32]) -> Result<IdentityPublic, Error> {
    if blob.len() != AUTH_LEN {
        return Err(Error::Malformed);
    }
    let peer = IdentityPublic::decode(&blob[..PUBLIC_LEN])?;
    peer.verify(label, th, &blob[PUBLIC_LEN..])?;
    Ok(peer)
}

/// Viewer side, before the first message is sent.
pub struct Initiator {
    me: Identity,
    x_sk: EphemeralSecret,
    dk: Dk,
    msg1: Vec<u8>,
}

/// Viewer side, waiting for msg2.
pub struct InitiatorWaiting(Initiator);

impl Initiator {
    /// Returns the state and msg1 to send.
    pub fn start(me: Identity) -> Result<(InitiatorWaiting, Vec<u8>), Error> {
        let x_sk = EphemeralSecret::random_from_rng(OsRng);
        let x_pk = PublicKey::from(&x_sk);
        let (dk, ek) = MlKem768::generate(&mut OsRng);
        let mut msg1 = Vec::with_capacity(MSG1_LEN);
        msg1.extend_from_slice(M1);
        msg1.extend_from_slice(x_pk.as_bytes());
        msg1.extend_from_slice(&ek.as_bytes());
        msg1.extend_from_slice(&rand32()?);
        debug_assert_eq!(msg1.len(), MSG1_LEN);
        let out = msg1.clone();
        Ok((InitiatorWaiting(Self { me, x_sk, dk, msg1 }), out))
    }
}

impl InitiatorWaiting {
    /// Process msg2. `accept_peer` receives the responder's verified identity and decides whether to
    /// continue (pin check, prompt). On success returns the session and msg3 to send.
    pub fn finish(
        self,
        msg2: &[u8],
        accept_peer: impl FnOnce(&IdentityPublic) -> bool,
    ) -> Result<(Session, IdentityPublic, Vec<u8>), Error> {
        let Initiator { me, x_sk, dk, msg1 } = self.0;
        if msg2.len() != MSG2_LEN || &msg2[..4] != M2 {
            return Err(Error::Malformed);
        }
        let x_r: [u8; 32] = msg2[4..36].try_into().map_err(|_| Error::Malformed)?;
        let ct_bytes = &msg2[36..36 + CT_LEN];
        let clear = &msg2[..MSG2_CLEAR];
        let enc_auth = &msg2[MSG2_CLEAR..];

        let ss_x = x_sk.diffie_hellman(&PublicKey::from(x_r));
        if !ss_x.was_contributory() {
            return Err(Error::Auth); // low-order point
        }
        let ct = Ciphertext::<MlKem768>::try_from(ct_bytes).map_err(|_| Error::Malformed)?;
        let ss_k = dk.decapsulate(&ct).map_err(|_| Error::Auth)?;

        let th1 = hash(&[&msg1, clear]);
        let mut ikm = Zeroizing::new(Vec::with_capacity(64));
        ikm.extend_from_slice(ss_x.as_bytes());
        ikm.extend_from_slice(ss_k.as_slice());
        let prk = extract(&th1, &ikm);
        let k_r2i = expand(&*prk, b"hs r2i")?;
        let k_i2r = expand(&*prk, b"hs i2r")?;

        let auth_r = aead_open(&k_r2i, &th1, enc_auth)?;
        let peer = check_auth(&auth_r, LABEL_R, &th1)?;
        if !accept_peer(&peer) {
            return Err(Error::Rejected);
        }

        let th2 = hash(&[&th1, enc_auth]);
        let enc3 = aead_seal(&k_i2r, &th2, &auth_blob(&me, LABEL_I, &th2)?)?;
        let mut msg3 = Vec::with_capacity(MSG3_LEN);
        msg3.extend_from_slice(M3);
        msg3.extend_from_slice(&enc3);

        let th3 = hash(&[&th2, &enc3]);
        let prk2 = extract(&th3, &*prk);
        let session = Session::new(
            expand(&*prk2, b"app i2r")?,
            expand(&*prk2, b"app r2i")?,
            expand(&*prk2, b"session id")?,
        );
        Ok((session, peer, msg3))
    }
}

/// Agent side, after replying to msg1 and waiting for msg3.
pub struct ResponderWaiting {
    prk: Zeroizing<[u8; 32]>,
    k_i2r: [u8; 32],
    th2: [u8; 32],
    enc_auth: Vec<u8>,
}

pub struct Responder;

impl Responder {
    /// Process msg1 and produce msg2.
    pub fn respond(me: &Identity, msg1: &[u8]) -> Result<(ResponderWaiting, Vec<u8>), Error> {
        if msg1.len() != MSG1_LEN {
            return Err(Error::Malformed);
        }
        if &msg1[..4] != M1 {
            return Err(Error::Version);
        }
        let x_i: [u8; 32] = msg1[4..36].try_into().map_err(|_| Error::Malformed)?;
        let ek_bytes = &msg1[36..36 + EK_LEN];
        let enc = Encoded::<Ek>::try_from(ek_bytes).map_err(|_| Error::Malformed)?;
        let ek = Ek::from_bytes(&enc);

        let x_sk = EphemeralSecret::random_from_rng(OsRng);
        let x_pk = PublicKey::from(&x_sk);
        let ss_x = x_sk.diffie_hellman(&PublicKey::from(x_i));
        if !ss_x.was_contributory() {
            return Err(Error::Auth);
        }
        let (ct, ss_k) = ek.encapsulate(&mut OsRng).map_err(|_| Error::Rng)?;

        let mut msg2 = Vec::with_capacity(MSG2_LEN);
        msg2.extend_from_slice(M2);
        msg2.extend_from_slice(x_pk.as_bytes());
        msg2.extend_from_slice(ct.as_slice());
        msg2.extend_from_slice(&rand32()?);
        debug_assert_eq!(msg2.len(), MSG2_CLEAR);

        let th1 = hash(&[msg1, &msg2]);
        let mut ikm = Zeroizing::new(Vec::with_capacity(64));
        ikm.extend_from_slice(ss_x.as_bytes());
        ikm.extend_from_slice(ss_k.as_slice());
        let prk = extract(&th1, &ikm);
        let k_r2i = expand(&*prk, b"hs r2i")?;
        let k_i2r = expand(&*prk, b"hs i2r")?;

        let enc_auth = aead_seal(&k_r2i, &th1, &auth_blob(me, LABEL_R, &th1)?)?;
        msg2.extend_from_slice(&enc_auth);
        let th2 = hash(&[&th1, &enc_auth]);
        Ok((ResponderWaiting { prk, k_i2r, th2, enc_auth }, msg2))
    }
}

impl ResponderWaiting {
    /// Process msg3. `accept_peer` receives the initiator's verified identity (allow-list, consent prompt).
    pub fn finish(
        self,
        msg3: &[u8],
        accept_peer: impl FnOnce(&IdentityPublic) -> bool,
    ) -> Result<(Session, IdentityPublic), Error> {
        if msg3.len() != MSG3_LEN || &msg3[..4] != M3 {
            return Err(Error::Malformed);
        }
        let enc3 = &msg3[4..];
        let auth_i = aead_open(&self.k_i2r, &self.th2, enc3)?;
        let peer = check_auth(&auth_i, LABEL_I, &self.th2)?;
        if !accept_peer(&peer) {
            return Err(Error::Rejected);
        }
        let _ = &self.enc_auth;
        let th3 = hash(&[&self.th2, enc3]);
        let prk2 = extract(&th3, &*self.prk);
        let session = Session::new(
            expand(&*prk2, b"app r2i")?,
            expand(&*prk2, b"app i2r")?,
            expand(&*prk2, b"session id")?,
        );
        Ok((session, peer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(
        viewer: Identity,
        agent: &Identity,
        tamper: impl Fn(usize, &mut Vec<u8>),
        trust_agent: bool,
        trust_viewer: bool,
    ) -> Result<(Session, Session, IdentityPublic, IdentityPublic), Error> {
        let (iw, mut m1) = Initiator::start(viewer)?;
        tamper(1, &mut m1);
        let (rw, mut m2) = Responder::respond(agent, &m1)?;
        tamper(2, &mut m2);
        let (si, peer_r, mut m3) = iw.finish(&m2, |_| trust_agent)?;
        tamper(3, &mut m3);
        let (sr, peer_i) = rw.finish(&m3, |_| trust_viewer)?;
        Ok((si, sr, peer_r, peer_i))
    }

    fn no(_: usize, _: &mut Vec<u8>) {}

    #[test]
    fn handshake_and_records_roundtrip() {
        let (v, a) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        let (vp, ap) = (v.public().clone(), a.public().clone());
        let (mut si, mut sr, peer_r, peer_i) = run(v, &a, no, true, true).unwrap();
        assert_eq!(peer_r, ap);
        assert_eq!(peer_i, vp);
        assert_eq!(si.session_id(), sr.session_id());
        let c = si.seal(b"hello agent", b"ch0").unwrap();
        assert_eq!(sr.open(&c, b"ch0").unwrap(), b"hello agent");
        let c = sr.seal(b"hello viewer", b"").unwrap();
        assert_eq!(si.open(&c, b"").unwrap(), b"hello viewer");
        for i in 0..50u8 {
            let c = si.seal(&[i; 1000], b"").unwrap();
            assert_eq!(sr.open(&c, b"").unwrap(), vec![i; 1000]);
        }
    }

    #[test]
    fn every_handshake_byte_is_authenticated() {
        let a = Identity::generate().unwrap();
        // flipping any byte of any message must fail somewhere (sample positions across each message)
        for (msg, len) in [(1usize, MSG1_LEN), (2, MSG2_LEN), (3, MSG3_LEN)] {
            for pos in (0..len).step_by(97).chain([len - 1]) {
                let v = Identity::generate().unwrap();
                let r = run(v, &a, |n, m| if n == msg { m[pos] ^= 0x01 }, true, true);
                assert!(r.is_err(), "msg{msg} byte {pos} flip was accepted");
            }
        }
    }

    #[test]
    fn rejected_peers_stop_the_handshake() {
        let (v, a) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        assert_eq!(run(v, &a, no, false, true).err(), Some(Error::Rejected));
        let v = Identity::generate().unwrap();
        assert_eq!(run(v, &a, no, true, false).err(), Some(Error::Rejected));
    }

    #[test]
    fn wrong_length_and_version_are_refused() {
        let a = Identity::generate().unwrap();
        assert_eq!(Responder::respond(&a, &[0u8; 10]).err(), Some(Error::Malformed));
        let (_, mut m1) = Initiator::start(Identity::generate().unwrap()).unwrap();
        m1[3] = b'9';
        assert_eq!(Responder::respond(&a, &m1).err(), Some(Error::Version));
    }

    #[test]
    fn impostor_agent_with_other_identity_is_visible_to_the_pin_check() {
        let (v, real, fake) = (Identity::generate().unwrap(), Identity::generate().unwrap(), Identity::generate().unwrap());
        let pinned = real.public().fingerprint();
        let (iw, m1) = Initiator::start(v).unwrap();
        let (_rw, m2) = Responder::respond(&fake, &m1).unwrap();
        let r = iw.finish(&m2, |p| p.fingerprint() == pinned);
        assert_eq!(r.err(), Some(Error::Rejected));
    }

    #[test]
    fn records_reject_replay_reorder_tamper_and_wrong_aad() {
        let (v, a) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        let (mut si, mut sr, _, _) = run(v, &a, no, true, true).unwrap();
        let c1 = si.seal(b"one", b"x").unwrap();
        let c2 = si.seal(b"two", b"x").unwrap();
        assert!(sr.open(&c2, b"x").is_err(), "out of order");
        assert!(sr.open(&c1, b"y").is_err(), "wrong aad");
        let mut bad = c1.clone();
        bad[0] ^= 1;
        assert!(sr.open(&bad, b"x").is_err(), "tampered");
        assert_eq!(sr.open(&c1, b"x").unwrap(), b"one");
        assert!(sr.open(&c1, b"x").is_err(), "replay");
        assert_eq!(sr.open(&c2, b"x").unwrap(), b"two");
        // the other direction has an independent key: a record from I does not open as R->I
        let c3 = si.seal(b"three", b"").unwrap();
        assert!(si.open(&c3, b"").is_err(), "reflection");
    }

    #[test]
    fn rekey_keeps_both_sides_in_sync() {
        let (v, a) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        let (mut si, mut sr, _, _) = run(v, &a, no, true, true).unwrap();
        let edge = (1u64 << 20) - 2;
        si.set_counters(edge, 0);
        sr.set_counters(0, edge);
        for i in 0..5u8 {
            let c = si.seal(&[i], b"").unwrap();
            assert_eq!(sr.open(&c, b"").unwrap(), vec![i]);
        }
    }

    #[test]
    fn two_sessions_get_different_keys() {
        let a = Identity::generate().unwrap();
        let v1 = Identity::generate().unwrap();
        let seeds = v1.to_seeds();
        let v2 = Identity::from_seed_bytes(&*seeds).unwrap();
        let (s1, _, _, _) = run(v1, &a, no, true, true).unwrap();
        let (s2, _, _, _) = run(v2, &a, no, true, true).unwrap();
        assert_ne!(s1.session_id(), s2.session_id());
    }

    #[test]
    fn identity_seeds_roundtrip_and_fingerprint_format() {
        let id = Identity::generate().unwrap();
        let back = Identity::from_seed_bytes(&*id.to_seeds()).unwrap();
        assert_eq!(id.public(), back.public());
        let f = id.public().fingerprint_string();
        assert!(f.starts_with("SHA256:") && f.len() == 7 + 64 + 7);
        let enc = id.public().encode();
        assert_eq!(IdentityPublic::decode(&enc).unwrap(), *id.public());
        assert!(IdentityPublic::decode(&enc[..enc.len() - 1]).is_err());
        assert!(Identity::from_seed_bytes(&[0u8; 63]).is_err());
    }

    #[test]
    fn single_signature_is_not_enough() {
        // Valid Ed25519 signature but corrupted ML-DSA signature (and vice versa) must both fail.
        let id = Identity::generate().unwrap();
        let th = [7u8; 32];
        let sig = id.sign(LABEL_R, &th).unwrap();
        assert!(id.public().verify(LABEL_R, &th, &sig).is_ok());
        assert!(id.public().verify(LABEL_I, &th, &sig).is_err(), "role separation");
        let mut a = sig.clone();
        a[70] ^= 1; // inside the ML-DSA part
        assert!(id.public().verify(LABEL_R, &th, &a).is_err());
        let mut b = sig.clone();
        b[3] ^= 1; // inside the Ed25519 part
        assert!(id.public().verify(LABEL_R, &th, &b).is_err());
        assert!(id.public().verify(LABEL_R, &[8u8; 32], &sig).is_err(), "other transcript");
    }
}
