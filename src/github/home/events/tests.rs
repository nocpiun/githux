use super::*;
use crate::network::test_server::{LocalHttpServer, ScriptedResponse};
use crate::network::transport::{HttpResponse, MockTransport, ReqwestTransport};
use std::sync::Arc;
use std::time::Duration;

fn client(mock: Arc<MockTransport>) -> GitHubClient {
    GitHubClient::new(Default::default(), mock)
}

fn login(name: &str) -> Login {
    Login::new(name).expect("valid login")
}

fn pat() -> Pat {
    Pat::from_str("ghp_test")
}

fn event(id: &str, type_name: &str, payload: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "type": type_name,
        "actor": {
            "id": 7,
            "login": "actor-user",
            "display_login": "actor-user",
            "avatar_url": "https://example.invalid/a.png"
        },
        "repo": {
            "id": 99,
            "name": "acme/repo",
            "url": "https://api.github.com/repos/acme/repo"
        },
        "payload": payload,
        "public": true,
        "created_at": "2024-06-01T00:00:00Z"
    })
}

fn events_body(events: Vec<serde_json::Value>) -> String {
    serde_json::to_string(&events).expect("json")
}

fn ok(body: String, headers: Vec<(&str, &str)>) -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: headers
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        body,
    }
}

fn query_of(url: &str, key: &str) -> Option<String> {
    Url::parse(url)
        .ok()?
        .query_pairs()
        .find_map(|(k, v)| (k.as_ref() == key).then(|| v.to_string()))
}

async fn run(
    mock: &Arc<MockTransport>,
    etag: Option<&str>,
    limits: &FeedLimits,
) -> Result<FeedFetch, HomeError> {
    fetch_received_events(
        &client(mock.clone()),
        &pat(),
        &login("test-user"),
        etag,
        limits,
        CancellationToken::default(),
    )
    .await
}

#[tokio::test]
async fn parses_common_event_types() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(
        events_body(vec![
            event("1", "PushEvent", serde_json::json!({"ref": "refs/heads/main", "before": "a", "head": "b", "size": 2})),
            event("2", "PullRequestEvent", serde_json::json!({"action": "opened", "number": 5, "pull_request": {"title": "Fix", "html_url": "https://github.com/acme/repo/pull/5", "state": "open", "merged": false}})),
            event("3", "IssuesEvent", serde_json::json!({"action": "closed", "issue": {"number": 9, "title": "Bug", "html_url": "https://github.com/acme/repo/issues/9", "state": "closed"}})),
            event("4", "WatchEvent", serde_json::json!({"action": "started"})),
            event("5", "ReleaseEvent", serde_json::json!({"action": "published", "release": {"tag_name": "v1.0.0", "name": "First", "html_url": "https://github.com/acme/repo/releases/tag/v1.0.0"}})),
        ]),
        vec![("ETag", "\"feed-1\""), ("X-Poll-Interval", "60")],
    )))
    .await;

    let FeedFetch::Events(outcome) = run(&mock, None, &FeedLimits::default()).await.expect("ok")
    else {
        panic!("expected events");
    };
    assert_eq!(outcome.events.len(), 5);
    assert_eq!(outcome.etag.as_deref(), Some("\"feed-1\""));
    assert_eq!(outcome.poll_interval_seconds, Some(60));
    assert_eq!(outcome.readiness, FeedReadiness::Complete);

    assert_eq!(outcome.events[0].event_type, FeedEventType::Push);
    assert!(matches!(
        &outcome.events[0].payload,
        FeedPayload::Push { git_ref, size: Some(2), .. }
            if git_ref.as_deref() == Some("refs/heads/main")
    ));
    assert_eq!(outcome.events[1].event_type, FeedEventType::PullRequest);
    assert!(matches!(
        &outcome.events[1].payload,
        FeedPayload::PullRequest { action, number: Some(5), merged: Some(false), .. }
            if action == "opened"
    ));
    assert_eq!(outcome.events[2].event_type, FeedEventType::Issues);
    assert_eq!(outcome.events[3].event_type, FeedEventType::Watch);
    assert!(matches!(
        &outcome.events[4].payload,
        FeedPayload::Release { tag_name, .. } if tag_name.as_deref() == Some("v1.0.0")
    ));
    assert_eq!(outcome.events[0].actor.login, "actor-user");
    assert_eq!(outcome.events[0].repository.name, "acme/repo");
}

