//! Credential boundaries. Profiles intentionally never contain passwords.

use std::fmt;
use zeroize::{Zeroize, ZeroizeOnDrop};

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(REDACTED)")
    }
}

pub trait CredentialStore: Send + Sync {
    fn get_password(&self, profile_id: &str) -> Result<Option<Secret>, CredentialError>;
    fn set_password(&self, profile_id: &str, password: Secret) -> Result<(), CredentialError>;
    fn delete_password(&self, profile_id: &str) -> Result<(), CredentialError>;
}

#[derive(Debug)]
pub enum CredentialError {
    Unavailable(String),
    Failed(String),
}
impl fmt::Display for CredentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(s) => write!(f, "credential store unavailable: {s}"),
            Self::Failed(s) => write!(f, "credential operation failed: {s}"),
        }
    }
}
impl std::error::Error for CredentialError {}

#[derive(Default)]
pub struct EphemeralCredentialStore;
impl CredentialStore for EphemeralCredentialStore {
    fn get_password(&self, _profile_id: &str) -> Result<Option<Secret>, CredentialError> {
        Ok(std::env::var("NEXDESK_PASSWORD").ok().map(Secret::new))
    }
    fn set_password(&self, _profile_id: &str, _password: Secret) -> Result<(), CredentialError> {
        Err(CredentialError::Unavailable(
            "ephemeral store cannot persist passwords".into(),
        ))
    }
    fn delete_password(&self, _profile_id: &str) -> Result<(), CredentialError> {
        Ok(())
    }
}
