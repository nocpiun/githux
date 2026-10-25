use super::*;
use crate::domain::repository::*;
use crate::github::{discover_readme, fetch_branches, fetch_directory, resolve_revision};
use crate::network::test_server::{LocalHttpServer, ScriptedResponse};
use crate::network::transport::{MockFailure, MockTransport, ReqwestTransport, json_response};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const ROOT: &str = "1111111111111111111111111111111111111111";
const CHILD: &str = "2222222222222222222222222222222222222222";
const BLOB: &str = "3333333333333333333333333333333333333333";

fn identity() -> RepositoryIdentity {
    RepositoryIdentity {
        id: 42,
        locator: RepositoryLocator::new("octo", "repo").unwrap(),
    }
}
fn snapshot() -> RepoSnapshot {
    RepoSnapshot {
        repository: identity(),
        requested: RevisionSpec::Ref(DynamicRef::new("main").unwrap()),
        commit: ObjectSha::new(A).unwrap(),
        root_tree: ObjectSha::new(ROOT).unwrap(),
        message: "Commit A".into(),
        author: None,
        committer: None,
    }
}
fn repository_json() -> Value {
    json!({"id":42,"name":"repo","owner":{"id":7,"login":"octo"},"private":false,"visibility":"public","default_branch":"main","fork":false,"archived":false,"stargazers_count":5,"forks_count":2,"open_issues_count":3,"size":12,"html_url":"https://github.com/octo/repo"})
}
fn branches_json(names: &[&str], sha: &str) -> Value {
    Value::Array(
        names
            .iter()
            .map(|name| json!({"name":name,"commit":{"sha":sha},"protected":false}))
            .collect(),
    )
}
fn commit_json(sha: &str) -> Value {
    json!({"sha":sha,"commit":{"message":format!("Commit {sha}"),"tree":{"sha":ROOT},"author":{"name":"Alice","email":"a@example.invalid","date":"2026-10-25T00:00:00Z"},"committer":null}})
}
fn entry(path: &str, git_type: &str, mode: &str, sha: &str) -> Value {
    json!({"path":path,"type":git_type,"mode":mode,"sha":sha,"size":12,"url":"https://evil.invalid/ignore"})
}
fn tree_json(sha: &str, entries: Vec<Value>, truncated: bool) -> Value {
    json!({"sha":sha,"tree":entries,"truncated":truncated})
}
fn readme_json() -> Value {
    json!({"type":"file","path":"docs/README 空间.md","sha":BLOB,"size":12,"download_url":"https://evil.invalid/never-fetch","content":"ignored-not-decoded"})
}
fn client(mock: Arc<MockTransport>) -> GitHubClient {
    GitHubClient::new(Default::default(), mock)
}
fn pat() -> Pat {
    Pat::from_str("synthetic-local-test")
}
fn limits() -> RepositoryLimits {
    RepositoryLimits::default()
}
async fn enqueue(mock: &MockTransport, status: u16, body: Value) {
    mock.enqueue(Ok(json_response(status, &body.to_string())))
        .await;
}
async fn branch_page(mock: &MockTransport, body: Value, next: &str) {
    let mut response = json_response(200, &body.to_string());
    response
        .headers
        .push(("link".into(), format!("<{next}>; rel=\"next\"")));
    mock.enqueue(Ok(response)).await;
}

