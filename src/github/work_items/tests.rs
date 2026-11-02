use super::*;
use crate::{
    domain::home::Login,
    github::GitHubConfig,
    network::transport::{MockTransport, json_response},
};
use serde_json::json;
use std::sync::Arc;
async fn mock(responses: Vec<crate::network::transport::HttpResponse>) -> Arc<MockTransport> {
    let transport = Arc::new(MockTransport::new());
    transport
        .enqueue_many(responses.into_iter().map(Ok).collect())
        .await;
    transport
}
fn query(kind: WorkItemKind) -> WorkItemQuery {
    WorkItemQuery::new(kind, Login::new("test-user").unwrap())
}
fn item(id: u64, pr: bool, merged: bool) -> serde_json::Value {
    let mut item = json!({"id":id,"number":42,"title":"Example","state":"closed","updated_at":"2026-11-01T00:00:00Z","repository_url":"https://api.github.com/repos/acme/repo"});
    if pr {
        item["pull_request"] =
            json!({"merged_at":if merged { Some("2026-10-31T00:00:00Z") } else { None }});
    }
    item
}
async fn fetch(
    response: crate::network::transport::HttpResponse,
    query: &WorkItemQuery,
) -> Result<WorkItemPage, WorkItemError> {
    let transport = mock(vec![response]).await;
    let client = GitHubClient::new(GitHubConfig::default(), transport);
    fetch_page(
        &client,
        &Pat::from_str("synthetic"),
        query,
        None,
        &WorkItemLimits::default(),
        CancellationToken::default(),
    )
    .await
}
#[test]
fn exact_queries_cover_every_filter_and_never_involves() {
    for kind in [WorkItemKind::Issue, WorkItemKind::PullRequest] {
        for relation in [Relation::Assigned, Relation::Created] {
            for state in [StateFilter::Open, StateFilter::Closed, StateFilter::All] {
                let mut q = query(kind);
                q.relation = relation;
                q.state = state;
                let expected = format!(
                    "is:{} {}:test-user{}",
                    if kind == WorkItemKind::Issue {
                        "issue"
                    } else {
                        "pr"
                    },
                    if relation == Relation::Assigned {
                        "assignee"
                    } else {
                        "author"
                    },
                    match state {
                        StateFilter::Open => " is:open",
                        StateFilter::Closed => " is:closed",
                        StateFilter::All => "",
                    }
                );
                assert_eq!(q.expression(), expected);
                assert!(!q.expression().contains("involves"));
            }
        }
    }
    assert!(Login::new("test-user is:pr").is_err());
}
#[test]
fn mention_or_authorship_alone_does_not_match_assigned() {
    // Counterexample to the Home involves relation: this user authored and
    // commented on the issue but somebody else is the sole assignee.
    let authored_by = "test-user";
    let commented_by = "test-user";
    let assignees = ["another-user"];
    let q = query(WorkItemKind::Issue);
    assert!(authored_by == q.login().as_str() || commented_by == q.login().as_str());
    assert!(!assignees.contains(&q.login().as_str()));
    assert_eq!(q.expression(), "is:issue assignee:test-user is:open");
}
#[tokio::test]
async fn merged_and_unmerged_closures_are_distinct_and_ids_dedup() {
    let response = json_response(200, &json!({"total_count":3,"incomplete_results":false,"items":[item(1,true,true),item(2,true,false),item(1,true,true)]}).to_string());
    let page = fetch(response, &query(WorkItemKind::PullRequest))
        .await
        .unwrap();
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items[0].state, ItemState::Merged);
    assert_eq!(page.items[1].state, ItemState::Closed);
    assert_eq!(page.items[0].repository.full_name(), "acme/repo");
    let mut missing = item(3, true, false);
    missing["pull_request"] = json!({});
    let response = json_response(
        200,
        &json!({"total_count":1,"incomplete_results":false,"items":[missing]}).to_string(),
    );
    let page = fetch(response, &query(WorkItemKind::PullRequest))
        .await
        .unwrap();
    assert_eq!(
        page.items[0].state,
        ItemState::Unknown,
        "missing merge metadata is not an unmerged closure"
    );
}
#[tokio::test]
async fn one_page_cursor_rebuilds_own_query_and_rejects_filter_origin_size_changes() {
    let mut first = json_response(
        200,
        &json!({"total_count":2,"incomplete_results":false,"items":[item(1,false,false)]})
            .to_string(),
    );
    first.headers.push((
        "link".into(),
        "<https://api.github.com/search/issues?q=involves:someone-else&page=2>; rel=\"next\""
            .into(),
    ));
    let last = json_response(
        200,
        &json!({"total_count":2,"incomplete_results":false,"items":[item(2,false,false)]})
            .to_string(),
    );
    let transport = mock(vec![first, last]).await;
    let client = GitHubClient::new(GitHubConfig::default(), transport.clone());
    let q = query(WorkItemKind::Issue);
    let pat = Pat::from_str("synthetic");
    let limits = WorkItemLimits::default();
    let page = fetch_page(
        &client,
        &pat,
        &q,
        None,
        &limits,
        CancellationToken::default(),
    )
    .await
    .unwrap();
    assert_eq!(transport.recorded_requests().await.len(), 1);
    let cursor = page.next.unwrap();
    let mut changed = q.clone();
    changed.relation = Relation::Created;
    assert!(
        fetch_page(
            &client,
            &pat,
            &changed,
            Some(&cursor),
            &limits,
            CancellationToken::default()
        )
        .await
        .is_err()
    );
    assert!(
        fetch_page(
            &client,
            &pat,
            &q,
            Some(&cursor),
            &WorkItemLimits {
                per_page: 20,
                ..limits.clone()
            },
            CancellationToken::default()
        )
        .await
        .is_err()
    );
    fetch_page(
        &client,
        &pat,
        &q,
        Some(&cursor),
        &limits,
        CancellationToken::default(),
    )
    .await
    .unwrap();
    let url = Url::parse(&transport.recorded_requests().await[1].url).unwrap();
    assert_eq!(
        url.query_pairs().find(|(key, _)| key == "q").unwrap().1,
        q.expression()
    );
    assert_eq!(
        url.query_pairs().find(|(key, _)| key == "sort").unwrap().1,
        "updated"
    );
    assert_eq!(
        url.query_pairs().find(|(key, _)| key == "order").unwrap().1,
        "desc"
    );
}
#[tokio::test]
async fn partial_empty_is_unknown_and_pagination_is_bounded() {
    for (link, reason) in [
        (
            "https://evil.example/search/issues?page=2",
            PartialReason::UntrustedPagination,
        ),
        (
            "https://api.github.com/search/issues?page=1",
            PartialReason::PaginationCycle,
        ),
        (
            "https://api.github.com/search/issues?page=3",
            PartialReason::InvalidPagination,
        ),
        (
            "https://api.github.com/other/search/issues?page=2",
            PartialReason::InvalidPagination,
        ),
        (
            "https://api.github.com/search/issues?page=2&page=3",
            PartialReason::InvalidPagination,
        ),
    ] {
        let mut response = json_response(
            200,
            "{\"total_count\":1200,\"incomplete_results\":true,\"items\":[]}",
        );
        response
            .headers
            .push(("link".into(), format!("<{link}>; rel=\"next\"")));
        response.headers.push((
            "x-github-sso".into(),
            "partial-results; organizations=42".into(),
        ));
        let page = fetch(response, &query(WorkItemKind::Issue)).await.unwrap();
        assert!(page.next.is_none());
        assert!(page.partial.contains(&reason));
        assert!(page.partial.contains(&PartialReason::ResultLimit));
        assert!(page.partial.contains(&PartialReason::IncompleteResults));
        assert!(
            page.partial
                .iter()
                .any(|reason| matches!(reason, PartialReason::SsoPartial { .. }))
        );
    }
    let complete = fetch(
        json_response(
            200,
            "{\"total_count\":0,\"incomplete_results\":false,\"items\":[]}",
        ),
        &query(WorkItemKind::Issue),
    )
    .await
    .unwrap();
    assert!(complete.partial.is_empty());
}
#[tokio::test]
async fn thousand_boundary_and_typed_failures() {
    let q = query(WorkItemKind::Issue);
    let mut response = json_response(
        200,
        "{\"total_count\":1000,\"incomplete_results\":false,\"items\":[]}",
    );
    response.headers.push((
        "link".into(),
        "<https://api.github.com/search/issues?page=11>; rel=\"next\"".into(),
    ));
    let client = GitHubClient::new(Default::default(), mock(vec![response]).await);
    let limits = WorkItemLimits {
        per_page: 100,
        ..Default::default()
    };
    let cursor = WorkItemCursor {
        query: q.clone(),
        page: 10,
        per_page: 100,
        origin: "https://api.github.com/search/issues".into(),
    };
    let page = fetch_page(
        &client,
        &Pat::from_str("synthetic"),
        &q,
        Some(&cursor),
        &limits,
        CancellationToken::default(),
    )
    .await
    .unwrap();
    assert_eq!(page.partial, vec![PartialReason::ResultLimit]);
    assert!(page.next.is_none());
    for (status, error) in [
        (401, GitHubError::Unauthorized),
        (403, GitHubError::Forbidden),
        (422, GitHubError::UnexpectedStatus { status: 422 }),
        (503, GitHubError::UnexpectedStatus { status: 503 }),
    ] {
        assert_eq!(
            fetch(json_response(status, "{}"), &q).await,
            Err(error.into())
        );
    }
    let mut response = json_response(429, "{}");
    response.headers.push(("retry-after".into(), "30".into()));
    assert!(matches!(
        fetch(response, &q).await,
        Err(WorkItemError::GitHub(GitHubError::RateLimited { .. }))
    ));
}
#[tokio::test]
async fn cancellation_bounds_parse_and_sso_are_not_empty_results() {
    let q = query(WorkItemKind::Issue);
    let transport = Arc::new(MockTransport::new());
    let client = GitHubClient::new(Default::default(), transport.clone());
    let (cancel, source) = CancellationToken::new();
    source.cancel();
    assert_eq!(
        fetch_page(
            &client,
            &Pat::from_str("synthetic"),
            &q,
            None,
            &Default::default(),
            cancel
        )
        .await,
        Err(GitHubError::Cancelled.into())
    );
    assert!(transport.recorded_requests().await.is_empty());
    transport
        .enqueue(Ok(json_response(200, "{\"total_count\":0,\"items\":[]}")))
        .await;
    assert!(matches!(
        fetch_page(
            &client,
            &Pat::from_str("synthetic"),
            &q,
            None,
            &Default::default(),
            Default::default()
        )
        .await,
        Err(WorkItemError::GitHub(GitHubError::Parse { .. }))
    ));
    let mut sso = json_response(403, "{}");
    sso.headers.push((
        "x-github-sso".into(),
        "required; url=https://github.com/orgs/acme/sso".into(),
    ));
    assert!(matches!(
        fetch(sso, &q).await,
        Err(WorkItemError::GitHub(GitHubError::SsoRequired { .. }))
    ));
    let page = fetch(
        json_response(
            200,
            "{\"total_count\":1,\"incomplete_results\":false,\"items\":[]}",
        ),
        &q,
    )
    .await
    .unwrap();
    assert!(
        !page.partial.is_empty(),
        "positive total with empty page is unknown, not empty"
    );
    transport.enqueue(Ok(json_response(200, "{}"))).await;
    assert!(matches!(
        fetch_page(
            &client,
            &Pat::from_str("synthetic"),
            &q,
            None,
            &WorkItemLimits {
                response_bytes: 1,
                ..Default::default()
            },
            Default::default()
        )
        .await,
        Err(WorkItemError::GitHub(GitHubError::ResponseTooLarge { .. }))
    ));
}
#[tokio::test]
async fn malicious_repository_urls_never_become_navigation() {
    for url in [
        "https://evil.example/repos/acme/repo",
        "https://evil.example\\@api.github.com/repos/acme/repo",
        "https://api.github.com:8443/repos/acme/repo",
        "https://api.github.com/repos/acme/repo?token=x",
    ] {
        let mut raw = item(1, false, false);
        raw["repository_url"] = json!(url);
        let response = json_response(
            200,
            &json!({"total_count":1,"incomplete_results":false,"items":[raw]}).to_string(),
        );
        assert!(fetch(response, &query(WorkItemKind::Issue)).await.is_err());
    }
}
