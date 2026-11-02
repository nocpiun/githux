use serde::Serialize;
use uuid::Uuid;

use super::{CacheError, Result};
use crate::domain::AccountKey;

/// Issued by CacheStore; retired partitions cannot be opened by account identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheScope {
    pub(super) account: AccountKey,
    pub(super) partition: String,
    /// Local uncertain-outcome barrier, deliberately not persisted as permission.
    pub(super) generation: u64,
}
impl CacheScope {
    pub fn account(&self) -> &AccountKey {
        &self.account
    }
    pub fn partition(&self) -> &str {
        &self.partition
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ObjectKind {
    Commit,
    Tree,
    Blob,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
enum Identity {
    Mutable { resource: String, request: String },
    Fixed { kind: ObjectKind, sha: String },
}

/// A repository is mandatory for private bytes. Account aggregates must use a
/// separate private scope and explicitly revalidate that scope after auth changes.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct CacheKey {
    pub(super) repository: Option<u64>,
    pub(super) private: bool,
    identity: Identity,
    representation: String,
    version: u32,
}
impl std::fmt::Debug for CacheKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CacheKey")
            .field("repository", &self.repository)
            .field("private", &self.private)
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}
impl CacheKey {
    /// request is a canonical non-secret identity including parameters/media/API
    /// version. Never pass Authorization, credentials or signed download URLs.
    pub fn mutable(
        repository: Option<u64>,
        private: bool,
        resource: &str,
        request: &str,
        representation: &str,
        version: u32,
    ) -> Result<Self> {
        let key = Self {
            repository,
            private,
            identity: Identity::Mutable {
                resource: resource.into(),
                request: request.into(),
            },
            representation: representation.into(),
            version,
        };
        key.validate()?;
        Ok(key)
    }
    pub fn fixed(
        repository: u64,
        private: bool,
        kind: ObjectKind,
        sha: &str,
        representation: &str,
        version: u32,
    ) -> Result<Self> {
        if !matches!(sha.len(), 40 | 64) || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(CacheError::InvalidInput);
        }
        let key = Self {
            repository: Some(repository),
            private,
            identity: Identity::Fixed {
                kind,
                sha: sha.to_ascii_lowercase(),
            },
            representation: representation.into(),
            version,
        };
        key.validate()?;
        Ok(key)
    }
    fn validate(&self) -> Result<()> {
        if self.version == 0 || self.representation.is_empty() || self.representation.len() > 256 {
            return Err(CacheError::InvalidInput);
        }
        if let Identity::Mutable { resource, request } = &self.identity
            && (resource.is_empty()
                || resource.len() > 256
                || request.len() > 4096
                || resource
                    .chars()
                    .chain(request.chars())
                    .any(char::is_control))
        {
            return Err(CacheError::InvalidInput);
        }
        Ok(())
    }
    pub(super) fn encoded(&self) -> Result<String> {
        serde_json::to_string(self).map_err(|_| CacheError::InvalidInput)
    }
    pub(super) fn repository_key(&self) -> String {
        repository_key(self.repository)
    }
    pub(super) fn version(&self) -> u32 {
        self.version
    }
}

pub(super) fn repository_key(repository: Option<u64>) -> String {
    repository.map_or_else(|| "account".into(), |id| id.to_string())
}
pub(super) fn object_name(value: &str) -> Result<&str> {
    if Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value) {
        Ok(value)
    } else {
        Err(CacheError::Corrupt)
    }
}
