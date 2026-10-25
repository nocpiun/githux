use super::*;
use crate::domain::repository::{RepositoryError, RepositoryIdentity, RepositoryLocator};
use crate::domain::repository_extras::{
    ContributorKind, ContributorsState, LanguageStat, LanguagesOutcome,
};
use crate::github::client::GitHubConfig;
use crate::network::test_server::{LocalHttpServer, ScriptedResponse};
use crate::network::transport::{HttpRequest, Transport, TransportError};
use crate::network::transport::{MockFailure, MockTransport, ReqwestTransport};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

fn repository() -> RepositoryIdentity {
    RepositoryIdentity {
        id: 42,
        locator: RepositoryLocator::new("owner", "repo").unwrap(),
    }
}

fn client(transport: Arc<MockTransport>) -> GitHubClient {
    GitHubClient::new(GitHubConfig::default(), transport)
}

fn token() -> Pat {
    Pat::from_str("ghp_synthetic_only")
}

fn response(status: u16, body: &str) -> HttpResponse {
    HttpResponse {
        status,
        headers: vec![],
        body: body.to_owned(),
    }
}

fn release(id: u64, draft: bool, prerelease: bool) -> String {
    serde_json::json!({
        "id": id,
        "node_id": format!("R{id}"),
        "tag_name": format!("v{id}.0.0"),
        "name": if draft { None } else { Some("A release") },
        "body": "notes",
        "html_url": format!("https://github.com/owner/repo/releases/{id}"),
        "draft": draft,
        "prerelease": prerelease,
        "created_at": "2026-01-01T00:00:00Z",
        "published_at": if draft { None } else { Some("2026-01-02T00:00:00Z") },
        "assets": [{"id": id + 1000, "browser_download_url": "https://evil.invalid/do-not-fetch"}],
        "future_field": {"preserved": true}
    })
    .to_string()
}

fn next(page: u32) -> (String, String) {
    (
        "link".to_owned(),
        format!(r#"<https://api.github.com/repos/owner/repo/releases?page={page}>; rel="next""#),
    )
}

#[tokio::test]
async fn releases_preserve_draft_prerelease_assets_and_unknown_fields() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(response(
        200,
        &format!("[{},{}]", release(1, true, false), release(2, false, true)),
    )))
    .await;
    let outcome = fetch_releases(
        &client(mock.clone()),
        &token(),
        &repository(),
        &RepositoryExtrasLimits::default(),
        CancellationToken::default(),
    )
    .await
    .unwrap();
    assert_eq!(outcome.repository, repository());
    assert_eq!(outcome.completeness, ExtrasCompleteness::Complete);
    assert!(outcome.releases[0].draft);
    assert!(outcome.releases[1].prerelease);
    assert_eq!(outcome.releases[0].raw["future_field"]["preserved"], true);
    assert_eq!(outcome.releases[0].raw["assets"][0]["id"], 1001);
    assert_eq!(mock.recorded().await.len(), 1);
}

#[tokio::test]
async fn releases_empty_and_exact_entry_limit_without_next_are_complete() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(response(200, "[]"))).await;
    let outcome = fetch_releases(
        &client(mock),
        &token(),
        &repository(),
        &RepositoryExtrasLimits::default(),
        CancellationToken::default(),
    )
    .await
    .unwrap();
    assert!(outcome.releases.is_empty());
    assert_eq!(outcome.completeness, ExtrasCompleteness::Complete);
}

