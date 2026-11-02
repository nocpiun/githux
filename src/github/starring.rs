//! GitHub Starring REST endpoints, limited to the three operations needed by
//! the repository Star button. No write is retried or redirected automatically.
//!
//! Official API (version 2022-11-28):
//! - GET `/user/starred/{owner}/{repo}`: 204 starred, 404 not starred/invisible.
//! - PUT/DELETE the same path: 204 confirms the requested change.
//!
//! Fine-grained tokens need Starring read for GET and Starring write plus
//! Metadata read for writes. A 404 does not prove that a repository is absent.

use crate::auth::token::Pat;
use crate::domain::error::GitHubError;
use crate::domain::repository::RepositoryIdentity;
use crate::domain::repository_actions::{RepositoryActionError, StarObservation, StarredState};
use crate::github::client::GitHubClient;
use crate::github::error::{classify_status, classify_transport};
use crate::network::cancel::CancellationToken;
use reqwest::Url;

const EMPTY_RESPONSE_LIMIT: usize = 16 * 1024;

fn endpoint(
    client: &GitHubClient,
    repository: &RepositoryIdentity,
) -> Result<Url, RepositoryActionError> {
    let mut url = Url::parse(&client.config().base_url).map_err(|_| {
        crate::domain::repository::RepositoryError::InvalidInput {
            field: "API base URL",
        }
    })?;
    if url.query().is_some() || url.fragment().is_some() {
        return Err(crate::domain::repository::RepositoryError::InvalidInput {
            field: "API base URL",
        }
        .into());
    }
    url.path_segments_mut()
        .map_err(
            |_| crate::domain::repository::RepositoryError::InvalidInput {
                field: "API base URL",
            },
        )?
        .pop_if_empty()
        .extend([
            "user",
            "starred",
            repository.locator().owner(),
            repository.locator().name(),
        ]);
    if !client.url_is_trusted(url.as_str()) {
        return Err(crate::domain::repository::RepositoryError::InvalidInput {
            field: "API origin",
        }
        .into());
    }
    Ok(url)
}

async fn execute(
    client: &GitHubClient,
    request: crate::network::transport::HttpRequest,
    cancel: CancellationToken,
) -> Result<crate::network::transport::HttpResponse, GitHubError> {
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled);
    }
    let response = client
        .transport()
        .execute_bounded(request, cancel.clone(), EMPTY_RESPONSE_LIMIT)
        .await
        .map_err(classify_transport)?;
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled);
    }
    Ok(response)
}

pub async fn fetch_starred(
    client: &GitHubClient,
    token: &Pat,
    repository: &RepositoryIdentity,
    cancel: CancellationToken,
) -> Result<StarObservation, RepositoryActionError> {
    let url = endpoint(client, repository)?;
    let response = execute(client, client.build_get(url.as_str(), token), cancel).await?;
    let state = match response.status {
        204 => StarredState::Starred,
        // GitHub also uses 404 when the repository is not visible. This is a
        // valid "not starred" observation for this account, not proof of absence.
        404 => StarredState::Unstarred,
        _ => {
            if let Some(error) = classify_status(&response) {
                return Err(error.into());
            }
            return Err(GitHubError::UnexpectedStatus {
                status: response.status,
            }
            .into());
        }
    };
    Ok(StarObservation {
        repository: repository.clone(),
        state,
    })
}

