use super::*;
use crate::network::transport::{MockTransport, Transport, json_response};
use std::sync::Arc;

#[path = "../../bin_support/notifications_fixture.rs"]
mod fixture;
use fixture::{Fixture, Scene};
fn client(transport: Arc<dyn crate::network::transport::Transport>) -> GitHubClient {
    GitHubClient::new(crate::github::client::GitHubConfig::default(), transport)
}
async fn fetch(client: &GitHubClient) -> Result<NotificationPage, PageFailure> {
    fetch_page(
        client,
        &Pat::from_str("synthetic"),
        None,
        None,
        &NotificationLimits::default(),
        CancellationToken::default(),
    )
    .await
}
#[tokio::test]
async fn real_shape_query_headers_and_opaque_subjects() {
    let fixture = Arc::new(Fixture::new(Scene::Unknown));
    let page = fetch(&client(fixture.clone())).await.unwrap();
    assert_eq!(page.items[0].id, "9000");
    assert!(matches!(
        page.items[0].subject.kind,
        SubjectKind::Unknown(_)
    ));
    assert_eq!(page.items[0].reason, "future_reason");
    assert_eq!(page.items[0].subject.url, None);
    assert_eq!(page.items[0].repository.id(), 100);
    assert_eq!(page.metadata.poll_interval_seconds, Some(1));
    let requests = fixture.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(
        request.url,
        "https://api.github.com/notifications?all=false&participating=false&per_page=50&page=1"
    );
    for name in [
        "Authorization",
        "User-Agent",
        "Accept",
        "X-GitHub-Api-Version",
    ] {
        assert!(request.headers.iter().any(|(key, _)| key == name));
    }
}
#[tokio::test]
async fn trusted_continuation_and_page_limit() {
    let fixture = Arc::new(Fixture::new(Scene::Mixed));
    let client = client(fixture.clone());
    let first = fetch(&client).await.unwrap();
    let next = fetch_page(
        &client,
        &Pat::from_str("synthetic"),
        first.next.as_ref(),
        None,
        &NotificationLimits::default(),
        CancellationToken::default(),
    )
    .await
    .unwrap();
    assert_eq!(next.items[0].id, "9007");
    assert!(next.next.is_none());
    let limit = NotificationLimits {
        max_pages: 1,
        ..Default::default()
    };
    let page = fetch_page(
        &client,
        &Pat::from_str("synthetic"),
        None,
        None,
        &limit,
        CancellationToken::default(),
    )
    .await
    .unwrap();
    assert_eq!(page.partial, vec![NotificationPartial::PageLimit]);
}
#[tokio::test]
async fn unsafe_links_never_issue_authenticated_requests() {
    for url in [
        "https://evil.example/notifications?all=false&participating=false&per_page=50&page=2",
        "https://api.github.com:8443/notifications?all=false&participating=false&per_page=50&page=2",
        "https://evil.example\\@api.github.com/notifications?all=false&participating=false&per_page=50&page=2",
        "https://api.github.com/user?all=false&participating=false&per_page=50&page=2",
        "https://api.github.com/notifications?all=true&participating=false&per_page=50&page=2",
        "https://api.github.com/notifications?all=false&participating=true&per_page=50&page=2",
        "https://api.github.com/notifications?all=false&participating=false&per_page=50&page=1",
        "https://api.github.com/notifications?all=false&participating=false&per_page=50&page=2&since=2026",
        "https://u@api.github.com/notifications?all=false&participating=false&per_page=50&page=2",
    ] {
        let transport = Arc::new(MockTransport::new());
        let mut response = json_response(200, "[]");
        response
            .headers
            .push(("link".into(), format!("<{url}>; rel=\"next\"")));
        transport.enqueue(Ok(response)).await;
        let page = fetch(&client(transport.clone())).await.unwrap();
        assert!(page.next.is_none(), "{url}");
        assert_eq!(page.partial, vec![NotificationPartial::UnsafePagination]);
        assert_eq!(transport.recorded_requests().await.len(), 1);
    }
}
#[tokio::test]
async fn empty_partial_errors_and_parse_are_distinct() {
    let empty = fetch(&client(Arc::new(Fixture::new(Scene::Empty))))
        .await
        .unwrap();
    assert!(empty.items.is_empty() && empty.partial.is_empty());
    let partial = fetch(&client(Arc::new(Fixture::new(Scene::SsoPartial))))
        .await
        .unwrap();
    assert!(matches!(
        &partial.partial[..],
        [NotificationPartial::Sso { .. }]
    ));
    for (status, body) in [
        (200, "{}"),
        (200, "[{}]"),
        (401, "[]"),
        (403, "[]"),
        (429, "[]"),
        (503, "[]"),
        (304, ""),
    ] {
        let transport = Arc::new(MockTransport::new());
        transport.enqueue(Ok(json_response(status, body))).await;
        assert!(fetch(&client(transport)).await.is_err());
    }
}
#[tokio::test]
async fn duplicates_bounded_body_and_exact_conditional_header() {
    let transport = Arc::new(MockTransport::new());
    transport
        .enqueue(Ok(json_response(
            200,
            &serde_json::json!([
                fixture::thread(0, "2026-11-01T12:00:00Z", true),
                fixture::thread(0, "2026-11-01T12:00:00Z", true),
                fixture::thread(1, "2026-11-01T12:00:00Z", true)
            ])
            .to_string(),
        )))
        .await;
    assert_eq!(fetch(&client(transport)).await.unwrap().items.len(), 2);
    let fixture = Arc::new(Fixture::new(Scene::Mixed));
    let limit = NotificationLimits {
        max_body_bytes: 16,
        ..Default::default()
    };
    assert!(
        fetch_page(
            &client(fixture),
            &Pat::from_str("synthetic"),
            None,
            None,
            &limit,
            CancellationToken::default()
        )
        .await
        .is_err()
    );
    let transport = Arc::new(MockTransport::new());
    transport.enqueue(Ok(json_response(304, ""))).await;
    let value = "Thu, 01 Nov 2026 12:00:00 GMT";
    let page = fetch_page(
        &client(transport.clone()),
        &Pat::from_str("synthetic"),
        None,
        Some(value),
        &NotificationLimits::default(),
        CancellationToken::default(),
    )
    .await
    .unwrap();
    assert!(page.not_modified);
    assert!(
        transport.recorded_requests().await[0]
            .headers
            .iter()
            .any(|(k, v)| k == "If-Modified-Since" && v == value)
    );
}
#[tokio::test]
async fn reusable_fixture_failure_recovers_and_unknown_request_is_599() {
    let fixture = Arc::new(Fixture::new(Scene::FirstFailure));
    let client = client(fixture.clone());
    assert!(fetch(&client).await.is_err());
    assert!(fetch(&client).await.is_ok());
    assert!(fetch(&client).await.is_ok());
    let response = fixture
        .execute(
            crate::network::transport::HttpRequest::get("https://api.github.com/unmodelled"),
            CancellationToken::default(),
        )
        .await
        .unwrap();
    assert_eq!(response.status, 599);
    fixture.set_scene(Scene::Empty);
    assert!(fetch(&client).await.unwrap().items.is_empty());
    for scene in Scene::ALL {
        assert!(!scene.label().is_empty());
    }
}

