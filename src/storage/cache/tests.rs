use super::*;
use crate::domain::{HostId, cache::CacheCompleteness};
use tempfile::TempDir;

include!("additional_tests.rs");

fn config(dir: &TempDir) -> CacheConfig {
    CacheConfig::at(dir.path().join("business"))
}
fn account(id: u64) -> AccountKey {
    AccountKey::new(id, HostId::github())
}
fn key(repo: u64, name: &str) -> CacheKey {
    CacheKey::mutable(Some(repo), true, name, "query-v1", "json-api-2022", 1).unwrap()
}
fn metadata() -> CacheMetadata {
    CacheMetadata {
        representation_version: 1,
        fetched_at_ms: 100,
        verified_at_ms: 200,
        completeness: CacheCompleteness::Complete,
        etag: Some("etag".into()),
        last_modified: None,
        cache_control: None,
    }
}
fn put(store: &CacheStore, scope: &CacheScope, key: &CacheKey, value: &[u8]) {
    store
        .put(store.begin_write(scope, key).unwrap(), value, metadata())
        .unwrap();
}
fn hit(store: &CacheStore, scope: &CacheScope, key: &CacheKey) -> CacheLease {
    match store.read(scope, key).unwrap() {
        CacheRead::Hit(value) => value,
        CacheRead::Miss(reason) => panic!("miss: {reason:?}"),
    }
}
fn value(store: &CacheStore, scope: &CacheScope, key: &CacheKey) -> Vec<u8> {
    hit(store, scope, key).snapshot().unwrap().unwrap().bytes
}
fn miss(store: &CacheStore, scope: &CacheScope, key: &CacheKey, reason: CacheMiss) {
    assert!(matches!(store.read(scope,key).unwrap(), CacheRead::Miss(actual) if actual==reason));
}
fn revalidate(store: &CacheStore, scope: &CacheScope, repository: u64) {
    store
        .permission_confirmed(store.begin_revalidation(scope, Some(repository)).unwrap())
        .unwrap();
}
fn denied(store: &CacheStore, scope: &CacheScope, key: &CacheKey) {
    assert!(matches!(
        store.read(scope, key),
        Err(CacheError::AccessDenied)
    ));
    assert!(matches!(
        store.begin_write(scope, key),
        Err(CacheError::AccessDenied)
    ));
}

#[test]
fn reopen_isolates_hosts_accounts_repositories_representations_and_fixed_objects() {
    let dir = TempDir::new().unwrap();
    let cfg = config(&dir);
    let store = CacheStore::open(cfg.clone()).unwrap();
    let a = store.saved_account(account(u64::MAX)).unwrap();
    let b = store.saved_account(account(2)).unwrap();
    let c = store
        .saved_account(AccountKey::new(u64::MAX, HostId::new("enterprise.example")))
        .unwrap();
    let fixed = CacheKey::fixed(9, true, ObjectKind::Blob, &"a".repeat(40), "raw-v1", 1).unwrap();
    put(&store, &a, &fixed, b"private A");
    put(&store, &b, &fixed, b"private B");
    drop(store);
    let store = CacheStore::open(cfg).unwrap();
    assert_eq!(store.saved_account(account(u64::MAX)).unwrap(), a);
    assert_eq!(value(&store, &a, &fixed), b"private A");
    assert_eq!(value(&store, &b, &fixed), b"private B");
    miss(&store, &c, &fixed, CacheMiss::NotCached);
    let mutable = CacheKey::mutable(Some(9), true, "blob", &"a".repeat(40), "raw-v1", 1).unwrap();
    miss(&store, &a, &mutable, CacheMiss::NotCached);
    for k in [
        CacheKey::fixed(10, true, ObjectKind::Blob, &"a".repeat(40), "raw-v1", 1).unwrap(),
        CacheKey::fixed(9, true, ObjectKind::Tree, &"a".repeat(40), "raw-v1", 1).unwrap(),
        CacheKey::fixed(9, true, ObjectKind::Blob, &"a".repeat(40), "raw-v2", 1).unwrap(),
    ] {
        miss(&store, &a, &k, CacheMiss::NotCached);
    }
}

