use super::*;
use crate::domain::home::{DateBound, Login, UpdatedWindow};
use crate::network::transport::{HttpResponse, MockTransport};
use std::sync::Arc;

fn client(mock: Arc<MockTransport>) -> GitHubClient {
    GitHubClient::new(Default::default(), mock)
}

fn item(id: u64, number: u64, is_pr: bool) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert("id".into(), id.into());
    obj.insert("number".into(), number.into());
    obj.insert("title".into(), format!("item {number}").into());
    obj.insert(
        "html_url".into(),
        format!("https://github.com/acme/repo/issues/{number}").into(),
    );
    obj.insert("state".into(), "open".into());
    obj.insert("updated_at".into(), "2024-06-01T00:00:00Z".into());
    obj.insert(
        "repository_url".into(),
        "https://api.github.com/repos/acme/repo".into(),
    );
    if is_pr {
        obj.insert(
            "pull_request".into(),
            serde_json::json!({"url": "https://api.github.com/repos/acme/repo/pulls/1"}),
        );
    }
    serde_json::Value::Object(obj)
}

fn body(total: u64, incomplete: bool, items: Vec<serde_json::Value>) -> String {
    serde_json::json!({
        "total_count": total,
        "incomplete_results": incomplete,
        "items": items,
    })
    .to_string()
}

fn ok(body: String, link: Option<&str>) -> HttpResponse {
    let mut headers = Vec::new();
    if let Some(link) = link {
        headers.push(("link".to_string(), link.to_string()));
    }
    HttpResponse {
        status: 200,
        headers,
        body,
    }
}

fn query_of(url: &str, key: &str) -> Option<String> {
    Url::parse(url)
        .ok()?
        .query_pairs()
        .find_map(|(k, v)| (k.as_ref() == key).then(|| v.to_string()))
}

fn login(name: &str) -> Login {
    Login::new(name).expect("valid login")
}

fn pat() -> Pat {
    Pat::from_str("ghp_test")
}

async fn run(
    mock: &Arc<MockTransport>,
    query: &HomeQuery,
    limits: &HomeSearchLimits,
) -> Result<HomeSearchOutcome, HomeError> {
    search_involving(
        &client(mock.clone()),
        &pat(),
        query,
        limits,
        CancellationToken::default(),
    )
    .await
}

#[tokio::test]
async fn query_is_encoded_and_fixed_to_involves() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(body(0, false, vec![]), None))).await;

    run(
        &mock,
        &HomeQuery::new(login("test-user")),
        &HomeSearchLimits::default(),
    )
    .await
    .expect("ok");

    let recorded = mock.recorded().await;
    assert_eq!(recorded.len(), 1);
    let url = &recorded[0].0;
    assert_eq!(
        query_of(url, "q").as_deref(),
        Some("involves:test-user"),
        "the query must be a fixed involves lookup"
    );
    assert_eq!(query_of(url, "sort").as_deref(), Some("updated"));
    assert_eq!(query_of(url, "order").as_deref(), Some("desc"));
    // The raw URL must not leak an unencoded qualifier form.
    assert!(
        !url.contains("involves:"),
        "the colon must be percent-encoded, got {url}"
    );
}

#[tokio::test]
async fn updated_window_is_explicit_and_optional() {
    // No window: no `updated:` qualifier at all.
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(body(0, false, vec![]), None))).await;
    run(
        &mock,
        &HomeQuery::new(login("test-user")),
        &HomeSearchLimits::default(),
    )
    .await
    .expect("ok");
    let url = &mock.recorded().await[0].0;
    assert_eq!(
        query_of(url, "q").as_deref(),
        Some("involves:test-user"),
        "no product default time window may be added"
    );

    // Explicit window: both bounds are parameterized.
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(body(0, false, vec![]), None))).await;
    let window = UpdatedWindow::new(
        Some(DateBound::new("2024-01-01").unwrap()),
        Some(DateBound::new("2024-12-31").unwrap()),
    )
    .unwrap();
    let query = HomeQuery::new(login("test-user")).with_updated_window(window);
    run(&mock, &query, &HomeSearchLimits::default())
        .await
        .expect("ok");
    let url = &mock.recorded().await[0].0;
    assert_eq!(
        query_of(url, "q").as_deref(),
        Some("involves:test-user updated:>=2024-01-01 updated:<=2024-12-31")
    );
}

#[tokio::test]
async fn distinguishes_issue_and_pull_request() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(
        body(2, false, vec![item(1, 10, false), item(2, 11, true)]),
        None,
    )))
    .await;

    let outcome = run(
        &mock,
        &HomeQuery::new(login("test-user")),
        &HomeSearchLimits::default(),
    )
    .await
    .expect("ok");
    assert_eq!(outcome.items.len(), 2);
    assert_eq!(outcome.items[0].kind, HomeItemKind::Issue);
    assert_eq!(outcome.items[1].kind, HomeItemKind::PullRequest);
    assert_eq!(outcome.items[0].repository.owner, "acme");
    assert_eq!(outcome.items[0].repository.name, "repo");
    assert_eq!(outcome.items[1].number, 11);
    assert_eq!(outcome.items[1].state, "open");
    assert!(outcome.readiness.is_complete());
}

