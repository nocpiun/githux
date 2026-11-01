use super::*;
use crate::network::transport::{HttpResponse, MockTransport};
use std::sync::Arc;

fn client(mock: Arc<MockTransport>) -> GitHubClient {
    GitHubClient::new(Default::default(), mock)
}

fn pat() -> Pat {
    Pat::from_str("ghp_test")
}

/// The real pull-request detail response shape, trimmed to the fields the app
/// reads. Verified against a public `GET /repos/{owner}/{repo}/pulls/{number}`
/// response (API version 2022-11-28) at 2026-10-30.
fn detail_json(number: u64, author: Option<(&str, u64)>, merged: bool) -> String {
    let user = author.map_or(serde_json::Value::Null, |(login, id)| {
        serde_json::json!({
            "login": login,
            "id": id,
            "avatar_url": format!("https://avatars.githubusercontent.com/u/{id}?v=4"),
            "type": "User"
        })
    });
    serde_json::json!({
        "id": 4_838_297_932u64,
        "number": number,
        "state": if merged { "closed" } else { "open" },
        "merged": merged,
        "merged_at": if merged { serde_json::json!("2026-10-15T10:13:06Z") } else { serde_json::Value::Null },
        "title": "shearwater_predator_parser: gate GTR-mode sidemount detection by log version",
        "html_url": format!("https://github.com/subsurface/libdc/pull/{number}"),
        "user": user,
        "base": {"ref": "master", "sha": "8d3f629702a7de2f510186614e293260a1dd6f70"},
        "head": {"ref": "feature", "sha": "8cc8ec82a733ac545195ce5d6f04e8939cd706b1"}
    })
    .to_string()
}

async fn run(
    mock: &Arc<MockTransport>,
    owner: &str,
    repository: &str,
    number: u64,
) -> Result<PullRequestDetail, HomeError> {
    fetch_pull_request_detail(
        &client(mock.clone()),
        &pat(),
        owner,
        repository,
        number,
        &PullRequestDetailLimits::default(),
        CancellationToken::default(),
    )
    .await
}

#[tokio::test]
async fn resolves_the_author_from_the_documented_detail_endpoint() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(HttpResponse {
        status: 200,
        headers: vec![],
        body: detail_json(144, Some(("mikeller", 4_742_747)), true),
    }))
    .await;

    let detail = run(&mock, "subsurface", "libdc", 144).await.expect("ok");
    assert!(detail.merged);
    match detail.outcome {
        PullRequestDetailOutcome::Attributed(resolved) => {
            assert_eq!(resolved.author.login, "mikeller");
            assert_eq!(resolved.author.id, 4_742_747);
            assert!(resolved.merged);
            assert!(resolved.title.unwrap().contains("shearwater"));
        }
        other => panic!("expected an attributed author, got {other:?}"),
    }
    let recorded = mock.recorded().await;
    assert_eq!(recorded.len(), 1);
    assert!(
        recorded[0].0.ends_with("/repos/subsurface/libdc/pulls/144"),
        "got {}",
        recorded[0].0
    );
}

#[tokio::test]
async fn a_detail_without_a_user_is_unattributable_not_a_merger_fallback() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(HttpResponse {
        status: 200,
        headers: vec![],
        body: detail_json(144, None, true),
    }))
    .await;
    let detail = run(&mock, "subsurface", "libdc", 144).await.expect("ok");
    assert_eq!(detail.outcome, PullRequestDetailOutcome::Unattributable);
}

#[tokio::test]
async fn a_detail_that_is_not_merged_yields_no_merge_evidence() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(HttpResponse {
        status: 200,
        headers: vec![],
        body: detail_json(144, Some(("someone", 7)), false),
    }))
    .await;
    let detail = run(&mock, "subsurface", "libdc", 144).await.expect("ok");
    assert!(!detail.merged);
    match detail.outcome {
        PullRequestDetailOutcome::Attributed(resolved) => assert!(!resolved.merged),
        other => panic!("expected an attributed author, got {other:?}"),
    }
}

#[tokio::test]
async fn the_payload_url_is_never_used_for_the_request() {
    // The adapter is given owner/repository/number, so identity comes from the
    // parsed event and the URL is rebuilt from the trusted base. A hostile
    // repository segment cannot escape the path.
    let mock = Arc::new(MockTransport::new());
    for (owner, repository) in [
        ("owner", ".."),
        ("owner/x", "repo"),
        ("owner", "repo?next=https://evil.example"),
    ] {
        let result = fetch_pull_request_detail(
            &client(mock.clone()),
            &pat(),
            owner,
            repository,
            1,
            &PullRequestDetailLimits::default(),
            CancellationToken::default(),
        )
        .await;
        assert!(
            matches!(result, Err(HomeError::InvalidInput { .. })),
            "{owner}/{repository} must be rejected, got {result:?}"
        );
    }
    assert!(mock.recorded().await.is_empty());

    // A trusted base with an absolute URL string is never accepted as identity.
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(HttpResponse {
        status: 200,
        headers: vec![],
        body: detail_json(1, Some(("a", 1)), true),
    }))
    .await;
    run(&mock, "owner", "repo", 1).await.expect("ok");
    let recorded = mock.recorded().await;
    assert_eq!(
        recorded[0].0,
        "https://api.github.com/repos/owner/repo/pulls/1"
    );
}

#[tokio::test]
async fn a_mismatched_number_is_not_usable_evidence() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(HttpResponse {
        status: 200,
        headers: vec![],
        body: detail_json(999, Some(("someone", 7)), true),
    }))
    .await;
    let error = run(&mock, "subsurface", "libdc", 144)
        .await
        .expect_err("mismatched number must fail");
    assert!(matches!(
        error,
        HomeError::GitHub(crate::domain::error::GitHubError::Parse { .. })
    ));
}

#[tokio::test]
async fn failures_and_cancellation_are_typed() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(HttpResponse {
        status: 404,
        headers: vec![],
        body: "{}".to_string(),
    }))
    .await;
    let error = run(&mock, "owner", "repo", 1)
        .await
        .expect_err("404 must fail");
    assert!(matches!(
        error,
        HomeError::GitHub(crate::domain::error::GitHubError::NotFound)
    ));

    let mock = Arc::new(MockTransport::new());
    let (cancel, source) = CancellationToken::new();
    source.cancel();
    let result = fetch_pull_request_detail(
        &client(mock.clone()),
        &pat(),
        "owner",
        "repo",
        1,
        &PullRequestDetailLimits::default(),
        cancel,
    )
    .await;
    assert!(matches!(
        result,
        Err(HomeError::GitHub(
            crate::domain::error::GitHubError::Cancelled
        ))
    ));
    assert!(mock.recorded().await.is_empty());
}