#[tokio::test]
async fn release_pagination_limits_and_malformed_links_are_partial() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(HttpResponse {
        headers: vec![next(2)],
        ..response(200, &format!("[{}]", release(1, false, false)))
    }))
    .await;
    let limits = RepositoryExtrasLimits {
        max_entries: 1,
        ..Default::default()
    };
    let outcome = fetch_releases(
        &client(mock),
        &token(),
        &repository(),
        &limits,
        CancellationToken::default(),
    )
    .await
    .unwrap();
    assert!(matches!(
        outcome.completeness,
        ExtrasCompleteness::Partial { ref reasons }
            if reasons.contains(&ExtrasIncompleteReason::EntryLimit { limit: 1 })
    ));

    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(HttpResponse {
        headers: vec![next(2)],
        ..response(200, &format!("[{}]", release(1, false, false)))
    }))
    .await;
    let outcome = fetch_releases(
        &client(mock),
        &token(),
        &repository(),
        &RepositoryExtrasLimits {
            max_pages: 1,
            ..Default::default()
        },
        CancellationToken::default(),
    )
    .await
    .unwrap();
    assert!(matches!(
        outcome.completeness,
        ExtrasCompleteness::Partial { ref reasons }
            if reasons.contains(&ExtrasIncompleteReason::PageLimit { limit: 1 })
    ));

    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(HttpResponse {
        headers: vec![("link".to_owned(), "not a link".to_owned())],
        ..response(200, "[]")
    }))
    .await;
    let outcome = fetch_releases(
        &client(mock),
        &token(),
        &repository(),
        &RepositoryExtrasLimits::default(),
        CancellationToken::default(),
    )
    .await
    .unwrap();
    assert!(matches!(
        outcome.completeness,
        ExtrasCompleteness::Partial { ref reasons }
            if reasons.contains(&ExtrasIncompleteReason::InvalidPagination)
    ));
}

#[tokio::test]
async fn release_multiple_pages_and_sso_are_explicitly_observed() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(HttpResponse {
        headers: vec![next(2)],
        ..response(200, &format!("[{}]", release(1, false, false)))
    }))
    .await;
    mock.enqueue(Ok(HttpResponse {
        headers: vec![
            ("etag".to_owned(), "\"page-2\"".to_owned()),
            (
                "x-github-sso".to_owned(),
                "partial-results; organizations=acme".to_owned(),
            ),
        ],
        ..response(200, &format!("[{}]", release(2, false, false)))
    }))
    .await;
    let outcome = fetch_releases(
        &client(mock),
        &token(),
        &repository(),
        &RepositoryExtrasLimits::default(),
        CancellationToken::default(),
    )
    .await
    .unwrap();
    assert_eq!(outcome.releases.len(), 2);
    assert_eq!(outcome.page_metadata.len(), 2);
    assert!(matches!(
        outcome.completeness,
        ExtrasCompleteness::Partial { ref reasons }
            if matches!(reasons.as_slice(), [ExtrasIncompleteReason::SsoPartial { organizations }] if organizations == &["acme".to_owned()])
    ));
}

#[tokio::test]
async fn release_later_failures_keep_typed_partial_and_first_errors_stay_errors() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(HttpResponse {
        headers: vec![next(2)],
        ..response(200, &format!("[{}]", release(1, false, false)))
    }))
    .await;
    mock.enqueue(Ok(HttpResponse {
        headers: vec![("x-ratelimit-remaining".to_owned(), "0".to_owned())],
        ..response(403, "{}")
    }))
    .await;
    let outcome = fetch_releases(
        &client(mock),
        &token(),
        &repository(),
        &RepositoryExtrasLimits::default(),
        CancellationToken::default(),
    )
    .await
    .unwrap();
    assert_eq!(outcome.releases.len(), 1);
    assert!(matches!(
        outcome.completeness,
        ExtrasCompleteness::Partial { ref reasons }
            if matches!(reasons.as_slice(), [ExtrasIncompleteReason::Failed(RepositoryExtrasError::GitHub(GitHubError::RateLimited { .. }))])
    ));

    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(response(404, "{}"))).await;
    let error = fetch_releases(
        &client(mock),
        &token(),
        &repository(),
        &RepositoryExtrasLimits::default(),
        CancellationToken::default(),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        RepositoryExtrasError::GitHub(GitHubError::NotFound)
    ));
}

