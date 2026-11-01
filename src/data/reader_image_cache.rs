//! Durable caching adapter and external freshness policy for reader images.
//!
//! Reader images come in two varieties:
//! 1. Repository-relative pictures:
//!    Bound to a specific repository, commit, path, and blob SHA.
//!    These are immutable Git objects. Once resolved, the blob bytes can be cached
//!    under a fixed cache key and served offline across service/store restarts
//!    as long as the user's account scope and repository access gate allow it.
//! 2. External pictures (e.g. badges, documentation assets):
//!    Identified by canonical URL and fetched without credentials.
//!    These are mutable resources whose freshness is governed by HTTP caching directives
//!    (`Cache-Control`, `ETag`, `Last-Modified`). Revalidation (e.g. conditional requests
//!    yielding 304 Not Modified) updates verified timestamps and retains cached bytes.
//!    Responses specifying `no-store` invalidate any previously cached representation
//!    and are never persisted.

use std::time::SystemTime;

use crate::domain::cache::{CacheCompleteness, CacheMetadata, CacheSnapshot};
use crate::domain::repository::ObjectSha;
use crate::network::reader_images::ExternalImageMetadata;
use crate::storage::cache::{
    CacheError, CacheKey, CacheRead, CacheScope, CacheStore, ObjectKind, WriteOutcome,
};

pub const READER_IMAGE_REPRESENTATION: &str = "reader-image-bytes-v1";
pub const READER_IMAGE_REPRESENTATION_VERSION: u32 = 1;

/// External badge / image cache metadata holding HTTP validation info.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalFreshness {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub cache_control: Option<String>,
    pub fetched_at_ms: i64,
    pub verified_at_ms: i64,
    pub age_secs: u64,
}

impl ExternalFreshness {
    /// Evaluates if an external cached asset is fresh according to max-age directives.
    pub fn is_fresh_at(&self, now_ms: i64) -> bool {
        if let Some(ref cc) = self.cache_control {
            let lower = cc.to_ascii_lowercase();
            if lower.contains("no-cache") {
                return false;
            }
            for part in lower.split(',') {
                let part = part.trim();
                if let Some(rest) = part.strip_prefix("max-age=")
                    && let Ok(max_age_secs) = rest.trim().parse::<u64>()
                {
                    let age_since_verified_secs =
                        now_ms.saturating_sub(self.verified_at_ms).max(0) as u64 / 1000;
                    let total_current_age_secs =
                        self.age_secs.saturating_add(age_since_verified_secs);
                    return total_current_age_secs <= max_age_secs;
                }
            }
        }
        false
    }
}

/// Optional persistent cache configuration for repository and external images.
#[derive(Clone)]
pub struct ReaderImageCache {
    store: CacheStore,
    scope: CacheScope,
    repo_numeric_id: Option<u64>,
    repo_private: bool,
}

impl ReaderImageCache {
    pub fn new(
        store: CacheStore,
        scope: CacheScope,
        repo_numeric_id: Option<u64>,
        repo_private: bool,
    ) -> Self {
        Self {
            store,
            scope,
            repo_numeric_id,
            repo_private,
        }
    }

    pub fn store(&self) -> &CacheStore {
        &self.store
    }

    pub fn scope(&self) -> &CacheScope {
        &self.scope
    }

    pub fn repo_numeric_id(&self) -> Option<u64> {
        self.repo_numeric_id
    }

    pub fn is_repo_private(&self) -> bool {
        self.repo_private
    }

    /// Attempts to read cached resolved blob SHA for a (commit, path).
    pub fn read_resolved_blob(
        &self,
        commit: &ObjectSha,
        path: &str,
    ) -> Result<Option<ObjectSha>, CacheError> {
        let repo_id = self.repo_numeric_id.ok_or(CacheError::InvalidInput)?;
        let request = format!("commit={};path={}", commit.as_str(), path);
        let key = CacheKey::mutable(
            Some(repo_id),
            self.repo_private,
            "reader-image-resolution",
            &request,
            READER_IMAGE_REPRESENTATION,
            READER_IMAGE_REPRESENTATION_VERSION,
        )?;
        match self.store.read(&self.scope, &key)? {
            CacheRead::Hit(lease) => {
                if let Some(snapshot) = lease.snapshot()?
                    && let Ok(sha_str) = std::str::from_utf8(&snapshot.bytes)
                    && let Ok(sha) = ObjectSha::new(sha_str)
                {
                    return Ok(Some(sha));
                }
                Ok(None)
            }
            CacheRead::Miss(_) => Ok(None),
        }
    }

