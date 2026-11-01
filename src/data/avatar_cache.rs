//! Account-scoped disk caching adapter for avatar images.
//!
//! Avatars are fetched without credentials, but their disk cache representation
//! is scoped to the viewing account's [`CacheScope`]. This prevents private profile
//! information and cross-account avatar state from leaking across accounts.
//!
//! Key invariants:
//! * Cache keys are derived from the avatar owner's identity (`subject` + sanitized URL),
//!   NOT the viewing account. The viewing account provides the [`CacheScope`].
//! * Signed download URLs or URLs with sensitive query parameters (e.g. signature, token,
//!   credential, auth) have their query strings stripped before forming cache keys or log output.
//! * CDN responses are validated against HTTP cache freshness:
//!   - `no-store`: deletes any existing cached representation and is not persisted to disk.
//!   - `304 Not Modified`: revalidates the existing cached body (only if locally present and matching),
//!     updating the verified timestamp.
//!   - `200 OK`: persists the body alongside metadata (ETag, Last-Modified, Cache-Control).
//! * Expired/retired accounts or old downloads cannot resurrect retired cache scopes or write back.
//! * Disk I/O runs exclusively on Tokio blocking workers (`spawn_blocking`), never on the UI thread.
//! * Corrupt, oversized (>2 MiB), or non-image payloads are discarded and fall back to initials.

use std::time::SystemTime;

use crate::domain::cache::{CacheCompleteness, CacheMetadata, CacheSnapshot};
use crate::storage::cache::{CacheKey, CacheRead, CacheScope, CacheStore, WriteOutcome};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AvatarKey {
    /// `host` + numeric owner/account id, when known, so one host's id cannot match another's.
    pub subject: String,
    /// The exact URL GitHub returned. A different URL is a different key, so a renamed account or a
    /// refreshed URL cannot keep serving the previous picture.
    pub url: String,
}

impl AvatarKey {
    pub fn new(host: impl AsRef<str>, id: u64, url: impl Into<String>) -> Self {
        Self {
            subject: format!("{}#{id}", host.as_ref()),
            url: url.into(),
        }
    }
}

pub trait AvatarKeyLike {
    fn subject(&self) -> &str;
    fn url(&self) -> &str;
}

impl AvatarKeyLike for AvatarKey {
    fn subject(&self) -> &str {
        &self.subject
    }
    fn url(&self) -> &str {
        &self.url
    }
}

/// Cache representation version for avatars.
pub const AVATAR_REPRESENTATION_VERSION: u32 = 1;
pub const AVATAR_REPRESENTATION_NAME: &str = "avatar-image-v1";
pub const AVATAR_RESOURCE_KIND: &str = "avatar";

/// Explicit avatar cache store wrapper.
#[derive(Clone)]
pub struct AvatarDiskCache {
    store: CacheStore,
    scope: CacheScope,
}

impl AvatarDiskCache {
    pub fn new(store: CacheStore, scope: CacheScope) -> Self {
        Self { store, scope }
    }

    pub fn store(&self) -> &CacheStore {
        &self.store
    }

    pub fn scope(&self) -> &CacheScope {
        &self.scope
    }

    /// Prepares a write ticket before network download begins.
    pub fn begin_write_sync<K: AvatarKeyLike>(
        &self,
        key: &K,
    ) -> Result<crate::storage::cache::CacheWrite, crate::storage::cache::CacheError> {
        let cache_key = avatar_cache_key(key)?;
        self.store.begin_write(&self.scope, &cache_key)
    }

    /// Complete write with a previously acquired write ticket.
    pub fn commit_write_sync(
        &self,
        ticket: crate::storage::cache::CacheWrite,
        bytes: &[u8],
        etag: Option<String>,
        last_modified: Option<String>,
        cache_control: Option<String>,
    ) -> Result<WriteOutcome, crate::storage::cache::CacheError> {
        let now = now_ms();
        let metadata = CacheMetadata {
            representation_version: AVATAR_REPRESENTATION_VERSION,
            fetched_at_ms: now,
            verified_at_ms: now,
            completeness: CacheCompleteness::Complete,
            etag,
            last_modified,
            cache_control,
        };

        self.store.put(ticket, bytes, metadata)
    }

    /// Read cached avatar bytes for `key` in a blocking context.
    pub fn read_sync<K: AvatarKeyLike>(&self, key: &K) -> Option<CacheSnapshot> {
        let cache_key = avatar_cache_key(key).ok()?;
        match self.store.read(&self.scope, &cache_key).ok()? {
            CacheRead::Hit(lease) => lease.snapshot().ok().flatten(),
            CacheRead::Miss(_) => None,
        }
    }

