use super::*;
use crate::{
    auth::{memory::InMemoryCredentialStore, token::Pat},
    data::AccountService,
    github::client::GitHubConfig,
    network::{
        executor::BackgroundExecutor,
        test_server::{LocalHttpServer, ScriptedResponse},
        transport::ReqwestTransport,
    },
    storage::cache::CacheConfig,
};

#[tokio::test]
async fn localhost_profile_restart_preserves_time_and_never_contacts_offline_server() {
    let server = LocalHttpServer::start(vec![
        ScriptedResponse::json(200, r#"{"id":1,"login":"octo"}"#),
        ScriptedResponse::json(200, r#"{"id":2,"login":"target","name":"Old profile"}"#)
            .with_header("ETag", "\"profile-v1\""),
    ]);
    let executor = Arc::new(BackgroundExecutor::new(2));
    let client = Arc::new(GitHubClient::new(
        GitHubConfig {
            base_url: server.base_url().into(),
            ..Default::default()
        },
        Arc::new(ReqwestTransport::new(std::time::Duration::from_secs(5)).unwrap()),
    ));
    let service = AccountService::new(Arc::new(InMemoryCredentialStore::new()), client, executor);
    service
        .login(Pat::from_str("synthetic-localhost"))
        .await
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let store = CacheStore::open(CacheConfig::at(directory.path().to_path_buf())).unwrap();
    let cached = super::tests::Fixture::cached_for(&service, &store);
    let (_, _, before) = super::tests::value(
        cached
            .profile(PageReadMode::Online, "target", Default::default())
            .await
            .unwrap(),
    );
    drop(cached);
    drop(store);
    let store = CacheStore::open(CacheConfig::at(directory.path().to_path_buf())).unwrap();
    let cached = super::tests::Fixture::cached_for(&service, &store);
    let (profile, origin, after) = super::tests::value(
        cached
            .profile(PageReadMode::Offline, "target", Default::default())
            .await
            .unwrap(),
    );
    assert_eq!(profile.name.as_deref(), Some("Old profile"));
    assert_eq!(origin, PageOrigin::HistoricalSnapshot);
    assert_eq!(before, after);
    assert_eq!(server.recorded().len(), 2);
}

#[tokio::test]
async fn representation_versions_and_corrupt_dtos_are_explicit_misses() {
    let fixture = super::tests::Fixture::new().await;
    let (_directory, store) = super::tests::temp();
    let cached = fixture.cached(&store);
    let key = cached
        .key(None, "profile-basic", serde_json::json!("target"), false)
        .unwrap();
    let ticket = store.begin_write(&cached.scope, &key).unwrap();
    let mut encoded = serde_json::to_vec(&SnapshotV1 {
        version: 2,
        exchanges: vec![],
    })
    .unwrap();
    store
        .put(
            ticket,
            &encoded,
            metadata(1, CacheCompleteness::Complete, false),
        )
        .unwrap();
    assert!(matches!(
        cached
            .profile(PageReadMode::Offline, "target", Default::default())
            .await
            .unwrap()
            .value,
        PageValue::Miss(CacheMiss::RepresentationVersion)
    ));
    encoded.clear();
    encoded.extend_from_slice(b"broken");
    let ticket = store.begin_write(&cached.scope, &key).unwrap();
    store
        .put(
            ticket,
            &encoded,
            metadata(1, CacheCompleteness::Complete, false),
        )
        .unwrap();
    assert!(matches!(
        cached
            .profile(PageReadMode::Offline, "target", Default::default())
            .await
            .unwrap()
            .value,
        PageValue::Miss(CacheMiss::Corrupt)
    ));
    let version2 = CacheKey::mutable(
        None,
        true,
        "profile-basic",
        "another representation",
        "page-response-set-v2",
        2,
    )
    .unwrap();
    let ticket = store.begin_write(&cached.scope, &version2).unwrap();
    let mut meta = metadata(1, CacheCompleteness::Complete, false);
    meta.representation_version = 2;
    store.put(ticket, b"{}", meta).unwrap();
    assert!(matches!(
        store.read(&cached.scope, &version2).unwrap(),
        CacheRead::Hit(_)
    ));
    let (cancel, source) = CancellationToken::new();
    source.cancel();
    assert!(matches!(
        cached
            .profile(PageReadMode::Offline, "target", cancel)
            .await,
        Err(PageCacheError::Context(GitHubError::Cancelled))
    ));
}