#[tokio::test]
async fn latest_release_success_and_404_visibility_ambiguity_are_distinct() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(HttpResponse {
        headers: vec![("etag".to_owned(), "\"release-1\"".to_owned())],
        ..response(200, &release(1, false, false))
    }))
    .await;
    let outcome = fetch_latest_release(
        &client(mock),
        &token(),
        &repository(),
        &RepositoryExtrasLimits::default(),
        None,
        CancellationToken::default(),
    )
    .await
    .unwrap();
    assert!(outcome.release.is_some());
    assert_eq!(outcome.metadata.etag.as_deref(), Some("\"release-1\""));

    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(response(404, "{}"))).await;
    let outcome = fetch_latest_release(
        &client(mock),
        &token(),
        &repository(),
        &RepositoryExtrasLimits::default(),
        None,
        CancellationToken::default(),
    )
    .await
    .unwrap();
    assert_eq!(outcome.release, None);
}

#[tokio::test]
async fn latest_forbidden_sso_and_rate_limits_are_not_none() {
    for (status, headers, expected) in [
        (403, vec![], "forbidden"),
        (
            403,
            vec![(
                "x-github-sso".to_owned(),
                "required; url=https://github.com/orgs/acme/sso?authorization_request=synthetic"
                    .to_owned(),
            )],
            "sso",
        ),
        (
            403,
            vec![("retry-after".to_owned(), "60".to_owned())],
            "rate",
        ),
    ] {
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(HttpResponse {
            status,
            headers,
            body: "{}".to_owned(),
        }))
        .await;
        let error = fetch_latest_release(
            &client(mock),
            &token(),
            &repository(),
            &RepositoryExtrasLimits::default(),
            None,
            CancellationToken::default(),
        )
        .await
        .unwrap_err();
        assert!(match expected {
            "forbidden" => matches!(error, RepositoryExtrasError::GitHub(GitHubError::Forbidden)),
            "sso" => matches!(
                error,
                RepositoryExtrasError::GitHub(GitHubError::SsoRequired { .. })
            ),
            _ => matches!(
                error,
                RepositoryExtrasError::GitHub(GitHubError::RateLimited { .. })
            ),
        });
    }
}

fn contributors_body() -> String {
    serde_json::json!([
        {"login":"octocat","id":1,"contributions":12,"type":"User","future":true},
        {"name":"Anonymous","email":"anon@example.invalid","contributions":3,"type":"Anonymous"}
    ])
    .to_string()
}

#[tokio::test]
async fn contributors_anonymous_204_empty_403_and_short_page_have_clear_meanings() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(response(200, &contributors_body()))).await;
    let outcome = fetch_contributors(
        &client(mock.clone()),
        &token(),
        &repository(),
        &RepositoryExtrasLimits {
            per_page: 2,
            ..Default::default()
        },
        CancellationToken::default(),
    )
    .await
    .unwrap();
    assert_eq!(outcome.state, ContributorsState::Content);
    assert_eq!(outcome.contributors[1].kind(), ContributorKind::Anonymous);
    assert_eq!(outcome.observed_entries(), 2);
    assert_eq!(outcome.completeness, ExtrasCompleteness::Complete);
    assert_eq!(outcome.contributors[0].raw["future"], true);
    let recorded = mock.recorded().await;
    assert!(recorded[0].0.contains("anon=true"));
    assert!(recorded[0].0.contains("per_page=2"));

    for (status, body, state) in [
        (204, "", ContributorsState::EmptyRepository),
        (200, "[]", ContributorsState::Content),
    ] {
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(response(status, body))).await;
        let outcome = fetch_contributors(
            &client(mock),
            &token(),
            &repository(),
            &RepositoryExtrasLimits::default(),
            CancellationToken::default(),
        )
        .await
        .unwrap();
        assert_eq!(outcome.state, state);
        assert_eq!(outcome.observed_entries(), 0);
        assert_eq!(outcome.completeness, ExtrasCompleteness::Complete);
        if state == ContributorsState::EmptyRepository {
            assert_eq!(outcome.page_metadata.len(), 1);
        }
    }

    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(response(403, "{}"))).await;
    assert!(matches!(
        fetch_contributors(
            &client(mock),
            &token(),
            &repository(),
            &RepositoryExtrasLimits::default(),
            CancellationToken::default(),
        )
        .await,
        Err(RepositoryExtrasError::GitHub(GitHubError::Forbidden))
    ));
}

