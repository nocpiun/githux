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

/// A valid PAT replacement discards the previous credential's account-aggregate snapshots and
/// starts a fresh aggregate generation. Re-confirming the aggregate for the new credential can then
/// save and read new snapshots, while the old per-repository bytes stay behind their own proof.
#[test]
fn a_pat_replacement_purges_old_aggregates_and_opens_a_new_generation() {
    let dir = TempDir::new().unwrap();
    let store = CacheStore::open(config(&dir)).unwrap();
    let scope = store.saved_account(account(1)).unwrap();
    let aggregate = CacheKey::mutable(None, true, "account-feed", "page=1", "json-v1", 1).unwrap();
    let repo = key(10, "details");
    put(&store, &scope, &aggregate, b"old aggregate");
    put(&store, &scope, &repo, b"old repo");
    let aggregate_lease = hit(&store, &scope, &aggregate);
    let repository_lease = hit(&store, &scope, &repo);
    let old_aggregate = store.authorization(&scope, None).unwrap();
    let old_repo = store.authorization(&scope, Some(10)).unwrap();

    store
        .access_event(&scope, AccessEvent::PatReplaced)
        .unwrap();

    // The pre-publication quarantine still denies reads, including the old lease.
    denied(&store, &scope, &aggregate);
    assert!(aggregate_lease.snapshot().is_err());
    // Only after the catalog publishes new credentials does the lifecycle reset the aggregate.
    store.start_fresh_aggregate(&scope).unwrap();
    miss(&store, &scope, &aggregate, CacheMiss::NotCached);
    assert!(aggregate_lease.snapshot().is_err());
    // Captured UI authorizations are invalidated for the whole replaced credential, not only the
    // resource that happened to be displayed.
    assert!(!store.is_authorization_current(&old_aggregate));
    assert!(!store.is_authorization_current(&old_repo));
    // Per-repository bytes stay quarantined until their own proof.
    denied(&store, &scope, &repo);

    // A new-credential online read proves the aggregate and can publish a fresh snapshot.
    store
        .permission_confirmed(store.begin_revalidation(&scope, None).unwrap())
        .unwrap();
    put(&store, &scope, &aggregate, b"new aggregate");
    assert_eq!(value(&store, &scope, &aggregate), b"new aggregate");

    // The aggregate proof does not unlock the repository; its own proof does.
    denied(&store, &scope, &repo);
    store
        .permission_confirmed(store.begin_revalidation(&scope, Some(10)).unwrap())
        .unwrap();
    assert_eq!(value(&store, &scope, &repo), b"old repo");
    assert!(repository_lease.snapshot().is_err(), "new repository proof cannot revive an old credential's lease");
}

/// The lightweight authorization generation follows only the affected partition/resource: a
/// repository revocation invalidates its own bytes and the account aggregate, never an unrevoked
/// sibling repository or another account.
#[test]
fn authorization_generations_follow_revocation_scope_only() {
    let dir = TempDir::new().unwrap();
    let store = CacheStore::open(config(&dir)).unwrap();
    let scope = store.saved_account(account(1)).unwrap();
    let sibling = store.saved_account(account(2)).unwrap();
    let aggregate = store.authorization(&scope, None).unwrap();
    let revoked = store.authorization(&scope, Some(10)).unwrap();
    let sibling_repo = store.authorization(&scope, Some(20)).unwrap();
    let other_account = store.authorization(&sibling, None).unwrap();

    store.revoke_repository(&scope, Some(10)).unwrap();

    assert!(!store.is_authorization_current(&aggregate));
    assert!(!store.is_authorization_current(&revoked));
    assert!(store.is_authorization_current(&sibling_repo));
    assert!(store.is_authorization_current(&other_account));
}

/// A classified credential rejection keeps its durable barrier: no later aggregate proof (or a
/// restart) may turn it back into access.
#[test]
fn credential_rejection_is_not_recovered_by_a_new_proof() {
    let dir = TempDir::new().unwrap();
    let cfg = config(&dir);
    let store = CacheStore::open(cfg.clone()).unwrap();
    let scope = store.saved_account(account(1)).unwrap();
    store
        .access_event(&scope, AccessEvent::CredentialRejected)
        .unwrap();
    assert!(store.begin_revalidation(&scope, None).is_err());
    drop(store);
    let store = CacheStore::open(cfg).unwrap();
    let scope = store.saved_account(account(1)).unwrap();
    assert!(store.begin_revalidation(&scope, None).is_err());
}

#[test]
fn authorization_checks_do_not_wait_for_sqlite_and_old_tokens_never_revive() {
    let dir = TempDir::new().unwrap();
    let store = CacheStore::open(config(&dir)).unwrap();
    let scope = store.saved_account(account(1)).unwrap();
    let authorization = store.authorization(&scope, None).unwrap();
    // Hold the exact mutex every SQLite/object operation uses. Capturing/validating tokens must
    // still complete on an independent thread, without waiting for the database lock.
    let state = store.lock().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let copy = store.clone();
    let s = scope.clone();
    let a = authorization.clone();
    let job = std::thread::spawn(move || {
        tx.send(a.is_current() && copy.authorization(&s, None).unwrap().is_current()).unwrap();
    });
    let result = rx.recv_timeout(Duration::from_secs(2));
    drop(state);
    job.join().unwrap();
    assert!(result.unwrap());
    let aggregate = CacheKey::mutable(None, true, "feed", "page=1", "json", 1).unwrap();
    let old_write = store.begin_write(&scope, &aggregate).unwrap();
    let old_proof = store.begin_revalidation(&scope, None).unwrap();
    store.revoke_repository(&scope, Some(10)).unwrap();
    store.permission_confirmed(store.begin_revalidation(&scope, None).unwrap()).unwrap();
    put(&store, &scope, &aggregate, b"new current data");
    assert!(store.authorization(&scope, None).unwrap().is_current());
    assert!(!authorization.is_current(), "a queued old reply cannot borrow the new gate");
    assert!(matches!(store.put(old_write, b"late", metadata()), Err(CacheError::Stale)));
    assert!(matches!(store.permission_confirmed(old_proof), Err(CacheError::Stale)));
}
