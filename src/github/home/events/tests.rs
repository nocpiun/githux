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
    // The adapter publishes the parsed **source** list: selecting retained
    // activities is the product policy applied later by `build_feed`, so nothing
    // is dropped here.
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
async fn a_source_page_keeps_every_event_in_server_order_and_identity() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(
        events_body(vec![
            event("1", "WatchEvent", serde_json::json!({"action": "started"})),
            event(
                "2",
                "PushEvent",
                serde_json::json!({"ref": "refs/heads/main", "size": 2}),
            ),
            event(
                "3",
                "PullRequestEvent",
                serde_json::json!({
                    "action": "merged", "number": 5,
                    "pull_request": {
                        "url": "https://api.github.com/repos/acme/repo/pulls/5",
                        "id": 55, "number": 5,
                        "head": {"ref": "x", "sha": "a"},
                        "base": {"ref": "main", "sha": "b"}
                    }
                }),
            ),
            event(
                "4",
                "CreateEvent",
                serde_json::json!({"ref_type": "repository", "ref": null}),
            ),
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
    assert_eq!(
        ids,
        vec!["1", "2", "3", "4"],
        "the source list keeps server order and every event"
    );
    assert_eq!(outcome.events[2].repository.name, "acme/repo");
}

/// The live `action=merged` shape observed from the public Events API: the
/// payload contains a slim `pull_request` object with only
/// `url/id/number/head/base`, no `merged` flag and no `user`. Parsing must keep
/// the action and the target identity, and must not invent an author.
#[tokio::test]
async fn parses_a_live_merged_event_with_a_slim_pull_request_object() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(
        events_body(vec![event(
            "1",
            "PullRequestEvent",
            serde_json::json!({
                "action": "merged",
                "number": 144,
                "pull_request": {
                    "url": "https://api.github.com/repos/subsurface/libdc/pulls/144",
                    "id": 4838297932u64,
                    "number": 144,
                    "head": {"ref": "feat/x", "sha": "8cc8ec82a733ac545195ce5d6f04e8939cd706b1"},
                    "base": {"ref": "master", "sha": "8d3f629702a7de2f510186614e293260a1dd6f70"}
                }
            }),
        )]),
        vec![],
    )))
    .await;

    let FeedFetch::Events(outcome) = run(&mock, None, &FeedLimits::default()).await.expect("ok")
    else {
        panic!("expected events");
    };
    assert_eq!(outcome.events.len(), 1);
    assert!(
        matches!(
            &outcome.events[0].payload,
            FeedPayload::PullRequest {
                action,
                number: Some(144),
                merged: None,
                author: None,
                ..
            } if action == "merged"
        ),
        "the merged action and target survive; no author is invented: {:?}",
        outcome.events[0].payload
    );
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
    // The source list keeps the event; the product policy drops it later. The
    // parser deliberately keeps no payload, so a future shape cannot grow.
    assert_eq!(outcome.events.len(), 1);
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
    // A missing payload must not panic the parser; the source list keeps it and
    // the product policy is what eventually excludes it.
    assert_eq!(outcome.events.len(), 1);
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
async fn partial_outcome_withholds_the_reusable_validator() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(
        events_body(vec![event(
            "1",
            "WatchEvent",
            serde_json::json!({"action": "started"}),
        )]),
        vec![
            ("ETag", "\"feed-1\""),
            (
                "Link",
                "<https://api.github.com/users/test-user/received_events?page=2>; rel=\"next\"",
            ),
        ],
    )))
    .await;
    mock.enqueue(Ok(HttpResponse {
        status: 503,
        headers: vec![],
        body: "{}".to_string(),
    }))
    .await;

    let FeedFetch::Events(outcome) = run(&mock, None, &FeedLimits::default()).await.expect("ok")
    else {
        panic!("expected events");
    };
    assert!(matches!(outcome.readiness, FeedReadiness::Partial { .. }));
    assert_eq!(
        outcome.etag, None,
        "a partial pagination must not return a validator that would 304 away the retry"
    );
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

#[tokio::test]
async fn incremental_page_path_enforces_max_events_and_max_pages() {
    // max_events: a page with more events than the budget truncates and reports
    // EntryLimit, exposing no continuation.
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
    let limits = FeedLimits {
        max_events: 1,
        ..FeedLimits::default()
    };
    let page = fetch_received_events_page(
        &client(mock.clone()),
        &pat(),
        &login("test-user"),
        FeedSourceCursor { page: 1 },
        &limits,
        CancellationToken::default(),
    )
    .await
    .expect("ok");
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.next, None, "truncation must not expose a continuation");
    assert!(page.reasons.contains(&FeedPartialReason::EntryLimit));

    // max_pages: the current page equals the budget, so no next link is honored.
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
    let page = fetch_received_events_page(
        &client(mock.clone()),
        &pat(),
        &login("test-user"),
        FeedSourceCursor { page: 1 },
        &limits,
        CancellationToken::default(),
    )
    .await
    .expect("ok");
    assert_eq!(
        page.next, None,
        "the page budget bounds the incremental walk"
    );
    assert!(page.reasons.contains(&FeedPartialReason::PageLimit));

    // A complete final page that lands exactly on the page budget (no next link)
    // is NOT truncation: importing 201-300 events must stay Complete.
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(
        events_body(vec![event(
            "1",
            "WatchEvent",
            serde_json::json!({"action": "started"}),
        )]),
        vec![],
    )))
    .await;
    let limits = FeedLimits {
        max_pages: 3,
        ..FeedLimits::default()
    };
    let page = fetch_received_events_page(
        &client(mock.clone()),
        &pat(),
        &login("test-user"),
        FeedSourceCursor { page: 3 },
        &limits,
        CancellationToken::default(),
    )
    .await
    .expect("ok");
    assert_eq!(page.next, None);
    assert!(
        page.reasons.is_empty(),
        "a complete final page at the page budget is not partial: {:?}",
        page.reasons
    );
}