#[tokio::test]
async fn deduplicates_items_across_pages() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(
        body(3, false, vec![item(1, 1, false), item(2, 2, false)]),
        Some("<https://api.github.com/search/issues?page=2>; rel=\"next\""),
    )))
    .await;
    mock.enqueue(Ok(ok(
        body(3, false, vec![item(2, 2, false), item(3, 3, true)]),
        None,
    )))
    .await;

    let outcome = run(
        &mock,
        &HomeQuery::new(login("test-user")),
        &HomeSearchLimits::default(),
    )
    .await
    .expect("ok");
    let ids: Vec<u64> = outcome.items.iter().map(|item| item.id).collect();
    assert_eq!(ids, vec![1, 2, 3], "duplicate ids must collapse");
    assert_eq!(outcome.readiness, HomeReadiness::Complete);
}

#[tokio::test]
async fn incomplete_results_marks_partial() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(body(5, true, vec![item(1, 1, false)]), None)))
        .await;

    let outcome = run(
        &mock,
        &HomeQuery::new(login("test-user")),
        &HomeSearchLimits::default(),
    )
    .await
    .expect("ok");
    assert!(outcome.incomplete_results);
    assert!(matches!(
        outcome.readiness,
        HomeReadiness::Partial { ref reasons }
            if reasons.contains(&HomePartialReason::IncompleteResults)
    ));
}

#[tokio::test]
async fn local_page_limit_marks_partial() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(
        body(5, false, vec![item(1, 1, false)]),
        Some("<https://api.github.com/search/issues?page=2>; rel=\"next\""),
    )))
    .await;

    let limits = HomeSearchLimits {
        max_pages: 1,
        ..HomeSearchLimits::default()
    };
    let outcome = run(&mock, &HomeQuery::new(login("test-user")), &limits)
        .await
        .expect("ok");
    assert_eq!(outcome.items.len(), 1);
    assert!(matches!(
        outcome.readiness,
        HomeReadiness::Partial { ref reasons }
            if reasons.contains(&HomePartialReason::PageLimit)
    ));
}

#[tokio::test]
async fn service_result_limit_marks_partial() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(body(1500, false, vec![item(1, 1, false)]), None)))
        .await;

    let outcome = run(
        &mock,
        &HomeQuery::new(login("test-user")),
        &HomeSearchLimits::default(),
    )
    .await
    .expect("ok");
    assert!(matches!(
        outcome.readiness,
        HomeReadiness::Partial { ref reasons }
            if reasons.contains(&HomePartialReason::ResultLimit)
    ));
}

#[tokio::test]
async fn empty_result_is_complete_not_error() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(body(0, false, vec![]), None))).await;

    let outcome = run(
        &mock,
        &HomeQuery::new(login("test-user")),
        &HomeSearchLimits::default(),
    )
    .await
    .expect("ok");
    assert!(outcome.items.is_empty());
    assert_eq!(outcome.total_count, 0);
    assert_eq!(outcome.readiness, HomeReadiness::Complete);
}

#[tokio::test]
async fn unauthorized_is_a_hard_error() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(HttpResponse {
        status: 401,
        headers: vec![],
        body: "{}".to_string(),
    }))
    .await;

    let error = run(
        &mock,
        &HomeQuery::new(login("test-user")),
        &HomeSearchLimits::default(),
    )
    .await
    .expect_err("401 must fail");
    assert!(matches!(
        error,
        HomeError::GitHub(GitHubError::Unauthorized)
    ));
}

#[tokio::test]
async fn rate_limited_is_a_hard_error() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(HttpResponse {
        status: 403,
        headers: vec![("x-ratelimit-remaining".to_string(), "0".to_string())],
        body: "{}".to_string(),
    }))
    .await;

    let error = run(
        &mock,
        &HomeQuery::new(login("test-user")),
        &HomeSearchLimits::default(),
    )
    .await
    .expect_err("rate limit must fail");
    assert!(matches!(
        error,
        HomeError::GitHub(GitHubError::RateLimited { .. })
    ));
}

#[tokio::test]
async fn later_page_failure_keeps_earlier_items() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(
        body(5, false, vec![item(1, 1, false)]),
        Some("<https://api.github.com/search/issues?page=2>; rel=\"next\""),
    )))
    .await;
    mock.enqueue(Ok(HttpResponse {
        status: 503,
        headers: vec![],
        body: "{}".to_string(),
    }))
    .await;

    let outcome = run(
        &mock,
        &HomeQuery::new(login("test-user")),
        &HomeSearchLimits::default(),
    )
    .await
    .expect("partial outcome");
    assert_eq!(outcome.items.len(), 1);
    assert!(matches!(
        outcome.readiness,
        HomeReadiness::Partial { ref reasons }
            if reasons.iter().any(|r| matches!(r, HomePartialReason::PageFailed { .. }))
    ));
}