#[tokio::test]
async fn done_is_a_delete_and_requires_204() {
    use crate::network::transport::{HttpMethod, HttpResponse};
    for (status, ok) in [
        (204, true),
        (200, false),
        (403, false),
        (404, false),
        (503, false),
    ] {
        let transport = Arc::new(MockTransport::new());
        transport
            .enqueue(Ok(HttpResponse {
                status,
                headers: vec![],
                body: String::new(),
            }))
            .await;
        let result = mark_done(
            &client(transport.clone()),
            &Pat::from_str("synthetic"),
            "9000",
            CancellationToken::default(),
        )
        .await;
        assert_eq!(result.is_ok(), ok, "status {status}");
        let requests = transport.recorded_requests().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, HttpMethod::Delete);
        assert!(requests[0].url.ends_with("/notifications/threads/9000"));
        assert!(!requests[0].url.contains("/subscription"));
        assert!(requests[0].body.is_none());
        assert!(
            requests[0]
                .headers
                .iter()
                .any(|(k, v)| k == "Authorization" && v.contains("synthetic"))
        );
    }
}

#[tokio::test]
async fn unsubscribe_puts_ignored_json_then_done() {
    use crate::network::transport::{HttpMethod, json_response};
    let transport = Arc::new(MockTransport::new());
    transport
        .enqueue(Ok(json_response(200, r#"{"ignored":true}"#)))
        .await;
    transport
        .enqueue(Ok(crate::network::transport::HttpResponse {
            status: 204,
            headers: vec![],
            body: String::new(),
        }))
        .await;
    let outcome = unsubscribe_thread(
        &client(transport.clone()),
        &Pat::from_str("synthetic"),
        "9000",
        CancellationToken::default(),
    )
    .await
    .unwrap();
    assert!(matches!(outcome, UnsubscribeOutcome::Completed(_)));
    let requests = transport.recorded_requests().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, HttpMethod::Put);
    assert!(
        requests[0]
            .url
            .ends_with("/notifications/threads/9000/subscription")
    );
    assert_eq!(requests[0].body.as_deref(), Some(r#"{"ignored":true}"#));
    assert!(
        requests[0]
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("content-type") && v == "application/json")
    );
    assert_eq!(requests[1].method, HttpMethod::Delete);
    assert!(requests[1].url.ends_with("/notifications/threads/9000"));
}

#[tokio::test]
async fn unsubscribe_rejects_a_subscription_that_is_not_ignored() {
    use crate::network::transport::json_response;
    let transport = Arc::new(MockTransport::new());
    transport
        .enqueue(Ok(json_response(200, r#"{"ignored":false}"#)))
        .await;
    let result = unsubscribe_thread(
        &client(transport.clone()),
        &Pat::from_str("synthetic"),
        "9000",
        CancellationToken::default(),
    )
    .await;
    assert!(result.is_err());
    // No follow-up DELETE is sent when the ignore was not confirmed.
    assert_eq!(transport.recorded_requests().await.len(), 1);
}

#[tokio::test]
async fn unsubscribe_keeps_the_confirmed_ignore_when_done_fails() {
    use crate::network::transport::json_response;
    let transport = Arc::new(MockTransport::new());
    transport
        .enqueue(Ok(json_response(200, r#"{"ignored":true}"#)))
        .await;
    transport.enqueue(Ok(json_response(503, "{}"))).await;
    let outcome = unsubscribe_thread(
        &client(transport.clone()),
        &Pat::from_str("synthetic"),
        "9000",
        CancellationToken::default(),
    )
    .await
    .unwrap();
    match outcome {
        UnsubscribeOutcome::IgnoredDoneFailed { .. } => {}
        other => panic!("expected IgnoredDoneFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn thread_writes_are_not_swallowed_on_forbidden_or_timeout() {
    use crate::network::transport::{MockFailure, json_response};
    let transport = Arc::new(MockTransport::new());
    transport
        .enqueue(Ok(json_response(403, r#"{"message":"forbidden"}"#)))
        .await;
    let forbidden = mark_done(
        &client(transport.clone()),
        &Pat::from_str("synthetic"),
        "9000",
        CancellationToken::default(),
    )
    .await;
    assert!(matches!(
        forbidden,
        Err(PageFailure {
            error: NotificationError::GitHub(GitHubError::Forbidden),
            ..
        })
    ));
    transport.enqueue(Err(MockFailure::Timeout)).await;
    let timeout = mark_done(
        &client(transport),
        &Pat::from_str("synthetic"),
        "9000",
        CancellationToken::default(),
    )
    .await;
    assert!(matches!(
        timeout,
        Err(PageFailure {
            error: NotificationError::GitHub(GitHubError::Timeout { .. }),
            ..
        })
    ));
}

#[tokio::test]
async fn a_thread_id_cannot_escape_the_fixed_endpoint() {
    use crate::network::transport::HttpMethod;
    let transport = Arc::new(MockTransport::new());
    for hostile in ["../user", "9000/../../user", "9000?all=true", ""] {
        let result = mark_done(
            &client(transport.clone()),
            &Pat::from_str("synthetic"),
            hostile,
            CancellationToken::default(),
        )
        .await;
        assert!(result.is_err(), "{hostile}");
    }
    assert_eq!(
        transport
            .recorded_requests()
            .await
            .iter()
            .filter(|r| r.method != HttpMethod::Get)
            .count(),
        0
    );
}

#[tokio::test]
async fn unsubscribe_completed_preserves_put_cooldown_metadata() {
    use crate::network::transport::HttpResponse;
    let transport = Arc::new(MockTransport::new());
    // PUT responds with a 120s polling interval and an exhausted quota reset.
    let mut put = HttpResponse {
        status: 200,
        headers: vec![
            ("x-poll-interval".into(), "120".into()),
            ("x-ratelimit-remaining".into(), "0".into()),
            ("retry-after".into(), "45".into()),
        ],
        body: r#"{"ignored":true}"#.into(),
    };
    // DELETE responds with default headers that do not mention the restriction.
    let done = HttpResponse {
        status: 204,
        headers: vec![("x-poll-interval".into(), "60".into())],
        body: String::new(),
    };
    transport.enqueue(Ok(put)).await;
    transport.enqueue(Ok(done)).await;
    let outcome = unsubscribe_thread(
        &client(transport),
        &Pat::from_str("synthetic"),
        "9000",
        CancellationToken::default(),
    )
    .await
    .unwrap();
    match outcome {
        UnsubscribeOutcome::Completed(meta) => {
            assert_eq!(meta.poll_interval_seconds, Some(120));
            assert_eq!(meta.rate_limit.remaining, Some(0));
            assert_eq!(meta.rate_limit.retry_after, Some(45));
        }
        other => panic!("expected Completed with merged metadata, got {other:?}"),
    }
}
