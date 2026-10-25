//! Pinned repositories and the itemShowcase ("Popular") union.
//!
//! Facts only:
//! - `pinnedItems(types: [REPOSITORY])` gives the real repository pins, in the
//!   user's order.
//! - `itemShowcase` is what the profile displays when it has no repository pin
//!   to show. Its items are a `PinnableItem` union (repositories and gists) and
//!   `hasPinnedItems` counts both, so a gist-only profile must not be reported
//!   as having no repository.
//!
//! This adapter preserves that uncertainty in [`Showcase`] and does not choose
//! a fallback policy (no "sort by stars" substitute for GitHub Popular).

use crate::auth::token::Pat;
use crate::domain::profile::{ProfileError, ProfilePinned, ProfileRepo, Showcase};
use crate::github::client::GitHubClient;
use crate::github::profile::graphql::{GraphQlOutcome, ProfileLimits, require_user, run_query};
use crate::network::cancel::CancellationToken;
use serde::Deserialize;
use serde_json::Value;

const PINNED_QUERY: &str = r#"
query ProfilePinned($login: String!, $pinnedFirst: Int!, $showcaseFirst: Int!) {
  user(login: $login) {
    pinnedItems(first: $pinnedFirst, types: [REPOSITORY]) {
      totalCount
      nodes {
        __typename
        ... on Repository {
          databaseId
          name
          nameWithOwner
          description
          url
          isPrivate
          isFork
          stargazerCount
          forkCount
          primaryLanguage { name }
          owner { login }
        }
      }
    }
    itemShowcase {
      hasPinnedItems
      items(first: $showcaseFirst) {
        nodes {
          __typename
          ... on Repository {
            databaseId
            name
            nameWithOwner
            description
            url
            isPrivate
            isFork
            stargazerCount
            forkCount
            primaryLanguage { name }
            owner { login }
          }
        }
      }
    }
  }
}
"#;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PinnedUser {
    #[serde(default)]
    pinned_items: Option<PinnedItemsConnection>,
    #[serde(default)]
    item_showcase: Option<ItemShowcase>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PinnedItemsConnection {
    #[serde(default)]
    total_count: u32,
    #[serde(default)]
    nodes: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItemShowcase {
    #[serde(default)]
    has_pinned_items: bool,
    #[serde(default)]
    items: Option<ShowcaseItemsConnection>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShowcaseItemsConnection {
    #[serde(default)]
    nodes: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepositoryNode {
    #[serde(rename = "__typename", default)]
    typename: Option<String>,
    #[serde(default)]
    database_id: Option<u64>,
    name: String,
    #[serde(default)]
    name_with_owner: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    is_private: Option<bool>,
    #[serde(default)]
    is_fork: Option<bool>,
    #[serde(default)]
    stargazer_count: Option<u64>,
    #[serde(default)]
    fork_count: Option<u64>,
    #[serde(default)]
    primary_language: Option<LanguageNode>,
    #[serde(default)]
    owner: Option<OwnerNode>,
}

#[derive(Deserialize)]
struct LanguageNode {
    name: String,
}

#[derive(Deserialize)]
struct OwnerNode {
    login: String,
}

enum ShowcaseNode {
    Repository(ProfileRepo),
    Gist,
    Unknown,
}

/// Fetch the Pinned/Popular section for a login.
pub async fn fetch_profile_pinned(
    client: &GitHubClient,
    token: &Pat,
    login: &str,
    limits: &ProfileLimits,
    cancel: CancellationToken,
) -> Result<ProfilePinned, ProfileError> {
    crate::github::profile::validate_login(login)?;
    let variables = serde_json::json!({
        "login": login,
        "pinnedFirst": limits.pinned_items,
        "showcaseFirst": limits.showcase_items,
    });
    let outcome = run_query(client, token, PINNED_QUERY, &variables, limits, cancel).await?;
    build_pinned(&outcome, limits)
}

fn build_pinned(
    outcome: &GraphQlOutcome,
    limits: &ProfileLimits,
) -> Result<ProfilePinned, ProfileError> {
    let (user, errors) = require_user::<PinnedUser>(outcome)?;

    let mut repos = Vec::new();
    let mut total_repos = 0u32;
    if let Some(connection) = user.pinned_items {
        total_repos = connection.total_count;
        if connection.nodes.len() > limits.pinned_items as usize {
            return Err(ProfileError::LimitExceeded {
                resource: "pinned items",
                limit: limits.pinned_items as usize,
            });
        }
        for node in &connection.nodes {
            if let Some(repo) = parse_repository_node(node) {
                repos.push(repo);
            }
        }
    }

    let (has_pinned_items, showcase) = match user.item_showcase {
        Some(showcase) => match showcase.items {
            Some(items) => (
                showcase.has_pinned_items,
                classify_showcase(showcase.has_pinned_items, &items.nodes),
            ),
            // The connection itself failed or was absent: keep it explicitly
            // unknown rather than claiming the user has no pins/repos.
            None => (showcase.has_pinned_items, Showcase::Unclassified),
        },
        None => (false, Showcase::Unclassified),
    };

    Ok(ProfilePinned {
        repos,
        total_repos,
        has_pinned_items,
        showcase,
        errors,
    })
}

/// A node maps to a repository only when its `__typename` is `Repository`. Any
/// other union member is not silently treated as a repository.
fn parse_repository_node(node: &Value) -> Option<ProfileRepo> {
    let repo: RepositoryNode = serde_json::from_value(node.clone()).ok()?;
    if let Some(typename) = repo.typename.as_deref()
        && typename != "Repository"
    {
        return None;
    }
    Some(ProfileRepo {
        id: repo.database_id,
        name: repo.name,
        name_with_owner: repo.name_with_owner,
        description: repo.description,
        url: repo.url,
        is_private: repo.is_private,
        is_fork: repo.is_fork,
        stargazer_count: repo.stargazer_count,
        fork_count: repo.fork_count,
        primary_language: repo.primary_language.map(|language| language.name),
        owner_login: repo.owner.map(|owner| owner.login),
    })
}

fn parse_showcase_node(node: &Value) -> ShowcaseNode {
    match node.get("__typename").and_then(Value::as_str) {
        Some("Repository") => match parse_repository_node(node) {
            Some(repo) => ShowcaseNode::Repository(repo),
            None => ShowcaseNode::Unknown,
        },
        Some("Gist") => ShowcaseNode::Gist,
        _ => ShowcaseNode::Unknown,
    }
}

fn classify_showcase(has_pinned_items: bool, nodes: &[Value]) -> Showcase {
    let mut repositories = Vec::new();
    let mut gists = 0usize;
    let mut unknown = false;
    for node in nodes {
        match parse_showcase_node(node) {
            ShowcaseNode::Repository(repo) => repositories.push(repo),
            ShowcaseNode::Gist => gists += 1,
            ShowcaseNode::Unknown => unknown = true,
        }
    }
    if unknown {
        return Showcase::Unclassified;
    }
    if !has_pinned_items {
        // No pins: the server returned its Popular repository fallback.
        if gists > 0 {
            return Showcase::Unclassified;
        }
        return Showcase::PopularRepositories(repositories);
    }
    match (repositories.is_empty(), gists) {
        (true, 0) => Showcase::Unclassified,
        (true, _) => Showcase::PinnedGistsOnly { gists },
        (false, 0) => Showcase::PinnedRepositories(repositories),
        (false, _) => Showcase::PinnedWithGists {
            repositories,
            gists,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::profile::GraphqlErrorKind;
    use crate::network::transport::{MockTransport, json_response};
    use std::sync::Arc;

    fn client(mock: Arc<MockTransport>) -> GitHubClient {
        GitHubClient::new(Default::default(), mock)
    }

    fn repo_node(name: &str, stars: u64) -> Value {
        serde_json::json!({
            "__typename": "Repository",
            "databaseId": 1,
            "name": name,
            "nameWithOwner": format!("octocat/{name}"),
            "description": "desc",
            "url": format!("https://github.com/octocat/{name}"),
            "isPrivate": false,
            "isFork": false,
            "stargazerCount": stars,
            "forkCount": 3,
            "primaryLanguage": {"name": "Rust"},
            "owner": {"login": "octocat"}
        })
    }

    fn gist_node(name: &str) -> Value {
        serde_json::json!({ "__typename": "Gist", "name": name })
    }

    async fn enqueue(mock: &MockTransport, body: Value) {
        mock.enqueue(Ok(json_response(200, &body.to_string())))
            .await;
    }

    #[tokio::test]
    async fn pinned_repositories_keep_their_order() {
        let mock = Arc::new(MockTransport::new());
        enqueue(
            &mock,
            serde_json::json!({
                "data": {"user": {
                    "pinnedItems": {
                        "totalCount": 3,
                        "nodes": [repo_node("alpha", 10), repo_node("beta", 5), repo_node("gamma", 1)]
                    },
                    "itemShowcase": {
                        "hasPinnedItems": true,
                        "items": {"nodes": [repo_node("alpha", 10)]}
                    }
                }}
            }),
        )
        .await;
        let result = fetch_profile_pinned(
            &client(mock),
            &Pat::from_str("ghp_test"),
            "octocat",
            &ProfileLimits::default(),
            CancellationToken::default(),
        )
        .await
        .expect("pinned");
        assert_eq!(
            result
                .repos
                .iter()
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta", "gamma"]
        );
        assert_eq!(result.total_repos, 3);
        assert!(result.has_pinned_items);
        assert!(matches!(result.showcase, Showcase::PinnedRepositories(_)));
    }

    /// A gist-only profile has `hasPinnedItems = true` but no repository pin.
    /// The result must say so rather than claiming the user has no repository.
    #[tokio::test]
    async fn gist_only_profile_is_not_reported_as_no_repositories() {
        let mock = Arc::new(MockTransport::new());
        enqueue(
            &mock,
            serde_json::json!({
                "data": {"user": {
                    "pinnedItems": {"totalCount": 0, "nodes": []},
                    "itemShowcase": {
                        "hasPinnedItems": true,
                        "items": {"nodes": [gist_node("notes"), gist_node("dotfiles")]}
                    }
                }}
            }),
        )
        .await;
        let result = fetch_profile_pinned(
            &client(mock),
            &Pat::from_str("ghp_test"),
            "octocat",
            &ProfileLimits::default(),
            CancellationToken::default(),
        )
        .await
        .expect("pinned");
        assert!(result.repos.is_empty());
        assert!(result.has_pinned_items);
        assert_eq!(result.showcase, Showcase::PinnedGistsOnly { gists: 2 });
    }

    /// Mixed pins keep the repositories and count the gists, rather than
    /// dropping either side.
    #[tokio::test]
    async fn mixed_pins_keep_both_kinds() {
        let mock = Arc::new(MockTransport::new());
        enqueue(
            &mock,
            serde_json::json!({
                "data": {"user": {
                    "pinnedItems": {"totalCount": 1, "nodes": [repo_node("alpha", 10)]},
                    "itemShowcase": {
                        "hasPinnedItems": true,
                        "items": {"nodes": [repo_node("alpha", 10), gist_node("notes")]}
                    }
                }}
            }),
        )
        .await;
        let result = fetch_profile_pinned(
            &client(mock),
            &Pat::from_str("ghp_test"),
            "octocat",
            &ProfileLimits::default(),
            CancellationToken::default(),
        )
        .await
        .expect("pinned");
        assert!(matches!(
            result.showcase,
            Showcase::PinnedWithGists { gists: 1, .. }
        ));
    }

    /// No pins at all: the server returned its Popular fallback.
    #[tokio::test]
    async fn no_pins_returns_the_popular_fallback() {
        let mock = Arc::new(MockTransport::new());
        enqueue(
            &mock,
            serde_json::json!({
                "data": {"user": {
                    "pinnedItems": {"totalCount": 0, "nodes": []},
                    "itemShowcase": {
                        "hasPinnedItems": false,
                        "items": {"nodes": [repo_node("alpha", 10), repo_node("beta", 5)]}
                    }
                }}
            }),
        )
        .await;
        let result = fetch_profile_pinned(
            &client(mock),
            &Pat::from_str("ghp_test"),
            "octocat",
            &ProfileLimits::default(),
            CancellationToken::default(),
        )
        .await
        .expect("pinned");
        assert!(!result.has_pinned_items);
        match result.showcase {
            Showcase::PopularRepositories(repos) => {
                assert_eq!(repos.len(), 2);
                // Not re-sorted by stars: server order is preserved.
                assert_eq!(repos[0].name, "alpha");
                assert_eq!(repos[1].name, "beta");
            }
            other => panic!("expected PopularRepositories, got {other:?}"),
        }
    }

    /// An unknown union member is preserved as `Unclassified`, not dropped.
    #[tokio::test]
    async fn unknown_union_member_is_unclassified() {
        let mock = Arc::new(MockTransport::new());
        enqueue(
            &mock,
            serde_json::json!({
                "data": {"user": {
                    "pinnedItems": {"totalCount": 0, "nodes": []},
                    "itemShowcase": {
                        "hasPinnedItems": true,
                        "items": {"nodes": [{"__typename": "FuturePinnable"}]}
                    }
                }}
            }),
        )
        .await;
        let result = fetch_profile_pinned(
            &client(mock),
            &Pat::from_str("ghp_test"),
            "octocat",
            &ProfileLimits::default(),
            CancellationToken::default(),
        )
        .await
        .expect("pinned");
        assert_eq!(result.showcase, Showcase::Unclassified);
    }

    /// A `200` with `data` and `errors` keeps the pinned data and carries the
    /// classified errors as a partial result.
    #[tokio::test]
    async fn partial_data_keeps_errors() {
        let mock = Arc::new(MockTransport::new());
        enqueue(
            &mock,
            serde_json::json!({
                "data": {"user": {
                    "pinnedItems": {"totalCount": 1, "nodes": [repo_node("alpha", 10)]},
                    "itemShowcase": null
                }},
                "errors": [{
                    "type": "FORBIDDEN",
                    "path": ["user", "itemShowcase"],
                    "message": "Resource not accessible"
                }]
            }),
        )
        .await;
        let result = fetch_profile_pinned(
            &client(mock),
            &Pat::from_str("ghp_test"),
            "octocat",
            &ProfileLimits::default(),
            CancellationToken::default(),
        )
        .await
        .expect("partial");
        assert_eq!(result.repos.len(), 1);
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].kind, GraphqlErrorKind::Forbidden);
        // A failed itemShowcase is explicitly unknown, not an empty Popular list.
        assert_eq!(result.showcase, Showcase::Unclassified);
    }

    /// An unknown login returns `data.user = null` with a NOT_FOUND error; that
    /// is a typed error, not an empty pinned list.
    #[tokio::test]
    async fn unknown_login_is_not_found() {
        let mock = Arc::new(MockTransport::new());
        enqueue(
            &mock,
            serde_json::json!({
                "data": {"user": null},
                "errors": [{"type": "NOT_FOUND", "path": ["user"], "message": "no such user"}]
            }),
        )
        .await;
        let result = fetch_profile_pinned(
            &client(mock),
            &Pat::from_str("ghp_test"),
            "ghost",
            &ProfileLimits::default(),
            CancellationToken::default(),
        )
        .await;
        assert!(matches!(
            result,
            Err(ProfileError::Graphql {
                kind: GraphqlErrorKind::NotFound,
                ..
            })
        ));
    }

    /// The POST body carries the REPOSITORY type filter, so gists are excluded
    /// from the real pin list.
    #[tokio::test]
    async fn pinned_query_filters_to_repository_type() {
        let mock = Arc::new(MockTransport::new());
        enqueue(
            &mock,
            serde_json::json!({"data": {"user": {
                "pinnedItems": {"totalCount": 0, "nodes": []},
                "itemShowcase": {"hasPinnedItems": false, "items": {"nodes": []}}
            }}}),
        )
        .await;
        let _ = fetch_profile_pinned(
            &client(mock.clone()),
            &Pat::from_str("ghp_test"),
            "octocat",
            &ProfileLimits::default(),
            CancellationToken::default(),
        )
        .await
        .expect("pinned");
        let recorded = mock.recorded_requests().await;
        let body: Value = serde_json::from_str(recorded[0].body.as_deref().unwrap()).unwrap();
        assert!(
            body["query"]
                .as_str()
                .unwrap()
                .contains("types: [REPOSITORY]"),
            "pinnedItems must filter to REPOSITORY"
        );
    }
}