/// Real HTTP transport high-level example: repo -> default/selected branch ->
/// commit A -> root and child tree A -> README A, after observing main move B.
#[tokio::test]
async fn localhost_snapshot_pipeline_stays_on_a_after_branch_moves_to_b() {
    let server = LocalHttpServer::start(vec![]);
    let next = format!("{}repos/octo/repo/branches?page=2", server.base_url());
    server.script(vec![
        ScriptedResponse::json(200, &repository_json().to_string()),
        ScriptedResponse::json(200, &branches_json(&["main"], A).to_string())
            .with_header("Link", &format!("<{next}>; rel=\"next\"")),
        ScriptedResponse::json(
            200,
            &branches_json(&["main", "feature/空间 branch?x=#%"], A).to_string(),
        ),
        ScriptedResponse::json(200, &commit_json(A).to_string()),
        ScriptedResponse::json(200, &branches_json(&["main"], B).to_string()),
        ScriptedResponse::json(
            200,
            &tree_json(
                ROOT,
                vec![
                    entry("src", "tree", "040000", CHILD),
                    entry("README.md", "blob", "100644", BLOB),
                ],
                false,
            )
            .to_string(),
        ),
        ScriptedResponse::json(
            200,
            &tree_json(CHILD, vec![entry("main.rs", "blob", "100644", BLOB)], false).to_string(),
        ),
        ScriptedResponse::json(200, &readme_json().to_string()),
    ]);
    let transport = Arc::new(ReqwestTransport::new(Duration::from_secs(5)).unwrap());
    let client = GitHubClient::new(
        crate::github::GitHubConfig {
            base_url: server.base_url().into(),
            ..Default::default()
        },
        transport,
    );
    let token = pat();
    let cancel = CancellationToken::default();
    let repo = fetch_repository(
        &client,
        &token,
        &identity().locator,
        &limits(),
        cancel.clone(),
    )
    .await
    .unwrap();
    assert_eq!(repo.identity.id(), 42);
    assert_eq!(
        (repo.stars, repo.forks, repo.open_issues_and_prs),
        (5, 2, 3)
    );
    let branches = fetch_branches(&client, &token, &repo.identity, &limits(), cancel.clone())
        .await
        .unwrap();
    assert_eq!(branches.branches.len(), 2);
    assert_eq!(branches.completeness, Completeness::Complete);
    let selected = repo.default_branch.unwrap();
    let snap = resolve_revision(
        &client,
        &token,
        &repo.identity,
        RevisionSpec::Ref(selected),
        &limits(),
        cancel.clone(),
    )
    .await
    .unwrap();
    assert_eq!(snap.commit().as_str(), A);
    assert_eq!(snap.author().unwrap().name.as_deref(), Some("Alice"));
    let moved = fetch_branches(&client, &token, &repo.identity, &limits(), cancel.clone())
        .await
        .unwrap();
    assert_eq!(moved.branches[0].observed_commit.as_str(), B);
    let tree = fetch_directory(
        &client,
        &token,
        &SnapshotDirectory::root(&snap),
        &limits(),
        cancel.clone(),
    )
    .await
    .unwrap();
    let child = tree.child_directory(0).unwrap();
    assert_eq!(child.path().unwrap().as_str(), "src");
    let subtree = fetch_directory(&client, &token, &child, &limits(), cancel.clone())
        .await
        .unwrap();
    assert_eq!(subtree.directory.commit().as_str(), A);
    let ReadmeDiscovery::Found(readme) = discover_readme(&client, &token, &snap, &limits(), cancel)
        .await
        .unwrap()
    else {
        panic!("expected descriptor")
    };
    assert_eq!(readme.commit.as_str(), A);
    assert_eq!(readme.blob.as_str(), BLOB);
    assert_eq!(readme.path.as_str(), "docs/README 空间.md");
    let recorded = server.recorded();
    let targets: Vec<_> = recorded.iter().map(|r| r.request_line.clone()).collect();
    assert_eq!(
        targets,
        vec![
            "GET /repos/octo/repo HTTP/1.1".into(),
            "GET /repos/octo/repo/branches?per_page=100 HTTP/1.1".into(),
            "GET /repos/octo/repo/branches?page=2 HTTP/1.1".into(),
            "GET /repos/octo/repo/commits/main HTTP/1.1".into(),
            "GET /repos/octo/repo/branches?per_page=100 HTTP/1.1".into(),
            format!("GET /repos/octo/repo/git/trees/{ROOT} HTTP/1.1"),
            format!("GET /repos/octo/repo/git/trees/{CHILD} HTTP/1.1"),
            format!("GET /repos/octo/repo/readme?ref={A} HTTP/1.1"),
        ]
    );
    for request in recorded {
        assert_eq!(
            request.header("authorization"),
            Some("Bearer synthetic-local-test")
        );
        assert_eq!(
            request.header("accept"),
            Some("application/vnd.github+json")
        );
        assert_eq!(request.header("x-github-api-version"), Some("2022-11-28"));
        assert_eq!(
            request.header("user-agent"),
            Some(client.config().user_agent.as_str())
        );
    }
}

