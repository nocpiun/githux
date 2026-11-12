use super::*;
use crate::{
    data::page_cache::{CachedPageReadSession, PageCacheError, PageReadMode, PageValue},
    domain::cache::CacheMiss,
    storage::cache::{CacheConfig, CacheStore},
};
fn cache_session(svc: &AccountService, store: &CacheStore) -> CachedPageReadSession {
    let session = svc.page_session().unwrap();
    let scope = store
        .saved_account(session.context().auth().account.clone())
        .unwrap();
    CachedPageReadSession::new(session, store.clone(), scope).unwrap()
}
#[tokio::test]
async fn page_cache_old_refresh_cannot_replace_new_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(&temp, transport.clone(), 4).await;
    login(&svc, &transport, 1, "viewer", "synthetic").await;
    let cache_dir = tempfile::tempdir().unwrap();
    let store = CacheStore::open(CacheConfig::at(cache_dir.path().to_path_buf())).unwrap();
    let cached = cache_session(&svc, &store);
    let (entered, release) = transport.block(200, json!({"id":2,"login":"target","bio":"old"}));
    let reading = {
        let cached = cached.clone();
        tokio::spawn(async move {
            cached
                .profile(PageReadMode::Online, "target", Default::default())
                .await
        })
    };
    ready(entered).await;
    transport.push(200, json!({"id":2,"login":"target","bio":"new"}));
    cached
        .profile(PageReadMode::Online, "target", Default::default())
        .await
        .unwrap();
    release.send(()).unwrap();
    assert!(matches!(
        reading.await.unwrap(),
        Err(PageCacheError::Cache(
            crate::storage::cache::CacheError::Stale
        ))
    ));
    let offline = cached
        .profile(PageReadMode::Offline, "target", Default::default())
        .await
        .unwrap();
    assert!(
        matches!(offline.value, PageValue::Value { value, .. } if value.bio.as_deref() == Some("new"))
    );
}
#[tokio::test]
async fn page_cache_switch_during_read_discards_success_and_cache_write() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(&temp, transport.clone(), 4).await;
    login(&svc, &transport, 2, "other", "synthetic-other").await;
    login(&svc, &transport, 1, "viewer", "synthetic").await;
    let cache_dir = tempfile::tempdir().unwrap();
    let store = CacheStore::open(CacheConfig::at(cache_dir.path().to_path_buf())).unwrap();
    let cached = cache_session(&svc, &store);
    let (entered, release) = transport.block(200, json!({"id":2,"login":"target","bio":"stale"}));
    let reading = {
        let cached = cached.clone();
        tokio::spawn(async move {
            cached
                .profile(PageReadMode::Online, "target", Default::default())
                .await
        })
    };
    ready(entered).await;
    svc.set_active(key(2)).await.unwrap();
    release.send(()).unwrap();
    assert!(matches!(
        reading.await.unwrap(),
        Err(PageCacheError::Context(GitHubError::StaleContext))
    ));
    svc.set_active(key(1)).await.unwrap();
    let fresh = cache_session(&svc, &store);
    assert!(matches!(
        fresh
            .profile(PageReadMode::Offline, "target", Default::default())
            .await
            .unwrap()
            .value,
        PageValue::Miss(CacheMiss::NotCached)
    ));
}

/// A cache replay answers from persistent storage, so it must never wait for a bounded network
/// permit. A cache-first paint stays available even while every request slot is busy with the
/// revalidation it runs beside — otherwise the snapshot would only appear once the network freed a
/// slot, which is exactly the wait it exists to avoid.
#[tokio::test]
async fn offline_replay_does_not_wait_for_a_network_permit() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(&temp, transport.clone(), 1).await;
    login(&svc, &transport, 1, "viewer", "synthetic").await;
    let cache_dir = tempfile::tempdir().unwrap();
    let store = CacheStore::open(CacheConfig::at(cache_dir.path().to_path_buf())).unwrap();
    let cached = cache_session(&svc, &store);
    transport.push(
        200,
        json!({"total_count": 0, "incomplete_results": false, "items": []}),
    );
    cached
        .home(
            PageReadMode::Online,
            None,
            &Default::default(),
            Default::default(),
        )
        .await
        .unwrap();

    // Hold the only network permit.
    let permit = svc.executor.semaphore().acquire_owned().await.unwrap();
    let replay = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        cached.home(
            PageReadMode::Offline,
            None,
            &Default::default(),
            Default::default(),
        ),
    )
    .await
    .expect("a cache replay must not queue behind a network permit");
    assert!(matches!(
        replay.unwrap().value,
        PageValue::Value {
            origin: crate::data::page_cache::PageOrigin::HistoricalSnapshot,
            ..
        }
    ));
    drop(permit);

    // The same replay must still honour cancellation and the account gate.
    let (cancel, source) = crate::network::cancel::CancellationToken::new();
    source.cancel();
    assert!(matches!(
        cached
            .home(PageReadMode::Offline, None, &Default::default(), cancel)
            .await,
        Err(PageCacheError::Context(GitHubError::Cancelled))
    ));
}