#[tokio::test]
async fn languages_empty_zero_shares_overflow_and_failures_are_distinct() {
    for body in ["{}", r#"{"Rust":0,"C":0}"#] {
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(response(200, body))).await;
        let outcome = fetch_languages(
            &client(mock),
            &token(),
            &repository(),
            &RepositoryExtrasLimits::default(),
            None,
            CancellationToken::default(),
        )
        .await
        .unwrap();
        assert_eq!(outcome.total_bytes, 0);
        assert!(outcome.languages.iter().all(|lang| lang.share.is_none()));
    }
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(response(
        200,
        &format!(r#"{{"Rust":{},"C":1}}"#, u64::MAX),
    )))
    .await;
    assert!(matches!(
        fetch_languages(
            &client(mock),
            &token(),
            &repository(),
            &RepositoryExtrasLimits::default(),
            None,
            CancellationToken::default(),
        )
        .await,
        Err(RepositoryExtrasError::LanguageByteOverflow)
    ));

    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(response(200, r#"{"Rust":3,"Shell":1}"#)))
        .await;
    let outcome = fetch_languages(
        &client(mock),
        &token(),
        &repository(),
        &RepositoryExtrasLimits::default(),
        None,
        CancellationToken::default(),
    )
    .await
    .unwrap();
    assert_eq!(outcome.languages[0].share, Some(0.75));
    assert_eq!(outcome.languages[1].share, Some(0.25));
    assert_eq!(outcome.total_bytes, 4);

    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(response(503, "{}"))).await;
    assert!(matches!(
        fetch_languages(
            &client(mock),
            &token(),
            &repository(),
            &RepositoryExtrasLimits::default(),
            None,
            CancellationToken::default(),
        )
        .await,
        Err(RepositoryExtrasError::GitHub(
            GitHubError::UnexpectedStatus { status: 503 }
        ))
    ));
}

#[test]
fn language_derivation_is_byte_based_and_rejects_invalid_counts() {
    let bytes = serde_json::Map::from_iter([
        ("A".to_owned(), serde_json::Value::from(1)),
        ("B".to_owned(), serde_json::Value::from(3)),
    ]);
    let outcome = LanguagesOutcome::from_bytes(repository(), bytes, Default::default()).unwrap();
    assert_eq!(
        outcome.languages,
        vec![
            LanguageStat {
                name: "B".to_owned(),
                bytes: 3,
                share: Some(0.75)
            },
            LanguageStat {
                name: "A".to_owned(),
                bytes: 1,
                share: Some(0.25)
            },
        ]
    );
    let invalid = serde_json::Map::from_iter([("A".to_owned(), serde_json::Value::from(-1))]);
    assert!(matches!(
        LanguagesOutcome::from_bytes(repository(), invalid, Default::default()),
        Err(RepositoryExtrasError::InvalidResponse { .. })
    ));
}

#[tokio::test]
async fn response_limit_during_later_page_preserves_partial_data() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(HttpResponse {
        headers: vec![next(2)],
        ..response(200, &format!("[{}]", release(1, false, false)))
    }))
    .await;
    mock.enqueue(Ok(response(
        200,
        &format!(
            "[{},{}]",
            release(2, false, false),
            release(3, false, false)
        ),
    )))
    .await;
    let outcome = fetch_releases(
        &client(mock),
        &token(),
        &repository(),
        &RepositoryExtrasLimits {
            max_response_bytes: release(1, false, false).len() + 2,
            ..Default::default()
        },
        CancellationToken::default(),
    )
    .await
    .unwrap();
    assert_eq!(outcome.releases.len(), 1);
    assert!(matches!(
        outcome.completeness,
        ExtrasCompleteness::Partial { ref reasons }
            if matches!(reasons.as_slice(), [ExtrasIncompleteReason::ResponseLimit { .. }])
    ));
}

