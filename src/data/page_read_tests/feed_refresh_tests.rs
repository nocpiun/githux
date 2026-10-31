use super::*;

#[tokio::test]
async fn author_reads_overlap_with_four_workers_and_preserve_source_order_and_budget() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    // More service permits than workers: the Feed must impose its own bound.
    let svc = service(&temp, transport.clone(), 8).await;
    login(&svc, &transport, 1, "viewer", "viewer-token").await;
    let session = svc.page_session().unwrap();
    let merged = |number| {
        json!({
            "id":format!("merge-{number}"), "type":"PullRequestEvent",
            "actor":{"id":10,"login":"maintainer"},
            "repo":{"id":99,"name":"acme/widgets"},
            "payload":{"action":"merged", "number":number,
                "pull_request":{"id":number,"number":number,"head":{},"base":{}}}
        })
    };
    let detail = |number| {
        json!({
            "id":number,"number":number,"merged":true,
            "user":{"id":31,"login":"friend"},"title":format!("PR {number}")
        })
    };
    // Duplicate targets must not reserve a second worker or spend the budget.
    transport.push(
        200,
        json!([
            merged(1),
            merged(1),
            merged(2),
            merged(3),
            merged(4),
            merged(5),
            merged(6)
        ]),
    );
    transport.push(200, json!([{"id":31,"login":"friend"}]));
    let gates: Vec<_> = (1..=4)
        .map(|number| transport.block(200, detail(number)))
        .collect();
    let (fifth_entered, fifth_release) = transport.block(200, detail(5));
    let read = tokio::spawn(async move {
        session
            .feed(
                &Default::default(),
                &Default::default(),
                &Default::default(),
                5,
                Default::default(),
            )
            .await
    });
    let mut releases = Vec::new();
    for (entered, release) in gates {
        // These all enter before ANY response is released. Serial code times out.
        ready(entered).await;
        releases.push(release);
    }
    assert_eq!(
        transport.count(),
        7,
        "login + events + following + four details"
    );
    assert!(!read.is_finished());
    // A fast worker starts the fifth lookup while the first three are blocked.
    releases.pop().unwrap().send(()).unwrap();
    ready(fifth_entered).await;
    fifth_release.send(()).unwrap();
    for release in releases.into_iter().rev() {
        release.send(()).unwrap();
    }
    let outcome = read.await.unwrap().unwrap().value;
    assert_eq!(
        outcome
            .activities
            .iter()
            .map(|a| a.event.id.as_str())
            .collect::<Vec<_>>(),
        ["merge-1", "merge-2", "merge-3", "merge-4", "merge-5"]
    );
    assert!(
        matches!(outcome.readiness, FeedReadiness::Partial { ref reasons }
        if reasons.contains(&FeedPartialReason::AuthorUnresolved { count: 1 }))
    );
    assert_eq!(
        transport.count(),
        8,
        "only five distinct details within the budget"
    );
}

#[tokio::test]
async fn author_budget_withholds_remaining_contributions_and_never_backfills() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(&temp, transport.clone(), 2).await;
    login(&svc, &transport, 1, "viewer", "viewer-token").await;
    let session = svc.page_session().unwrap();
    let merged = |number| {
        json!({
            "id":format!("merge-{number}"), "type":"PullRequestEvent",
            "actor":{"id":10,"login":"maintainer"},
            "repo":{"id":99,"name":"acme/widgets"},
            "payload":{"action":"merged", "number":number,
                "pull_request":{"id":number,"number":number,"head":{},"base":{}}}
        })
    };
    transport.push(200, json!([merged(42), merged(43)]));
    transport.push(200, json!([{"id":31,"login":"friend"}]));
    transport.push(
        200,
        json!({
            "id":42,"number":42,"merged":true,
            "user":{"id":31,"login":"friend"},"title":"Followed contribution"
        }),
    );
    let outcome = session
        .feed(
            &Default::default(),
            &Default::default(),
            &Default::default(),
            1,
            Default::default(),
        )
        .await
        .unwrap()
        .value;
    assert_eq!(outcome.activities.len(), 1);
    assert_eq!(outcome.activities[0].event.id, "merge-42");
    assert!(
        matches!(outcome.readiness, FeedReadiness::Partial { ref reasons }
        if reasons.contains(&FeedPartialReason::AuthorUnresolved { count:1 }))
    );
    assert!(outcome.etag.is_none());
    assert_eq!(
        transport.count(),
        4,
        "login + events + following + one detail"
    );
}

fn star() -> Value {
    json!([{
        "id":"star", "type":"WatchEvent",
        "actor":{"id":31,"login":"friend"},
        "repo":{"id":99,"name":"acme/widgets"},
        "payload":{"action":"started"},
        "created_at":"2026-10-30T12:00:00Z"
    }])
}

