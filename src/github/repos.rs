//! `GET /user/repos` with full `Link` pagination.

use crate::auth::token::Pat;
use crate::domain::error::GitHubError;
use crate::domain::repo::{PartialReason, ReposReadiness};
use crate::domain::repo::{RepoOwner, RepoSummary, RepoVisibility, ReposOutcome};
use crate::github::client::GitHubClient;
use crate::github::error::{classify_status, classify_transport};
use crate::github::headers::{parse_link_next, parse_sso_header};
use crate::network::cancel::CancellationToken;
use serde::Deserialize;

const AFFILIATION: &str = "owner,collaborator,organization_member";
const PER_PAGE: u32 = 100;
const MAX_PAGES: usize = 50;

#[derive(Deserialize)]
struct RepoResponse {
    id: u64,
    name: String,
    full_name: String,
    #[serde(default)]
    fork: bool,
    #[serde(default)]
    archived: bool,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    default_branch: Option<String>,
    #[serde(default)]
    visibility: Option<String>,
    owner: OwnerResponse,
}

#[derive(Deserialize)]
struct OwnerResponse {
    id: u64,
    login: String,
    #[serde(default)]
    avatar_url: Option<String>,
}

/// Fetch all visible repositories across `owner`, `collaborator`, and
/// `organization_member` affiliations, following the `Link` header fully.
///
/// Mid-pagination failures and SSO `partial-results` produce a partial outcome
/// with the items fetched so far rather than masquerading as a complete empty
/// list.
pub async fn fetch_user_repos(
    client: &GitHubClient,
    token: &Pat,
    cancel: CancellationToken,
) -> Result<ReposOutcome, GitHubError> {
    let initial_url = format!(
        "{}user/repos?affiliation={}&per_page={}&sort=pushed",
        client.config().base_url,
        AFFILIATION,
        PER_PAGE
    );

    let mut url = initial_url;
    let mut all_repos: Vec<RepoSummary> = Vec::new();
    let mut partial_reason: Option<PartialReason> = None;
    let mut page = 0usize;

    loop {
        page += 1;
        if page > MAX_PAGES {
            partial_reason = Some(PartialReason::PageFailed {
                message: format!("pagination limit ({MAX_PAGES}) exceeded"),
            });
            break;
        }

        let request = client.build_get(&url, token);
        let response = match client.transport().execute(request, cancel.clone()).await {
            Ok(response) => response,
            Err(transport_error) => {
                let err = classify_transport(transport_error);
                if all_repos.is_empty() {
                    return Err(err);
                }
                // A transport failure on page 2+ must not throw away the pages
                // already parsed; it is a partial result like a later HTTP
                // failure, never a completed empty list.
                if partial_reason.is_none() {
                    partial_reason = Some(PartialReason::PageFailed {
                        message: err.to_string(),
                    });
                }
                break;
            }
        };

        // Check SSO partial-results on any page.
        if let Some(orgs) = parse_sso_header(response.header("x-github-sso"))
            && partial_reason.is_none()
        {
            partial_reason = Some(PartialReason::SsoPartial {
                organizations: orgs,
            });
        }

        if let Some(err) = classify_status(&response) {
            if all_repos.is_empty() {
                return Err(err);
            }
            // A later page failed; keep partial data but mark it incomplete.
            if partial_reason.is_none() {
                partial_reason = Some(PartialReason::PageFailed {
                    message: format!("{err}"),
                });
            }
            break;
        }

        let page_repos: Vec<RepoResponse> = match serde_json::from_str(&response.body) {
            Ok(page_repos) => page_repos,
            Err(e) => {
                // A parse failure on page 2+ keeps earlier pages as partial data;
                // only a first-page parse failure is a hard error.
                if all_repos.is_empty() {
                    return Err(GitHubError::Parse {
                        message: e.to_string(),
                    });
                }
                if partial_reason.is_none() {
                    partial_reason = Some(PartialReason::PageFailed {
                        message: format!("failed to parse page {page}: {e}"),
                    });
                }
                break;
            }
        };

        for r in page_repos {
            all_repos.push(RepoSummary {
                id: r.id,
                full_name: r.full_name,
                name: r.name,
                owner: RepoOwner {
                    id: r.owner.id,
                    login: r.owner.login,
                    avatar_url: r.owner.avatar_url,
                },
                visibility: classify_visibility(r.visibility.as_deref()),
                fork: r.fork,
                archived: r.archived,
                description: r.description,
                default_branch: r.default_branch,
            });
        }

        match parse_link_next(response.header("link")) {
            Some(next_url) if client.url_is_trusted(&next_url) => {
                url = next_url;
            }
            Some(_untrusted) => {
                // Refuse to follow a next link to an untrusted host.
                if partial_reason.is_none() {
                    partial_reason = Some(PartialReason::PageFailed {
                        message: "pagination link targets untrusted host".to_string(),
                    });
                }
                break;
            }
            None => break, // complete
        }
    }

    let readiness = match partial_reason {
        Some(reason) => ReposReadiness::Partial { reason },
        None => ReposReadiness::Complete,
    };

    Ok(ReposOutcome {
        repositories: all_repos,
        readiness,
    })
}