#[tokio::test]
async fn localhost_ref_and_repository_segments_are_encoded_without_path_or_query_injection() {
    let server = LocalHttpServer::start(vec![ScriptedResponse::json(
        200,
        &commit_json(A).to_string(),
    )]);
    let client = GitHubClient::new(
        crate::github::GitHubConfig {
            base_url: server.base_url().into(),
            ..Default::default()
        },
        Arc::new(ReqwestTransport::new(Duration::from_secs(5)).unwrap()),
    );
    let repository = RepositoryIdentity {
        id: 42,
        locator: RepositoryLocator::new("o?@#", "r 空%").unwrap(),
    };
    let reference = DynamicRef::new("heads/feature/空间 branch?x=#%\\@evil.invalid").unwrap();
    resolve_revision(
        &client,
        &pat(),
        &repository,
        RevisionSpec::Ref(reference),
        &limits(),
        CancellationToken::default(),
    )
    .await
    .unwrap();
    let target = &server.recorded()[0].request_line;
    assert_eq!(
        target,
        "GET /repos/o%3F@%23/r%20%E7%A9%BA%25/commits/heads%2Ffeature%2F%E7%A9%BA%E9%97%B4%20branch%3Fx=%23%25%5C@evil.invalid HTTP/1.1"
    );
}

#[test]
fn invalid_identifiers_are_rejected_before_network() {
    for sha in [
        "main",
        "abc",
        "../trees",
        "https://evil.invalid",
        "gggggggggggggggggggggggggggggggggggggggg",
    ] {
        assert!(ObjectSha::new(sha).is_err());
    }
    assert_eq!(ObjectSha::new(A.to_uppercase()).unwrap().as_str(), A);
    for path in [
        "",
        "/abs",
        "../README",
        "a/../b",
        "a//b",
        "a/./b",
        "C:\\README",
        "a\n",
    ] {
        assert!(RepositoryPath::new(path).is_err(), "{path}");
    }
    for name in [".", "..", "a/b", "a\\b", ""] {
        assert!(RepositoryLocator::new("owner", name).is_err());
    }
    for reference in ["", ".", "..", "heads/../x", "a\n"] {
        assert!(DynamicRef::new(reference).is_err());
    }
    assert!(RepositoryPath::new("docs/空间 ?#%.md").is_ok());
}

