use super::{tests::*, *};
use crate::{
    domain::{
        home::FeedSourceCursor,
        profile::{ContributionRange, Showcase},
    },
    github::{
        home::{FeedLimits, FollowingLimits, PullRequestDetailLimits},
        profile::ProfileLimits,
        repository_extras::RepositoryExtrasLimits,
    },
    network::transport::json_response,
};
use serde_json::json;

#[tokio::test]
async fn graphql_partial_unknown_and_range_identity_survive_replay_without_etag() {
    let fixture = Fixture::new().await;
    let (_directory, store) = temp();
    let cached = fixture.cached(&store);
    let limits = ProfileLimits::default();
    fixture.enqueue(&json!({"data":{"user":{"pinnedItems":{"totalCount":0,"nodes":[]}}},"errors":[{"type":"FORBIDDEN","path":["user","itemShowcase"],"message":"sensitive server echo"}]}).to_string()).await;
    let (_, _, online) = value(
        cached
            .profile_pinned(PageReadMode::Online, "target", &limits, Default::default())
            .await
            .unwrap(),
    );
    assert!(matches!(
        online.completeness,
        CacheCompleteness::Partial { .. }
    ));
    assert_eq!(online.etag, None);
    let (pinned, _, _) = value(
        cached
            .profile_pinned(PageReadMode::Offline, "target", &limits, Default::default())
            .await
            .unwrap(),
    );
    assert!(matches!(pinned.showcase, Showcase::Unclassified));
    assert_eq!(pinned.errors.len(), 1);
    let range = ContributionRange::new("2024-01-01", "2024-01-07").unwrap();
    fixture.enqueue(&json!({"data":{"user":{"contributionsCollection":{"contributionCalendar":{"totalContributions":0,"weeks":[],"months":[]}}}}}).to_string()).await;
    cached
        .contribution_calendar(
            PageReadMode::Online,
            "target",
            &range,
            &limits,
            Default::default(),
        )
        .await
        .unwrap();
    let (calendar, _, _) = value(
        cached
            .contribution_calendar(
                PageReadMode::Offline,
                "target",
                &range,
                &limits,
                Default::default(),
            )
            .await
            .unwrap(),
    );
    assert_eq!(calendar.total_contributions, 0);
    let other = ContributionRange::new("2024-01-02", "2024-01-07").unwrap();
    assert!(matches!(
        cached
            .contribution_calendar(
                PageReadMode::Offline,
                "target",
                &other,
                &limits,
                Default::default()
            )
            .await
            .unwrap()
            .value,
        PageValue::Miss(_)
    ));
    let requests = fixture.mock.recorded_requests().await;
    assert!(requests[1..].iter().all(|request| {
        !request
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("if-none-match"))
    }));
}
#[tokio::test]
async fn feed_older_pages_replay_filter_and_deduplicate_with_current_seen_ids() {
    let fixture = Fixture::new().await;
    let (_directory, store) = temp();
    let cached = fixture.cached(&store);
    let limits = FeedLimits::default();
    let following = FollowingLimits::default();
    let authors = PullRequestDetailLimits::default();
    let event = |id: &str, kind: &str| json!({"id":id,"type":kind,"actor":{"id":10,"login":"friend"},"repo":{"id":42,"name":"octo/repo","url":"https://api.github.com/repos/octo/repo"},"payload":{"action":"started"},"created_at":"2026-10-30T12:00:00Z","public":true});
    fixture
        .enqueue(&json!([event("star", "WatchEvent"), event("noise", "PushEvent")]).to_string())
        .await;
    fixture.enqueue(r#"[{"id":10,"login":"friend"}]"#).await;
    let mut seen = std::collections::HashSet::new();
    let (page, _, _) = value(
        cached
            .feed_page(
                PageReadMode::Online,
                FeedSourceCursor { page: 2 },
                &limits,
                &following,
                &authors,
                4,
                &mut seen,
                Default::default(),
            )
            .await
            .unwrap(),
    );
    assert_eq!(page.activities.len(), 1);
    assert!(seen.contains("star"));
    assert!(seen.contains("noise"));
    let mut different_seen = std::collections::HashSet::from(["star".into()]);
    let (page, _, _) = value(
        cached
            .feed_page(
                PageReadMode::Offline,
                FeedSourceCursor { page: 2 },
                &limits,
                &following,
                &authors,
                4,
                &mut different_seen,
                Default::default(),
            )
            .await
            .unwrap(),
    );
    assert!(page.activities.is_empty());
    assert!(different_seen.contains("noise"));
    assert!(matches!(
        cached
            .feed_page(
                PageReadMode::Offline,
                FeedSourceCursor { page: 3 },
                &limits,
                &following,
                &authors,
                4,
                &mut different_seen,
                Default::default()
            )
            .await
            .unwrap()
            .value,
        PageValue::Miss(_)
    ));
}
#[tokio::test]
async fn no_store_on_failed_dependency_invalidates_previous_feed() {
    let fixture = Fixture::new().await;
    let (_directory, store) = temp();
    let cached = fixture.cached(&store);
    let limits = FeedLimits::default();
    let following = FollowingLimits::default();
    let authors = PullRequestDetailLimits::default();
    fixture.enqueue("[]").await;
    fixture.enqueue("[]").await;
    cached
        .feed(
            PageReadMode::Online,
            &limits,
            &following,
            &authors,
            4,
            Default::default(),
        )
        .await
        .unwrap();
    fixture.enqueue("[]").await;
    let mut response = json_response(503, "{}");
    response
        .headers
        .push(("cache-control".into(), "no-store".into()));
    fixture.mock.enqueue(Ok(response)).await;
    assert!(matches!(
        cached
            .feed(
                PageReadMode::Online,
                &limits,
                &following,
                &authors,
                4,
                Default::default()
            )
            .await,
        Err(PageCacheError::Read(_))
    ));
    assert!(matches!(
        cached
            .feed(
                PageReadMode::Offline,
                &limits,
                &following,
                &authors,
                4,
                Default::default()
            )
            .await
            .unwrap()
            .value,
        PageValue::Miss(_)
    ));
}
#[tokio::test]
async fn remaining_extras_preserve_empty_unknown_and_per_field_count_errors() {
    let fixture = Fixture::new().await;
    let (_directory, store) = temp();
    let cached = fixture.cached(&store);
    let limits = RepositoryExtrasLimits::default();
    let repository = repo();
    fixture.mock.enqueue(Ok(json_response(204, ""))).await;
    cached
        .contributors(
            PageReadMode::Online,
            &repository,
            &limits,
            Default::default(),
        )
        .await
        .unwrap();
    let (contributors, _, _) = value(
        cached
            .contributors(
                PageReadMode::Offline,
                &repository,
                &limits,
                Default::default(),
            )
            .await
            .unwrap(),
    );
    assert_eq!(
        contributors.state,
        crate::domain::repository_extras::ContributorsState::EmptyRepository
    );
    fixture.mock.enqueue(Ok(json_response(404, "{}"))).await;
    cached
        .latest_release(
            PageReadMode::Online,
            &repository,
            &limits,
            Default::default(),
        )
        .await
        .unwrap();
    let (latest, _, metadata) = value(
        cached
            .latest_release(
                PageReadMode::Offline,
                &repository,
                &limits,
                Default::default(),
            )
            .await
            .unwrap(),
    );
    assert!(latest.release.is_none());
    assert!(matches!(
        metadata.completeness,
        CacheCompleteness::Partial { .. }
    ));
    fixture.enqueue(&json!({"data":{"repository":{"databaseId":42,"issues":{"totalCount":0},"pullRequests":{"totalCount":99}}},"errors":[{"type":"FORBIDDEN","path":["repository","pullRequests"]}]}).to_string()).await;
    cached
        .repository_counts(PageReadMode::Online, &repository, Default::default())
        .await
        .unwrap();
    let (counts, _, metadata) = value(
        cached
            .repository_counts(PageReadMode::Offline, &repository, Default::default())
            .await
            .unwrap(),
    );
    assert_eq!(counts.issues, Ok(0));
    assert!(counts.pull_requests.is_err());
    assert!(matches!(
        metadata.completeness,
        CacheCompleteness::Partial { .. }
    ));
}
