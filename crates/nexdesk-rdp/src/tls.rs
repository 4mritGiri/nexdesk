//! Server certificate verification.
//!
//! Stock IronRDP accepts *any* certificate (and does not even check the handshake signature),
//! so anyone on the network path could impersonate the server and harvest credentials.
//! This verifier:
//!   1. accepts certificates that chain to the system trust store and match the host name;
//!   2. otherwise (RDP servers are usually self-signed) pins the SHA-256 fingerprint,
//!      trust-on-first-use, in `~/.config/nexdesk/known_hosts`;
//!   3. rejects a *changed* certificate unless the user explicitly accepts it;
//!   4. always verifies the TLS handshake signature, so the peer must hold the private key.
use std::sync::{Arc, Mutex};

use ironrdp_tls::rustls_crate as rustls;
use nexdesk_core::knownhosts::{fingerprint_string, host_key, KnownHosts, Lookup};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, Error, RootCertStore, SignatureScheme};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// Ask the user about unknown or changed certificates (default).
    Ask,
    /// Pin unknown certificates silently (like ssh `accept-new`); still refuse changed ones.
    AcceptNew,
    /// Only connect to already pinned or properly signed certificates.
    Strict,
    /// No verification at all (the old behaviour). Dangerous.
    Insecure,
}

impl Policy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::AcceptNew => "accept-new",
            Self::Strict => "strict",
            Self::Insecure => "insecure",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "ask" => Self::Ask,
            "accept-new" => Self::AcceptNew,
            "strict" => Self::Strict,
            "insecure" => Self::Insecure,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct CertInfo {
    pub host: String,
    pub subject: String,
    pub fingerprint: String,
    /// `Some(old)` when the server presented a different certificate than the pinned one.
    pub pinned: Option<String>,
}

pub type Prompt = Arc<dyn Fn(CertInfo) -> bool + Send + Sync>;

pub struct PolicyVerifier {
    key: String,
    policy: Policy,
    store: Mutex<KnownHosts>,
    webpki: Option<Arc<WebPkiServerVerifier>>,
    provider: Arc<CryptoProvider>,
    prompt: Prompt,
}

impl std::fmt::Debug for PolicyVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PolicyVerifier")
            .field("key", &self.key)
            .field("policy", &self.policy)
            .finish()
    }
}

impl PolicyVerifier {
    pub fn new(
        host: &str,
        port: u16,
        policy: Policy,
        store: KnownHosts,
        prompt: Prompt,
    ) -> Arc<Self> {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let mut roots = RootCertStore::empty();
        for c in rustls_native_certs::load_native_certs().certs {
            let _ = roots.add(c);
        }
        let webpki = if roots.is_empty() {
            None
        } else {
            WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone())
                .build()
                .ok()
        };
        Arc::new(Self {
            key: host_key(host, port),
            policy,
            store: Mutex::new(store),
            webpki,
            provider,
            prompt,
        })
    }

    fn describe(der: &CertificateDer<'_>) -> String {
        use x509_cert::der::Decode;
        x509_cert::Certificate::from_der(der.as_ref())
            .map(|c| c.tbs_certificate.subject.to_string())
            .unwrap_or_else(|_| "(unreadable subject)".into())
    }
}