#[tokio::test]
async fn all_read_operations_preserve_auth_sso_limit_404_and_409_errors() {
    for (status, headers, expected) in [
        (
            401,
            vec![],
            RepositoryError::GitHub(GitHubError::Unauthorized),
        ),
        (403, vec![], RepositoryError::GitHub(GitHubError::Forbidden)),
        (
            403,
            vec![(
                "x-github-sso",
                "required; url=https://github.com/orgs/acme/sso",
            )],
            RepositoryError::GitHub(GitHubError::SsoRequired {
                authorization_url: Some("https://github.com/orgs/acme/sso".into()),
            }),
        ),
        (
            403,
            vec![("x-ratelimit-remaining", "0")],
            RepositoryError::GitHub(GitHubError::RateLimited {
                retry: crate::domain::error::RetryAfter {
                    kind: crate::domain::error::RateLimitKind::Primary,
                    seconds: None,
                },
            }),
        ),
        (
            429,
            vec![("retry-after", "17")],
            RepositoryError::GitHub(GitHubError::RateLimited {
                retry: crate::domain::error::RetryAfter {
                    kind: crate::domain::error::RateLimitKind::Secondary,
                    seconds: Some(17),
                },
            }),
        ),
        (404, vec![], RepositoryError::GitHub(GitHubError::NotFound)),
        (409, vec![], RepositoryError::EmptyOrUnavailable),
    ] {
        for operation in 0..5 {
            let mock = Arc::new(MockTransport::new());
            let mut response = json_response(status, "{}");
            response.headers = headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            mock.enqueue(Ok(response.clone())).await;
            // README 404 checks its immutable root; inaccessible remains an error.
            mock.enqueue(Ok(response)).await;
            let c = client(mock);
            let result = match operation {
                0 => fetch_repository(
                    &c,
                    &pat(),
                    identity().locator(),
                    &limits(),
                    CancellationToken::default(),
                )
                .await
                .map(|_| ()),
                1 => fetch_branches(
                    &c,
                    &pat(),
                    &identity(),
                    &limits(),
                    CancellationToken::default(),
                )
                .await
                .map(|_| ()),
                2 => resolve_revision(
                    &c,
                    &pat(),
                    &identity(),
                    RevisionSpec::Commit(ObjectSha::new(A).unwrap()),
                    &limits(),
                    CancellationToken::default(),
                )
                .await
                .map(|_| ()),
                3 => fetch_directory(
                    &c,
                    &pat(),
                    &SnapshotDirectory::root(&snapshot()),
                    &limits(),
                    CancellationToken::default(),
                )
                .await
                .map(|_| ()),
                _ => discover_readme(
                    &c,
                    &pat(),
                    &snapshot(),
                    &limits(),
                    CancellationToken::default(),
                )
                .await
                .map(|_| ()),
            };
            assert_eq!(
                result.unwrap_err(),
                expected,
                "operation {operation}, status {status}"
            );
        }
    }
}

#[tokio::test]
async fn every_network_operation_honors_pre_cancelled_token() {
    let mock = Arc::new(MockTransport::new());
    let c = client(mock.clone());
    let (cancel, source) = CancellationToken::new();
    source.cancel();
    let expected = RepositoryError::GitHub(GitHubError::Cancelled);
    assert_eq!(
        fetch_repository(&c, &pat(), identity().locator(), &limits(), cancel.clone())
            .await
            .unwrap_err(),
        expected
    );
    assert_eq!(
        fetch_branches(&c, &pat(), &identity(), &limits(), cancel.clone())
            .await
            .unwrap_err(),
        expected
    );
    assert_eq!(
        resolve_revision(
            &c,
            &pat(),
            &identity(),
            RevisionSpec::Ref(DynamicRef::new("main").unwrap()),
            &limits(),
            cancel.clone()
        )
        .await
        .unwrap_err(),
        expected
    );
    assert_eq!(
        fetch_directory(
            &c,
            &pat(),
            &SnapshotDirectory::root(&snapshot()),
            &limits(),
            cancel.clone()
        )
        .await
        .unwrap_err(),
        expected
    );
    assert_eq!(
        discover_readme(&c, &pat(), &snapshot(), &limits(), cancel)
            .await
            .unwrap_err(),
        expected
    );
    assert!(mock.recorded().await.is_empty());
}

#[tokio::test]
async fn later_pagination_errors_preserve_typed_failure_even_after_empty_first_page() {
    for first in [branches_json(&[], A), branches_json(&["main"], A)] {
        for failure in [
            Ok(json_response(401, "{}")),
            Ok(json_response(409, "{}")),
            Ok(json_response(200, "not-json")),
            Err(MockFailure::Timeout),
            Err(MockFailure::Network("offline".into())),
        ] {
            let mock = Arc::new(MockTransport::new());
            branch_page(
                &mock,
                first.clone(),
                "https://api.github.com/repos/octo/repo/branches?page=2",
            )
            .await;
            mock.enqueue(failure).await;
            let outcome = fetch_branches(
                &client(mock),
                &pat(),
                &identity(),
                &limits(),
                CancellationToken::default(),
            )
            .await
            .unwrap();
            assert!(
                matches!(outcome.completeness, Completeness::Partial { ref reasons } if matches!(reasons[0], IncompleteReason::Failed(_)))
            );
        }
    }
}