#[tokio::test]
async fn unknown_event_type_is_compatible_and_keeps_no_payload() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(
        events_body(vec![event(
            "1",
            "SomeFutureEvent",
            serde_json::json!({"secret": "x".repeat(1000)}),
        )]),
        vec![],
    )))
    .await;

    let FeedFetch::Events(outcome) = run(&mock, None, &FeedLimits::default()).await.expect("ok")
    else {
        panic!("expected events");
    };
    assert_eq!(
        outcome.events[0].event_type,
        FeedEventType::Unknown("SomeFutureEvent".to_string())
    );
    assert_eq!(outcome.events[0].payload, FeedPayload::Unknown);
}

#[tokio::test]
async fn missing_payload_does_not_panic() {
    let mock = Arc::new(MockTransport::new());
    let mut raw = event("1", "PublicEvent", serde_json::json!({}));
    raw.as_object_mut().unwrap().remove("payload");
    mock.enqueue(Ok(ok(events_body(vec![raw]), vec![]))).await;

    let FeedFetch::Events(outcome) = run(&mock, None, &FeedLimits::default()).await.expect("ok")
    else {
        panic!("expected events");
    };
    assert_eq!(outcome.events[0].event_type, FeedEventType::Public);
    assert_eq!(outcome.events[0].payload, FeedPayload::Public);
}

#[tokio::test]
async fn deduplicates_events_across_pages() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(
        events_body(vec![
            event("1", "WatchEvent", serde_json::json!({"action": "started"})),
            event("2", "WatchEvent", serde_json::json!({"action": "started"})),
        ]),
        vec![(
            "Link",
            "<https://api.github.com/users/test-user/received_events?page=2>; rel=\"next\"",
        )],
    )))
    .await;
    mock.enqueue(Ok(ok(
        events_body(vec![
            event("2", "WatchEvent", serde_json::json!({"action": "started"})),
            event("3", "WatchEvent", serde_json::json!({"action": "started"})),
        ]),
        vec![],
    )))
    .await;

    let FeedFetch::Events(outcome) = run(&mock, None, &FeedLimits::default()).await.expect("ok")
    else {
        panic!("expected events");
    };
    let ids: Vec<&str> = outcome
        .events
        .iter()
        .map(|event| event.id.as_str())
        .collect();
    assert_eq!(ids, vec!["1", "2", "3"]);
    assert_eq!(outcome.readiness, FeedReadiness::Complete);
}

#[tokio::test]
async fn not_modified_returns_etag_and_poll_interval() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(HttpResponse {
        status: 304,
        headers: vec![
            ("etag".to_string(), "\"feed-1\"".to_string()),
            ("x-poll-interval".to_string(), "60".to_string()),
        ],
        body: String::new(),
    }))
    .await;

    let result = run(&mock, Some("\"feed-1\""), &FeedLimits::default())
        .await
        .expect("ok");
    assert_eq!(
        result,
        FeedFetch::NotModified {
            etag: Some("\"feed-1\"".to_string()),
            poll_interval_seconds: Some(60),
        }
    );
}

#[tokio::test]
async fn not_modified_without_etag_echoes_the_caller_validator() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(HttpResponse {
        status: 304,
        headers: vec![("x-poll-interval".to_string(), "30".to_string())],
        body: String::new(),
    }))
    .await;

    let result = run(&mock, Some("\"feed-1\""), &FeedLimits::default())
        .await
        .expect("ok");
    assert_eq!(
        result,
        FeedFetch::NotModified {
            etag: Some("\"feed-1\"".to_string()),
            poll_interval_seconds: Some(30),
        }
    );
}

#[tokio::test]
async fn conditional_request_sends_if_none_match() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(events_body(vec![]), vec![]))).await;

    run(&mock, Some("\"feed-1\""), &FeedLimits::default())
        .await
        .expect("ok");
    let recorded = mock.recorded().await;
    assert_eq!(recorded.len(), 1);
    assert!(
        recorded[0].1.iter().any(
            |(name, value)| name.eq_ignore_ascii_case("if-none-match") && value == "\"feed-1\""
        )
    );
}