#[tokio::test]
async fn refresh_reloads_membership_so_an_unfollowed_subject_disappears() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(&temp, transport.clone(), 2).await;
    login(&svc, &transport, 1, "viewer", "viewer-token").await;
    let session = svc.page_session().unwrap();
    transport.push(200, star());
    transport.push(200, json!([{"id":31,"login":"friend"}]));
    let first = feed_call(&session, &Default::default(), Default::default())
        .await
        .unwrap();
    assert_eq!(first.value.activities.len(), 1);
    transport.push(200, star());
    transport.push(200, json!([]));
    let second = feed_call(&session, &Default::default(), Default::default())
        .await
        .unwrap();
    assert!(second.value.activities.is_empty());
    assert!(second.value.readiness.is_complete());
    let requests = transport.requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.url.contains("/user/following"))
            .count(),
        2
    );
    assert!(requests.iter().all(|r| {
        !r.headers
            .iter()
            .any(|(key, _)| key.eq_ignore_ascii_case("if-none-match"))
    }));
}

#[tokio::test]
async fn switching_accounts_while_following_is_delayed_rejects_the_whole_feed() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(&temp, transport.clone(), 2).await;
    login(&svc, &transport, 2, "other", "other-token").await;
    login(&svc, &transport, 1, "viewer", "viewer-token").await;
    let session = svc.page_session().unwrap();
    transport.push(200, star());
    let (entered, release) = transport.block(200, json!([{"id":31,"login":"friend"}]));
    let task =
        tokio::spawn(
            async move { feed_call(&session, &Default::default(), Default::default()).await },
        );
    ready(entered).await;
    svc.set_active(key(2)).await.unwrap();
    svc.set_active(key(1)).await.unwrap();
    release.send(()).unwrap();
    assert_eq!(
        task.await.unwrap().unwrap_err(),
        HomeError::GitHub(GitHubError::StaleContext)
    );
}

#[tokio::test]
async fn cancelled_delayed_following_does_not_publish_a_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(&temp, transport.clone(), 2).await;
    login(&svc, &transport, 1, "viewer", "viewer-token").await;
    let session = svc.page_session().unwrap();
    transport.push(200, star());
    let (entered, release) = transport.block(200, json!([{"id":31,"login":"friend"}]));
    let (cancel, source) = CancellationToken::new();
    let task = tokio::spawn(async move { feed_call(&session, &Default::default(), cancel).await });
    ready(entered).await;
    source.cancel();
    release.send(()).unwrap();
    assert_eq!(
        task.await.unwrap().unwrap_err(),
        HomeError::GitHub(GitHubError::Cancelled)
    );
}

fn starred_page(ids: &[&str]) -> Value {
    let events: Vec<Value> = ids
        .iter()
        .map(|id| {
            json!({
                "id": id, "type": "WatchEvent",
                "actor": {"id": 31, "login": "friend"},
                "repo": {"id": 99, "name": "acme/widgets"},
                "payload": {"action": "started"},
                "created_at": "2026-10-30T12:00:00Z"
            })
        })
        .collect();
    json!(events)
}

fn with_link(body: Value, link: Option<&str>) -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: link
            .map(|l| vec![("link".to_string(), l.to_string())])
            .unwrap_or_default(),
        body: body.to_string(),
    }
}

/// The real incremental path: page 1 exposes page 2, which is filtered-empty but
/// page 3 has eligible data. The cursor must survive the empty middle page, and
/// each append re-reads membership and deduplicates by source id.
#[tokio::test]
async fn filtered_empty_page_keeps_continuation_and_later_page_appends() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(&temp, transport.clone(), 4).await;
    login(&svc, &transport, 1, "viewer", "viewer-token").await;
    let session = svc.page_session().unwrap();

    // First page: one eligible star + next link.
    transport.push_response(with_link(
        starred_page(&["s1"]),
        Some("<https://api.github.com/users/viewer/received_events?per_page=100&page=2>; rel=\"next\""),
    ));
    transport.push(200, json!([{"id":31,"login":"friend"}]));

    let first = feed_call(&session, &Default::default(), Default::default())
        .await
        .unwrap()
        .value;
    assert_eq!(first.activities.len(), 1);
    assert_eq!(
        first.next_page,
        Some(crate::domain::home::FeedSourceCursor { page: 2 })
    );

    // Page 2: only technical noise (push), but a next link still exists.
    transport.push_response(with_link(
        json!([{
            "id": "noise", "type": "PushEvent",
            "actor": {"id": 31, "login": "friend"},
            "repo": {"id": 99, "name": "acme/widgets"},
            "payload": {"ref": "refs/heads/main", "size": 1},
            "created_at": "2026-10-30T11:00:00Z"
        }]),
        Some("<https://api.github.com/users/viewer/received_events?per_page=100&page=3>; rel=\"next\""),
    ));
    transport.push(200, json!([{"id":31,"login":"friend"}]));

    let mut seen = std::collections::HashSet::new();
    seen.insert("s1".to_string());
    let page2 = session
        .feed_page(
            crate::domain::home::FeedSourceCursor { page: 2 },
            &Default::default(),
            &Default::default(),
            &Default::default(),
            4,
            &mut seen,
            Default::default(),
        )
        .await
        .unwrap()
        .value;
    assert!(page2.activities.is_empty());
    assert_eq!(
        page2.next_page,
        Some(crate::domain::home::FeedSourceCursor { page: 3 }),
        "a filtered-empty page is not end-of-history"
    );

    // Page 3: one eligible star + no next link.
    transport.push_response(with_link(starred_page(&["s2"]), None));
    transport.push(200, json!([{"id":31,"login":"friend"}]));
    let page3 = session
        .feed_page(
            crate::domain::home::FeedSourceCursor { page: 3 },
            &Default::default(),
            &Default::default(),
            &Default::default(),
            4,
            &mut seen,
            Default::default(),
        )
        .await
        .unwrap()
        .value;
    assert_eq!(page3.activities.len(), 1);
    assert_eq!(page3.activities[0].event.id, "s2");
    assert_eq!(page3.next_page, None, "true exhaustion");
}