#[tokio::test]
async fn first_pagination_transport_and_parse_failure_are_errors() {
    for failure in [
        Ok(json_response(200, "not-json")),
        Err(MockFailure::Timeout),
        Err(MockFailure::Network("offline".into())),
    ] {
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(failure).await;
        assert!(
            fetch_branches(
                &client(mock),
                &pat(),
                &identity(),
                &limits(),
                CancellationToken::default()
            )
            .await
            .is_err()
        );
    }
}

#[tokio::test]
async fn unsafe_cyclic_malformed_pagination_and_limits_are_partial() {
    for (next, reason) in [
        (
            "https://evil.invalid/steal",
            IncompleteReason::UntrustedPagination,
        ),
        (
            "https://evil.invalid\\@api.github.com/steal",
            IncompleteReason::UntrustedPagination,
        ),
        (
            "https://api.github.com:8443/repos/octo/repo/branches?page=2",
            IncompleteReason::UntrustedPagination,
        ),
        (
            "https://api.github.com/user",
            IncompleteReason::InvalidPagination,
        ),
        (
            "https://api.github.com/repos/octo/repo/branches?per_page=100",
            IncompleteReason::PaginationCycle,
        ),
    ] {
        let mock = Arc::new(MockTransport::new());
        branch_page(&mock, branches_json(&["main"], A), next).await;
        let outcome = fetch_branches(
            &client(mock.clone()),
            &pat(),
            &identity(),
            &limits(),
            CancellationToken::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            outcome.completeness,
            Completeness::Partial {
                reasons: vec![reason]
            }
        );
        assert_eq!(mock.recorded().await.len(), 1);
    }
    for (configuration, expected) in [
        (
            RepositoryLimits {
                branch_pages: 1,
                ..limits()
            },
            IncompleteReason::PageLimit,
        ),
        (
            RepositoryLimits {
                branch_entries: 0,
                ..limits()
            },
            IncompleteReason::EntryLimit,
        ),
    ] {
        let mock = Arc::new(MockTransport::new());
        branch_page(
            &mock,
            branches_json(&["main"], A),
            "https://api.github.com/repos/octo/repo/branches?page=2",
        )
        .await;
        let outcome = fetch_branches(
            &client(mock.clone()),
            &pat(),
            &identity(),
            &configuration,
            CancellationToken::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            outcome.completeness,
            Completeness::Partial {
                reasons: vec![expected]
            }
        );
        assert_eq!(mock.recorded().await.len(), 1);
    }
    for (link, expected) in [
        (
            "broken; rel=\"next\"",
            Completeness::Partial {
                reasons: vec![IncompleteReason::InvalidPagination],
            },
        ),
        (
            "<https://api.github.com/repos/octo/repo/branches?page=1>; rel=\"prev\", broken; rel=\"next\"",
            Completeness::Partial {
                reasons: vec![IncompleteReason::InvalidPagination],
            },
        ),
        (
            "<https://api.github.com/repos/octo/repo/branches?page=1>; rel=prev",
            Completeness::Complete,
        ),
    ] {
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(json_response(200, "[]").with_test_header("link", link)))
            .await;
        let outcome = fetch_branches(
            &client(mock),
            &pat(),
            &identity(),
            &limits(),
            CancellationToken::default(),
        )
        .await
        .unwrap();
        assert_eq!(outcome.completeness, expected);
    }
}

