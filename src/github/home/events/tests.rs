use super::*;
use crate::domain::home::FeedActivityKind;
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
    // Only the starred repository is a retained Feed activity here: push,
    // unmerged pull request, issue and release events are technical noise.
    assert_eq!(outcome.events.len(), 1);
    assert_eq!(outcome.etag.as_deref(), Some("\"feed-1\""));
    assert_eq!(outcome.poll_interval_seconds, Some(60));
    assert_eq!(
        outcome.readiness,
        FeedReadiness::Complete,
        "filtering is not truncation"
    );

    assert_eq!(outcome.events[0].event_type, FeedEventType::Watch);
    assert!(matches!(
        &outcome.events[0].payload,
        FeedPayload::Watch { action } if action == "started"
    ));
    assert_eq!(outcome.events[0].actor.login, "actor-user");
    assert_eq!(outcome.events[0].repository.name, "acme/repo");
}

#[tokio::test]
async fn retained_activities_keep_source_order_and_identity() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(
        events_body(vec![
            event("1", "WatchEvent", serde_json::json!({"action": "started"})),
            event("2", "PushEvent", serde_json::json!({"ref": "refs/heads/main", "size": 2})),
            event(
                "3",
                "PullRequestEvent",
                serde_json::json!({
                    "action": "closed", "number": 5,
                    "pull_request": {
                        "title": "Fix", "html_url": "https://github.com/acme/repo/pull/5",
                        "state": "closed", "merged": true,
                        "user": {"id": 31, "login": "contributor", "avatar_url": "https://example.invalid/c.png"}
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
    assert_eq!(ids, vec!["1", "3", "4"], "source order is preserved");

    // The merged pull request credits the payload author, not the event actor.
    let merged = classify_feed_activity(&outcome.events[1]).expect("merged activity");
    assert_eq!(merged.kind, FeedActivityKind::PullRequestMerged);
    assert_eq!(merged.subject.id, 31);
    assert_eq!(merged.subject.login, "contributor");
    assert_eq!(
        merged.subject.avatar_url.as_deref(),
        Some("https://example.invalid/c.png")
    );
    assert_ne!(merged.subject.login, outcome.events[1].actor.login);
    let target = merged.pull_request.expect("pull request target");
    assert_eq!(target.number, Some(5));
    assert_eq!(target.title.as_deref(), Some("Fix"));

    let starred = classify_feed_activity(&outcome.events[0]).expect("star activity");
    assert_eq!(starred.kind, FeedActivityKind::RepositoryStarred);
    let created = classify_feed_activity(&outcome.events[2]).expect("create activity");
    assert_eq!(created.kind, FeedActivityKind::RepositoryCreated);
}

#[tokio::test]
async fn wrong_actions_ref_types_and_missing_merge_evidence_are_excluded() {
    let mock = Arc::new(MockTransport::new());
    let mut events = vec![
        // Stopping a watch is not starring.
        event("1", "WatchEvent", serde_json::json!({"action": "stopped"})),
        // Branch/tag creation is not repository creation.
        event(
            "2",
            "CreateEvent",
            serde_json::json!({"ref_type": "branch", "ref": "feature"}),
        ),
        event(
            "3",
            "CreateEvent",
            serde_json::json!({"ref_type": "tag", "ref": "v1.0.0"}),
        ),
        // Delete is never a create.
        event(
            "4",
            "DeleteEvent",
            serde_json::json!({"ref_type": "repository", "ref": null}),
        ),
        event(
            "5",
            "ForkEvent",
            serde_json::json!({"forkee": {"full_name": "x/y"}}),
        ),
        event(
            "6",
            "ReleaseEvent",
            serde_json::json!({"action": "published", "release": {"tag_name": "v1"}}),
        ),
        event(
            "7",
            "MemberEvent",
            serde_json::json!({"action": "added", "member": {"login": "x"}}),
        ),
        event("8", "PublicEvent", serde_json::json!({})),
        event(
            "9",
            "IssuesEvent",
            serde_json::json!({"action": "closed", "issue": {"number": 1}}),
        ),
        event(
            "10",
            "IssueCommentEvent",
            serde_json::json!({"action": "created", "issue": {"number": 1}}),
        ),
    ];
    let pr = |id: &str, action: &str, merged: serde_json::Value, author: serde_json::Value| {
        event(
            id,
            "PullRequestEvent",
            serde_json::json!({
                "action": action, "number": 1,
                "pull_request": {"title": "T", "state": "closed", "merged": merged, "user": author}
            }),
        )
    };
    let author = serde_json::json!({"id": 31, "login": "contributor"});
    events.push(pr("11", "opened", serde_json::Value::Null, author.clone()));
    events.push(pr("12", "closed", serde_json::json!(false), author.clone()));
    events.push(pr("13", "closed", serde_json::Value::Null, author.clone()));
    // There is no `action=merged` in the official Events schema.
    events.push(pr("14", "merged", serde_json::json!(true), author.clone()));
    // No authoritative author: attribution would be a guess.
    events.push(pr(
        "15",
        "closed",
        serde_json::json!(true),
        serde_json::Value::Null,
    ));
    // Unrecognised type.
    events.push(event(
        "16",
        "SomeFutureEvent",
        serde_json::json!({"secret": "x".repeat(100)}),
    ));

    mock.enqueue(Ok(ok(events_body(events), vec![]))).await;
    let FeedFetch::Events(outcome) = run(&mock, None, &FeedLimits::default()).await.expect("ok")
    else {
        panic!("expected events");
    };
    assert!(
        outcome.events.is_empty(),
        "no non-activity may become a Feed card: {:?}",
        outcome
            .events
            .iter()
            .map(|event| (event.id.clone(), event.event_type.clone()))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        outcome.readiness,
        FeedReadiness::Complete,
        "an all-filtered page is a normal complete empty Feed"
    );
}

#[tokio::test]
async fn an_all_filtered_feed_stays_complete_and_empty() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(
        events_body(vec![
            event(
                "1",
                "PushEvent",
                serde_json::json!({"ref": "refs/heads/main", "before": "a", "head": "b", "size": 1}),
            ),
            event(
                "2",
                "PullRequestEvent",
                serde_json::json!({
                    "action": "closed", "number": 9,
                    "pull_request": {"title": "Abandoned", "state": "closed", "merged": false,
                                     "user": {"id": 31, "login": "contributor"}}
                }),
            ),
        ]),
        vec![("ETag", "\"feed-1\""), ("X-Poll-Interval", "60")],
    )))
    .await;

    let FeedFetch::Events(outcome) = run(&mock, None, &FeedLimits::default()).await.expect("ok")
    else {
        panic!("expected events");
    };
    assert!(outcome.events.is_empty());
    assert_eq!(outcome.readiness, FeedReadiness::Complete);
    // The validator still describes the read representation, and filtering does
    // not make the page partial, so it is safe to reuse.
    assert_eq!(outcome.etag.as_deref(), Some("\"feed-1\""));
    assert_eq!(outcome.poll_interval_seconds, Some(60));
}

#[tokio::test]
async fn filtered_activities_do_not_trigger_extra_fetching_to_fill_the_page() {
    // One full page of noise plus a `next` link. The policy must not spend more
    // requests to backfill: source caps and pagination semantics stay as read.
    let noise: Vec<serde_json::Value> = (0..100)
        .map(|index| {
            event(
                &format!("n{index}"),
                "PushEvent",
                serde_json::json!({"ref": "refs/heads/main", "size": 1}),
            )
        })
        .collect();
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(
        events_body(noise),
        vec![(
            "Link",
            "<https://api.github.com/users/test-user/received_events?page=2>; rel=\"next\"",
        )],
    )))
    .await;
    mock.enqueue(Ok(ok(
        events_body(vec![event(
            "2",
            "WatchEvent",
            serde_json::json!({"action": "started"}),
        )]),
        vec![],
    )))
    .await;

    let FeedFetch::Events(outcome) = run(&mock, None, &FeedLimits::default()).await.expect("ok")
    else {
        panic!("expected events");
    };
    assert_eq!(outcome.events.len(), 1);
    let recorded = mock.recorded().await;
    assert_eq!(
        recorded.len(),
        2,
        "the normal page walk (1 → 2) is used, with no extra requests"
    );
    assert_eq!(query_of(&recorded[1].0, "page").as_deref(), Some("2"));
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
    assert!(
        outcome.events.is_empty(),
        "an unknown event type is not a Feed activity"
    );

    // The parser itself still tolerates it and deliberately keeps no payload, so
    // a future event shape cannot grow without bound.
    let (event_type, payload) = convert_payload(
        "SomeFutureEvent",
        &serde_json::json!({"secret": "x".repeat(1000)}),
    );
    assert_eq!(
        event_type,
        FeedEventType::Unknown("SomeFutureEvent".to_string())
    );
    assert_eq!(payload, FeedPayload::Unknown);
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
    assert!(
        outcome.events.is_empty(),
        "publishing a repository is not a retained Feed activity"
    );

    // A missing payload must not panic the parser either.
    let (event_type, payload) = convert_payload("PublicEvent", &serde_json::Value::Null);
    assert_eq!(event_type, FeedEventType::Public);
    assert_eq!(payload, FeedPayload::Public);
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
