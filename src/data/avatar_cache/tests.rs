//! Deterministic offline tests for avatar disk cache integration.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tempfile::tempdir;

use crate::app::avatar::{
    AvatarBytes, AvatarDetailedFuture, AvatarFetcher, AvatarKey, AvatarResponseMetadata,
    AvatarSource, MAX_AVATAR_BYTES,
};
use crate::data::avatar_cache::{AvatarDiskCache, avatar_cache_key, sanitize_avatar_url};
use crate::domain::{AccountKey, HostId};
use crate::network::cancel::CancellationToken;
use crate::storage::cache::{CacheConfig, CacheStore};

type MockResponse = (u16, AvatarResponseMetadata, Vec<u8>);
type MockRequest = (String, Option<String>, Option<String>);

struct MockAvatarSource {
    responses: Mutex<HashMap<String, MockResponse>>,
    requests: Mutex<Vec<MockRequest>>,
    request_count: AtomicUsize,
}

impl MockAvatarSource {
    fn new() -> Self {
        Self {
            responses: Mutex::new(HashMap::new()),
            requests: Mutex::new(Vec::new()),
            request_count: AtomicUsize::new(0),
        }
    }

    fn serve(
        &self,
        url: &str,
        status: u16,
        etag: Option<&str>,
        last_modified: Option<&str>,
        cache_control: Option<&str>,
        body: Vec<u8>,
    ) {
        self.responses.lock().unwrap().insert(
            url.to_string(),
            (
                status,
                AvatarResponseMetadata {
                    etag: etag.map(|s| s.to_string()),
                    last_modified: last_modified.map(|s| s.to_string()),
                    cache_control: cache_control.map(|s| s.to_string()),
                },
                body,
            ),
        );
    }
}

impl AvatarSource for MockAvatarSource {
    fn load_detailed<'a>(
        &'a self,
        url: &'a str,
        _max_bytes: usize,
        etag: Option<&'a str>,
        last_modified: Option<&'a str>,
        _cancel: CancellationToken,
    ) -> AvatarDetailedFuture<'a> {
        Box::pin(async move {
            self.request_count.fetch_add(1, Ordering::SeqCst);
            self.requests.lock().unwrap().push((
                url.to_string(),
                etag.map(|s| s.to_string()),
                last_modified.map(|s| s.to_string()),
            ));
            self.responses.lock().unwrap().get(url).cloned().ok_or(())
        })
    }
}

impl AvatarBytes for MockAvatarSource {
    fn load<'a>(
        &'a self,
        url: &'a str,
        max_bytes: usize,
        cancel: CancellationToken,
    ) -> crate::app::avatar::AvatarBytesFuture<'a> {
        Box::pin(async move {
            let (status, _meta, body) = self
                .load_detailed(url, max_bytes, None, None, cancel)
                .await?;
            Ok((status, body))
        })
    }
}

/// A source that simulates a newer writer replacing the cached entry while the
/// request is in flight, then fails the network read. The offline fallback must
/// use the current entry, not the permissive snapshot captured before the call.
struct SupersedingSource {
    cache: AvatarDiskCache,
    key: AvatarKey,
    replacement_body: Vec<u8>,
    replacement_cache_control: Option<String>,
    calls: AtomicUsize,
}

impl AvatarSource for SupersedingSource {
    fn load_detailed<'a>(
        &'a self,
        _url: &'a str,
        _max_bytes: usize,
        _etag: Option<&'a str>,
        _last_modified: Option<&'a str>,
        _cancel: CancellationToken,
    ) -> AvatarDetailedFuture<'a> {
        Box::pin(async move {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                self.cache
                    .put_sync(
                        &self.key,
                        &self.replacement_body,
                        None,
                        None,
                        self.replacement_cache_control.clone(),
                    )
                    .expect("the superseding write is accepted");
            }
            Err(())
        })
    }
}

impl AvatarBytes for SupersedingSource {
    fn load<'a>(
        &'a self,
        url: &'a str,
        max_bytes: usize,
        cancel: CancellationToken,
    ) -> crate::app::avatar::AvatarBytesFuture<'a> {
        Box::pin(async move {
            let (status, _meta, body) = self
                .load_detailed(url, max_bytes, None, None, cancel)
                .await?;
            Ok((status, body))
        })
    }
}