trait TestHeader {
    fn with_test_header(self, name: &str, value: &str) -> Self;
}
impl TestHeader for HttpResponse {
    fn with_test_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

#[tokio::test]
async fn sso_partial_is_never_complete_or_silent_on_single_resource() {
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(json_response(
        200,
        &branches_json(&["main"], A).to_string(),
    )
    .with_test_header("x-github-sso", "partial-results; organizations=acme")))
        .await;
    let outcome = fetch_branches(
        &client(mock),
        &pat(),
        &identity(),
        &limits(),
        CancellationToken::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        outcome.completeness,
        Completeness::Partial {
            reasons: vec![IncompleteReason::SsoPartial {
                organizations: vec!["acme".into()]
            }]
        }
    );
    let mock = Arc::new(MockTransport::new());
    mock.enqueue(Ok(json_response(200, &repository_json().to_string())
        .with_test_header(
            "x-github-sso",
            "partial-results; organizations=acme",
        )))
    .await;
    assert!(matches!(
        fetch_repository(
            &client(mock),
            &pat(),
            identity().locator(),
            &limits(),
            CancellationToken::default()
        )
        .await,
        Err(RepositoryError::SsoPartial { .. })
    ));
}

#[tokio::test]
async fn tree_modes_unknown_truncation_and_local_limits_remain_explicit() {
    let entries = vec![
        entry("plain", "blob", "100644", BLOB),
        entry("exec", "blob", "100755", BLOB),
        entry("dir", "tree", "040000", CHILD),
        entry("sym", "blob", "120000", BLOB),
        entry("sub", "commit", "160000", B),
        entry("future", "other", "777777", BLOB),
    ];
    let mock = Arc::new(MockTransport::new());
    enqueue(&mock, 200, tree_json(ROOT, entries.clone(), true)).await;
    let result = fetch_directory(
        &client(mock),
        &pat(),
        &SnapshotDirectory::root(&snapshot()),
        &limits(),
        CancellationToken::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        result.entries[0].kind,
        TreeEntryKind::File { executable: false }
    );
    assert_eq!(
        result.entries[1].kind,
        TreeEntryKind::File { executable: true }
    );
    assert_eq!(result.entries[3].kind, TreeEntryKind::Symlink);
    assert_eq!(result.entries[4].kind, TreeEntryKind::Submodule);
    assert!(result.child_directory(3).is_err());
    assert!(result.child_directory(4).is_err());
    assert_eq!(result.truncated, Some(true));
    assert_eq!(
        result.completeness,
        Completeness::Partial {
            reasons: vec![
                IncompleteReason::ServerTruncated,
                IncompleteReason::UnknownEntryType
            ]
        }
    );
    for (configuration, expected) in [
        (
            RepositoryLimits {
                tree_entries: 1,
                ..limits()
            },
            IncompleteReason::EntryLimit,
        ),
        (
            RepositoryLimits {
                response_bytes: 1,
                ..limits()
            },
            IncompleteReason::ResponseLimit,
        ),
    ] {
        let mock = Arc::new(MockTransport::new());
        enqueue(&mock, 200, tree_json(ROOT, entries.clone(), false)).await;
        let result = fetch_directory(
            &client(mock),
            &pat(),
            &SnapshotDirectory::root(&snapshot()),
            &configuration,
            CancellationToken::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            result.completeness,
            Completeness::Partial {
                reasons: vec![expected]
            }
        );
    }
    let mock = Arc::new(MockTransport::new());
    enqueue(&mock, 200, json!({"sha":ROOT,"tree":[]})).await;
    let result = fetch_directory(
        &client(mock),
        &pat(),
        &SnapshotDirectory::root(&snapshot()),
        &limits(),
        CancellationToken::default(),
    )
    .await
    .unwrap();
    assert_eq!(result.truncated, None);
    assert_ne!(result.completeness, Completeness::Complete);
}