struct CancelAfterFirst;
impl Transport for CancelAfterFirst {
    fn execute_bounded<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
        _max_body_bytes: usize,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        self.execute(request, cancel)
    }

    fn execute<'a>(
        &'a self,
        _request: HttpRequest,
        _cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async { Err(TransportError::Cancelled) })
    }
}

#[tokio::test]
async fn cancellation_is_never_an_empty_or_partial_success() {
    let client = GitHubClient::new(Default::default(), Arc::new(CancelAfterFirst));
    for result in [
        fetch_releases(
            &client,
            &token(),
            &repository(),
            &RepositoryExtrasLimits::default(),
            CancellationToken::default(),
        )
        .await
        .map(|_| ()),
        fetch_contributors(
            &client,
            &token(),
            &repository(),
            &RepositoryExtrasLimits::default(),
            CancellationToken::default(),
        )
        .await
        .map(|_| ()),
        fetch_languages(
            &client,
            &token(),
            &repository(),
            &RepositoryExtrasLimits::default(),
            None,
            CancellationToken::default(),
        )
        .await
        .map(|_| ()),
    ] {
        assert!(
            matches!(
                result,
                Err(RepositoryExtrasError::GitHub(GitHubError::Cancelled))
            ),
            "expected cancellation, got {result:?}"
        );
    }
}

#[tokio::test]
async fn cancellation_after_first_page_is_hard_cancelled() {
    struct CancelOnSecondPage {
        source: crate::network::cancel::CancellationTokenSource,
        calls: std::sync::atomic::AtomicUsize,
    }
    impl Transport for CancelOnSecondPage {
        fn execute_bounded<'a>(
            &'a self,
            request: HttpRequest,
            cancel: CancellationToken,
            _max_body_bytes: usize,
        ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>>
        {
            self.execute(request, cancel)
        }
        fn execute<'a>(
            &'a self,
            _request: HttpRequest,
            _cancel: CancellationToken,
        ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>>
        {
            Box::pin(async {
                if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    Ok(HttpResponse {
                        headers: vec![next(2)],
                        ..response(200, &format!("[{}]", release(1, false, false)))
                    })
                } else {
                    self.source.cancel();
                    Err(TransportError::Cancelled)
                }
            })
        }
    }
    let (cancel, source) = CancellationToken::new();
    let transport = Arc::new(CancelOnSecondPage {
        source,
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    let client = GitHubClient::new(Default::default(), transport);
    assert!(matches!(
        fetch_releases(
            &client,
            &token(),
            &repository(),
            &RepositoryExtrasLimits::default(),
            cancel,
        )
        .await,
        Err(RepositoryExtrasError::GitHub(GitHubError::Cancelled))
    ));
}

#[tokio::test]
async fn pagination_rejects_cross_origin_and_same_origin_different_resource() {
    for (link, expected) in [
        (
            r#"<https://evil.invalid/repos/owner/repo/releases?page=2>; rel="next""#,
            "untrusted",
        ),
        (
            r#"<https://api.github.com/repos/owner/other/releases?page=2>; rel="next""#,
            "invalid",
        ),
        (
            r#"<https://user@api.github.com/repos/owner/repo/releases?page=2>; rel="next""#,
            "invalid",
        ),
    ] {
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(HttpResponse {
            headers: vec![("link".to_owned(), link.to_owned())],
            ..response(200, "[]")
        }))
        .await;
        let outcome = fetch_releases(
            &client(mock.clone()),
            &token(),
            &repository(),
            &RepositoryExtrasLimits::default(),
            CancellationToken::default(),
        )
        .await
        .unwrap();
        assert!(matches!(
            outcome.completeness,
            ExtrasCompleteness::Partial { ref reasons }
                if reasons.iter().any(|reason| matches!(reason,
                    ExtrasIncompleteReason::UntrustedPagination if expected == "untrusted")
                    || matches!(reason, ExtrasIncompleteReason::InvalidPagination if expected == "invalid"))
        ));
        assert_eq!(mock.recorded().await.len(), 1);
        assert!(
            !mock.recorded().await[0]
                .1
                .iter()
                .any(|(name, _)| name == "If-None-Match")
        );
    }
}