    /// Persists resolved blob SHA for a (commit, path).
    pub fn put_resolved_blob(
        &self,
        commit: &ObjectSha,
        path: &str,
        blob_sha: &ObjectSha,
    ) -> Result<WriteOutcome, CacheError> {
        let repo_id = self.repo_numeric_id.ok_or(CacheError::InvalidInput)?;
        let request = format!("commit={};path={}", commit.as_str(), path);
        let key = CacheKey::mutable(
            Some(repo_id),
            self.repo_private,
            "reader-image-resolution",
            &request,
            READER_IMAGE_REPRESENTATION,
            READER_IMAGE_REPRESENTATION_VERSION,
        )?;
        let ticket = self.store.begin_write(&self.scope, &key)?;
        let now = now_ms();
        let metadata = CacheMetadata {
            representation_version: READER_IMAGE_REPRESENTATION_VERSION,
            fetched_at_ms: now,
            verified_at_ms: now,
            completeness: CacheCompleteness::Complete,
            etag: None,
            last_modified: None,
            cache_control: None,
        };
        self.store
            .put(ticket, blob_sha.as_str().as_bytes(), metadata)
    }
    pub fn repository_blob_key(&self, blob_sha: &ObjectSha) -> Result<CacheKey, CacheError> {
        let repo_id = self.repo_numeric_id.ok_or(CacheError::InvalidInput)?;
        CacheKey::fixed(
            repo_id,
            self.repo_private,
            ObjectKind::Blob,
            blob_sha.as_str(),
            READER_IMAGE_REPRESENTATION,
            READER_IMAGE_REPRESENTATION_VERSION,
        )
    }

    /// Creates a mutable CacheKey for an external image URL.
    pub fn external_image_key(&self, url: &str) -> Result<CacheKey, CacheError> {
        // External pictures are not tied to any repository gate and are public within the scope.
        CacheKey::mutable(
            None,
            false,
            "external-image",
            url,
            READER_IMAGE_REPRESENTATION,
            READER_IMAGE_REPRESENTATION_VERSION,
        )
    }

    /// Attempts to read cached repository blob bytes.
    pub fn read_repository_blob(
        &self,
        blob_sha: &ObjectSha,
    ) -> Result<Option<Vec<u8>>, CacheError> {
        let key = self.repository_blob_key(blob_sha)?;
        match self.store.read(&self.scope, &key)? {
            CacheRead::Hit(lease) => {
                if let Some(snapshot) = lease.snapshot()? {
                    Ok(Some(snapshot.bytes))
                } else {
                    Ok(None)
                }
            }
            CacheRead::Miss(_) => Ok(None),
        }
    }

    /// Begins a write ticket for repository blob bytes before making a network request.
    pub fn begin_write_repository_blob(
        &self,
        blob_sha: &ObjectSha,
    ) -> Result<crate::storage::cache::CacheWrite, CacheError> {
        let key = self.repository_blob_key(blob_sha)?;
        self.store.begin_write(&self.scope, &key)
    }

    /// Persists repository blob bytes using a pre-acquired write ticket.
    pub fn put_repository_blob_with_ticket(
        &self,
        ticket: crate::storage::cache::CacheWrite,
        bytes: &[u8],
    ) -> Result<WriteOutcome, CacheError> {
        let now_ms = now_ms();
        let metadata = CacheMetadata {
            representation_version: READER_IMAGE_REPRESENTATION_VERSION,
            fetched_at_ms: now_ms,
            verified_at_ms: now_ms,
            completeness: CacheCompleteness::Complete,
            etag: None,
            last_modified: None,
            cache_control: None,
        };
        self.store.put(ticket, bytes, metadata)
    }

    /// Reads external image snapshot if cached.
    pub fn read_external_image(&self, url: &str) -> Result<Option<CacheSnapshot>, CacheError> {
        let key = self.external_image_key(url)?;
        match self.store.read(&self.scope, &key)? {
            CacheRead::Hit(lease) => {
                if let Some(snapshot) = lease.snapshot()? {
                    Ok(Some(snapshot))
                } else {
                    Ok(None)
                }
            }
            CacheRead::Miss(_) => Ok(None),
        }
    }

    /// Stores newly fetched external image bytes with metadata.
    pub fn put_external_image(
        &self,
        url: &str,
        bytes: &[u8],
        meta: &ExternalImageMetadata,
    ) -> Result<WriteOutcome, CacheError> {
        let ticket = self.begin_write_external(url)?;
        self.put_external_image_with_ticket(ticket, bytes, meta)
    }

