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
