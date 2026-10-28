//! One bounded read-only query; no list enumeration or Search count limits.
use crate::{
    auth::token::Pat,
    domain::{
        profile::ProfileError,
        repository::RepositoryIdentity,
        repository_counts::{CountsError, RepositoryCounts},
    },
    github::{
        client::GitHubClient,
        profile::graphql::{GraphQlOutcome, ProfileLimits, run_query},
    },
    network::cancel::CancellationToken,
};
const QUERY: &str = "query RepositoryOpenCounts($owner: String!, $name: String!) { repository(owner: $owner, name: $name) { databaseId issues(states: OPEN) { totalCount } pullRequests(states: OPEN) { totalCount } } }";
pub async fn fetch(
    client: &GitHubClient,
    pat: &Pat,
    repo: &RepositoryIdentity,
    cancel: CancellationToken,
) -> Result<RepositoryCounts, CountsError> {
    let outcome = run_query(
        client,
        pat,
        QUERY,
        &serde_json::json!({"owner":repo.locator().owner(),"name":repo.locator().name()}),
        &ProfileLimits {
            response_bytes: 64 * 1024,
            ..Default::default()
        },
        cancel,
    )
    .await?;
    decode(repo, outcome)
}
fn decode(
    repo: &RepositoryIdentity,
    outcome: GraphQlOutcome,
) -> Result<RepositoryCounts, CountsError> {
    let identity_error = outcome.errors.iter().any(|error| {
        error.path.is_empty()
            || error.path[0] != "repository"
            || error.path.len() < 2
            || !matches!(error.path[1].as_str(), "issues" | "pullRequests")
    });
    if identity_error && let Some(error) = outcome.errors.first() {
        return Err(CountsError::Read(ProfileError::Graphql {
            kind: error.kind.clone(),
            path: error.path.clone(),
        }));
    }
    let value = outcome
        .data
        .as_ref()
        .and_then(|data| data.get("repository"))
        .ok_or(CountsError::RepositoryIdentity)?;
    if identity_error
        || value.get("databaseId").and_then(serde_json::Value::as_u64) != Some(repo.id())
    {
        return Err(CountsError::RepositoryIdentity);
    }
    let field = |name: &'static str| {
        if let Some(error) = outcome
            .errors
            .iter()
            .find(|error| error.path.get(1).is_some_and(|part| part == name))
        {
            return Err(CountsError::Read(ProfileError::Graphql {
                kind: error.kind.clone(),
                path: error.path.clone(),
            }));
        }
        value
            .get(name)
            .and_then(|v| v.get("totalCount"))
            .and_then(serde_json::Value::as_u64)
            .ok_or(CountsError::InvalidCount(name))
    };
    Ok(RepositoryCounts {
        repository: repo.clone(),
        issues: field("issues"),
        pull_requests: field("pullRequests"),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::{
            profile::{GraphqlErrorKind, ProfileGraphqlError},
            repository::RepositoryLocator,
        },
        network::transport::{HttpMethod, MockTransport, json_response},
    };
    use std::sync::Arc;
    fn repo() -> RepositoryIdentity {
        RepositoryIdentity {
            id: 42,
            locator: RepositoryLocator::new("octo", "ledger").unwrap(),
        }
    }
    #[tokio::test]
    async fn query_reads_separate_open_connections_and_matches_identity() {
        let transport = Arc::new(MockTransport::new());
        transport.enqueue(Ok(json_response(200,r#"{"data":{"repository":{"databaseId":42,"issues":{"totalCount":3},"pullRequests":{"totalCount":1}}}}"#))).await;
        let value = fetch(
            &GitHubClient::new(Default::default(), transport.clone()),
            &Pat::from_str("synthetic"),
            &repo(),
            Default::default(),
        )
        .await
        .unwrap();
        assert_eq!(value.issues, Ok(3));
        assert_eq!(value.pull_requests, Ok(1));
        let sent = transport.recorded_requests().await;
        assert_eq!(sent[0].method, HttpMethod::Post);
        let body: serde_json::Value = serde_json::from_str(sent[0].body.as_ref().unwrap()).unwrap();
        assert_eq!(body["variables"]["owner"], "octo");
        assert!(
            body["query"]
                .as_str()
                .unwrap()
                .contains("pullRequests(states: OPEN)")
        );
        assert!(!body["query"].as_str().unwrap().contains("mutation"));
    }
    #[test]
    fn partial_error_only_invalidates_the_affected_field() {
        let data = serde_json::json!({"repository":{"databaseId":42,"issues":{"totalCount":0},"pullRequests":{"totalCount":99}}});
        let value = decode(
            &repo(),
            GraphQlOutcome {
                data: Some(data.clone()),
                errors: vec![ProfileGraphqlError {
                    kind: GraphqlErrorKind::Forbidden,
                    path: vec!["repository".into(), "pullRequests".into()],
                }],
            },
        )
        .unwrap();
        assert_eq!(value.issues, Ok(0));
        assert!(value.pull_requests.is_err());
        assert!(
            decode(
                &repo(),
                GraphQlOutcome {
                    data: Some(data),
                    errors: vec![ProfileGraphqlError {
                        kind: GraphqlErrorKind::Forbidden,
                        path: vec!["repository".into()]
                    }]
                }
            )
            .is_err()
        );
        assert!(
            decode(
                &repo(),
                GraphQlOutcome {
                    data: Some(serde_json::json!({"repository":{"databaseId":43}})),
                    errors: vec![]
                }
            )
            .is_err()
        );
    }
}
