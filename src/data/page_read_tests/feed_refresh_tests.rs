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
    // Credential loads run on blocking workers, so request arrival order is
    // deliberately not part of this fixture's contract.
    let block_detail = |number| {
        transport.block_url(
            format!("https://api.github.com/repos/acme/widgets/pulls/{number}"),
            200,
            detail(number),
        )
    };
    let gates: Vec<_> = (1..=4).map(block_detail).collect();
    let (fifth_entered, fifth_release) = block_detail(5);
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

/// Two starred cards for the same repository must only issue one repository detail
/// lookup (deduplication), and an optional lookup failure (e.g. 404) must return None
/// rather than failing the caller.
#[tokio::test]
async fn repository_descriptions_deduplicates_requests_and_handles_failures() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(&temp, transport.clone(), 4).await;
    login(&svc, &transport, 1, "viewer", "viewer-token").await;
    let session = svc.page_session().unwrap();

    // acme/demo succeeds with a description; acme/missing fails with 404.
    let demo_repo = json!({
        "id": 10, "name": "demo", "full_name": "acme/demo",
        "owner": {"id": 1, "login": "acme", "avatar_url": null},
        "visibility": "public", "private": false, "default_branch": "main",
        "fork": false, "archived": false, "stargazers_count": 42,
        "forks_count": 2, "open_issues_count": 0, "size": 100,
        "html_url": "https://github.com/acme/demo",
        "description": "Towards a More Autonomous Agent System"
    });
    let (demo_entered, demo_release) = transport.block_url(
        "https://api.github.com/repos/acme/demo".to_string(),
        200,
        demo_repo,
    );
    let (missing_entered, missing_release) = transport.block_url(
        "https://api.github.com/repos/acme/missing".to_string(),
        404,
        json!({"message": "Not Found"}),
    );

    // Pass duplicates for acme/demo plus acme/missing.
    let repos = vec![
        ("acme".to_string(), "demo".to_string()),
        ("acme".to_string(), "demo".to_string()),
        ("acme".to_string(), "missing".to_string()),
    ];
    let read = tokio::spawn(async move {
        session
            .repository_descriptions(repos, 20, Default::default())
            .await
    });
    // Both requests must be in flight before either can finish. Release them in
    // the opposite order from the input without assuming their arrival order.
    ready(demo_entered).await;
    ready(missing_entered).await;
    assert!(!read.is_finished());
    missing_release.send(()).unwrap();
    demo_release.send(()).unwrap();
    let descriptions = read.await.unwrap().expect("repository descriptions").value;

    assert_eq!(descriptions.len(), 2);
    assert_eq!(
        descriptions.get("acme/demo"),
        Some(&Some("Towards a More Autonomous Agent System".to_string()))
    );
    assert_eq!(descriptions.get("acme/missing"), Some(&None));

    let requests = transport.requests.lock().unwrap();
    // 1 login + 2 detail requests (acme/demo requested only once, despite appearing twice!).
    assert_eq!(
        requests.len(),
        3,
        "detail requests must be deduplicated: {:?}",
        requests.iter().map(|r| &r.url).collect::<Vec<_>>()
    );
    for url in [
        "https://api.github.com/repos/acme/demo",
        "https://api.github.com/repos/acme/missing",
    ] {
        assert_eq!(
            requests.iter().filter(|request| request.url == url).count(),
            1,
            "each repository identity must be requested exactly once"
        );
    }
}

/// Merged PR detail response body is parsed and extracted into a compact plain-text
/// BodyPreview on the activity target.
#[tokio::test]
async fn feed_preserves_pr_body_and_extracts_clean_preview() {
    let temp = tempfile::tempdir().unwrap();
    let transport = Arc::new(Controlled::default());
    let svc = service(&temp, transport.clone(), 4).await;
    login(&svc, &transport, 1, "viewer", "viewer-token").await;
    let session = svc.page_session().unwrap();

    // Merged PR event in received_events.
    let merged_event = json!({
        "id": "evt-merged",
        "type": "PullRequestEvent",
        "actor": {"id": 10, "login": "merger"},
        "repo": {"id": 99, "name": "acme/widgets"},
        "payload": {
            "action": "merged",
            "number": 77,
            "pull_request": {
                "url": "https://api.github.com/repos/acme/widgets/pulls/77",
                "id": 777, "number": 77,
                "head": {"ref": "x", "sha": "a"}, "base": {"ref": "main", "sha": "b"}
            }
        },
        "created_at": "2026-10-30T12:00:41Z"
    });
    transport.push(200, json!([merged_event]));
    transport.push(200, json!([{"id": 31, "login": "contributor"}]));
    // PR detail response with a Markdown body containing HTML comments and styling.
    transport.push(
        200,
        json!({
            "id": 777,
            "number": 77,
            "state": "closed",
            "merged": true,
            "title": "Fix memory leak in parser",
            "html_url": "https://github.com/acme/widgets/pull/77",
            "user": {"id": 31, "login": "contributor"},
            "body": "<!-- hidden comment -->\n## Changes\nThis fixes **critical** memory leak in `tokenizer`.\n\nTested with all fixtures."
        }),
    );

    let feed = feed_call(&session, &Default::default(), Default::default())
        .await
        .expect("feed")
        .value;
    assert_eq!(feed.activities.len(), 1);
    let target = feed.activities[0].pull_request.as_ref().unwrap();
    assert_eq!(target.number, Some(77));
    assert_eq!(target.title.as_deref(), Some("Fix memory leak in parser"));
    let preview = target.body_preview.as_ref().expect("body preview");
    assert_eq!(
        preview.excerpt,
        "Changes This fixes critical memory leak in tokenizer. Tested with all fixtures."
    );
    assert!(!preview.is_truncated);

    // Initial feed publication was independent of repository descriptions: no /repos/* request was made.
    let requests = transport.requests.lock().unwrap();
    assert!(
        requests
            .iter()
            .all(|r| !r.url.contains("/repos/acme/widgets") || r.url.contains("/pulls/77"))
    );
}