fn valid_png() -> Vec<u8> {
    vec![
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
        0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 96, 96, 96, 248, 15,
        0, 1, 4, 1, 0, 95, 229, 195, 75, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ]
}

#[tokio::test]
async fn avatar_populates_cache_and_reopen_hits_offline() {
    let dir = tempdir().unwrap();
    let config = CacheConfig::at(dir.path().to_path_buf());
    let store = CacheStore::open(config.clone()).unwrap();

    let account = AccountKey::new(100, HostId::github());
    let scope = store.saved_account(account).unwrap();
    let disk_cache = AvatarDiskCache::new(store.clone(), scope.clone());

    let source = Arc::new(MockAvatarSource::new());
    source.serve(
        "https://avatars.test/octocat.png",
        200,
        Some("\"etag-1\""),
        Some("Wed, 21 Oct 2026 07:28:00 GMT"),
        Some("max-age=3600"),
        valid_png(),
    );

    let fetcher = AvatarFetcher::new(source.clone(), false)
        .with_source(source.clone())
        .with_cache(disk_cache);

    let key = AvatarKey::new("github.com", 100, "https://avatars.test/octocat.png");

    // 1. Initial online fetch fills cache
    let img = fetcher.fetch(&key).await;
    assert!(img.is_some(), "initial fetch decodes PNG successfully");
    assert_eq!(source.request_count.load(Ordering::SeqCst), 1);

    // Drop original store and fetcher, simulate restart by opening CacheStore afresh from same directory
    drop(fetcher);
    drop(store);

    let store2 = CacheStore::open(config).unwrap();
    let account = AccountKey::new(100, HostId::github());
    let scope2 = store2.saved_account(account).unwrap();
    assert_eq!(scope.partition(), scope2.partition());

    let disk_cache2 = AvatarDiskCache::new(store2.clone(), scope2);

    // Empty/failing network source simulating offline
    let offline_source = Arc::new(MockAvatarSource::new());
    let fetcher2 = AvatarFetcher::new(offline_source.clone(), false)
        .with_source(offline_source.clone())
        .with_cache(disk_cache2);

    // 2. Offline fetch: network fails, but disk cache hits and decodes
    let img_offline = fetcher2.fetch(&key).await;
    assert!(
        img_offline.is_some(),
        "offline fetch succeeds from disk cache"
    );
    assert_eq!(
        offline_source.request_count.load(Ordering::SeqCst),
        1,
        "attempted network once then fell back to disk"
    );
}

#[tokio::test]
async fn avatar_same_url_updates_on_new_image_and_304_revalidates() {
    let dir = tempdir().unwrap();
    let config = CacheConfig::at(dir.path().to_path_buf());
    let store = CacheStore::open(config).unwrap();
    let account = AccountKey::new(200, HostId::github());
    let scope = store.saved_account(account).unwrap();
    let disk_cache = AvatarDiskCache::new(store.clone(), scope);

    let source = Arc::new(MockAvatarSource::new());
    let url = "https://avatars.test/user.png";
    let key = AvatarKey::new("github.com", 200, url);

    // First serving
    source.serve(
        url,
        200,
        Some("\"v1\""),
        Some("Wed, 21 Oct 2026 07:00:00 GMT"),
        None,
        valid_png(),
    );

    let fetcher = AvatarFetcher::new(source.clone(), false)
        .with_source(source.clone())
        .with_cache(disk_cache.clone());

    let img1 = fetcher.fetch(&key).await;
    assert!(img1.is_some());

    // Second fetch: server responds 304 Not Modified
    source.serve(url, 304, Some("\"v1\""), None, None, vec![]);
    let img2 = fetcher.fetch(&key).await;
    assert!(img2.is_some(), "304 revalidates and returns cached image");

    // Verify conditional headers were sent
    let reqs = source.requests.lock().unwrap().clone();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[1].1.as_deref(), Some("\"v1\""));

    // Third fetch: server responds with updated image bytes
    source.serve(url, 200, Some("\"v2\""), None, None, valid_png());
    let img3 = fetcher.fetch(&key).await;
    assert!(img3.is_some(), "200 with new representation updates cache");
}