#[test]
fn replacement_quarantines_across_restart_and_only_repository_revalidation_unhides() {
    let dir = TempDir::new().unwrap();
    let cfg = config(&dir);
    let store = CacheStore::open(cfg.clone()).unwrap();
    let scope = store.saved_account(account(1)).unwrap();
    let k = key(1, "details");
    let other = key(2, "details");
    put(&store, &scope, &k, b"one");
    put(&store, &scope, &other, b"two");
    let stale = store.begin_write(&scope, &k).unwrap();
    let stale_permission = store.begin_revalidation(&scope, Some(1)).unwrap();
    let lease = hit(&store, &scope, &k);
    store
        .access_event(&scope, AccessEvent::PatReplaced)
        .unwrap();
    assert!(matches!(lease.snapshot(), Err(CacheError::AccessDenied)));
    assert!(matches!(
        store.put(stale, b"old", metadata()),
        Err(CacheError::Stale)
    ));
    assert!(matches!(
        store.permission_confirmed(stale_permission),
        Err(CacheError::Stale)
    ));
    drop(lease);
    drop(store);
    let store = CacheStore::open(cfg).unwrap();
    let restored = store.saved_account(account(1)).unwrap();
    assert_eq!(restored, scope);
    denied(&store, &scope, &k);
    denied(&store, &scope, &other);
    revalidate(&store, &scope, 1);
    assert_eq!(value(&store, &scope, &k), b"one");
    denied(&store, &scope, &other);
    // A never-before-seen repository also requires new permission evidence.
    denied(&store, &scope, &key(3, "details"));
    revalidate(&store, &scope, 3);
    put(&store, &scope, &key(3, "details"), b"new");
}

#[test]
fn rejected_credentials_stay_hidden_and_transient_failures_do_not_revoke() {
    let dir = TempDir::new().unwrap();
    let cfg = config(&dir);
    let store = CacheStore::open(cfg.clone()).unwrap();
    let scope = store.saved_account(account(1)).unwrap();
    let k = key(1, "details");
    put(&store, &scope, &k, b"cached");
    for event in [
        AccessEvent::Offline,
        AccessEvent::Timeout,
        AccessEvent::ServerFailure,
        AccessEvent::RateLimited,
        AccessEvent::AmbiguousDenial,
    ] {
        store.access_event(&scope, event).unwrap();
        assert_eq!(value(&store, &scope, &k), b"cached");
    }
    store
        .access_event(&scope, AccessEvent::CredentialRejected)
        .unwrap();
    drop(store);
    let store = CacheStore::open(cfg).unwrap();
    denied(&store, &scope, &k);
    assert!(matches!(
        store.begin_revalidation(&scope, Some(1)),
        Err(CacheError::AccessDenied)
    ));
    store.clear().unwrap();
    store.recover().unwrap();
    drop(store);
    let store = CacheStore::open(config(&dir)).unwrap();
    denied(&store, &scope, &k);
    // Accepting replacement credentials does not itself restore repository access.
    store
        .access_event(&scope, AccessEvent::PatReplaced)
        .unwrap();
    denied(&store, &scope, &k);
    revalidate(&store, &scope, 1);
    miss(&store, &scope, &k, CacheMiss::NotCached);
}