pub async fn set_starred(
    client: &GitHubClient,
    token: &Pat,
    repository: &RepositoryIdentity,
    desired: StarredState,
    cancel: CancellationToken,
) -> Result<StarObservation, RepositoryActionError> {
    let url = endpoint(client, repository)?;
    let request = match desired {
        StarredState::Starred => client.build_put(url.as_str(), token),
        StarredState::Unstarred => client.build_delete(url.as_str(), token),
    };
    let response = execute(client, request, cancel).await?;
    if response.status != 204 {
        if let Some(error) = classify_status(&response) {
            return Err(error.into());
        }
        return Err(GitHubError::UnexpectedStatus {
            status: response.status,
        }
        .into());
    }
    Ok(StarObservation {
        repository: repository.clone(),
        state: desired,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::repository::RepositoryLocator;
    use crate::github::GitHubConfig;
    use crate::network::transport::{HttpMethod, HttpResponse, MockFailure, MockTransport};
    use std::sync::Arc;

    fn repository() -> RepositoryIdentity {
        RepositoryIdentity {
            id: 42,
            locator: RepositoryLocator::new("octo org", "ledger+one").unwrap(),
        }
    }

    fn response(status: u16) -> HttpResponse {
        HttpResponse {
            status,
            headers: vec![],
            body: String::new(),
        }
    }

    #[tokio::test]
    async fn official_status_shapes_and_encoded_paths_are_deterministic() {
        for (status, expected) in [(204, StarredState::Starred), (404, StarredState::Unstarred)] {
            let mock = Arc::new(MockTransport::new());
            mock.enqueue(Ok(response(status))).await;
            let client = GitHubClient::new(GitHubConfig::default(), mock.clone());
            let observed = fetch_starred(
                &client,
                &Pat::from_str("synthetic"),
                &repository(),
                Default::default(),
            )
            .await
            .unwrap();
            assert_eq!(observed.state, expected);
            assert_eq!(observed.repository.id(), 42);
            let requests = mock.recorded_requests().await;
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].method, HttpMethod::Get);
            assert_eq!(
                requests[0].url,
                "https://api.github.com/user/starred/octo%20org/ledger+one"
            );
        }
    }

    #[tokio::test]
    async fn only_204_confirms_star_and_unstar() {
        for (desired, method) in [
            (StarredState::Starred, HttpMethod::Put),
            (StarredState::Unstarred, HttpMethod::Delete),
        ] {
            let mock = Arc::new(MockTransport::new());
            mock.enqueue(Ok(response(204))).await;
            let client = GitHubClient::new(GitHubConfig::default(), mock.clone());
            let observed = set_starred(
                &client,
                &Pat::from_str("synthetic"),
                &repository(),
                desired,
                Default::default(),
            )
            .await
            .unwrap();
            assert_eq!(observed.state, desired);
            let requests = mock.recorded_requests().await;
            assert_eq!(requests[0].method, method);
            assert!(requests[0].body.is_none());
        }

        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(response(200))).await;
        let client = GitHubClient::new(GitHubConfig::default(), mock);
        let error = set_starred(
            &client,
            &Pat::from_str("synthetic"),
            &repository(),
            StarredState::Starred,
            Default::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            RepositoryActionError::GitHub(GitHubError::UnexpectedStatus { status: 200 })
        ));
    }

    #[tokio::test]
    async fn write_rejection_and_rate_limit_remain_typed() {
        let mock = Arc::new(MockTransport::new());
        mock.enqueue(Ok(response(403))).await;
        mock.enqueue(Ok(HttpResponse {
            status: 429,
            headers: vec![("retry-after".into(), "17".into())],
            body: String::new(),
        }))
        .await;
        let client = GitHubClient::new(GitHubConfig::default(), mock);
        for expected in [
            GitHubError::Forbidden,
            GitHubError::RateLimited {
                retry: crate::domain::error::RetryAfter {
                    kind: crate::domain::error::RateLimitKind::Secondary,
                    seconds: Some(17),
                },
            },
        ] {
            let error = set_starred(
                &client,
                &Pat::from_str("synthetic"),
                &repository(),
                StarredState::Starred,
                Default::default(),
            )
            .await
            .unwrap_err();
            assert_eq!(error, RepositoryActionError::GitHub(expected));
        }
    }

    #[tokio::test]
    async fn cancelled_status_does_not_consume_a_response() {
        let mock = Arc::new(MockTransport::new());
        let client = GitHubClient::new(GitHubConfig::default(), mock.clone());
        let (cancel, source) = CancellationToken::new();
        source.cancel();
        let error = fetch_starred(&client, &Pat::from_str("synthetic"), &repository(), cancel)
            .await
            .unwrap_err();
        assert_eq!(error, RepositoryActionError::GitHub(GitHubError::Cancelled));
        mock.enqueue(Err(MockFailure::Network("unused".into())))
            .await;
        assert!(mock.recorded_requests().await.is_empty());
        let client = GitHubClient::new(GitHubConfig::default(), mock);
        let observed = fetch_starred(
            &client,
            &Pat::from_str("synthetic"),
            &repository(),
            Default::default(),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(
                observed,
                RepositoryActionError::GitHub(GitHubError::Network { .. })
            ),
            "the unused response remained queued for the next call"
        );
    }
}