#[tokio::test]
async fn avatar_no_store_invalidates_old_cache_and_is_not_persisted() {
    let dir = tempdir().unwrap();
    let config = CacheConfig::at(dir.path().to_path_buf());
    let store = CacheStore::open(config).unwrap();
    let account = AccountKey::new(300, HostId::github());
    let scope = store.saved_account(account).unwrap();
    let disk_cache = AvatarDiskCache::new(store.clone(), scope.clone());

    let source = Arc::new(MockAvatarSource::new());
    let url = "https://avatars.test/ephemeral.png";
    let key = AvatarKey::new("github.com", 300, url);

    // Step 1: initial 200 stores in cache
    source.serve(url, 200, Some("\"e1\""), None, None, valid_png());
    let fetcher = AvatarFetcher::new(source.clone(), false)
        .with_source(source.clone())
        .with_cache(disk_cache.clone());
    assert!(fetcher.fetch(&key).await.is_some());
    assert!(disk_cache.read_sync(&key).is_some());

    // Step 2: same URL returns 200 with Cache-Control: no-store
    source.serve(
        url,
        200,
        None,
        None,
        Some("no-cache, no-store, must-revalidate"),
        valid_png(),
    );
    assert!(fetcher.fetch(&key).await.is_some());

    // Verify disk cache entry is invalidated / removed
    assert!(
        disk_cache.read_sync(&key).is_none(),
        "no-store invalidates old cached representation and does not persist new one"
    );
}

#[tokio::test]
async fn no_store_with_an_unusable_body_still_invalidates_the_stored_avatar() {
    let dir = tempdir().unwrap();
    let config = CacheConfig::at(dir.path().to_path_buf());
    let store = CacheStore::open(config).unwrap();
    let account = AccountKey::new(350, HostId::github());
    let scope = store.saved_account(account).unwrap();
    let disk_cache = AvatarDiskCache::new(store.clone(), scope.clone());

    let source = Arc::new(MockAvatarSource::new());
    let url = "https://avatars.test/no_store_broken.png";
    let key = AvatarKey::new("github.com", 351, url);

    // A usable avatar is stored first.
    source.serve(url, 200, Some("\"b1\""), None, None, valid_png());
    let fetcher = AvatarFetcher::new(source.clone(), false)
        .with_source(source.clone())
        .with_cache(disk_cache.clone());
    assert!(fetcher.fetch(&key).await.is_some());
    assert!(disk_cache.read_sync(&key).is_some());

    // The same key then answers `no-store` with bytes that are not an image:
    // the old representation must still be dropped, and nothing is published.
    source.serve(
        url,
        200,
        None,
        None,
        Some("no-store"),
        b"not a png image".to_vec(),
    );
    assert!(
        fetcher.fetch(&key).await.is_none(),
        "an unusable body is never rendered"
    );
    assert!(
        disk_cache.read_sync(&key).is_none(),
        "no-store invalidates the stored representation even when the body is unusable"
    );
}

#[tokio::test]
async fn corrupt_or_oversized_payload_falls_back_and_does_not_cache() {
    let dir = tempdir().unwrap();
    let config = CacheConfig::at(dir.path().to_path_buf());
    let store = CacheStore::open(config).unwrap();
    let account = AccountKey::new(400, HostId::github());
    let scope = store.saved_account(account).unwrap();
    let disk_cache = AvatarDiskCache::new(store.clone(), scope.clone());

    let source = Arc::new(MockAvatarSource::new());
    let broken_url = "https://avatars.test/broken.png";
    let broken_key = AvatarKey::new("github.com", 401, broken_url);
    source.serve(
        broken_url,
        200,
        None,
        None,
        None,
        b"not a png image".to_vec(),
    );

    let fetcher = AvatarFetcher::new(source.clone(), false)
        .with_source(source.clone())
        .with_cache(disk_cache.clone());

    assert!(fetcher.fetch(&broken_key).await.is_none());
    assert!(
        disk_cache.read_sync(&broken_key).is_none(),
        "invalid payload is not written to cache"
    );

    // Oversized URL
    let big_url = "https://avatars.test/big.png";
    let big_key = AvatarKey::new("github.com", 402, big_url);
    source.serve(
        big_url,
        200,
        None,
        None,
        None,
        vec![0u8; MAX_AVATAR_BYTES + 10],
    );
    assert!(fetcher.fetch(&big_key).await.is_none());
    assert!(
        disk_cache.read_sync(&big_key).is_none(),
        "oversized payload is not written to cache"
    );
}

