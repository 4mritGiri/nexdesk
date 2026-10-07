//! Long-term device identity: an Ed25519 key and an ML-DSA-65 key, both derived from stored 32-byte seeds.
use ed25519_dalek::{Signature as EdSig, Signer, SigningKey as EdSk, Verifier, VerifyingKey as EdVk};
use ml_dsa::{EncodedSignature, EncodedVerifyingKey, KeyGen, MlDsa65, Signature as DsaSig, VerifyingKey as DsaVk};
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::Error;

pub const ED_PK: usize = 32;
pub const ED_SIG: usize = 64;
pub const DSA_PK: usize = 1952;
pub const DSA_SIG: usize = 3309;
/// Encoded size of [`IdentityPublic`].
pub const PUBLIC_LEN: usize = ED_PK + DSA_PK;
/// Encoded size of a dual signature.
pub const SIG_LEN: usize = ED_SIG + DSA_SIG;

/// Public half of an identity: what peers pin or put on an allow-list.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct IdentityPublic {
    ed: [u8; ED_PK],
    dsa: Vec<u8>,
}

impl IdentityPublic {
    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(PUBLIC_LEN);
        v.extend_from_slice(&self.ed);
        v.extend_from_slice(&self.dsa);
        v
    }

    pub fn decode(b: &[u8]) -> Result<Self, Error> {
        if b.len() != PUBLIC_LEN {
            return Err(Error::Malformed);
        }
        let ed: [u8; ED_PK] = b[..ED_PK].try_into().map_err(|_| Error::Malformed)?;
        EdVk::from_bytes(&ed).map_err(|_| Error::Malformed)?;
        let dsa = b[ED_PK..].to_vec();
        let enc = EncodedVerifyingKey::<MlDsa65>::try_from(dsa.as_slice()).map_err(|_| Error::Malformed)?;
        let _ = DsaVk::<MlDsa65>::decode(&enc);
        Ok(Self { ed, dsa })
    }

    /// SHA-256 over a domain label and both public keys. Pin this value.
    pub fn fingerprint(&self) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(b"nexdesk-identity-v1");
        h.update(self.ed);
        h.update(&self.dsa);
        h.finalize().into()
    }

    /// `SHA256:` + 64 hex digits in groups of 8, e.g. for a trust prompt.
    pub fn fingerprint_string(&self) -> String {
        let hex: String = self.fingerprint().iter().map(|b| format!("{b:02x}")).collect();
        let groups: Vec<&str> = (0..8).map(|i| &hex[i * 8..i * 8 + 8]).collect();
        format!("SHA256:{}", groups.join("-"))
    }

    /// Both signatures must verify (logical AND), so breaking only one algorithm is not enough.
    pub(crate) fn verify(&self, label: &[u8], transcript: &[u8; 32], sig: &[u8]) -> Result<(), Error> {
        if sig.len() != SIG_LEN {
            return Err(Error::Auth);
        }
        let msg = signed_message(label, transcript);
        let vk = EdVk::from_bytes(&self.ed).map_err(|_| Error::Auth)?;
        let es = EdSig::from_slice(&sig[..ED_SIG]).map_err(|_| Error::Auth)?;
        let ed_ok = vk.verify(&msg, &es).is_ok();

        let enc = EncodedVerifyingKey::<MlDsa65>::try_from(self.dsa.as_slice()).map_err(|_| Error::Auth)?;
        let dvk = DsaVk::<MlDsa65>::decode(&enc);
        let ds = EncodedSignature::<MlDsa65>::try_from(&sig[ED_SIG..]).map_err(|_| Error::Auth)?;
        let ds = DsaSig::<MlDsa65>::decode(&ds).ok_or(Error::Auth)?;
        let dsa_ok = dvk.verify_with_context(&msg, label, &ds);
        // evaluate both before deciding, so timing does not reveal which one failed
        if ed_ok & dsa_ok {
            Ok(())
        } else {
            Err(Error::Auth)
        }
    }
}

fn signed_message(label: &[u8], transcript: &[u8; 32]) -> Vec<u8> {
    let mut m = Vec::with_capacity(label.len() + 33);
    m.extend_from_slice(label);
    m.push(0);
    m.extend_from_slice(transcript);
    m
}

/// A device identity with its secret seeds. Store the 64-byte [`Identity::to_seeds`] value encrypted
/// (the password vault is the intended place).
pub struct Identity {
    ed_seed: Zeroizing<[u8; 32]>,
    dsa_seed: Zeroizing<[u8; 32]>,
    public: IdentityPublic,
}

impl Identity {
    pub fn generate() -> Result<Self, Error> {
        let mut a = Zeroizing::new([0u8; 32]);
        let mut b = Zeroizing::new([0u8; 32]);
        OsRng.try_fill_bytes(&mut *a).map_err(|_| Error::Rng)?;
        OsRng.try_fill_bytes(&mut *b).map_err(|_| Error::Rng)?;
        Ok(Self::from_seeds(*a, *b))
    }

    pub fn from_seeds(ed_seed: [u8; 32], dsa_seed: [u8; 32]) -> Self {
        let ed = EdSk::from_bytes(&ed_seed).verifying_key().to_bytes();
        let kp = <MlDsa65 as KeyGen>::key_gen_internal(&dsa_seed.into());
        let dsa = kp.verifying_key().encode().to_vec();
        Self {
            ed_seed: Zeroizing::new(ed_seed),
            dsa_seed: Zeroizing::new(dsa_seed),
            public: IdentityPublic { ed, dsa },
        }
    }

    pub fn from_seed_bytes(b: &[u8]) -> Result<Self, Error> {
        if b.len() != 64 {
            return Err(Error::Malformed);
        }
        let mut a = [0u8; 32];
        let mut c = [0u8; 32];
        a.copy_from_slice(&b[..32]);
        c.copy_from_slice(&b[32..]);
        Ok(Self::from_seeds(a, c))
    }

    pub fn to_seeds(&self) -> Zeroizing<[u8; 64]> {
        let mut out = Zeroizing::new([0u8; 64]);
        out[..32].copy_from_slice(&*self.ed_seed);
        out[32..].copy_from_slice(&*self.dsa_seed);
        out
    }

    pub fn public(&self) -> &IdentityPublic {
        &self.public
    }

    pub(crate) fn sign(&self, label: &[u8], transcript: &[u8; 32]) -> Result<Vec<u8>, Error> {
        let msg = signed_message(label, transcript);
        let es = EdSk::from_bytes(&self.ed_seed).sign(&msg);
        let kp = <MlDsa65 as KeyGen>::key_gen_internal(&(*self.dsa_seed).into());
        let ds = kp.signing_key()
            .sign_deterministic(&msg, label)
            .map_err(|_| Error::Auth)?;
        let mut out = Vec::with_capacity(SIG_LEN);
        out.extend_from_slice(&es.to_bytes());
        out.extend_from_slice(&ds.encode());
        Ok(out)
    }
}
