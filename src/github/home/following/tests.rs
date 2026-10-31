use super::*;
use crate::network::transport::{HttpResponse, MockTransport};
use std::sync::Arc;

fn client(mock: Arc<MockTransport>) -> GitHubClient {
    GitHubClient::new(Default::default(), mock)
}

fn login(name: &str) -> Login {
    Login::new(name).expect("valid login")
}

fn pat() -> Pat {
    Pat::from_str("ghp_test")
}

fn account(id: u64, login: &str) -> serde_json::Value {
    // Trimmed but realistic `/user/following` item: the endpoint returns full
    // user objects, and the app only relies on id/login/avatar_url.
    serde_json::json!({
        "login": login,
        "id": id,
        "node_id": format!("MDQ6VXNlcj{id}"),
        "avatar_url": format!("https://avatars.githubusercontent.com/u/{id}?v=4"),
        "html_url": format!("https://github.com/{login}"),
        "type": "User",
        "site_admin": false
    })
}

fn body(accounts: Vec<serde_json::Value>) -> String {
    serde_json::to_string(&accounts).expect("json")
}

fn ok(body: String, link: Option<&str>) -> HttpResponse {
    let mut headers = Vec::new();
    if let Some(link) = link {
        headers.push(("link".to_string(), link.to_string()));
    }
    HttpResponse {
        status: 200,
        headers,
        body,
    }
}

fn query_of(url: &str, key: &str) -> Option<String> {
    Url::parse(url)
        .ok()?
        .query_pairs()
        .find_map(|(k, v)| (k.as_ref() == key).then(|| v.to_string()))
}

async fn run(mock: &Arc<MockTransport>, limits: &FollowingLimits) -> Result<Following, HomeError> {
    fetch_following(
        &client(mock.clone()),
        &pat(),
        &login("test-user"),
        limits,
        CancellationToken::default(),
    )
    .await
}

#[tokio::test]
async fn reads_the_authenticated_following_endpoint_with_paging_and_membership() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(
        body(vec![account(1, "followed-one"), account(2, "Followed-Two")]),
        Some(
            "<https://api.github.com/user/following?per_page=2&page=2>; rel=\"next\", \
             <https://api.github.com/user/following?per_page=2&page=9>; rel=\"last\"",
        ),
    )))
    .await;
    mock.enqueue(Ok(ok(body(vec![account(3, "followed-three")]), None)))
        .await;

    let following = run(
        &mock,
        &FollowingLimits {
            per_page: 2,
            ..FollowingLimits::default()
        },
    )
    .await
    .expect("ok");
    assert_eq!(following.readiness, FollowingReadiness::Complete);
    assert_eq!(following.len(), 3);
    assert!(following.follows("followed-one"));
    // GitHub logins are case-insensitive.
    assert!(following.follows("followed-two"));
    assert!(following.follows("FOLLOWED-THREE"));
    assert!(!following.follows("not-followed"));
    assert!(!following.follows(""));

    let recorded = mock.recorded().await;
    assert_eq!(recorded.len(), 2);
    assert!(
        recorded[0].0.contains("/user/following"),
        "must read the authenticated list, got {}",
        recorded[0].0
    );
    // The live endpoint's own next link is `/user/{id}/following`; the adapter
    // rebuilds from its own identity instead of trusting the link's path.
    assert!(recorded[1].0.contains("/user/following"));
    assert_eq!(query_of(&recorded[1].0, "page").as_deref(), Some("2"));
}

#[tokio::test]
async fn empty_following_list_is_a_complete_negative_answer() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(body(vec![]), None))).await;
    let following = run(&mock, &FollowingLimits::default()).await.expect("ok");
    assert!(following.is_empty());
    // Only a complete empty list means "follows nobody".
    assert_eq!(following.readiness, FollowingReadiness::Complete);
    assert!(!following.follows("anyone"));
}

