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
        let policy = CachePolicy::parse(self.cache_control.as_deref());
        !policy.no_cache
            && !policy.no_store
            && policy
                .max_age
                .is_some_and(|max_age| self.current_age_ms(now_ms) < max_age.saturating_mul(1000))
    }
    fn current_age_ms(&self, now_ms: i64) -> u64 {
        self.age_secs
            .saturating_mul(1000)
            .saturating_add(now_ms.saturating_sub(self.verified_at_ms).max(0) as u64)
    }
    /// RFC 5861 permission; revalidation prohibitions take precedence.
    pub fn stale_window_remaining_ms(&self, now_ms: i64) -> Option<u64> {
        let policy = CachePolicy::parse(self.cache_control.as_deref());
        if policy.no_cache || policy.no_store || policy.must_revalidate || self.is_fresh_at(now_ms)
        {
            return None;
        }
        let end = policy
            .max_age?
            .saturating_add(policy.swr?)
            .saturating_mul(1000);
        let age = self.current_age_ms(now_ms);
        (age < end).then_some(end.saturating_sub(age))
    }
    pub fn from_snapshot(snapshot: &CacheSnapshot) -> Self {
        let age_secs = snapshot
            .metadata
            .cache_control
            .as_deref()
            .and_then(|cc| {
                cc.split(',').find_map(|part| {
                    let (key, value) = part.trim().split_once('=')?;
                    key.eq_ignore_ascii_case("githux-age")
                        .then(|| value.trim().parse().ok())
                        .flatten()
                })
            })
            .unwrap_or(0);
        let age_policy_current = snapshot
            .metadata
            .cache_control
            .as_deref()
            .is_some_and(|cc| {
                cc.split(',')
                    .any(|part| part.trim().eq_ignore_ascii_case("githux-age-v2"))
            });
        // Older entries did not include Date/response delay in current age.
        // Keep their bytes and validators, but validate once before reuse.
        let cache_control = if age_policy_current {
            snapshot.metadata.cache_control.clone()
        } else {
            Some(format!(
                "no-cache, {}",
                snapshot.metadata.cache_control.as_deref().unwrap_or("")
            ))
        };
        Self {
            etag: snapshot.metadata.etag.clone(),
            last_modified: snapshot.metadata.last_modified.clone(),
            cache_control,
            fetched_at_ms: snapshot.metadata.fetched_at_ms,
            verified_at_ms: snapshot.metadata.verified_at_ms,
            age_secs,
        }
    }
}
#[derive(Default)]
struct CachePolicy {
    no_cache: bool,
    no_store: bool,
    must_revalidate: bool,
    max_age: Option<u64>,
    swr: Option<u64>,
}
impl CachePolicy {
    fn parse(value: Option<&str>) -> Self {
        let mut policy = Self::default();
        let mut seen_age = false;
        let mut seen_swr = false;
        for directive in value.unwrap_or("").split(',') {
            let (name, value) = directive
                .trim()
                .split_once('=')
                .map_or((directive.trim(), None), |(name, value)| {
                    (name.trim(), Some(value.trim().trim_matches('"')))
                });
            match name.to_ascii_lowercase().as_str() {
                "no-cache" => policy.no_cache = true,
                "no-store" => policy.no_store = true,
                "must-revalidate" | "proxy-revalidate" => policy.must_revalidate = true,
                "max-age" => {
                    if seen_age {
                        policy.no_cache = true;
                    }
                    seen_age = true;
                    policy.max_age = value.and_then(|s| s.parse().ok());
                }
                "stale-while-revalidate" => {
                    if seen_swr {
                        policy.no_cache = true;
                    }
                    seen_swr = true;
                    policy.swr = value.and_then(|s| s.parse().ok());
                }
                _ => {}
            }
        }
        policy
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
        self.put_resolved_blob_with_ticket(self.begin_resolution(commit, path)?, blob_sha)
    }
    pub fn begin_resolution(
        &self,
        commit: &ObjectSha,
        path: &str,
    ) -> Result<crate::storage::cache::CacheWrite, CacheError> {
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
        self.store.begin_write(&self.scope, &key)
    }
    pub fn put_resolved_blob_with_ticket(
        &self,
        ticket: crate::storage::cache::CacheWrite,
        blob_sha: &ObjectSha,
    ) -> Result<WriteOutcome, CacheError> {
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
        self.put_repository_blob_response(ticket, bytes, None)
    }

    pub fn put_repository_blob_response(
        &self,
        ticket: crate::storage::cache::CacheWrite,
        bytes: &[u8],
        cache_control: Option<String>,
    ) -> Result<WriteOutcome, CacheError> {
        let now_ms = now_ms();
        let metadata = CacheMetadata {
            representation_version: READER_IMAGE_REPRESENTATION_VERSION,
            fetched_at_ms: now_ms,
            verified_at_ms: now_ms,
            completeness: CacheCompleteness::Complete,
            etag: None,
            last_modified: None,
            cache_control,
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
        self.revalidate_external_image_with_ticket(ticket, url, cached, meta)
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
        let mut directives = meta
            .cache_control
            .as_deref()
            .unwrap_or("")
            .split(',')
            .map(str::trim)
            .filter(|part| {
                !part.is_empty()
                    && !part.split('=').next().is_some_and(|name| {
                        name.trim().eq_ignore_ascii_case("githux-age")
                            || name.trim().eq_ignore_ascii_case("githux-age-v2")
                    })
            })
            .map(str::to_string)
            .collect::<Vec<_>>();
        if let Some(age) = meta.age_secs {
            directives.push(format!("githux-age={age}"));
        }
        directives.push("githux-age-v2".into());
        let cache_control = Some(directives.join(", "));

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
        url: &str,
        cached: CacheSnapshot,
        meta: &ExternalImageMetadata,
    ) -> Result<WriteOutcome, CacheError> {
        let current = self.read_external_image(url)?.ok_or(CacheError::Stale)?;
        if current.bytes != cached.bytes
            || current.metadata.etag != cached.metadata.etag
            || current.metadata.last_modified != cached.metadata.last_modified
            || current.metadata.cache_control != cached.metadata.cache_control
        {
            return Err(CacheError::Stale);
        }
        let now = now_ms();
        let cc_base = meta
            .cache_control
            .as_ref()
            .or(cached.metadata.cache_control.as_ref());
        let cache_control = match cc_base {
            Some(cc) => {
                let stripped = cc
                    .split(',')
                    .filter(|p| {
                        !p.trim().split('=').next().is_some_and(|name| {
                            name.trim().eq_ignore_ascii_case("githux-age")
                                || name.trim().eq_ignore_ascii_case("githux-age-v2")
                        })
                    })
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

        let cache_control = Some(cache_control.map_or_else(
            || "githux-age-v2".into(),
            |cc| format!("{cc}, githux-age-v2"),
        ));
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