#[tokio::test]
async fn response_object_mismatch_and_unsafe_tree_or_readme_paths_fail() {
    for body in [
        tree_json(CHILD, vec![], false),
        tree_json(ROOT, vec![entry("../evil", "blob", "100644", BLOB)], false),
        tree_json(
            ROOT,
            vec![entry("nested/file", "blob", "100644", BLOB)],
            false,
        ),
        tree_json(
            ROOT,
            vec![entry("bad", "blob", "100644", "https://evil.invalid")],
            false,
        ),
    ] {
        let mock = Arc::new(MockTransport::new());
        enqueue(&mock, 200, body).await;
        assert!(
            fetch_directory(
                &client(mock.clone()),
                &pat(),
                &SnapshotDirectory::root(&snapshot()),
                &limits(),
                CancellationToken::default()
            )
            .await
            .is_err()
        );
        assert_eq!(mock.recorded().await.len(), 1);
    }
    let mock = Arc::new(MockTransport::new());
    enqueue(&mock, 200, commit_json(B)).await;
    assert!(matches!(
        resolve_revision(
            &client(mock),
            &pat(),
            &identity(),
            RevisionSpec::Commit(ObjectSha::new(A).unwrap()),
            &limits(),
            CancellationToken::default()
        )
        .await,
        Err(RepositoryError::InvalidResponse { .. })
    ));
    let mock = Arc::new(MockTransport::new());
    let mut body = readme_json();
    body["path"] = json!("../../evil");
    enqueue(&mock, 200, body).await;
    assert!(
        discover_readme(
            &client(mock.clone()),
            &pat(),
            &snapshot(),
            &limits(),
            CancellationToken::default()
        )
        .await
        .is_err()
    );
    assert_eq!(mock.recorded().await.len(), 1);
}

#[tokio::test]
async fn readme_404_only_claims_absence_with_positive_empty_tree_evidence() {
    for (entries, truncated, absent) in [
        (vec![], false, true),
        (vec![entry("code.rs", "blob", "100644", BLOB)], false, false),
        (vec![], true, false),
    ] {
        let mock = Arc::new(MockTransport::new());
        enqueue(&mock, 404, json!({})).await;
        enqueue(&mock, 200, tree_json(ROOT, entries, truncated)).await;
        let result = discover_readme(
            &client(mock.clone()),
            &pat(),
            &snapshot(),
            &limits(),
            CancellationToken::default(),
        )
        .await
        .unwrap();
        if absent {
            assert!(matches!(result, ReadmeDiscovery::AbsentInEmptyTree { .. }));
        } else {
            assert!(matches!(
                result,
                ReadmeDiscovery::NotFoundOrInaccessible { .. }
            ));
        }
        let recorded = mock.recorded().await;
        assert!(recorded[0].0.ends_with(&format!("readme?ref={A}")));
        assert!(recorded[1].0.ends_with(ROOT));
    }
}

#[tokio::test]
async fn later_page_cancellation_cannot_return_partial_data() {
    use crate::network::transport::{HttpRequest, Transport, TransportError};
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct CancelSecond {
        calls: AtomicUsize,
        source: crate::network::cancel::CancellationTokenSource,
    }
    impl Transport for CancelSecond {
        fn execute<'a>(
            &'a self,
            _request: HttpRequest,
            _cancel: CancellationToken,
        ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>>
        {
            Box::pin(async move {
                if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    Ok(json_response(200,&branches_json(&["main"],A).to_string()).with_test_header("link","<https://api.github.com/repos/octo/repo/branches?page=2>; rel=\"next\""))
                } else {
                    self.source.cancel();
                    Err(TransportError::Cancelled)
                }
            })
        }
    }
    let (cancel, source) = CancellationToken::new();
    let c = GitHubClient::new(
        Default::default(),
        Arc::new(CancelSecond {
            calls: AtomicUsize::new(0),
            source,
        }),
    );
    assert_eq!(
        fetch_branches(&c, &pat(), &identity(), &limits(), cancel)
            .await
            .unwrap_err(),
        RepositoryError::GitHub(GitHubError::Cancelled)
    );
}
