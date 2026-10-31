use super::*;

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
