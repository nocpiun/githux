//! Offline fixtures for the repository pull request list endpoint.
//!
//! Every case is a synthetic, credential-free response. The continuation shape is the one
//! observed live on 2026-11-26: `/repositories/{id}/pulls` with only
//! `state`/`per_page`/`page`, never an echo of this client's own request.

use super::*;
use crate::domain::repository::RepositoryLocator;
use crate::github::client::GitHubConfig;
use crate::network::transport::{MockTransport, json_response};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

const REPOSITORY_ID: u64 = 42;

fn identity() -> RepositoryIdentity {
    RepositoryIdentity {
        id: REPOSITORY_ID,
        locator: RepositoryLocator::new("octo", "repo").unwrap(),
    }
}

fn other_identity() -> RepositoryIdentity {
    RepositoryIdentity {
        id: 43,
        locator: RepositoryLocator::new("octo", "repo").unwrap(),
    }
}

fn token() -> Pat {
    Pat::from_str("synthetic-offline-fixture")
}

fn mock_transport() -> Arc<MockTransport> {
    Arc::new(MockTransport::new())
}

fn client(mock: Arc<MockTransport>) -> GitHubClient {
    GitHubClient::new(GitHubConfig::default(), mock)
}

fn pull(
    id: u64,
    number: u64,
    state: &str,
    draft: bool,
    merged_at: Option<&str>,
    title: &str,
) -> Value {
    json!({
        "id": id,
        "number": number,
        "title": title,
        "state": state,
        "draft": draft,
        "merged_at": merged_at,
        "user": {"login": "octocat"},
        "head": {"ref": "feature/pr-list"},
        "base": {"ref": "main"},
        "created_at": "2026-11-01T00:00:00Z",
        "updated_at": "2026-11-20T00:00:00Z"
    })
}

fn page(pulls: Vec<Value>, next: Option<&str>) -> crate::network::transport::HttpResponse {
    let mut response = json_response(200, &Value::Array(pulls).to_string());
    if let Some(next) = next {
        response
            .headers
            .push(("link".into(), format!("<{next}>; rel=\"next\"")));
    }
    response
}

async fn fetch(
    mock: &Arc<MockTransport>,
    filter: PullFilter,
    cursor: Option<&PullPageCursor>,
    limits: &PullListLimits,
) -> Result<PullListPage, RepositoryError> {
    fetch_page(
        &client(mock.clone()),
        &token(),
        &identity(),
        filter,
        cursor,
        limits,
        CancellationToken::default(),
    )
    .await
}

fn query_map(url: &str) -> BTreeMap<String, String> {
    Url::parse(url)
        .expect("recorded URL")
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect()
}

/// The continuation exactly as GitHub returns it for this repository.
const NEXT_PAGE_2: &str =
    "https://api.github.com/repositories/42/pulls?state=open&per_page=30&page=2";

#[tokio::test]
async fn requests_the_repository_endpoint_with_an_explicit_state_and_order() {
    let mock = mock_transport();
    mock.enqueue(Ok(page(vec![], None))).await;
    let page = fetch(&mock, PullFilter::Open, None, &PullListLimits::default())
        .await
        .unwrap();
    assert!(page.pulls.is_empty());
    assert_eq!(page.page, 1);
    let recorded = mock.recorded_requests().await;
    assert_eq!(recorded.len(), 1);
    let url = Url::parse(&recorded[0].url).unwrap();
    assert_eq!(url.path(), "/repos/octo/repo/pulls");
    assert_eq!(
        query_map(&recorded[0].url),
        BTreeMap::from([
            ("direction".into(), "desc".into()),
            ("page".into(), "1".into()),
            ("per_page".into(), "30".into()),
            ("sort".into(), "created".into()),
            ("state".into(), "open".into()),
        ])
    );
}