#[test]
fn retirement_readd_and_cleanup_are_crash_recoverable_and_do_not_touch_other_accounts() {
    let dir = TempDir::new().unwrap();
    let mut cfg = config(&dir);
    cfg.inline_bytes = 0;
    let store = CacheStore::open(cfg.clone()).unwrap();
    let a = store.saved_account(account(1)).unwrap();
    let b = store.saved_account(account(2)).unwrap();
    let k = key(1, "blob");
    put(&store, &a, &k, b"a");
    put(&store, &b, &k, b"b");
    let old = store.begin_write(&a, &k).unwrap();
    store.retire(&a).unwrap();
    denied(&store, &a, &k);
    let fresh = store.saved_account(account(1)).unwrap();
    assert_ne!(fresh.partition(), a.partition());
    miss(&store, &fresh, &k, CacheMiss::NotCached);
    assert!(matches!(
        store.put(old, b"resurrect", metadata()),
        Err(CacheError::AccessDenied)
    ));
    drop(store);
    let store = CacheStore::open(cfg).unwrap();
    denied(&store, &a, &k);
    assert_eq!(value(&store, &b, &k), b"b");
    assert_eq!(store.usage().unwrap(), (1, 1));
    assert_eq!(
        std::fs::read_dir(store.inner.config.root.join("objects"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn explicit_revocation_clears_one_repository_and_invalidates_old_permission_tickets() {
    let dir = TempDir::new().unwrap();
    let store = CacheStore::open(config(&dir)).unwrap();
    let scope = store.saved_account(account(1)).unwrap();
    let k = key(1, "details");
    let other = key(2, "details");
    put(&store, &scope, &k, b"one");
    put(&store, &scope, &other, b"two");
    let permission = store.begin_revalidation(&scope, Some(1)).unwrap();
    let writer = store.begin_write(&scope, &k).unwrap();
    store.revoke_repository(&scope, Some(1)).unwrap();
    denied(&store, &scope, &k);
    assert!(matches!(
        store.permission_confirmed(permission),
        Err(CacheError::Stale)
    ));
    assert!(matches!(
        store.put(writer, b"stale", metadata()),
        Err(CacheError::Stale)
    ));
    revalidate(&store, &scope, 1);
    miss(&store, &scope, &k, CacheMiss::NotCached);
    assert_eq!(value(&store, &scope, &other), b"two");
}

#[test]
fn no_store_invalidates_previous_bytes_and_empty_partial_and_miss_remain_distinct() {
    let dir = TempDir::new().unwrap();
    let cfg = config(&dir);
    let store = CacheStore::open(cfg.clone()).unwrap();
    let scope = store.saved_account(account(1)).unwrap();
    let k = key(1, "list");
    miss(&store, &scope, &k, CacheMiss::NotCached);
    put(&store, &scope, &k, b"");
    assert_eq!(
        hit(&store, &scope, &k)
            .snapshot()
            .unwrap()
            .unwrap()
            .metadata
            .completeness,
        CacheCompleteness::Complete
    );
    let mut partial = metadata();
    partial.completeness = CacheCompleteness::Partial {
        reason: "page two unavailable".into(),
    };
    store
        .put(store.begin_write(&scope, &k).unwrap(), b"", partial.clone())
        .unwrap();
    assert_eq!(
        hit(&store, &scope, &k)
            .snapshot()
            .unwrap()
            .unwrap()
            .metadata,
        partial
    );
    let mut no_store = metadata();
    no_store.cache_control = Some("private, No-Store, max-age=60".into());
    assert_eq!(
        store
            .put(store.begin_write(&scope, &k).unwrap(), b"secret", no_store)
            .unwrap(),
        WriteOutcome::NotStored
    );
    drop(store);
    let store = CacheStore::open(cfg).unwrap();
    miss(&store, &scope, &k, CacheMiss::NotCached);
    let mut allowed = metadata();
    allowed.cache_control = Some("private, no-cache".into());
    store
        .put(store.begin_write(&scope, &k).unwrap(), b"valid", allowed)
        .unwrap();
    assert_eq!(value(&store, &scope, &k), b"valid");
}

#[test]
fn truly_overlapping_writers_cannot_publish_an_older_response() {
    let dir = TempDir::new().unwrap();
    let store = CacheStore::open(config(&dir)).unwrap();
    let scope = store.saved_account(account(1)).unwrap();
    let k = key(1, "branch-main");
    let old = store.begin_write(&scope, &k).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let worker = store.clone();
    let rendezvous = barrier.clone();
    let older = std::thread::spawn(move || {
        rendezvous.wait();
        worker.put(old, b"old", metadata())
    });
    let new = store.begin_write(&scope, &k).unwrap();
    barrier.wait();
    store.put(new, b"new", metadata()).unwrap();
    assert!(matches!(older.join().unwrap(), Err(CacheError::Stale)));
    assert_eq!(value(&store, &scope, &k), b"new");
}

#[test]
fn active_reads_protect_eviction_but_never_bypass_gates_and_capacity_failure_rolls_back() {
    let dir = TempDir::new().unwrap();
    let mut cfg = config(&dir);
    cfg.capacity_bytes = 8;
    cfg.max_entry_bytes = 8;
    cfg.inline_bytes = 0;
    let store = CacheStore::open(cfg).unwrap();
    let scope = store.saved_account(account(1)).unwrap();
    let a = key(1, "a");
    let b = key(1, "b");
    put(&store, &scope, &a, b"12345678");
    let lease = hit(&store, &scope, &a);
    assert!(matches!(
        store.put(
            store.begin_write(&scope, &b).unwrap(),
            b"87654321",
            metadata()
        ),
        Err(CacheError::Capacity)
    ));
    assert_eq!(store.usage().unwrap(), (8, 1));
    assert_eq!(lease.snapshot().unwrap().unwrap().bytes, b"12345678");
    store.recover().unwrap();
    assert_eq!(
        std::fs::read_dir(store.inner.config.root.join("objects"))
            .unwrap()
            .count(),
        1
    );
    drop(lease);
    put(&store, &scope, &b, b"87654321");
    miss(&store, &scope, &a, CacheMiss::NotCached);
    let lease = hit(&store, &scope, &b);
    store
        .access_event(&scope, AccessEvent::PatReplaced)
        .unwrap();
    assert!(matches!(lease.snapshot(), Err(CacheError::AccessDenied)));
}

#[test]
fn malformed_missing_and_hostile_objects_are_misses_without_following_paths() {
    for corruption in 0..5 {
        let dir = TempDir::new().unwrap();
        let mut cfg = config(&dir);
        cfg.inline_bytes = 0;
        let store = CacheStore::open(cfg.clone()).unwrap();
        let scope = store.saved_account(account(1)).unwrap();
        let k = key(1, "blob");
        let sibling = key(1, "sibling");
        put(&store, &scope, &k, b"original");
        put(&store, &scope, &sibling, b"sibling");
        let path = {
            let state = store.lock().unwrap();
            let name: String = state
                .db
                .query_row(
                    "SELECT object FROM entries WHERE key=?1",
                    [k.encoded().unwrap()],
                    |r| r.get(0),
                )
                .unwrap();
            cfg.root.join("objects").join(name)
        };
        match corruption {
            0 => std::fs::write(path, b"modified").unwrap(),
            1 => std::fs::remove_file(path).unwrap(),
            2 => {
                store
                    .lock()
                    .unwrap()
                    .db
                    .execute(
                        "UPDATE entries SET object='../../outside' WHERE key=?1",
                        [k.encoded().unwrap()],
                    )
                    .unwrap();
            }
            3 => {
                store
                    .lock()
                    .unwrap()
                    .db
                    .execute(
                        "UPDATE entries SET metadata='{}' WHERE key=?1",
                        [k.encoded().unwrap()],
                    )
                    .unwrap();
            }
            _ => {
                store
                    .lock()
                    .unwrap()
                    .db
                    .execute(
                        "UPDATE entries SET bytes=999999999 WHERE key=?1",
                        [k.encoded().unwrap()],
                    )
                    .unwrap();
            }
        }
        drop(store);
        let store = CacheStore::open(cfg).unwrap();
        miss(
            &store,
            &scope,
            &k,
            if corruption == 4 {
                CacheMiss::NotCached
            } else {
                CacheMiss::Corrupt
            },
        );
        assert_eq!(value(&store, &scope, &sibling), b"sibling");
    }
}

#[test]
fn transaction_failure_keeps_the_prior_metadata_and_recovers_published_orphan() {
    let dir = TempDir::new().unwrap();
    let mut cfg = config(&dir);
    cfg.inline_bytes = 0;
    let store = CacheStore::open(cfg.clone()).unwrap();
    let scope = store.saved_account(account(1)).unwrap();
    let k = key(1, "blob");
    put(&store, &scope, &k, b"old");
    let ticket = store.begin_write(&scope, &k).unwrap();
    store.lock().unwrap().db.execute_batch("CREATE TRIGGER fail_update BEFORE UPDATE ON entries BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(matches!(
        store.put(ticket, b"new", metadata()),
        Err(CacheError::Database)
    ));
    store
        .lock()
        .unwrap()
        .db
        .execute_batch("DROP TRIGGER fail_update")
        .unwrap();
    drop(store);
    let store = CacheStore::open(cfg).unwrap();
    assert_eq!(value(&store, &scope, &k), b"old");
    assert_eq!(
        std::fs::read_dir(store.inner.config.root.join("objects"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn crash_during_sql_transaction_recovers_old_value_and_collects_unindexed_file() {
    const VARIABLE: &str = "GITHUX_CACHE_CRASH_TEST_ROOT";
    if let Some(root) = std::env::var_os(VARIABLE) {
        let store = CacheStore::open(CacheConfig::at(root.into())).unwrap();
        let mut state = store.lock().unwrap();
        let tx = state.db.transaction().unwrap();
        tx.execute("UPDATE entries SET metadata='uncommitted'", [])
            .unwrap();
        objects::publish(&store.inner.config.root, &nonce(), b"unindexed").unwrap();
        std::process::exit(23);
    }
    let dir = TempDir::new().unwrap();
    let cfg = config(&dir);
    let store = CacheStore::open(cfg.clone()).unwrap();
    let scope = store.saved_account(account(1)).unwrap();
    let k = key(1, "blob");
    put(&store, &scope, &k, b"committed");
    drop(store);
    let status=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","storage::cache::tests::crash_during_sql_transaction_recovers_old_value_and_collects_unindexed_file","--nocapture"])
        .env(VARIABLE,&cfg.root).status().unwrap();
    assert_eq!(status.code(), Some(23));
    let store = CacheStore::open(cfg).unwrap();
    assert_eq!(value(&store, &scope, &k), b"committed");
    assert_eq!(
        std::fs::read_dir(store.inner.config.root.join("objects"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn clear_preserves_real_account_database_settings_and_quarantine_ledger() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("accounts.sqlite3");
    // A separate SQLite user-data file, with real pages whose bytes must survive.
    let accounts = Connection::open(&path).unwrap();
    accounts
        .execute_batch("CREATE TABLE accounts(id TEXT);INSERT INTO accounts VALUES('1');")
        .unwrap();
    drop(accounts);
    std::fs::write(dir.path().join("settings.json"), b"user settings").unwrap();
    let before = std::fs::read(&path).unwrap();
    let store = CacheStore::open(config(&dir)).unwrap();
    let scope = store.saved_account(account(1)).unwrap();
    let k = key(1, "blob");
    put(&store, &scope, &k, b"payload");
    let writer = store.begin_write(&scope, &k).unwrap();
    let lease = hit(&store, &scope, &k);
    store.clear().unwrap();
    assert!(lease.snapshot().unwrap().is_none());
    assert!(matches!(
        store.put(writer, b"old", metadata()),
        Err(CacheError::Stale)
    ));
    store.recover().unwrap();
    drop(lease);
    drop(store);
    assert_eq!(std::fs::read(path).unwrap(), before);
    assert_eq!(
        std::fs::read(dir.path().join("settings.json")).unwrap(),
        b"user settings"
    );
}

#[test]
fn schema_future_unknown_corrupt_and_exclusive_lease_fail_without_rebuilding() {
    let dir = TempDir::new().unwrap();
    let cfg = config(&dir);
    let store = CacheStore::open(cfg.clone()).unwrap();
    assert!(matches!(
        CacheStore::open(cfg.clone()),
        Err(CacheError::InUse)
    ));
    drop(store);
    let path = cfg.root.join("cache.sqlite3");
    let db = Connection::open(&path).unwrap();
    db.pragma_update(None, "user_version", 2).unwrap();
    drop(db);
    assert!(matches!(
        CacheStore::open(cfg.clone()),
        Err(CacheError::FutureSchema(2))
    ));
    let db = Connection::open(&path).unwrap();
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    drop(db);
    let other = TempDir::new().unwrap();
    let cfg = config(&other);
    std::fs::create_dir_all(&cfg.root).unwrap();
    let db = Connection::open(cfg.root.join("cache.sqlite3")).unwrap();
    db.execute_batch("CREATE TABLE unknown(value)").unwrap();
    drop(db);
    assert!(matches!(CacheStore::open(cfg), Err(CacheError::Corrupt)));
}

#[test]
fn crc_known_vector_and_size_validation() {
    assert_eq!(objects::checksum(b"123456789"), "6c40df5f0b497347");
    assert!(CacheKey::fixed(1, true, ObjectKind::Blob, "../secret", "raw", 1).is_err());
    let dir = TempDir::new().unwrap();
    let mut cfg = config(&dir);
    cfg.capacity_bytes = 8;
    cfg.max_entry_bytes = 8;
    cfg.inline_bytes = 8;
    let store = CacheStore::open(cfg).unwrap();
    let scope = store.saved_account(account(1)).unwrap();
    let k = key(1, "blob");
    assert!(matches!(
        store.put(
            store.begin_write(&scope, &k).unwrap(),
            b"123456789",
            metadata()
        ),
        Err(CacheError::TooLarge)
    ));
    assert_eq!(store.usage().unwrap(), (0, 0));
}

#[test]
fn measure_size_thresholds_and_bounded_eviction() {
    // Deterministic synthetic workload, not a promise of real-world hit rate.
    for threshold in [64 * 1024, 128 * 1024, 256 * 1024] {
        let dir = TempDir::new().unwrap();
        let mut cfg = config(&dir);
        cfg.capacity_bytes = 4 * 1024 * 1024;
        cfg.max_entry_bytes = 512 * 1024;
        cfg.inline_bytes = threshold;
        let store = CacheStore::open(cfg.clone()).unwrap();
        let scope = store.saved_account(account(1)).unwrap();
        let started = std::time::Instant::now();
        for i in 0..64 {
            let size = [4 * 1024, 64 * 1024, 128 * 1024, 512 * 1024][i % 4];
            put(
                &store,
                &scope,
                &key(1, &format!("blob-{i}")),
                &vec![(i % 251) as u8; size],
            );
        }
        store.recover().unwrap();
        let elapsed = started.elapsed();
        let (logical, count) = store.usage().unwrap();
        assert!(logical <= cfg.capacity_bytes);
        assert!(count < 64);
        let files = std::fs::read_dir(cfg.root.join("objects"))
            .unwrap()
            .map(|v| v.unwrap().metadata().unwrap().len())
            .sum::<u64>();
        let db = std::fs::metadata(cfg.root.join("cache.sqlite3"))
            .unwrap()
            .len();
        eprintln!(
            "cache measurement inline={threshold} logical={logical} entries={count} files={files} db={db} elapsed_ms={}",
            elapsed.as_millis()
        );
        assert!(files <= cfg.capacity_bytes);
        assert!(db <= cfg.database_bytes);
    }
}
