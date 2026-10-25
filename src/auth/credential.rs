//! Credential storage abstraction and platform store implementations.
//!
//! The trait is async-free and synchronous: platform credential APIs (Windows
//! Credential Manager) are blocking C calls, and the data service offloads the
//! entire request lifecycle (including credential reads) onto its background
//! executor, so blocking here never touches the GPUI/UI thread.

use crate::auth::token::Pat;
use crate::domain::HostId;
use thiserror::Error;

/// Error from a credential store operation.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CredentialError {
    #[error("credential store is not supported on this platform")]
    Unsupported,
    #[error("credential not found")]
    NotFound,
    #[error("platform credential operation failed: {message}")]
    Platform { message: String },
    #[error("credential value was corrupted in storage")]
    Corrupted,
}

/// Stable reference to a stored credential. Never contains the secret itself.
///
/// The reference is derived from the host and stable account id so it survives
/// login changes and is stable across restarts (future persistence is keyed by
/// this value, not by the token).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CredentialRef {
    host: HostId,
    account_id: u64,
}

impl CredentialRef {
    pub fn new(host: HostId, account_id: u64) -> Self {
        Self { host, account_id }
    }

    pub fn host(&self) -> &HostId {
        &self.host
    }

    pub fn account_id(&self) -> u64 {
        self.account_id
    }

    /// Target name used by platform stores. Contains no secret material.
    pub fn target_name(&self) -> String {
        format!("githux:{}:{}", self.host.as_str(), self.account_id)
    }
}

/// Platform-agnostic credential store. Implementations must be `Send + Sync`
/// because the data service reads them from a background thread.
pub trait CredentialStore: Send + Sync {
    fn save(&self, reference: &CredentialRef, token: &Pat) -> Result<(), CredentialError>;
    fn load(&self, reference: &CredentialRef) -> Result<Option<Pat>, CredentialError>;
    fn delete(&self, reference: &CredentialRef) -> Result<(), CredentialError>;
    /// Human-readable capability boundary, e.g. "Windows Credential Manager".
    fn describe(&self) -> &'static str;
}

/// Placeholder store for platforms without a supported backend. Always returns
/// [`CredentialError::Unsupported`]; there is deliberately **no** plaintext
/// fallback so secrets never reach an unprotected location.
#[derive(Debug, Clone, Default)]
pub struct UnsupportedStore;

impl CredentialStore for UnsupportedStore {
    fn save(&self, _: &CredentialRef, _: &Pat) -> Result<(), CredentialError> {
        Err(CredentialError::Unsupported)
    }
    fn load(&self, _: &CredentialRef) -> Result<Option<Pat>, CredentialError> {
        Err(CredentialError::Unsupported)
    }
    fn delete(&self, _: &CredentialRef) -> Result<(), CredentialError> {
        Err(CredentialError::Unsupported)
    }
    fn describe(&self) -> &'static str {
        "unsupported (no plaintext fallback)"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_ref_target_has_no_secret() {
        let host = HostId::github();
        let r#ref = CredentialRef::new(host, 42);
        assert_eq!(r#ref.target_name(), "githux:github.com:42");
    }

    #[test]
    fn unsupported_store_has_no_plaintext_fallback() {
        let store = UnsupportedStore;
        let host = HostId::github();
        let r#ref = CredentialRef::new(host, 7);
        let token = Pat::from_str("ghp_anything");
        assert!(matches!(
            store.save(&r#ref, &token),
            Err(CredentialError::Unsupported)
        ));
        assert!(matches!(
            store.load(&r#ref),
            Err(CredentialError::Unsupported)
        ));
        assert!(matches!(
            store.delete(&r#ref),
            Err(CredentialError::Unsupported)
        ));
    }
}
