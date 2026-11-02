use super::{tests::*, *};
use crate::{
    data::AccountService,
    domain::repository::RepositoryIdentity,
    github::{home::HomeSearchLimits, repository_extras::RepositoryExtrasLimits},
};

const PRIVATE_REPO: u64 = 42;
const OTHER_REPO: u64 = 43;

fn other_repo() -> RepositoryIdentity {
    RepositoryIdentity {
        id: OTHER_REPO,
        locator: crate::domain::repository::RepositoryLocator::new("octo", "other").unwrap(),
    }
}

/// One OPEN issue from the private repository under test, as Home would store it.
fn home_with_private_repository() -> String {
    serde_json::json!({
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
            "repository": {"id": PRIVATE_REPO}
        }]
    })
    .to_string()
}

fn empty_home() -> &'static str {
    r#"{"total_count":0,"incomplete_results":false,"items":[]}"#
}

/// A lease or slot is unreadable when it either loses the entry (Ok(None)) or
/// loses the authorization gate (Err). Both deny publication; neither may yield
/// a snapshot containing the revoked repository.
fn readable(lease: &crate::storage::cache::CacheLease) -> bool {
    matches!(lease.snapshot(), Ok(Some(_)))
}

#[tokio::test]
async fn revoking_a_repository_clears_complete_and_partial_account_aggregate_snapshots() {
    let fixture = Fixture::new().await;
    let (directory, store) = temp();
    let cached = fixture.cached(&store);

    fixture.enqueue(&home_with_private_repository()).await;
    let (complete_value, _, complete_metadata) = value(
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
    assert_eq!(complete_value.items.len(), 1);

    // A second page failure keeps the same aggregate in the Partial slot.
    let mut first_page =
        crate::network::transport::json_response(200, &home_with_private_repository());
    first_page.headers.push((
        "link".into(),
        "<https://api.github.com/search/issues?page=2>; rel=\"next\"".into(),
    ));
    fixture.mock.enqueue(Ok(first_page)).await;
    fixture
        .mock
        .enqueue(Ok(crate::network::transport::json_response(503, "{}")))
        .await;
    let (_, _, partial_metadata) = value(
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
        partial_metadata.completeness,
        CacheCompleteness::Partial { .. }
    ));
    assert_eq!(complete_metadata.completeness, CacheCompleteness::Complete);

    let (complete_key, partial_key) = cached
        .home_keys(None, &HomeSearchLimits::default())
        .unwrap();
    let CacheRead::Hit(complete_lease) = store.read(cached.scope(), &complete_key).unwrap() else {
        panic!("complete aggregate snapshot must exist before revocation");
    };
    assert!(readable(&complete_lease));

    store
        .revoke_repository(cached.scope(), Some(PRIVATE_REPO))
        .unwrap();

    assert!(
        !readable(&complete_lease),
        "a lease taken before revocation must not keep serving the aggregate"
    );
    let offline = cached
        .home(
            PageReadMode::Offline,
            None,
            &HomeSearchLimits::default(),
            Default::default(),
        )
        .await;
    assert!(
        !matches!(
            offline.as_ref().map(|scoped| &scoped.value),
            Ok(PageValue::Value { .. })
        ),
        "offline aggregate must not expose revoked repository content: {offline:?}"
    );
    assert!(
        matches!(
            store.read(cached.scope(), &partial_key),
            Ok(CacheRead::Miss(_)) | Err(_)
        ),
        "the Partial slot is not a bypass around revocation"
    );
    drop(complete_lease);
    drop(cached);
    drop(store);
    let store = CacheStore::open(crate::storage::cache::CacheConfig::at(
        directory.path().to_path_buf(),
    ))
    .unwrap();
    let cached = fixture.cached(&store);
    let offline = cached
        .home(
            PageReadMode::Offline,
            None,
            &HomeSearchLimits::default(),
            Default::default(),
        )
        .await
        .unwrap();
    assert!(
        matches!(offline.value, PageValue::Miss(CacheMiss::NotCached)),
        "restart must not revive either aggregate slot"
    );
}