#[tokio::test]
async fn no_cache_and_must_revalidate_prohibit_stale_offline_reuse() {
    let dir = tempdir().unwrap();
    let config = CacheConfig::at(dir.path().to_path_buf());
    let store = CacheStore::open(config.clone()).unwrap();
    let account = AccountKey::new(450, HostId::github());
    let scope = store.saved_account(account).unwrap();
    let disk_cache = AvatarDiskCache::new(store.clone(), scope.clone());

    let source = Arc::new(MockAvatarSource::new());
    let url_no_cache = "https://avatars.test/no_cache.png";
    let key_no_cache = AvatarKey::new("github.com", 451, url_no_cache);
    source.serve(
        url_no_cache,
        200,
        Some("\"nc1\""),
        None,
        Some("no-cache"),
        valid_png(),
    );

    let fetcher = AvatarFetcher::new(source.clone(), false)
        .with_source(source.clone())
        .with_cache(disk_cache.clone());

    // Online fetch stores it
    assert!(fetcher.fetch(&key_no_cache).await.is_some());
    assert!(disk_cache.read_sync(&key_no_cache).is_some());

    // Offline fetch with failing network: must NOT return stale avatar because no-cache forbids it!
    let offline_source = Arc::new(MockAvatarSource::new());
    let fetcher_offline = AvatarFetcher::new(offline_source.clone(), false)
        .with_source(offline_source.clone())
        .with_cache(disk_cache);

    assert!(
        fetcher_offline.fetch(&key_no_cache).await.is_none(),
        "no-cache forbids stale offline fallback"
    );
}

#[tokio::test]
async fn a_304_without_cache_control_preserves_the_stored_policy() {
    let dir = tempdir().unwrap();
    let config = CacheConfig::at(dir.path().to_path_buf());
    let store = CacheStore::open(config).unwrap();
    let account = AccountKey::new(460, HostId::github());
    let scope = store.saved_account(account).unwrap();
    let disk_cache = AvatarDiskCache::new(store.clone(), scope.clone());

    let source = Arc::new(MockAvatarSource::new());
    let url = "https://avatars.test/revalidated.png";
    let key = AvatarKey::new("github.com", 461, url);
    source.serve(
        url,
        200,
        Some("\"r1\""),
        None,
        Some("no-cache"),
        valid_png(),
    );

    let fetcher = AvatarFetcher::new(source.clone(), false)
        .with_source(source.clone())
        .with_cache(disk_cache.clone());

    // Online fetch stores the representation with its no-cache policy.
    assert!(fetcher.fetch(&key).await.is_some());

    // A 304 that omits Cache-Control must not erase the stored no-cache policy.
    source.serve(url, 304, Some("\"r1\""), None, None, vec![]);
    assert!(fetcher.fetch(&key).await.is_some());

    // Offline fallback is still forbidden because no-cache was preserved across revalidation.
    let offline_source = Arc::new(MockAvatarSource::new());
    let offline = AvatarFetcher::new(offline_source.clone(), false)
        .with_source(offline_source.clone())
        .with_cache(disk_cache);
    assert!(
        offline.fetch(&key).await.is_none(),
        "a header-less 304 preserves the stored no-cache policy"
    );
}