#[tokio::test]
async fn a_later_page_failure_is_partial_not_a_negative_answer() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(
        body(vec![account(1, "followed-one")]),
        Some("<https://api.github.com/user/following?page=2>; rel=\"next\""),
    )))
    .await;
    mock.enqueue(Ok(HttpResponse {
        status: 503,
        headers: vec![],
        body: "{}".to_string(),
    }))
    .await;

    let following = run(&mock, &FollowingLimits::default())
        .await
        .expect("partial outcome");
    assert_eq!(following.len(), 1);
    assert!(matches!(
        following.readiness,
        FollowingReadiness::Partial { .. }
    ));
    // The subject that was read is still positively known.
    assert!(following.follows("followed-one"));
}

#[tokio::test]
async fn a_first_page_failure_is_a_hard_error_not_an_empty_list() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(HttpResponse {
        status: 401,
        headers: vec![],
        body: "{}".to_string(),
    }))
    .await;
    let error = run(&mock, &FollowingLimits::default())
        .await
        .expect_err("401 must fail");
    assert!(matches!(
        error,
        HomeError::GitHub(crate::domain::error::GitHubError::Unauthorized)
    ));
}

#[tokio::test]
async fn truncated_pagination_budgets_are_partial() {
    // Local page budget.
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(
        body(vec![account(1, "a")]),
        Some("<https://api.github.com/user/following?page=2>; rel=\"next\""),
    )))
    .await;
    let following = run(
        &mock,
        &FollowingLimits {
            max_pages: 1,
            ..FollowingLimits::default()
        },
    )
    .await
    .expect("ok");
    assert!(matches!(
        following.readiness,
        FollowingReadiness::Partial { .. }
    ));

    // Local account budget.
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(body(vec![account(1, "a"), account(2, "b")]), None)))
        .await;
    let following = run(
        &mock,
        &FollowingLimits {
            max_accounts: 1,
            ..FollowingLimits::default()
        },
    )
    .await
    .expect("ok");
    assert_eq!(following.len(), 1);
    assert!(matches!(
        following.readiness,
        FollowingReadiness::Partial { .. }
    ));
}

#[tokio::test]
async fn untrusted_or_cycling_pagination_never_extends_the_set() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(
        body(vec![account(1, "a")]),
        Some("<https://evil.example/user/following?page=2>; rel=\"next\""),
    )))
    .await;
    let following = run(&mock, &FollowingLimits::default())
        .await
        .expect("partial");
    assert!(matches!(
        following.readiness,
        FollowingReadiness::Partial { .. }
    ));

    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(
        body(vec![account(1, "a")]),
        Some("<https://api.github.com/user/following?page=1>; rel=\"next\""),
    )))
    .await;
    let following = run(&mock, &FollowingLimits::default())
        .await
        .expect("partial");
    assert!(matches!(
        following.readiness,
        FollowingReadiness::Partial { .. }
    ));
}

#[tokio::test]
async fn entries_without_a_login_are_not_membership_evidence() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(ok(
        body(vec![
            account(1, "real-follow"),
            serde_json::json!({"id": 2, "login": "   "}),
        ]),
        None,
    )))
    .await;
    let following = run(&mock, &FollowingLimits::default()).await.expect("ok");
    assert_eq!(following.len(), 1);
    assert!(following.follows("real-follow"));
}

#[tokio::test]
async fn cancellation_is_a_hard_error() {
    let mock = Arc::new(MockTransport::new());
    let (cancel, source) = CancellationToken::new();
    source.cancel();
    let result = fetch_following(
        &client(mock.clone()),
        &pat(),
        &login("test-user"),
        &FollowingLimits::default(),
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

#[tokio::test]
async fn invalid_limits_are_rejected_without_network() {
    let mock = Arc::new(MockTransport::new());
    let error = run(
        &mock,
        &FollowingLimits {
            per_page: 0,
            ..FollowingLimits::default()
        },
    )
    .await
    .expect_err("per_page 0 must fail");
    assert!(matches!(
        error,
        HomeError::InvalidInput { field: "per_page" }
    ));
    assert!(mock.recorded().await.is_empty());
}