#[tokio::test]
async fn open_draft_merged_and_closed_without_merge_stay_distinct() {
    let mock = mock_transport();
    mock.enqueue(Ok(page(
        vec![
            pull(1, 11, "open", false, None, "Open change"),
            pull(2, 12, "open", true, None, "Draft change"),
            pull(
                3,
                13,
                "closed",
                false,
                Some("2026-11-02T00:00:00Z"),
                "Merged change",
            ),
            pull(4, 14, "closed", false, None, "Abandoned change"),
            pull(5, 15, "closed", true, None, "Closed draft"),
        ],
        None,
    )))
    .await;
    let page = fetch(&mock, PullFilter::Closed, None, &PullListLimits::default())
        .await
        .unwrap();
    let states: Vec<_> = page.pulls.iter().map(|pull| pull.state).collect();
    assert_eq!(
        states,
        vec![
            PullState::Open,
            PullState::Draft,
            PullState::Merged,
            PullState::Closed,
            PullState::Closed,
        ]
    );
    assert_eq!(page.pulls[2].state.label(), "Merged");
    assert!(page.pulls[0].state.is_open());
    assert!(!page.pulls[3].state.is_open());
    assert_eq!(page.pulls[2].author.as_deref(), Some("octocat"));
    assert_eq!(page.completeness, Completeness::Complete);
}

#[tokio::test]
async fn unknown_state_and_open_with_merged_at_are_invalid_responses() {
    for body in [
        json!({"state": "draft"}),
        json!({"state": "open", "merged_at": "2026-11-02T00:00:00Z"}),
    ] {
        let mut item = pull(1, 1, "open", false, None, "Example");
        item["state"] = body["state"].clone();
        item["merged_at"] = body["merged_at"].clone();
        let mock = mock_transport();
        mock.enqueue(Ok(page(vec![item], None))).await;
        assert!(matches!(
            fetch(&mock, PullFilter::Open, None, &PullListLimits::default()).await,
            Err(RepositoryError::InvalidResponse { .. })
        ));
    }
}

#[tokio::test]
async fn a_real_empty_page_is_a_complete_success() {
    let mock = mock_transport();
    mock.enqueue(Ok(page(vec![], None))).await;
    let page = fetch(&mock, PullFilter::Closed, None, &PullListLimits::default())
        .await
        .unwrap();
    assert!(page.pulls.is_empty());
    assert!(page.next.is_none());
    assert_eq!(page.completeness, Completeness::Complete);
}

#[tokio::test]
async fn the_by_id_continuation_is_recognised_and_the_next_request_is_rebuilt() {
    let mock = mock_transport();
    mock.enqueue(Ok(page(
        vec![pull(1, 1, "open", false, None, "One")],
        Some(NEXT_PAGE_2),
    )))
    .await;
    let first = fetch(&mock, PullFilter::Open, None, &PullListLimits::default())
        .await
        .unwrap();
    let cursor = first
        .next
        .expect("the by-id continuation is a valid next page");
    assert_eq!(cursor.page(), 2);
    assert_eq!(cursor.per_page(), 30);
    assert_eq!(cursor.repository_id(), REPOSITORY_ID);
    assert_eq!(cursor.query(), &first.query);

    mock.enqueue(Ok(page(vec![pull(2, 2, "open", false, None, "Two")], None)))
        .await;
    let second = fetch(
        &mock,
        PullFilter::Open,
        Some(&cursor),
        &PullListLimits::default(),
    )
    .await
    .unwrap();
    assert_eq!(second.page, 2);
    assert_eq!(second.pulls[0].number, 2);
    assert!(second.next.is_none());

    let recorded = mock.recorded_requests().await;
    assert_eq!(recorded.len(), 2);
    // Never fetched as given: the second request goes back to the repository path and
    // restores the sort order the continuation dropped.
    let url = Url::parse(&recorded[1].url).unwrap();
    assert_eq!(url.path(), "/repos/octo/repo/pulls");
    assert_eq!(
        query_map(&recorded[1].url),
        BTreeMap::from([
            ("direction".into(), "desc".into()),
            ("page".into(), "2".into()),
            ("per_page".into(), "30".into()),
            ("sort".into(), "created".into()),
            ("state".into(), "open".into()),
        ])
    );
}

