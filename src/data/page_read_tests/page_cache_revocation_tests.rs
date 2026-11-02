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

fn home_with_private_repository() -> serde_json::Value {
    json!({
        "total_count": 1,
        "incomplete_results": false,
        "items": [{
            "id": 9001,
            "number": 7,
            "title": "Private repository issue",
            "html_url": "https://github.com/octo/repo/issues/7",
            "state": "open",
            "updated_at": "2026-10-30T00:00:00Z",
            "repository_url": "https://api.github.com/repos/octo/repo",
            "repository": {"id": 42}
        }]
    })
}

/// A read started under valid authorization but completed after explicit
/// repository revocation must neither republish the stale value nor leave a
/// backfillable snapshot behind.
#[tokio::test]
async fn late_home_read_after_revocation_is_rejected_and_never_backfills() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(&temp, transport.clone(), 4).await;
    login(&svc, &transport, 1, "viewer", "synthetic").await;
    let cache_dir = tempfile::tempdir().unwrap();
    let store = CacheStore::open(CacheConfig::at(cache_dir.path().to_path_buf())).unwrap();
    let cached = cache_session(&svc, &store);

    // A pre-revocation snapshot exists for the aggregate.
    transport.push(200, home_with_private_repository());
    cached
        .home(
            PageReadMode::Online,
            None,
            &crate::github::home::HomeSearchLimits::default(),
            Default::default(),
        )
        .await
        .unwrap();

    // Block a second read, revoke the repository mid-flight, then release it.
    let (entered, release) = transport.block(200, home_with_private_repository());
    let reading = {
        let cached = cached.clone();
        tokio::spawn(async move {
            cached
                .home(
                    PageReadMode::Online,
                    None,
                    &crate::github::home::HomeSearchLimits::default(),
                    Default::default(),
                )
                .await
        })
    };
    ready(entered).await;
    store.revoke_repository(cached.scope(), Some(42)).unwrap();
    release.send(()).unwrap();

    let result = reading.await.unwrap();
    assert!(
        result.is_err(),
        "the late pre-revocation read must not publish as a successful Network result"
    );
    assert!(
        !matches!(result, Err(PageCacheError::Read(_))),
        "the rejection must come from the stale write/context, not a fresh adapter failure"
    );

    let offline = cached
        .home(
            PageReadMode::Offline,
            None,
            &crate::github::home::HomeSearchLimits::default(),
            Default::default(),
        )
        .await;
    assert!(
        !matches!(
            offline.as_ref().map(|scoped| &scoped.value),
            Ok(PageValue::Value { .. })
        ),
        "no revoked-repository content may remain readable or backfilled: {offline:?}"
    );
    assert!(matches!(
        offline.map(|scoped| scoped.value),
        Ok(PageValue::Miss(CacheMiss::NotCached))
    ));
}