    pub fn put_repository_blob(
        &self,
        blob_sha: &ObjectSha,
        bytes: &[u8],
    ) -> Result<WriteOutcome, CacheError> {
        let ticket = self.begin_write_repository_blob(blob_sha)?;
        self.put_repository_blob_with_ticket(ticket, bytes)
    }

    pub fn revalidate_external_image(
        &self,
        url: &str,
        cached: CacheSnapshot,
        meta: &ExternalImageMetadata,
    ) -> Result<WriteOutcome, CacheError> {
        let ticket = self.begin_write_external(url)?;
        self.revalidate_external_image_with_ticket(ticket, cached, meta)
    }

    /// Begins a write ticket for an external image URL before making a network request.
    pub fn begin_write_external(
        &self,
        url: &str,
    ) -> Result<crate::storage::cache::CacheWrite, CacheError> {
        let key = self.external_image_key(url)?;
        self.store.begin_write(&self.scope, &key)
    }

    /// Stores newly fetched external image bytes with metadata using a pre-acquired write ticket.
    pub fn put_external_image_with_ticket(
        &self,
        ticket: crate::storage::cache::CacheWrite,
        bytes: &[u8],
        meta: &ExternalImageMetadata,
    ) -> Result<WriteOutcome, CacheError> {
        let now = now_ms();
        // Pack age_secs into cache_control or separate representation without violating verified_at_ms >= fetched_at_ms invariant.
        let cache_control = match (meta.cache_control.as_deref(), meta.age_secs) {
            (Some(cc), Some(age)) if !cc.contains("githux-age=") => {
                Some(format!("{cc}, githux-age={age}"))
            }
            (None, Some(age)) => Some(format!("githux-age={age}")),
            (cc, _) => cc.map(str::to_string),
        };

        let metadata = CacheMetadata {
            representation_version: READER_IMAGE_REPRESENTATION_VERSION,
            fetched_at_ms: now,
            verified_at_ms: now,
            completeness: CacheCompleteness::Complete,
            etag: meta.etag.clone(),
            last_modified: meta.last_modified.clone(),
            cache_control,
        };
        self.store.put(ticket, bytes, metadata)
    }

    /// Updates verification timestamp on a 304 Not Modified response using a pre-acquired write ticket.
    pub fn revalidate_external_image_with_ticket(
        &self,
        ticket: crate::storage::cache::CacheWrite,
        cached: CacheSnapshot,
        meta: &ExternalImageMetadata,
    ) -> Result<WriteOutcome, CacheError> {
        let now = now_ms();
        let cc_base = meta
            .cache_control
            .as_ref()
            .or(cached.metadata.cache_control.as_ref());
        let cache_control = match cc_base {
            Some(cc) => {
                let stripped = cc
                    .split(',')
                    .filter(|p| !p.trim().starts_with("githux-age="))
                    .collect::<Vec<_>>()
                    .join(", ");
                match meta.age_secs {
                    Some(age) if stripped.is_empty() => Some(format!("githux-age={age}")),
                    Some(age) => Some(format!("{stripped}, githux-age={age}")),
                    None if stripped.is_empty() => None,
                    None => Some(stripped),
                }
            }
            None => meta.age_secs.map(|age| format!("githux-age={age}")),
        };

        let updated_metadata = CacheMetadata {
            representation_version: READER_IMAGE_REPRESENTATION_VERSION,
            fetched_at_ms: cached.metadata.fetched_at_ms,
            verified_at_ms: now.max(cached.metadata.fetched_at_ms),
            completeness: CacheCompleteness::Complete,
            etag: meta.etag.clone().or(cached.metadata.etag),
            last_modified: meta.last_modified.clone().or(cached.metadata.last_modified),
            cache_control,
        };
        self.store.put(ticket, &cached.bytes, updated_metadata)
    }

    /// Invalidate an external image entry using a pre-acquired write ticket.
    /// If another writer has updated the entry in the meantime, the invalidation is rejected as stale.
    pub fn invalidate_external_image_with_ticket(
        &self,
        ticket: crate::storage::cache::CacheWrite,
    ) -> Result<(), CacheError> {
        let metadata = CacheMetadata {
            representation_version: READER_IMAGE_REPRESENTATION_VERSION,
            fetched_at_ms: now_ms(),
            verified_at_ms: now_ms(),
            completeness: CacheCompleteness::Complete,
            etag: None,
            last_modified: None,
            cache_control: Some("no-store".into()),
        };
        // Putting metadata with no-store using ticket deletes the entry if the ticket is still current!
        let _ = self.store.put(ticket, &[], metadata)?;
        Ok(())
    }
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