fn classify_visibility(raw: Option<&str>) -> RepoVisibility {
    match raw {
        Some("private") => RepoVisibility::Private,
        Some("internal") => RepoVisibility::Internal,
        _ => RepoVisibility::Public,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::transport::{HttpResponse, MockTransport};
    use std::sync::Arc;

    fn page_body(repos: &[(u64, &str)]) -> String {
        let arr: Vec<_> = repos
            .iter()
            .map(|(id, name)| {
                serde_json::json!({
                    "id": id,
                    "name": name,
                    "full_name": format!("octocat/{name}"),
                    "owner": {"id": 1, "login": "octocat"},
                    "visibility": "public",
                    "fork": false,
                    "archived": false
                })
            })
            .collect();
        serde_json::json!(arr).to_string()
    }

    #[tokio::test]
    async fn merges_two_pages() {
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![
                (
                    "link".to_string(),
                    r#"<https://api.github.com/user/repos?page=2>; rel="next""#.to_string(),
                ),
                ("x-ratelimit-remaining".to_string(), "4998".to_string()),
            ],
            body: page_body(&[(1, "alpha"), (2, "beta")]),
        }))
        .await;
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: page_body(&[(3, "gamma")]),
        }))
        .await;

        let client = GitHubClient::new(Default::default(), mock.clone());
        let token = Pat::from_str("ghp_test");
        let outcome = fetch_user_repos(&client, &token, CancellationToken::default())
            .await
            .expect("ok");
        assert_eq!(outcome.repositories.len(), 3);
        assert_eq!(outcome.readiness, ReposReadiness::Complete);
    }

    #[tokio::test]
    async fn partial_on_mid_page_failure() {
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![(
                "link".to_string(),
                r#"<https://api.github.com/user/repos?page=2>; rel="next""#.to_string(),
            )],
            body: page_body(&[(1, "alpha")]),
        }))
        .await;
        mock.enqueue(Ok(HttpResponse {
            status: 503,
            headers: vec![],
            body: "{}".to_string(),
        }))
        .await;

        let client = GitHubClient::new(Default::default(), mock.clone());
        let token = Pat::from_str("ghp_test");
        let outcome = fetch_user_repos(&client, &token, CancellationToken::default())
            .await
            .expect("partial returns outcome");
        assert_eq!(outcome.repositories.len(), 1);
        assert!(matches!(outcome.readiness, ReposReadiness::Partial { .. }));
    }

    #[tokio::test]
    async fn sso_partial_results() {
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![
                (
                    "x-github-sso".to_string(),
                    "partial-results; organizations=acme".to_string(),
                ),
                ("x-ratelimit-remaining".to_string(), "4999".to_string()),
            ],
            body: page_body(&[(1, "alpha")]),
        }))
        .await;

        let client = GitHubClient::new(Default::default(), mock.clone());
        let token = Pat::from_str("ghp_test");
        let outcome = fetch_user_repos(&client, &token, CancellationToken::default())
            .await
            .expect("ok");
        assert_eq!(outcome.repositories.len(), 1);
        assert!(matches!(
            outcome.readiness,
            ReposReadiness::Partial {
                reason: PartialReason::SsoPartial { .. }
            }
        ));
    }

    #[tokio::test]
    async fn first_page_failure_returns_error() {
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(HttpResponse {
            status: 401,
            headers: vec![],
            body: "{}".to_string(),
        }))
        .await;

        let client = GitHubClient::new(Default::default(), mock.clone());
        let token = Pat::from_str("ghp_test");
        let result = fetch_user_repos(&client, &token, CancellationToken::default()).await;
        assert!(matches!(result, Err(GitHubError::Unauthorized)));
    }

    /// Regression: a transport failure on page 2+ keeps the pages already
    /// parsed and reports a partial result instead of discarding them.
    #[tokio::test]
    async fn transport_failure_on_later_page_keeps_earlier_pages() {
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![(
                "link".to_string(),
                r#"<https://api.github.com/user/repos?page=2>; rel="next""#.to_string(),
            )],
            body: page_body(&[(1, "alpha"), (2, "beta")]),
        }))
        .await;
        mock.enqueue(Err(crate::network::transport::MockFailure::Timeout))
            .await;

        let client = GitHubClient::new(Default::default(), mock.clone());
        let token = Pat::from_str("ghp_test");
        let outcome = fetch_user_repos(&client, &token, CancellationToken::default())
            .await
            .expect("partial outcome");
        assert_eq!(outcome.repositories.len(), 2);
        assert!(matches!(
            outcome.readiness,
            ReposReadiness::Partial {
                reason: PartialReason::PageFailed { .. }
            }
        ));
    }

    /// A parse failure on page 2+ also keeps earlier pages.
    #[tokio::test]
    async fn parse_failure_on_later_page_keeps_earlier_pages() {
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![(
                "link".to_string(),
                r#"<https://api.github.com/user/repos?page=2>; rel="next""#.to_string(),
            )],
            body: page_body(&[(1, "alpha")]),
        }))
        .await;
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: "not json".to_string(),
        }))
        .await;

        let client = GitHubClient::new(Default::default(), mock.clone());
        let token = Pat::from_str("ghp_test");
        let outcome = fetch_user_repos(&client, &token, CancellationToken::default())
            .await
            .expect("partial outcome");
        assert_eq!(outcome.repositories.len(), 1);
        assert!(matches!(outcome.readiness, ReposReadiness::Partial { .. }));
    }

    /// A first-page transport failure is still a hard error, not an empty list.
    #[tokio::test]
    async fn transport_failure_on_first_page_is_an_error() {
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Err(crate::network::transport::MockFailure::Network(
            "offline".to_string(),
        )))
        .await;

        let client = GitHubClient::new(Default::default(), mock.clone());
        let token = Pat::from_str("ghp_test");
        let result = fetch_user_repos(&client, &token, CancellationToken::default()).await;
        assert!(matches!(result, Err(GitHubError::Network { .. })));
    }

    #[tokio::test]
    async fn rejects_untrusted_pagination_link() {
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(HttpResponse {
            status: 200,
            headers: vec![(
                "link".to_string(),
                r#"<https://evil.com/user/repos?page=2>; rel="next""#.to_string(),
            )],
            body: page_body(&[(1, "alpha")]),
        }))
        .await;

        let client = GitHubClient::new(Default::default(), mock.clone());
        let token = Pat::from_str("ghp_test");
        let outcome = fetch_user_repos(&client, &token, CancellationToken::default())
            .await
            .expect("returns partial");
        assert!(matches!(outcome.readiness, ReposReadiness::Partial { .. }));
    }

    #[tokio::test]
    async fn cancellation_produces_cancelled_error() {
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Err(crate::network::transport::MockFailure::Timeout))
            .await;

        let client = GitHubClient::new(Default::default(), mock.clone());
        let token = Pat::from_str("ghp_test");
        let (cancel, source) = CancellationToken::new();
        source.cancel();
        let result = fetch_user_repos(&client, &token, cancel).await;
        assert!(matches!(result, Err(GitHubError::Cancelled)));
    }
}