/// Appending an overlapping page must not duplicate cards, and a failed page
/// leaves the cursor retryable (the host retries the same page).
#[tokio::test]
async fn append_deduplicates_overlap_and_failure_preserves_cursor() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(&temp, transport.clone(), 4).await;
    login(&svc, &transport, 1, "viewer", "viewer-token").await;
    let session = svc.page_session().unwrap();

    transport.push_response(with_link(
        starred_page(&["s1"]),
        Some("<https://api.github.com/users/viewer/received_events?per_page=100&page=2>; rel=\"next\""),
    ));
    transport.push(200, json!([{"id":31,"login":"friend"}]));
    let first = feed_call(&session, &Default::default(), Default::default())
        .await
        .unwrap()
        .value;

    // Page 2 overlaps `s1` and adds `s2`.
    transport.push_response(with_link(
        json!([
            {"id": "s1", "type": "WatchEvent", "actor": {"id":31,"login":"friend"},
             "repo": {"id":99,"name":"acme/widgets"}, "payload":{"action":"started"},
             "created_at":"2026-10-30T12:00:00Z"},
            {"id": "s2", "type": "WatchEvent", "actor": {"id":31,"login":"friend"},
             "repo": {"id":99,"name":"acme/widgets"}, "payload":{"action":"started"},
             "created_at":"2026-10-30T11:00:00Z"}
        ]),
        None,
    ));
    transport.push(200, json!([{"id":31,"login":"friend"}]));
    let mut seen = std::collections::HashSet::new();
    seen.insert("s1".to_string());
    let page2 = session
        .feed_page(
            crate::domain::home::FeedSourceCursor { page: 2 },
            &Default::default(),
            &Default::default(),
            &Default::default(),
            4,
            &mut seen,
            Default::default(),
        )
        .await
        .unwrap()
        .value;
    assert_eq!(page2.activities.len(), 1);
    assert_eq!(page2.activities[0].event.id, "s2");
    // Fold into the retained outcome and confirm no duplicate.
    let mut merged = first;
    merged.activities.extend(page2.activities);
    assert_eq!(merged.activities.len(), 2);
    let ids: Vec<&str> = merged
        .activities
        .iter()
        .map(|activity| activity.event.id.as_str())
        .collect();
    assert_eq!(ids, vec!["s1", "s2"]);
}

/// A refresh replaces the first-page snapshot and must reseed the dedup set, so
/// previously viewed older activities become reachable again by scrolling.
#[tokio::test]
async fn refresh_reseeds_the_dedup_set_so_older_pages_can_be_reappended() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(&temp, transport.clone(), 2).await;
    login(&svc, &transport, 1, "viewer", "viewer-token").await;
    let session = svc.page_session().unwrap();

    // First read: page 1 with next, then following.
    transport.push_response(with_link(
        starred_page(&["s1"]),
        Some("<https://api.github.com/users/viewer/received_events?per_page=100&page=2>; rel=\"next\""),
    ));
    transport.push(200, json!([{"id":31,"login":"friend"}]));
    let first = feed_call(&session, &Default::default(), Default::default())
        .await
        .unwrap()
        .value;
    assert_eq!(first.activities.len(), 1);

    // Refresh: same page-1 body again (no next link this time), following again.
    transport.push_response(with_link(starred_page(&["s1"]), None));
    transport.push(200, json!([{"id":31,"login":"friend"}]));
    let refreshed = feed_call(&session, &Default::default(), Default::default())
        .await
        .unwrap()
        .value;
    assert!(refreshed.activities.len() == 1);
    assert_eq!(refreshed.next_page, None);
}
