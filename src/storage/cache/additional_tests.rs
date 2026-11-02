// Included into the existing cache tests module; no duplicate Cargo test target.
#[test]
fn invalidation_and_reopen_reject_old_tickets_without_changing_siblings() {
    let dir = TempDir::new().unwrap();
    let cfg = config(&dir);
    let store = CacheStore::open(cfg.clone()).unwrap();
    let scope = store.saved_account(account(1)).unwrap();
    let k = key(1, "branch");
    let other = key(1, "blob");
    put(&store, &scope, &k, b"A");
    put(&store, &scope, &other, b"fixed");
    let old = store.begin_write(&scope, &k).unwrap();
    store.invalidate(&scope, &k).unwrap();
    assert!(matches!(store.put(old, b"A", metadata()), Err(CacheError::Stale)));
    miss(&store, &scope, &k, CacheMiss::NotCached);
    assert_eq!(value(&store, &scope, &other), b"fixed");
    let old = store.begin_write(&scope, &other).unwrap();
    drop(store);
    let store = CacheStore::open(cfg).unwrap();
    assert!(matches!(store.put(old, b"old process", metadata()), Err(CacheError::Stale)));
}

#[test]
fn object_directory_symlink_and_entry_symlink_never_read_external_bytes() {
    // Windows symlinks can require developer mode; Unix CI verifies the same
    // symlink_metadata path checks without requiring elevated privileges.
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let dir = TempDir::new().unwrap();
        let mut cfg = config(&dir);
        cfg.inline_bytes = 0;
        let store = CacheStore::open(cfg.clone()).unwrap();
        let scope = store.saved_account(account(1)).unwrap();
        let k = key(1, "blob");
        put(&store, &scope, &k, b"external");
        let outside = dir.path().join("outside");
        std::fs::write(&outside, b"external").unwrap();
        let name: String = store.lock().unwrap().db.query_row("SELECT object FROM entries", [], |r| r.get(0)).unwrap();
        let path = cfg.root.join("objects").join(&name);
        std::fs::remove_file(&path).unwrap();
        symlink(&outside, &path).unwrap();
        miss(&store, &scope, &k, CacheMiss::Corrupt);
        store.recover().unwrap();
        assert_eq!(std::fs::read(&outside).unwrap(), b"external");
        drop(store);
        std::fs::remove_dir(cfg.root.join("objects")).unwrap();
        symlink(dir.path(), cfg.root.join("objects")).unwrap();
        assert!(matches!(CacheStore::open(cfg), Err(CacheError::Corrupt)));
    }
}

#[test]
fn repository_revocation_invalidates_account_aggregate_without_blocking_fresh_reads() {
    let dir = TempDir::new().unwrap();
    let cfg = config(&dir);
    let store = CacheStore::open(cfg.clone()).unwrap();
    let scope = store.saved_account(account(1)).unwrap();
    let sibling = store.saved_account(account(2)).unwrap();
    let r = key(1, "details");
    let s = key(2, "details");
    let aggregate = CacheKey::mutable(None, true, "account-feed", "page=1", "json-v1", 1).unwrap();

    put(&store, &scope, &r, b"R");
    put(&store, &scope, &s, b"S");
    put(&store, &scope, &aggregate, b"complete");
    let mut partial = metadata();
    partial.completeness = CacheCompleteness::Partial {
        reason: "page two unavailable".into(),
    };
    store
        .put(
            store.begin_write(&scope, &aggregate).unwrap(),
            b"partial",
            partial,
        )
        .unwrap();
    put(&store, &sibling, &aggregate, b"other account");

    let aggregate_lease = hit(&store, &scope, &aggregate);
    let stale_write = store.begin_write(&scope, &aggregate).unwrap();
    let stale_proof = store.begin_revalidation(&scope, None).unwrap();

    store.revoke_repository(&scope, Some(1)).unwrap();

    denied(&store, &scope, &r);
    assert_eq!(value(&store, &scope, &s), b"S");
    assert_eq!(value(&store, &sibling, &aggregate), b"other account");
    // A held lease can no longer read the old Complete/Partial bytes.
    assert!(matches!(aggregate_lease.snapshot(), Ok(None)));
    // Pre-revocation tickets and proofs cannot repopulate or reopen the gate.
    assert!(matches!(
        store.put(stale_write, b"late", metadata()),
        Err(CacheError::Stale)
    ));
    assert!(matches!(
        store.permission_confirmed(stale_proof),
        Err(CacheError::Stale)
    ));
    drop(aggregate_lease);

    drop(store);
    let store = CacheStore::open(cfg).unwrap();
    miss(&store, &scope, &aggregate, CacheMiss::NotCached);
    // The aggregate gate is still allowed under the pre-existing authorization.
    put(&store, &scope, &aggregate, b"fresh");
    assert_eq!(value(&store, &scope, &aggregate), b"fresh");
    assert_eq!(value(&store, &scope, &s), b"S");
}

#[test]
fn revocation_without_aggregate_entries_still_blocks_inflight_writes_and_proofs() {
    let dir = TempDir::new().unwrap();
    let store = CacheStore::open(config(&dir)).unwrap();
    let scope = store.saved_account(account(1)).unwrap();
    let aggregate = CacheKey::mutable(None, true, "account-feed", "page=1", "json-v1", 1).unwrap();
    let write = store.begin_write(&scope, &aggregate).unwrap();
    let proof = store.begin_revalidation(&scope, None).unwrap();
    store.revoke_repository(&scope, Some(7)).unwrap();
    assert!(matches!(
        store.put(write, b"late", metadata()),
        Err(CacheError::Stale)
    ));
    assert!(matches!(
        store.permission_confirmed(proof),
        Err(CacheError::Stale)
    ));
}

#[test]
fn aggregate_stays_isolated_after_pat_replacement_and_repository_proof_does_not_unlock_it() {
    let dir = TempDir::new().unwrap();
    let cfg = config(&dir);
    let store = CacheStore::open(cfg.clone()).unwrap();
    let scope = store.saved_account(account(1)).unwrap();
    let aggregate = CacheKey::mutable(None, true, "account-feed", "page=1", "json-v1", 1).unwrap();
    put(&store, &scope, &aggregate, b"before");
    store.access_event(&scope, AccessEvent::PatReplaced).unwrap();
    denied(&store, &scope, &aggregate);
    store.revoke_repository(&scope, Some(1)).unwrap();
    denied(&store, &scope, &aggregate);
    // A single repository proof must not unlock the account aggregate (C3).
    revalidate(&store, &scope, 1);
    denied(&store, &scope, &aggregate);
    // Transient access events do not invalidate the aggregate beyond C3 policy.
    for event in [
        AccessEvent::Offline,
        AccessEvent::Timeout,
        AccessEvent::ServerFailure,
        AccessEvent::RateLimited,
        AccessEvent::AmbiguousDenial,
    ] {
        store.access_event(&scope, event).unwrap();
    }
    denied(&store, &scope, &aggregate);
    // Only an explicit aggregate proof (repository=None) reopens it.
    store
        .permission_confirmed(store.begin_revalidation(&scope, None).unwrap())
        .unwrap();
    let mut partial = metadata();
    partial.completeness = CacheCompleteness::Partial {
        reason: "stale window".into(),
    };
    store
        .put(store.begin_write(&scope, &aggregate).unwrap(), b"after", partial)
        .unwrap();
    assert_eq!(value(&store, &scope, &aggregate), b"after");
    drop(store);
    let store = CacheStore::open(cfg).unwrap();
    assert_eq!(value(&store, &scope, &aggregate), b"after");
}