#[tokio::test]
async fn offline_fallback_uses_the_current_snapshot_not_the_pre_request_one() {
    let dir = tempdir().unwrap();
    let config = CacheConfig::at(dir.path().to_path_buf());
    let store = CacheStore::open(config).unwrap();
    let account = AccountKey::new(470, HostId::github());
    let scope = store.saved_account(account).unwrap();
    let disk_cache = AvatarDiskCache::new(store.clone(), scope.clone());

    let url = "https://avatars.test/superseded.png";
    let key = AvatarKey::new("github.com", 471, url);

    // Seed a permissive entry directly, as if it were read at request start.
    disk_cache
        .put_sync(&key, &valid_png(), None, None, None)
        .unwrap();
    assert!(disk_cache.read_sync(&key).is_some());

    // While the request is in flight, a newer writer replaces the entry with a
    // `no-cache` policy, then the network read fails.
    let source = Arc::new(SupersedingSource {
        cache: disk_cache.clone(),
        key: key.clone(),
        replacement_body: valid_png(),
        replacement_cache_control: Some("no-cache".to_string()),
        calls: AtomicUsize::new(0),
    });
    let fetcher = AvatarFetcher::new(source.clone(), false)
        .with_source(source.clone())
        .with_cache(disk_cache.clone());

    assert!(
        fetcher.fetch(&key).await.is_none(),
        "the superseded permissive snapshot is not resurrected"
    );
    assert!(
        disk_cache.read_sync(&key).is_some(),
        "the newer writer's entry remains stored"
    );
}

#[tokio::test]
async fn oversized_no_store_body_invalidates_the_stored_avatar() {
    let dir = tempdir().unwrap();
    let config = CacheConfig::at(dir.path().to_path_buf());
    let store = CacheStore::open(config).unwrap();
    let account = AccountKey::new(480, HostId::github());
    let scope = store.saved_account(account).unwrap();
    let disk_cache = AvatarDiskCache::new(store.clone(), scope.clone());

    let source = Arc::new(MockAvatarSource::new());
    let url = "https://avatars.test/huge_no_store.png";
    let key = AvatarKey::new("github.com", 481, url);

    source.serve(url, 200, Some("\"h1\""), None, None, valid_png());
    let fetcher = AvatarFetcher::new(source.clone(), false)
        .with_source(source.clone())
        .with_cache(disk_cache.clone());
    assert!(fetcher.fetch(&key).await.is_some());
    assert!(disk_cache.read_sync(&key).is_some());

    source.serve(
        url,
        200,
        None,
        None,
        Some("no-store"),
        vec![0u8; MAX_AVATAR_BYTES + 10],
    );
    assert!(fetcher.fetch(&key).await.is_none());
    assert!(
        disk_cache.read_sync(&key).is_none(),
        "an oversized no-store body still drops the previous representation"
    );
}

#[test]
fn sensitive_url_query_parameters_are_sanitized() {
    let raw =
        "https://avatars.githubusercontent.com/u/1234?v=4&X-Amz-Signature=secret123&token=xyz";
    let clean = sanitize_avatar_url(raw);
    assert_eq!(
        clean, "https://avatars.githubusercontent.com/u/1234",
        "sensitive query params are stripped"
    );

    let regular = "https://avatars.githubusercontent.com/u/1234?v=4&s=40";
    let kept = sanitize_avatar_url(regular);
    assert_eq!(kept, regular, "safe query params are retained");

    let key = AvatarKey::new("github.com", 1234, raw);
    let cache_key = avatar_cache_key(&key).unwrap();
    // CacheKey debug does not expose query
    let debug_str = format!("{:?}", cache_key);
    assert!(!debug_str.contains("secret123"));
}

#[tokio::test]
async fn retired_account_scope_prevents_read_and_write() {
    let dir = tempdir().unwrap();
    let config = CacheConfig::at(dir.path().to_path_buf());
    let store = CacheStore::open(config).unwrap();
    let account = AccountKey::new(500, HostId::github());
    let scope = store.saved_account(account).unwrap();
    let disk_cache = AvatarDiskCache::new(store.clone(), scope.clone());

    let source = Arc::new(MockAvatarSource::new());
    let url = "https://avatars.test/account500.png";
    let key = AvatarKey::new("github.com", 500, url);
    source.serve(url, 200, None, None, None, valid_png());

    let fetcher = AvatarFetcher::new(source.clone(), false)
        .with_source(source.clone())
        .with_cache(disk_cache.clone());

    // Fill cache
    assert!(fetcher.fetch(&key).await.is_some());
    assert!(disk_cache.read_sync(&key).is_some());

    // Now retire account
    store.retire(&scope).unwrap();

    // After retirement, reading from cache must fail
    assert!(disk_cache.read_sync(&key).is_none());

    // Attempting to write to retired cache scope fails
    let put_res = disk_cache.put_sync(&key, &valid_png(), None, None, None);
    assert!(put_res.is_err());
}