#[tokio::test]
async fn revocation_spares_other_repositories_and_other_accounts() {
    let mock = std::sync::Arc::new(crate::network::transport::MockTransport::new());
    mock.enqueue(Ok(crate::network::transport::json_response(
        200,
        r#"{"id":1,"login":"octo"}"#,
    )))
    .await;
    let executor = std::sync::Arc::new(crate::network::executor::BackgroundExecutor::new(4));
    let service = AccountService::new(
        std::sync::Arc::new(crate::auth::memory::InMemoryCredentialStore::new()),
        std::sync::Arc::new(crate::github::client::GitHubClient::new(
            crate::github::client::GitHubConfig::default(),
            mock.clone(),
        )),
        executor,
    );
    service
        .login(crate::auth::token::Pat::from_str("synthetic-a"))
        .await
        .unwrap();
    let first = service.active_context().unwrap().account;
    let (_directory, store) = temp();

    let cached_first = Fixture::cached_for(&service, &store);
    mock.enqueue(Ok(crate::network::transport::json_response(
        200,
        &home_with_private_repository(),
    )))
    .await;
    cached_first
        .home(
            PageReadMode::Online,
            None,
            &HomeSearchLimits::default(),
            Default::default(),
        )
        .await
        .unwrap();
    mock.enqueue(Ok(crate::network::transport::json_response(200, "[]")))
        .await;
    cached_first
        .releases(
            PageReadMode::Online,
            &other_repo(),
            &RepositoryExtrasLimits::default(),
            Default::default(),
        )
        .await
        .unwrap();

    // A second saved account keeps its own aggregate snapshot.
    mock.enqueue(Ok(crate::network::transport::json_response(
        200,
        r#"{"id":2,"login":"other"}"#,
    )))
    .await;
    service
        .login(crate::auth::token::Pat::from_str("synthetic-b"))
        .await
        .unwrap();
    let second = service.active_context().unwrap().account;
    let cached_second = Fixture::cached_for(&service, &store);
    mock.enqueue(Ok(crate::network::transport::json_response(
        200,
        &home_with_private_repository(),
    )))
    .await;
    cached_second
        .home(
            PageReadMode::Online,
            None,
            &HomeSearchLimits::default(),
            Default::default(),
        )
        .await
        .unwrap();

    service.set_active(first.clone()).await.unwrap();
    let cached_first = Fixture::cached_for(&service, &store);
    store
        .revoke_repository(cached_first.scope(), Some(PRIVATE_REPO))
        .unwrap();

    assert!(
        !matches!(
            cached_first
                .home(
                    PageReadMode::Offline,
                    None,
                    &HomeSearchLimits::default(),
                    Default::default()
                )
                .await
                .map(|scoped| scoped.value),
            Ok(PageValue::Value { .. })
        ),
        "the revoked account aggregate is unreadable"
    );
    let sibling = cached_first
        .releases(
            PageReadMode::Offline,
            &other_repo(),
            &RepositoryExtrasLimits::default(),
            Default::default(),
        )
        .await
        .unwrap();
    assert!(
        matches!(sibling.value, PageValue::Value { .. }),
        "an independent repository {OTHER_REPO} keeps its own cached entries"
    );

    service.set_active(second).await.unwrap();
    let cached_second = Fixture::cached_for(&service, &store);
    let other_account = cached_second
        .home(
            PageReadMode::Offline,
            None,
            &HomeSearchLimits::default(),
            Default::default(),
        )
        .await
        .unwrap();
    assert!(
        matches!(other_account.value, PageValue::Value { .. }),
        "another account's aggregate must not be invalidated"
    );
}

#[tokio::test]
async fn after_revocation_a_new_online_aggregate_read_succeeds_and_pat_replacement_still_isolates_it()
 {
    let fixture = Fixture::new().await;
    let (_directory, store) = temp();
    let cached = fixture.cached(&store);
    fixture.enqueue(&home_with_private_repository()).await;
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
        .revoke_repository(cached.scope(), Some(PRIVATE_REPO))
        .unwrap();

    // The same still-allowed aggregate gate may store a fresh, R-free snapshot.
    fixture.enqueue(empty_home()).await;
    let (fresh, origin, _) = value(
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
    assert!(fresh.items.is_empty());
    assert_eq!(origin, PageOrigin::Network);
    let (offline, origin, _) = value(
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
    assert!(offline.items.is_empty());
    assert_eq!(origin, PageOrigin::HistoricalSnapshot);

    // C3: a replacement PAT plus a single-repository proof cannot open the
    // account aggregate again.
    store
        .access_event(
            cached.scope(),
            crate::storage::cache::AccessEvent::PatReplaced,
        )
        .unwrap();
    let proof = store
        .begin_revalidation(cached.scope(), Some(OTHER_REPO))
        .unwrap();
    store.permission_confirmed(proof).unwrap();
    assert!(
        matches!(
            store.read(
                cached.scope(),
                &cached
                    .home_keys(None, &HomeSearchLimits::default())
                    .unwrap()
                    .0
            ),
            Err(CacheError::AccessDenied)
        ),
        "repository proof must not unlock the aggregate after a PAT replacement"
    );
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
}
