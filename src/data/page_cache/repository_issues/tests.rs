use super::*;
use crate::data::page_cache::{
    PageOrigin,
    tests::{Fixture, temp, value},
};
use crate::domain::repository::RepositoryLocator;
use crate::network::transport::json_response;

#[tokio::test]
async fn repository_issue_cache_is_scoped_and_survives_transient_refresh_failure() {
    let fixture = Fixture::new().await;
    let (_temp, store) = temp();
    let cached = fixture.cached(&store);
    let first = RepositoryIdentity {
        id: 12,
        locator: RepositoryLocator::new("octo", "first").unwrap(),
    };
    let second = RepositoryIdentity {
        id: 13,
        locator: RepositoryLocator::new("octo", "second").unwrap(),
    };
    let read = |mode, repository, filter| {
        cached.repository_issues(mode, repository, filter, CancellationToken::default())
    };
    fixture.enqueue(r#"[{"number":42,"title":"First repo","state":"open","updated_at":"2026-11-01T12:00:00Z"}]"#).await;
    let (_, origin, _) = value(
        read(PageReadMode::Online, &first, IssueFilter::Open)
            .await
            .unwrap(),
    );
    assert_eq!(origin, PageOrigin::Network);
    fixture.mock.enqueue(Ok(json_response(503, "{}"))).await;
    assert!(
        read(PageReadMode::Online, &first, IssueFilter::Open)
            .await
            .is_err()
    );
    let (offline, origin, _) = value(
        read(PageReadMode::Offline, &first, IssueFilter::Open)
            .await
            .unwrap(),
    );
    assert_eq!(origin, PageOrigin::HistoricalSnapshot);
    assert_eq!(offline.rows[0].title, "First repo");
    for (repository, filter) in [(&second, IssueFilter::Open), (&first, IssueFilter::Closed)] {
        assert!(matches!(
            read(PageReadMode::Offline, repository, filter)
                .await
                .unwrap()
                .value,
            PageValue::Miss(_)
        ));
    }
    fixture
        .mock
        .enqueue(Ok(json_response(200, r#"{"id":2,"login":"other"}"#)))
        .await;
    fixture
        .service
        .login(crate::auth::token::Pat::from_str(
            "synthetic-second-account",
        ))
        .await
        .unwrap();
    let other = fixture.cached(&store);
    assert!(matches!(
        other
            .repository_issues(
                PageReadMode::Offline,
                &first,
                IssueFilter::Open,
                CancellationToken::default()
            )
            .await
            .unwrap()
            .value,
        PageValue::Miss(_)
    ));
    assert!(!cached.verify());
}
