//! Locator → numeric repository identity hint.
//!
//! The page is opened with a `RepositoryLocator` (owner/name), while the
//! persistent repository cache is keyed by the numeric repository id. To read
//! cached data after a restart without first contacting GitHub, the page keeps
//! a small account-scoped index of the identities it has already resolved.
//!
//! This index is **not** an authorization proof:
//!
//! * it is stored under the account aggregate (`repository = None`, private)
//!   cache key, so it obeys the same C1–C5 gate and is cleared when the account
//!   aggregate is invalidated (for example by an explicit repository revocation);
//! * a missing/invalid/denied entry simply means "resolve online";
//! * every cached repository value read through it is still re-verified against
//!   the numeric repository id by the storage DTO envelope, and the repository
//!   gate is still checked per repository;
//! * only an exact (case-insensitive) owner/name match is reused, so a renamed
//!   or transferred repository does not inherit the old id.

use crate::domain::repository::{RepositoryIdentity, RepositoryLocator};
use crate::storage::cache::{CacheKey, CacheRead, CacheScope, CacheStore};

/// Bump when the stored representation changes shape.
const VERSION: u32 = 1;
const REPRESENTATION: &str = "githux-repository-locator-v1";

fn index_key(locator: &RepositoryLocator) -> Result<CacheKey, ()> {
    let request = format!(
        "{};{}",
        locator.owner().to_ascii_lowercase(),
        locator.name().to_ascii_lowercase()
    );
    CacheKey::mutable(
        None,
        true,
        "repository-locator-index",
        &request,
        REPRESENTATION,
        VERSION,
    )
    .map_err(|_| ())
}

fn encode(identity: &RepositoryIdentity) -> Vec<u8> {
    let value = serde_json::json!({
        "version": VERSION,
        "id": identity.id(),
        "owner": identity.locator().owner(),
        "name": identity.locator().name(),
    });
    serde_json::to_vec(&value).unwrap_or_default()
}

fn decode(bytes: &[u8], locator: &RepositoryLocator) -> Option<RepositoryIdentity> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    if value.get("version").and_then(serde_json::Value::as_u64) != Some(u64::from(VERSION)) {
        return None;
    }
    let id = value.get("id").and_then(serde_json::Value::as_u64)?;
    let owner = value.get("owner").and_then(serde_json::Value::as_str)?;
    let name = value.get("name").and_then(serde_json::Value::as_str)?;
    // Only the exact locator that was cached may reuse this id.
    if !owner.eq_ignore_ascii_case(locator.owner()) || !name.eq_ignore_ascii_case(locator.name()) {
        return None;
    }
    Some(RepositoryIdentity {
        id,
        locator: RepositoryLocator::new(owner, name).ok()?,
    })
}

/// Reads the previously resolved identity for `locator`.
///
/// Blocking storage I/O: call from a blocking worker. Any cache error (including
/// `AccessDenied` after a credential change) is reported as "unknown", which
/// keeps the caller on the ordinary online path.
pub fn read(
    store: &CacheStore,
    scope: &CacheScope,
    locator: &RepositoryLocator,
) -> Option<RepositoryIdentity> {
    let key = index_key(locator).ok()?;
    match store.read(scope, &key).ok()? {
        CacheRead::Hit(lease) => {
            let snapshot = lease.snapshot().ok()??;
            decode(&snapshot.bytes, locator)
        }
        CacheRead::Miss(_) => None,
    }
}

/// Remembers that `identity` was authoritatively resolved for its locator.
///
/// Failures are ignored: the index is an optimization, and a missing entry only
/// means the next activation resolves online again.
pub fn write(store: &CacheStore, scope: &CacheScope, identity: &RepositoryIdentity) {
    let Ok(key) = index_key(identity.locator()) else {
        return;
    };
    let Ok(ticket) = store.begin_write(scope, &key) else {
        return;
    };
    let metadata = crate::domain::cache::CacheMetadata {
        representation_version: VERSION,
        fetched_at_ms: crate::data::reader_image_cache::now_ms(),
        verified_at_ms: crate::data::reader_image_cache::now_ms(),
        completeness: crate::domain::cache::CacheCompleteness::Complete,
        etag: None,
        last_modified: None,
        cache_control: None,
    };
    // A denied/failed write (retired partition, revoked aggregate, full cache)
    // is not a read error: the page already has its online value.
    let _ = store.put(ticket, &encode(identity), metadata);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::cache::CacheConfig;

    fn key() -> crate::domain::AccountKey {
        crate::domain::AccountKey::new(7, crate::domain::HostId::github())
    }

    #[test]
    fn exact_locator_round_trips_and_other_spellings_do_not() {
        let temporary = tempfile::tempdir().unwrap();
        let store = CacheStore::open(CacheConfig::at(temporary.path().join("cache"))).unwrap();
        let scope = store.saved_account(key()).unwrap();
        let identity = RepositoryIdentity {
            id: 42,
            locator: RepositoryLocator::new("Octo", "Ledger").unwrap(),
        };
        assert!(read(&store, &scope, identity.locator()).is_none());
        write(&store, &scope, &identity);
        let found = read(
            &store,
            &scope,
            &RepositoryLocator::new("octo", "ledger").unwrap(),
        )
        .expect("the same locator is remembered");
        assert_eq!(found.id, 42);
        assert!(
            read(
                &store,
                &scope,
                &RepositoryLocator::new("octo", "other").unwrap()
            )
            .is_none(),
            "a different repository name must not reuse the id"
        );
    }

    #[test]
    fn a_corrupt_or_versioned_entry_is_ignored() {
        let temporary = tempfile::tempdir().unwrap();
        let store = CacheStore::open(CacheConfig::at(temporary.path().join("cache"))).unwrap();
        let scope = store.saved_account(key()).unwrap();
        let locator = RepositoryLocator::new("octo", "ledger").unwrap();
        let key = index_key(&locator).unwrap();
        let ticket = store.begin_write(&scope, &key).unwrap();
        store
            .put(
                ticket,
                b"{not json",
                crate::domain::cache::CacheMetadata {
                    representation_version: VERSION,
                    fetched_at_ms: 1,
                    verified_at_ms: 1,
                    completeness: crate::domain::cache::CacheCompleteness::Complete,
                    etag: None,
                    last_modified: None,
                    cache_control: None,
                },
            )
            .unwrap();
        assert!(read(&store, &scope, &locator).is_none());
    }
}
