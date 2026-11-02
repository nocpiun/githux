use super::{tests::*, *};
use crate::{github::home::HomeSearchLimits, network::transport::json_response};
#[tokio::test]
async fn explicit_401_persistently_rejects_cache_but_ambiguous_denial_keeps_history() {
    let fixture = Fixture::new().await;
    let (directory, store) = temp();
    let cached = fixture.cached(&store);
    fixture.enqueue(r#"{"id":2,"login":"target"}"#).await;
    cached
        .profile(PageReadMode::Online, "target", Default::default())
        .await
        .unwrap();
    fixture.mock.enqueue(Ok(json_response(403, "{}"))).await;
    assert!(matches!(
        cached
            .profile(PageReadMode::Online, "target", Default::default())
            .await,
        Err(PageCacheError::Read(_))
    ));
    value(
        cached
            .profile(PageReadMode::Offline, "target", Default::default())
            .await
            .unwrap(),
    );
    fixture.mock.enqueue(Ok(json_response(401, "{}"))).await;
    assert!(matches!(
        cached
            .home(
                PageReadMode::Online,
                None,
                &HomeSearchLimits::default(),
                Default::default()
            )
            .await,
        Err(PageCacheError::Context(GitHubError::Unauthorized))
    ));
    drop(cached);
    drop(store);
    let store = CacheStore::open(crate::storage::cache::CacheConfig::at(
        directory.path().to_path_buf(),
    ))
    .unwrap();
    let cached = fixture.cached(&store);
    assert!(matches!(
        cached
            .profile(PageReadMode::Offline, "target", Default::default())
            .await,
        Err(PageCacheError::Cache(CacheError::AccessDenied))
    ));
}