impl ServerCertVerifier for PolicyVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        if self.policy == Policy::Insecure {
            return Ok(ServerCertVerified::assertion());
        }
        if let Some(w) = &self.webpki {
            if w.verify_server_cert(end_entity, intermediates, server_name, ocsp, now)
                .is_ok()
            {
                return Ok(ServerCertVerified::assertion());
            }
        }
        let fp = fingerprint_string(&Sha256::digest(end_entity.as_ref()));
        let mut store = self
            .store
            .lock()
            .map_err(|_| Error::General("known_hosts lock poisoned".into()))?;
        let pinned = match store.lookup(&self.key, &fp) {
            Lookup::Match => return Ok(ServerCertVerified::assertion()),
            Lookup::Unknown => {
                match self.policy {
                    Policy::Strict => {
                        nexdesk_core::logs::alarm(
                            nexdesk_core::logs::Level::Error,
                            "Untrusted certificate refused",
                            &self.key,
                            &format!("strict mode, fingerprint {fp}"),
                        );
                        return Err(Error::General(format!(
                            "server certificate {fp} is not trusted (strict mode; connect once with --tls ask to pin it)"
                        )));
                    }
                    Policy::AcceptNew => {
                        tracing::warn!("pinning new server certificate for {}: {fp}", self.key);
                        nexdesk_core::logs::alarm(
                            nexdesk_core::logs::Level::Warn,
                            "New certificate pinned (accept-new)",
                            &self.key,
                            &fp,
                        );
                        let _ = store.set(&self.key, &fp);
                        return Ok(ServerCertVerified::assertion());
                    }
                    _ => {}
                }
                None
            }
            Lookup::Mismatch { pinned } => {
                if self.policy != Policy::Ask {
                    nexdesk_core::logs::alarm(
                        nexdesk_core::logs::Level::Error,
                        "SERVER CERTIFICATE CHANGED - connection refused",
                        &self.key,
                        &format!("pinned {pinned}, got {fp}"),
                    );
                    return Err(Error::General(format!(
                        "SERVER CERTIFICATE CHANGED for {} (pinned {pinned}, got {fp}). Possible man-in-the-middle attack. \
                         If the server was reinstalled, run with --forget-host or use --tls ask.",
                        self.key
                    )));
                }
                Some(pinned)
            }
        };
        let info = CertInfo {
            host: self.key.clone(),
            subject: Self::describe(end_entity),
            fingerprint: fp.clone(),
            pinned,
        };
        drop(store); // the prompt can take minutes; do not hold the lock
        let changed = info.pinned.is_some();
        if !(self.prompt)(info) {
            nexdesk_core::logs::alarm(
                nexdesk_core::logs::Level::Warn,
                if changed {
                    "Changed certificate rejected by user"
                } else {
                    "Unknown certificate rejected by user"
                },
                &self.key,
                &fp,
            );
            return Err(Error::General(
                "server certificate rejected by the user".into(),
            ));
        }
        nexdesk_core::logs::alarm(
            if changed {
                nexdesk_core::logs::Level::Error
            } else {
                nexdesk_core::logs::Level::Warn
            },
            if changed {
                "CHANGED certificate trusted by user"
            } else {
                "Unknown certificate trusted by user"
            },
            &self.key,
            &fp,
        );
        if let Ok(mut s) = self.store.lock() {
            if let Err(e) = s.set(&self.key, &fp) {
                tracing::warn!("could not save pinned certificate: {e}");
            }
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls12_signature(
            m,
            c,
            d,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(
            m,
            c,
            d,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn der(n: u8) -> CertificateDer<'static> {
        CertificateDer::from(vec![n; 40])
    }

    fn verify(
        v: &PolicyVerifier,
        c: &CertificateDer<'static>,
    ) -> Result<ServerCertVerified, Error> {
        let name = ServerName::try_from("srv.example").unwrap();
        v.verify_server_cert(c, &[], &name, &[], UnixTime::now())
    }

    fn mk(
        policy: Policy,
        answer: bool,
        store: KnownHosts,
    ) -> (Arc<PolicyVerifier>, Arc<AtomicUsize>) {
        let asked = Arc::new(AtomicUsize::new(0));
        let a2 = asked.clone();
        let prompt: Prompt = Arc::new(move |_| {
            a2.fetch_add(1, Ordering::SeqCst);
            answer
        });
        (
            PolicyVerifier::new("srv.example", 3389, policy, store, prompt),
            asked,
        )
    }

    #[test]
    fn first_use_prompts_then_pins() {
        let (v, asked) = mk(Policy::Ask, true, KnownHosts::in_memory());
        assert!(verify(&v, &der(1)).is_ok());
        assert_eq!(asked.load(Ordering::SeqCst), 1);
        assert!(verify(&v, &der(1)).is_ok(), "pinned cert accepted silently");
        assert_eq!(asked.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn declined_prompt_rejects() {
        let (v, _) = mk(Policy::Ask, false, KnownHosts::in_memory());
        assert!(verify(&v, &der(1)).is_err());
    }

    #[test]
    fn changed_cert_is_refused_unless_user_accepts() {
        let (v, _) = mk(Policy::AcceptNew, true, KnownHosts::in_memory());
        assert!(verify(&v, &der(1)).is_ok());
        let e = verify(&v, &der(2)).unwrap_err();
        assert!(format!("{e}").contains("CHANGED"), "{e}");

        let (v, asked) = mk(Policy::Ask, true, KnownHosts::in_memory());
        verify(&v, &der(1)).unwrap();
        assert!(verify(&v, &der(2)).is_ok());
        assert_eq!(asked.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn strict_rejects_unknown_and_insecure_accepts_all() {
        let (v, asked) = mk(Policy::Strict, true, KnownHosts::in_memory());
        assert!(verify(&v, &der(1)).is_err());
        assert_eq!(asked.load(Ordering::SeqCst), 0);
        let (v, _) = mk(Policy::Insecure, false, KnownHosts::in_memory());
        assert!(verify(&v, &der(9)).is_ok());
    }
}