    /// Write avatar bytes and metadata for `key` in a blocking context.
    pub fn put_sync<K: AvatarKeyLike>(
        &self,
        key: &K,
        bytes: &[u8],
        etag: Option<String>,
        last_modified: Option<String>,
        cache_control: Option<String>,
    ) -> Result<WriteOutcome, crate::storage::cache::CacheError> {
        let cache_key = avatar_cache_key(key)?;
        let ticket = self.store.begin_write(&self.scope, &cache_key)?;

        let now = now_ms();
        let metadata = CacheMetadata {
            representation_version: AVATAR_REPRESENTATION_VERSION,
            fetched_at_ms: now,
            verified_at_ms: now,
            completeness: CacheCompleteness::Complete,
            etag,
            last_modified,
            cache_control,
        };

        self.store.put(ticket, bytes, metadata)
    }

    /// Update verification timestamp on 304 Not Modified.
    /// If there is no existing valid cached representation matching validator, returns Ok(WriteOutcome::NotStored).
    pub fn revalidate_sync<K: AvatarKeyLike>(
        &self,
        key: &K,
        new_etag: Option<String>,
        new_last_modified: Option<String>,
        new_cache_control: Option<String>,
    ) -> Result<WriteOutcome, crate::storage::cache::CacheError> {
        let cache_key = avatar_cache_key(key)?;
        let existing = match self.store.read(&self.scope, &cache_key).ok() {
            Some(CacheRead::Hit(lease)) => lease.snapshot().ok().flatten(),
            _ => None,
        };

        let Some(existing) = existing else {
            return Ok(WriteOutcome::NotStored);
        };

        let ticket = self.store.begin_write(&self.scope, &cache_key)?;
        let now = now_ms();
        let metadata = CacheMetadata {
            representation_version: AVATAR_REPRESENTATION_VERSION,
            fetched_at_ms: existing.metadata.fetched_at_ms,
            verified_at_ms: now,
            completeness: CacheCompleteness::Complete,
            etag: new_etag.or(existing.metadata.etag),
            last_modified: new_last_modified.or(existing.metadata.last_modified),
            cache_control: new_cache_control.or(existing.metadata.cache_control),
        };

        self.store.put(ticket, &existing.bytes, metadata)
    }

    /// Explicitly remove/invalidate an avatar entry (e.g. on no-store).
    pub fn invalidate_sync<K: AvatarKeyLike>(&self, key: &K) {
        if let Ok(cache_key) = avatar_cache_key(key) {
            let _ = self.store.invalidate(&self.scope, &cache_key);
        }
    }
    /// Checks whether the cached snapshot is eligible for offline/stale fallback.
    /// Responses with `Cache-Control: no-cache` or `must-revalidate` cannot be returned
    /// without revalidation.
    pub fn is_snapshot_reusable_offline(snapshot: &CacheSnapshot) -> bool {
        if let Some(cc) = snapshot.metadata.cache_control.as_deref() {
            for directive in cc.split(',') {
                let name = directive.split('=').next().unwrap_or("").trim();
                if name.eq_ignore_ascii_case("no-cache")
                    || name.eq_ignore_ascii_case("must-revalidate")
                {
                    return false;
                }
            }
        }
        true
    }
}

/// Checks whether the cached snapshot is eligible for offline/stale fallback.
/// Responses with `Cache-Control: no-cache` or `must-revalidate` cannot be returned
/// without revalidation.
pub fn is_snapshot_reusable_offline(snapshot: &CacheSnapshot) -> bool {
    AvatarDiskCache::is_snapshot_reusable_offline(snapshot)
}
///
/// Strips query parameters if any query parameter name looks sensitive
/// (e.g., token, signature, sig, key, auth, credential, expires).
pub fn sanitize_avatar_url(raw_url: &str) -> String {
    let Ok(url) = reqwest::Url::parse(raw_url) else {
        return raw_url.to_string();
    };

    let has_sensitive_query = url.query_pairs().any(|(k, _)| {
        let lower = k.to_ascii_lowercase();
        lower.contains("token")
            || lower.contains("sig")
            || lower.contains("auth")
            || lower.contains("key")
            || lower.contains("secret")
            || lower.contains("cred")
            || lower.contains("expires")
    });

    if has_sensitive_query {
        let mut clean = url.clone();
        clean.set_query(None);
        clean.to_string()
    } else {
        url.to_string()
    }
}

/// Builds a [`CacheKey`] for one avatar.
/// The key identity is the avatar owner's stable subject plus sanitized URL.
pub fn avatar_cache_key<K: AvatarKeyLike>(
    key: &K,
) -> Result<CacheKey, crate::storage::cache::CacheError> {
    let clean_url = sanitize_avatar_url(key.url());
    // Request identity format: "subject={subject};url={clean_url}"
    let request_id = format!("subject={};url={clean_url}", key.subject());
    // Request string must not contain control characters and length bounded
    CacheKey::mutable(
        None, // avatar is account-level, not tied to a single repository numeric ID
        true, // private to the viewing account's CacheScope
        AVATAR_RESOURCE_KIND,
        &request_id,
        AVATAR_REPRESENTATION_NAME,
        AVATAR_REPRESENTATION_VERSION,
    )
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