#[tokio::test]
async fn response_limit_and_transport_failure_are_not_silent_empty_data() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(response(200, &release(1, false, false))))
        .await;
    assert!(matches!(
        fetch_latest_release(
            &client(mock),
            &token(),
            &repository(),
            &RepositoryExtrasLimits {
                max_response_bytes: 8,
                ..Default::default()
            },
            None,
            CancellationToken::default(),
        )
        .await,
        Err(RepositoryExtrasError::Repository(
            RepositoryError::LimitExceeded { limit: 8, .. }
        ))
    ));

    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Err(MockFailure::Network("offline".to_owned())))
        .await;
    assert!(matches!(
        fetch_languages(
            &client(mock),
            &token(),
            &repository(),
            &RepositoryExtrasLimits::default(),
            None,
            CancellationToken::default(),
        )
        .await,
        Err(RepositoryExtrasError::GitHub(GitHubError::Network { .. }))
    ));
}

#[tokio::test]
async fn localhost_preserves_locators_cache_conditions_and_same_origin_security() {
    let server =
        LocalHttpServer::start(vec![ScriptedResponse::json(200, r#"{"Rust":3,"Shell":1}"#)]);
    let transport = Arc::new(ReqwestTransport::new(Duration::from_secs(5)).unwrap());
    let client = GitHubClient::new(
        GitHubConfig {
            base_url: server.base_url().to_owned(),
            ..Default::default()
        },
        transport,
    );
    let conditional = ResponseMetadata {
        etag: Some("\"v1\"".to_owned()),
        last_modified: Some("Sun, 25 Oct 2026 00:00:00 GMT".to_owned()),
        ..Default::default()
    };
    let outcome = fetch_languages(
        &client,
        &token(),
        &repository(),
        &RepositoryExtrasLimits::default(),
        Some(&conditional),
        CancellationToken::default(),
    )
    .await
    .unwrap();
    assert_eq!(outcome.total_bytes, 4);
    let requests = server.recorded();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0]
            .request_line
            .starts_with("GET /repos/owner/repo/languages")
    );
    assert_eq!(requests[0].header("if-none-match"), Some("\"v1\""));
    assert_eq!(
        requests[0].header("if-modified-since"),
        Some("Sun, 25 Oct 2026 00:00:00 GMT")
    );
    assert!(requests[0].header("authorization").is_some());
}

#[tokio::test]
async fn encoded_locator_cannot_escape_repository_endpoint() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(response(200, "{}"))).await;
    let identity = RepositoryIdentity {
        id: 1,
        locator: RepositoryLocator::new("owner name", "repo?x").unwrap(),
    };
    fetch_languages(
        &client(mock.clone()),
        &token(),
        &identity,
        &RepositoryExtrasLimits::default(),
        None,
        CancellationToken::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        mock.recorded().await[0].0,
        "https://api.github.com/repos/owner%20name/repo%3Fx/languages"
    );
}

#[test]
fn limits_reject_unbounded_or_invalid_requests() {
    let invalid = [
        RepositoryExtrasLimits {
            per_page: 0,
            ..Default::default()
        },
        RepositoryExtrasLimits {
            per_page: 101,
            ..Default::default()
        },
        RepositoryExtrasLimits {
            max_pages: 0,
            ..Default::default()
        },
        RepositoryExtrasLimits {
            max_entries: 0,
            ..Default::default()
        },
        RepositoryExtrasLimits {
            max_response_bytes: 0,
            ..Default::default()
        },
    ];
    for limits in invalid {
        assert!(matches!(
            limits.validate(),
            Err(RepositoryExtrasError::InvalidLimits(_))
        ));
    }
}