#[tokio::test]
async fn empty_feed_is_complete_not_error() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(events_body(vec![]), vec![]))).await;

    let FeedFetch::Events(outcome) = run(&mock, None, &FeedLimits::default()).await.expect("ok")
    else {
        panic!("expected events");
    };
    assert!(outcome.events.is_empty());
    assert_eq!(outcome.readiness, FeedReadiness::Complete);
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

    let error = run(&mock, None, &FeedLimits::default())
        .await
        .expect_err("rate limit must fail");
    assert!(matches!(
        error,
        HomeError::GitHub(GitHubError::RateLimited { .. })
    ));
}

#[tokio::test]
async fn forbidden_is_distinct_from_unauthorized() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(HttpResponse {
        status: 403,
        headers: vec![],
        body: "{}".to_string(),
    }))
    .await;

    let error = run(&mock, None, &FeedLimits::default())
        .await
        .expect_err("forbidden must fail");
    assert!(matches!(error, HomeError::GitHub(GitHubError::Forbidden)));
}

#[tokio::test]
async fn cancellation_is_a_hard_error() {
    let mock = Arc::new(MockTransport::new());
    let (cancel, source) = CancellationToken::new();
    source.cancel();

    let result = fetch_received_events(
        &client(mock.clone()),
        &pat(),
        &login("test-user"),
        None,
        &FeedLimits::default(),
        cancel,
    )
    .await;
    assert!(matches!(
        result,
        Err(HomeError::GitHub(GitHubError::Cancelled))
    ));
    assert!(mock.recorded().await.is_empty());
}

#[tokio::test]
async fn local_page_limit_marks_partial() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(
        events_body(vec![event(
            "1",
            "WatchEvent",
            serde_json::json!({"action": "started"}),
        )]),
        vec![(
            "Link",
            "<https://api.github.com/users/test-user/received_events?page=2>; rel=\"next\"",
        )],
    )))
    .await;

    let limits = FeedLimits {
        max_pages: 1,
        ..FeedLimits::default()
    };
    let FeedFetch::Events(outcome) = run(&mock, None, &limits).await.expect("ok") else {
        panic!("expected events");
    };
    assert_eq!(outcome.events.len(), 1);
    assert!(matches!(
        outcome.readiness,
        FeedReadiness::Partial { ref reasons }
            if reasons.contains(&FeedPartialReason::PageLimit)
    ));
}

#[tokio::test]
async fn untrusted_next_link_is_partial() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(
        events_body(vec![event(
            "1",
            "WatchEvent",
            serde_json::json!({"action": "started"}),
        )]),
        vec![(
            "Link",
            "<https://evil.example/users/test-user/received_events?page=2>; rel=\"next\"",
        )],
    )))
    .await;

    let FeedFetch::Events(outcome) = run(&mock, None, &FeedLimits::default()).await.expect("ok")
    else {
        panic!("expected events");
    };
    assert!(matches!(
        outcome.readiness,
        FeedReadiness::Partial { ref reasons }
            if reasons.contains(&FeedPartialReason::UntrustedPagination)
    ));
}

#[tokio::test]
async fn feed_url_is_scoped_to_the_explicit_login() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(events_body(vec![]), vec![]))).await;

    run(&mock, None, &FeedLimits::default()).await.expect("ok");
    let recorded = mock.recorded().await;
    assert!(
        recorded[0].0.contains("/users/test-user/received_events"),
        "got {}",
        recorded[0].0
    );
    assert_eq!(query_of(&recorded[0].0, "per_page").as_deref(), Some("100"));
}

#[tokio::test]
async fn localhost_304_round_trip_returns_not_modified() {
    let server = LocalHttpServer::start(vec![
        ScriptedResponse::json(304, "")
            .with_header("ETag", "\"feed-1\"")
            .with_header("X-Poll-Interval", "60"),
    ]);
    let transport = Arc::new(ReqwestTransport::new(Duration::from_secs(5)).expect("transport"));
    let client = GitHubClient::new(
        crate::github::client::GitHubConfig {
            base_url: server.base_url().to_string(),
            ..Default::default()
        },
        transport,
    );

    let result = fetch_received_events(
        &client,
        &pat(),
        &login("test-user"),
        Some("\"feed-1\""),
        &FeedLimits::default(),
        CancellationToken::default(),
    )
    .await
    .expect("ok");
    assert_eq!(
        result,
        FeedFetch::NotModified {
            etag: Some("\"feed-1\"".to_string()),
            poll_interval_seconds: Some(60),
        }
    );

    let recorded = server.recorded();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].header("if-none-match"), Some("\"feed-1\""));
}