#[tokio::test]
async fn untrusted_next_link_is_partial() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(
        body(5, false, vec![item(1, 1, false)]),
        Some("<https://evil.example/search/issues?page=2>; rel=\"next\""),
    )))
    .await;

    let outcome = run(
        &mock,
        &HomeQuery::new(login("test-user")),
        &HomeSearchLimits::default(),
    )
    .await
    .expect("partial outcome");
    assert!(matches!(
        outcome.readiness,
        HomeReadiness::Partial { ref reasons }
            if reasons.contains(&HomePartialReason::UntrustedPagination)
    ));
}

#[tokio::test]
async fn pagination_cycle_is_partial() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(
        body(5, false, vec![item(1, 1, false)]),
        Some("<https://api.github.com/search/issues?page=1>; rel=\"next\""),
    )))
    .await;

    let outcome = run(
        &mock,
        &HomeQuery::new(login("test-user")),
        &HomeSearchLimits::default(),
    )
    .await
    .expect("partial outcome");
    assert!(matches!(
        outcome.readiness,
        HomeReadiness::Partial { ref reasons }
            if reasons.contains(&HomePartialReason::PaginationCycle)
    ));
}

#[tokio::test]
async fn next_link_cannot_change_the_query_identity() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(
        body(5, false, vec![item(1, 1, false)]),
        Some(
            "<https://api.github.com/search/issues?q=involves%3Aattacker&sort=created&page=2>; rel=\"next\"",
        ),
    )))
    .await;
    mock.enqueue(Ok(ok(body(5, false, vec![item(2, 2, false)]), None)))
        .await;

    let outcome = run(
        &mock,
        &HomeQuery::new(login("test-user")),
        &HomeSearchLimits::default(),
    )
    .await
    .expect("ok");
    assert_eq!(outcome.items.len(), 2);
    let recorded = mock.recorded().await;
    assert_eq!(recorded.len(), 2);
    // The second request must still use our query, not the link's.
    assert_eq!(
        query_of(&recorded[1].0, "q").as_deref(),
        Some("involves:test-user")
    );
    assert_eq!(query_of(&recorded[1].0, "sort").as_deref(), Some("updated"));
    assert_eq!(query_of(&recorded[1].0, "page").as_deref(), Some("2"));
}

#[tokio::test]
async fn cancellation_is_a_hard_error() {
    let mock = Arc::new(MockTransport::new());
    let (cancel, source) = CancellationToken::new();
    source.cancel();

    let result = search_involving(
        &client(mock.clone()),
        &pat(),
        &HomeQuery::new(login("test-user")),
        &HomeSearchLimits::default(),
        cancel,
    )
    .await;
    assert!(matches!(
        result,
        Err(HomeError::GitHub(GitHubError::Cancelled))
    ));
    assert!(
        mock.recorded().await.is_empty(),
        "cancelled call must not connect"
    );
}

#[test]
fn invalid_logins_are_rejected() {
    for bad in [
        "",
        "bad login",
        "bad:user",
        "@me",
        "has_underscore",
        "-lead",
        "trail-",
        "a".repeat(40).as_str(),
    ] {
        assert!(Login::new(bad).is_err(), "{bad:?} must be rejected");
    }
    assert!(Login::new("test-user").is_ok());
}

#[test]
fn updated_window_requires_at_least_one_bound() {
    assert!(UpdatedWindow::new(None, None).is_err());
    let from = DateBound::new("2024-06-01").unwrap();
    let to = DateBound::new("2024-01-01").unwrap();
    assert!(UpdatedWindow::new(Some(from.clone()), Some(to)).is_err());
    assert!(UpdatedWindow::new(Some(from), None).is_ok());
}

#[test]
fn invalid_dates_are_rejected() {
    for bad in [
        "2024-6-1",
        "2024/01/01",
        "2024-13-01",
        "2024-00-10",
        "not-a-date",
    ] {
        assert!(DateBound::new(bad).is_err(), "{bad:?} must be rejected");
    }
    assert!(DateBound::new("2024-02-29").is_ok());
}

#[tokio::test]
async fn malformed_repository_url_is_invalid_response() {
    let mut raw = item(1, 1, false);
    raw["repository_url"] = serde_json::json!("not a url");
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(body(1, false, vec![raw]), None))).await;

    let error = run(
        &mock,
        &HomeQuery::new(login("test-user")),
        &HomeSearchLimits::default(),
    )
    .await
    .expect_err("bad repository_url must fail");
    assert!(matches!(error, HomeError::InvalidResponse { .. }));
}

#[tokio::test]
async fn invalid_limits_are_rejected_without_network() {
    let mock = Arc::new(MockTransport::new());
    let limits = HomeSearchLimits {
        per_page: 0,
        ..HomeSearchLimits::default()
    };
    let error = run(&mock, &HomeQuery::new(login("test-user")), &limits)
        .await
        .expect_err("per_page 0 must fail");
    assert!(matches!(
        error,
        HomeError::InvalidInput { field: "per_page" }
    ));
    assert!(mock.recorded().await.is_empty());
}
