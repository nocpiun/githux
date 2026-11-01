use super::*;
use crate::{
    auth::{memory::InMemoryCredentialStore, token::Pat},
    data::AccountService,
    github::{
        client::GitHubConfig,
        home::{FeedLimits, FollowingLimits, HomeSearchLimits, PullRequestDetailLimits},
        repository_extras::RepositoryExtrasLimits,
    },
    network::{
        executor::BackgroundExecutor,
        transport::{MockTransport, json_response},
    },
    storage::cache::CacheConfig,
};

pub(super) struct Fixture {
    pub(super) service: AccountService,
    pub(super) mock: Arc<MockTransport>,
    _executor: Arc<BackgroundExecutor>,
}
impl Fixture {
    pub(super) async fn new() -> Self {
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(json_response(200, r#"{"id":1,"login":"octo"}"#)))
            .await;
        let executor = Arc::new(BackgroundExecutor::new(4));
        let service = AccountService::new(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(GitHubClient::new(GitHubConfig::default(), mock.clone())),
            executor.clone(),
        );
        service
            .login(Pat::from_str("synthetic-page-cache"))
            .await
            .unwrap();
        Self {
            service,
            mock,
            _executor: executor,
        }
    }
    pub(super) fn cached(&self, store: &CacheStore) -> CachedPageReadSession {
        Self::cached_for(&self.service, store)
    }
    pub(super) fn cached_for(
        service: &AccountService,
        store: &CacheStore,
    ) -> CachedPageReadSession {
        let session = service.page_session().unwrap();
        let scope = store
            .saved_account(session.context().auth().account.clone())
            .unwrap();
        CachedPageReadSession::new(session, store.clone(), scope).unwrap()
    }
    pub(super) async fn enqueue(&self, body: &str) {
        self.mock.enqueue(Ok(json_response(200, body))).await;
    }
}
pub(super) fn temp() -> (tempfile::TempDir, CacheStore) {
    let directory = tempfile::tempdir().unwrap();
    let store = CacheStore::open(CacheConfig::at(directory.path().to_path_buf())).unwrap();
    (directory, store)
}
pub(super) fn value<T>(read: Scoped<PageValue<T>>) -> (T, PageOrigin, CacheMetadata) {
    match read.value {
        PageValue::Value {
            value,
            origin,
            metadata,
        } => (value, origin, metadata),
        PageValue::Miss(reason) => panic!("unexpected miss: {reason:?}"),
    }
}
const EMPTY_HOME: &str = r#"{"total_count":0,"incomplete_results":false,"items":[]}"#;
pub(super) fn repo() -> crate::domain::repository::RepositoryIdentity {
    crate::domain::repository::RepositoryIdentity {
        id: 42,
        locator: crate::domain::repository::RepositoryLocator::new("octo", "repo").unwrap(),
    }
}

#[tokio::test]
async fn actual_adapters_restart_offline_home_feed_profile_and_extras() {
    let fixture = Fixture::new().await;
    let (directory, store) = temp();
    let cached = fixture.cached(&store);
    fixture.enqueue(EMPTY_HOME).await;
    let (_, origin, metadata) = value(
        cached
            .home(
                PageReadMode::Online,
                None,
                &HomeSearchLimits::default(),
                Default::default(),
            )
            .await
            .unwrap(),
    );
    assert_eq!(origin, PageOrigin::Network);
    assert_eq!(metadata.completeness, CacheCompleteness::Complete);
    fixture.enqueue("[]").await;
    fixture.enqueue("[]").await;
    cached
        .feed(
            PageReadMode::Online,
            &FeedLimits::default(),
            &FollowingLimits::default(),
            &PullRequestDetailLimits::default(),
            4,
            Default::default(),
        )
        .await
        .unwrap();
    fixture
        .enqueue(r#"{"id":2,"login":"target","bio":"cached biography"}"#)
        .await;
    cached
        .profile(PageReadMode::Online, "target", Default::default())
        .await
        .unwrap();
    fixture.enqueue("{}").await;
    cached
        .languages(
            PageReadMode::Online,
            &repo(),
            &RepositoryExtrasLimits::default(),
            Default::default(),
        )
        .await
        .unwrap();
    fixture.enqueue("[]").await;
    cached
        .releases(
            PageReadMode::Online,
            &repo(),
            &RepositoryExtrasLimits::default(),
            Default::default(),
        )
        .await
        .unwrap();
    drop(cached);
    drop(store);
    let store = CacheStore::open(CacheConfig::at(directory.path().to_path_buf())).unwrap();
    let cached = fixture.cached(&store);
    let requests = fixture.mock.recorded_requests().await.len();
    let (home, origin, old) = value(
        cached
            .home(
                PageReadMode::Offline,
                None,
                &HomeSearchLimits::default(),
                Default::default(),
            )
            .await
            .unwrap(),
    );
    assert!(home.items.is_empty());
    assert_eq!(origin, PageOrigin::HistoricalSnapshot);
    assert_eq!(old, metadata);
    let (feed, _, _) = value(
        cached
            .feed(
                PageReadMode::Offline,
                &FeedLimits::default(),
                &FollowingLimits::default(),
                &PullRequestDetailLimits::default(),
                4,
                Default::default(),
            )
            .await
            .unwrap(),
    );
    assert!(feed.activities.is_empty());
    assert!(feed.readiness.is_complete());
    let (profile, _, _) = value(
        cached
            .profile(PageReadMode::Offline, "target", Default::default())
            .await
            .unwrap(),
    );
    assert_eq!(profile.bio.as_deref(), Some("cached biography"));
    value(
        cached
            .languages(
                PageReadMode::Offline,
                &repo(),
                &RepositoryExtrasLimits::default(),
                Default::default(),
            )
            .await
            .unwrap(),
    );
    value(
        cached
            .releases(
                PageReadMode::Offline,
                &repo(),
                &RepositoryExtrasLimits::default(),
                Default::default(),
            )
            .await
            .unwrap(),
    );
    assert_eq!(fixture.mock.recorded_requests().await.len(), requests);
    assert!(matches!(
        cached
            .profile(PageReadMode::Offline, "other", Default::default())
            .await
            .unwrap()
            .value,
        PageValue::Miss(CacheMiss::NotCached)
    ));
    let different = HomeSearchLimits {
        per_page: 10,
        ..Default::default()
    };
    assert!(matches!(
        cached
            .home(PageReadMode::Offline, None, &different, Default::default())
            .await
            .unwrap()
            .value,
        PageValue::Miss(_)
    ));
}

#[tokio::test]
async fn unsolicited_304_refetches_and_no_store_invalidates_both_snapshots() {
    let fixture = Fixture::new().await;
    let (_directory, store) = temp();
    let cached = fixture.cached(&store);
    fixture.mock.enqueue(Ok(json_response(304, ""))).await;
    fixture.enqueue(EMPTY_HOME).await;
    cached
        .home(
            PageReadMode::Online,
            None,
            &HomeSearchLimits::default(),
            Default::default(),
        )
        .await
        .unwrap();
    let mut response = json_response(200, EMPTY_HOME);
    response
        .headers
        .push(("Cache-Control".into(), "private, NO-STORE".into()));
    fixture.mock.enqueue(Ok(response)).await;
    let (_, _, metadata) = value(
        cached
            .home(
                PageReadMode::Online,
                None,
                &HomeSearchLimits::default(),
                Default::default(),
            )
            .await
            .unwrap(),
    );
    assert!(!metadata.permits_storage());
    assert!(matches!(
        cached
            .home(
                PageReadMode::Offline,
                None,
                &HomeSearchLimits::default(),
                Default::default()
            )
            .await
            .unwrap()
            .value,
        PageValue::Miss(_)
    ));
}

#[tokio::test]
async fn partial_query_failure_recovers_and_never_replaces_complete_history() {
    let fixture = Fixture::new().await;
    let (_directory, store) = temp();
    let cached = fixture.cached(&store);
    let mut first = json_response(200, EMPTY_HOME);
    first.headers.push((
        "link".into(),
        "<https://api.github.com/search/issues?page=2>; rel=\"next\"".into(),
    ));
    fixture.mock.enqueue(Ok(first.clone())).await;
    fixture.mock.enqueue(Ok(json_response(503, "{}"))).await;
    let (_, _, metadata) = value(
        cached
            .home(
                PageReadMode::Online,
                None,
                &HomeSearchLimits::default(),
                Default::default(),
            )
            .await
            .unwrap(),
    );
    assert!(matches!(
        metadata.completeness,
        CacheCompleteness::Partial { .. }
    ));
    let (partial, _, _) = value(
        cached
            .home(
                PageReadMode::Offline,
                None,
                &HomeSearchLimits::default(),
                Default::default(),
            )
            .await
            .unwrap(),
    );
    assert!(!partial.readiness.is_complete());
    fixture.enqueue(EMPTY_HOME).await;
    let (_, _, complete) = value(
        cached
            .home(
                PageReadMode::Online,
                None,
                &HomeSearchLimits::default(),
                Default::default(),
            )
            .await
            .unwrap(),
    );
    fixture.mock.enqueue(Ok(first)).await;
    fixture.mock.enqueue(Ok(json_response(503, "{}"))).await;
    cached
        .home(
            PageReadMode::Online,
            None,
            &HomeSearchLimits::default(),
            Default::default(),
        )
        .await
        .unwrap();
    let (home, _, historical) = value(
        cached
            .home(
                PageReadMode::Offline,
                None,
                &HomeSearchLimits::default(),
                Default::default(),
            )
            .await
            .unwrap(),
    );
    assert!(home.readiness.is_complete());
    assert_eq!(historical, complete);
}

#[tokio::test]
async fn scope_account_and_pat_gates_remain_independent_of_repository_proof() {
    use crate::storage::cache::AccessEvent;
    let fixture = Fixture::new().await;
    let (_directory, store) = temp();
    let cached = fixture.cached(&store);
    fixture.enqueue(EMPTY_HOME).await;
    cached
        .home(
            PageReadMode::Online,
            None,
            &HomeSearchLimits::default(),
            Default::default(),
        )
        .await
        .unwrap();
    store
        .access_event(&cached.scope, AccessEvent::PatReplaced)
        .unwrap();
    let proof = store.begin_revalidation(&cached.scope, Some(42)).unwrap();
    store.permission_confirmed(proof).unwrap();
    assert!(matches!(
        cached
            .home(
                PageReadMode::Offline,
                None,
                &HomeSearchLimits::default(),
                Default::default()
            )
            .await,
        Err(PageCacheError::Cache(CacheError::AccessDenied))
    ));
    let wrong = store
        .saved_account(crate::domain::AccountKey::new(
            2,
            crate::domain::HostId::github(),
        ))
        .unwrap();
    assert!(matches!(
        CachedPageReadSession::new(
            fixture.service.page_session().unwrap(),
            store.clone(),
            wrong
        ),
        Err(CacheError::AccessDenied)
    ));
    fixture
        .mock
        .enqueue(Ok(json_response(200, r#"{"id":1,"login":"octo"}"#)))
        .await;
    fixture
        .service
        .replace(
            cached.scope.account().clone(),
            Pat::from_str("synthetic-replacement"),
        )
        .await
        .unwrap();
    assert!(matches!(
        cached
            .home(
                PageReadMode::Offline,
                None,
                &HomeSearchLimits::default(),
                Default::default()
            )
            .await,
        Err(PageCacheError::Context(GitHubError::StaleContext))
    ));
}