#[tokio::test]
async fn a_cursor_cannot_be_reused_for_another_filter_size_repository_or_page_one() {
    let mock = mock_transport();
    mock.enqueue(Ok(page(
        vec![pull(1, 1, "open", false, None, "One")],
        Some(NEXT_PAGE_2),
    )))
    .await;
    let first = fetch(&mock, PullFilter::Open, None, &PullListLimits::default())
        .await
        .unwrap();
    let cursor = first.next.expect("next page");

    let wrong_filter = fetch(
        &mock,
        PullFilter::Closed,
        Some(&cursor),
        &PullListLimits::default(),
    )
    .await;
    assert_eq!(
        wrong_filter,
        Err(RepositoryError::InvalidInput {
            field: "pull cursor identity"
        })
    );
    let limits = PullListLimits {
        per_page: 50,
        ..PullListLimits::default()
    };
    assert_eq!(
        fetch(&mock, PullFilter::Open, Some(&cursor), &limits).await,
        Err(RepositoryError::InvalidInput {
            field: "pull cursor identity"
        })
    );
    let other = fetch_page(
        &client(mock.clone()),
        &token(),
        &other_identity(),
        PullFilter::Open,
        Some(&cursor),
        &PullListLimits::default(),
        CancellationToken::default(),
    )
    .await;
    assert_eq!(
        other,
        Err(RepositoryError::InvalidInput {
            field: "pull cursor identity"
        })
    );
    // Neither rejected call reached the network.
    assert_eq!(mock.recorded_requests().await.len(), 1);
}

#[tokio::test]
async fn a_continuation_may_not_change_repository_filter_order_size_or_page() {
    let cases = [
        (
            "https://evil.invalid/repositories/42/pulls?state=open&page=2",
            IncompleteReason::UntrustedPagination,
        ),
        (
            "https://api.github.com/repositories/43/pulls?state=open&page=2",
            IncompleteReason::InvalidPagination,
        ),
        (
            "https://api.github.com/repos/octo/other/pulls?state=open&page=2",
            IncompleteReason::InvalidPagination,
        ),
        (
            "https://api.github.com/repositories/42/pulls?state=closed&page=2",
            IncompleteReason::InvalidPagination,
        ),
        (
            "https://api.github.com/repositories/42/pulls?state=open&sort=updated&page=2",
            IncompleteReason::InvalidPagination,
        ),
        (
            "https://api.github.com/repositories/42/pulls?state=open&per_page=50&page=2",
            IncompleteReason::InvalidPagination,
        ),
        (
            "https://api.github.com/repositories/42/pulls?state=open&page=1",
            IncompleteReason::PaginationCycle,
        ),
        (
            "https://api.github.com/repositories/42/pulls?state=open&page=3",
            IncompleteReason::InvalidPagination,
        ),
        (
            "https://api.github.com/repositories/42/pulls?state=open&page=2&base=main",
            IncompleteReason::InvalidPagination,
        ),
        (
            "https://api.github.com/repositories/42/pulls?state=open",
            IncompleteReason::InvalidPagination,
        ),
        (
            "https://api.github.com/repositories/42/pulls/9?state=open&page=2",
            IncompleteReason::InvalidPagination,
        ),
    ];
    for (link, expected) in cases {
        let mock = mock_transport();
        mock.enqueue(Ok(page(
            vec![pull(1, 1, "open", false, None, "One")],
            Some(link),
        )))
        .await;
        let page = fetch(&mock, PullFilter::Open, None, &PullListLimits::default())
            .await
            .unwrap();
        assert!(page.next.is_none(), "{link}");
        // The rows already read are kept; only the continuation is rejected.
        assert_eq!(page.pulls.len(), 1, "{link}");
        assert_eq!(
            page.completeness,
            Completeness::Partial {
                reasons: vec![expected]
            },
            "{link}"
        );
    }
}

#[tokio::test]
async fn an_unparsable_next_entry_is_partial_instead_of_silently_last() {
    let mock = mock_transport();
    let mut response = page(vec![pull(1, 1, "open", false, None, "One")], None);
    response
        .headers
        .push(("link".into(), "garbage; rel=\"next\"".into()));
    mock.enqueue(Ok(response)).await;
    let page = fetch(&mock, PullFilter::Open, None, &PullListLimits::default())
        .await
        .unwrap();
    assert!(page.next.is_none());
    assert_eq!(page.pulls.len(), 1);
    assert_eq!(
        page.completeness,
        Completeness::Partial {
            reasons: vec![IncompleteReason::InvalidPagination]
        }
    );
}

#[tokio::test]
async fn sso_partial_results_are_reported_without_dropping_rows() {
    let mock = mock_transport();
    let mut response = page(vec![pull(1, 1, "open", false, None, "One")], None);
    response.headers.push((
        "x-github-sso".into(),
        "partial-results; organizations=acme".into(),
    ));
    mock.enqueue(Ok(response)).await;
    let page = fetch(&mock, PullFilter::Open, None, &PullListLimits::default())
        .await
        .unwrap();
    assert_eq!(page.pulls.len(), 1);
    assert_eq!(
        page.completeness,
        Completeness::Partial {
            reasons: vec![IncompleteReason::SsoPartial {
                organizations: vec!["acme".into()]
            }]
        }
    );
}

#[tokio::test]
async fn failed_responses_keep_their_typed_error_instead_of_returning_rows() {
    for (status, expected) in [
        (401, GitHubError::Unauthorized),
        (404, GitHubError::NotFound),
        (500, GitHubError::UnexpectedStatus { status: 500 }),
    ] {
        let mock = mock_transport();
        mock.enqueue(Ok(json_response(status, "{}"))).await;
        assert_eq!(
            fetch(&mock, PullFilter::Open, None, &PullListLimits::default()).await,
            Err(RepositoryError::GitHub(expected)),
            "{status}"
        );
    }
    let mock = mock_transport();
    let mut limited = json_response(403, "{}");
    limited.headers.push(("retry-after".into(), "45".into()));
    mock.enqueue(Ok(limited)).await;
    assert!(matches!(
        fetch(&mock, PullFilter::Open, None, &PullListLimits::default()).await,
        Err(RepositoryError::GitHub(GitHubError::RateLimited { .. }))
    ));
}

#[tokio::test]
async fn malformed_or_oversized_payloads_are_rejected() {
    let mock = mock_transport();
    mock.enqueue(Ok(json_response(200, "not json"))).await;
    assert!(matches!(
        fetch(&mock, PullFilter::Open, None, &PullListLimits::default()).await,
        Err(RepositoryError::GitHub(GitHubError::Parse { .. }))
    ));

    let mock = mock_transport();
    mock.enqueue(Ok(page(
        vec![
            pull(1, 1, "open", false, None, "One"),
            pull(2, 2, "open", false, None, "Two"),
            pull(3, 3, "open", false, None, "Three"),
        ],
        None,
    )))
    .await;
    let limits = PullListLimits {
        per_page: 2,
        ..PullListLimits::default()
    };
    assert_eq!(
        fetch(&mock, PullFilter::Open, None, &limits).await,
        Err(RepositoryError::InvalidResponse {
            message: "oversized pull page".into()
        })
    );

    let mock = mock_transport();
    mock.enqueue(Ok(page(
        vec![
            pull(1, 1, "open", false, None, "One"),
            pull(1, 2, "open", false, None, "Same id"),
        ],
        None,
    )))
    .await;
    assert_eq!(
        fetch(&mock, PullFilter::Open, None, &PullListLimits::default()).await,
        Err(RepositoryError::InvalidResponse {
            message: "duplicate pull request in one page".into()
        })
    );

    let mock = mock_transport();
    mock.enqueue(Ok(json_response(200, "[]"))).await;
    let limits = PullListLimits {
        per_page: 30,
        response_bytes: 1,
    };
    assert!(matches!(
        fetch(&mock, PullFilter::Open, None, &limits).await,
        Err(RepositoryError::GitHub(
            GitHubError::ResponseTooLarge { .. }
        ))
    ));
}

#[tokio::test]
async fn cancellation_never_consumes_a_scripted_response_or_returns_rows() {
    let mock = mock_transport();
    mock.enqueue(Ok(page(vec![pull(1, 1, "open", false, None, "One")], None)))
        .await;
    let (cancel, source) = CancellationToken::new();
    source.cancel();
    let result = fetch_page(
        &client(mock.clone()),
        &token(),
        &identity(),
        PullFilter::Open,
        None,
        &PullListLimits::default(),
        cancel,
    )
    .await;
    assert_eq!(result, Err(RepositoryError::GitHub(GitHubError::Cancelled)));
    assert!(mock.recorded_requests().await.is_empty());
}

#[tokio::test]
async fn invalid_limits_are_rejected_before_building_a_request() {
    for limits in [
        PullListLimits {
            per_page: 0,
            response_bytes: 1024,
        },
        PullListLimits {
            per_page: 101,
            response_bytes: 1024,
        },
        PullListLimits {
            per_page: 30,
            response_bytes: 0,
        },
    ] {
        let mock = mock_transport();
        assert_eq!(
            fetch(&mock, PullFilter::Open, None, &limits).await,
            Err(RepositoryError::InvalidInput {
                field: "pull list limits"
            })
        );
        assert!(mock.recorded_requests().await.is_empty());
    }
}
