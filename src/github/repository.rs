//! Repository observation and shared snapshot-service request plumbing.
//! Uses the existing client/transport; never loads credentials or follows URLs
//! supplied by response bodies.

use crate::auth::token::Pat;
use crate::domain::error::GitHubError;
use crate::domain::repo::{RepoOwner, RepoVisibility};
use crate::domain::repository::{
    DynamicRef, RepositoryDetails, RepositoryError, RepositoryIdentity, RepositoryLocator,
    RepositoryResponseMetadata,
};
use crate::github::client::GitHubClient;
use crate::github::error::{classify_status, classify_transport};
use crate::github::headers::parse_sso_header;
use crate::network::cancel::CancellationToken;
use crate::network::transport::HttpResponse;
use reqwest::Url;
use serde::Deserialize;
use serde::de::DeserializeOwned;

/// Defensive service limits, not completeness guarantees. Body size is checked
/// after the existing transport buffers it; transport streaming limits are a
/// separate concern. No polling, caching or automatic retries.
#[derive(Debug, Clone)]
pub struct RepositoryLimits {
    pub branch_pages: usize,
    pub branch_entries: usize,
    pub tree_entries: usize,
    pub response_bytes: usize,
}

impl Default for RepositoryLimits {
    fn default() -> Self {
        Self {
            branch_pages: 50,
            branch_entries: 50_000,
            tree_entries: 100_000,
            response_bytes: 8 * 1024 * 1024,
        }
    }
}

pub(crate) fn invalid_response(message: impl Into<String>) -> RepositoryError {
    RepositoryError::InvalidResponse {
        message: message.into(),
    }
}

pub(crate) fn endpoint(
    client: &GitHubClient,
    locator: &RepositoryLocator,
    suffix: &[&str],
) -> Result<Url, RepositoryError> {
    let mut url =
        Url::parse(&client.config().base_url).map_err(|_| RepositoryError::InvalidInput {
            field: "API base URL",
        })?;
    if url.query().is_some() || url.fragment().is_some() {
        return Err(RepositoryError::InvalidInput {
            field: "API base URL",
        });
    }
    url.path_segments_mut()
        .map_err(|_| RepositoryError::InvalidInput {
            field: "API base URL",
        })?
        .pop_if_empty()
        .extend(["repos", locator.owner(), locator.name()])
        .extend(suffix.iter().copied());
    // Delegate all origin decisions to the existing client's security logic.
    if !client.url_is_trusted(url.as_str()) {
        return Err(RepositoryError::InvalidInput {
            field: "API origin",
        });
    }
    Ok(url)
}

/// Response headers for the exact request representation, including an empty
/// metadata value for a real response without HTTP validators.
pub(crate) fn response_metadata(response: &HttpResponse) -> RepositoryResponseMetadata {
    RepositoryResponseMetadata {
        etag: response.header("etag").map(str::to_owned),
        last_modified: response.header("last-modified").map(str::to_owned),
        cache_control: response.header("cache-control").map(str::to_owned),
        network_observed: true,
    }
}

/// Minimal companion API for cached callers. A 304 is deliberately returned to
/// the consumer, which must match a cached representation and validator or
/// retry unconditionally. Existing uncached callers remain unaware of it.
pub(crate) async fn get_with_metadata(
    client: &GitHubClient,
    token: &Pat,
    url: &Url,
    conditional: Option<&RepositoryResponseMetadata>,
    cancel: CancellationToken,
) -> Result<(HttpResponse, RepositoryResponseMetadata), RepositoryError> {
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }
    if !client.url_is_trusted(url.as_str()) {
        return Err(RepositoryError::InvalidInput {
            field: "API origin",
        });
    }
    let mut request = client.build_get(url.as_str(), token);
    if let Some(metadata) = conditional {
        if let Some(etag) = &metadata.etag {
            request = request.with_header("If-None-Match", etag);
        }
        if let Some(last_modified) = &metadata.last_modified {
            request = request.with_header("If-Modified-Since", last_modified);
        }
    }
    let response = client
        .transport()
        .execute(request, cancel.clone())
        .await
        .map_err(classify_transport)?;
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }
    let metadata = response_metadata(&response);
    Ok((response, metadata))
}

pub(crate) async fn get(
    client: &GitHubClient,
    token: &Pat,
    url: &Url,
    cancel: CancellationToken,
) -> Result<HttpResponse, RepositoryError> {
    get_with_metadata(client, token, url, None, cancel)
        .await
        .map(|(response, _)| response)
}

pub(crate) fn check_status(response: &HttpResponse) -> Result<(), RepositoryError> {
    if response.status == 409 {
        return Err(RepositoryError::EmptyOrUnavailable);
    }
    if let Some(error) = classify_status(response) {
        return Err(error.into());
    }
    // These readers require a JSON representation, not 204 or partial content.
    if response.status != 200 {
        return Err(GitHubError::UnexpectedStatus {
            status: response.status,
        }
        .into());
    }
    Ok(())
}

pub(crate) fn check_single(response: &HttpResponse) -> Result<(), RepositoryError> {
    check_status(response)?;
    if let Some(organizations) = parse_sso_header(response.header("x-github-sso")) {
        return Err(RepositoryError::SsoPartial { organizations });
    }
    Ok(())
}

pub(crate) fn decode<T: DeserializeOwned>(
    response: &HttpResponse,
    limits: &RepositoryLimits,
) -> Result<T, RepositoryError> {
    if response.body.len() > limits.response_bytes {
        return Err(RepositoryError::LimitExceeded {
            resource: "response bytes",
            limit: limits.response_bytes,
        });
    }
    serde_json::from_str(&response.body).map_err(|e| {
        GitHubError::Parse {
            message: e.to_string(),
        }
        .into()
    })
}

#[derive(Deserialize)]
struct RepositoryResponse {
    id: u64,
    name: String,
    owner: OwnerResponse,
    description: Option<String>,
    visibility: Option<String>,
    private: bool,
    default_branch: Option<String>,
    fork: bool,
    archived: bool,
    stargazers_count: u64,
    forks_count: u64,
    open_issues_count: u64,
    subscribers_count: Option<u64>,
    size: u64,
    html_url: String,
    clone_url: Option<String>,
    pushed_at: Option<String>,
}

#[derive(Deserialize)]
struct OwnerResponse {
    id: u64,
    login: String,
    avatar_url: Option<String>,
}

pub async fn fetch_repository(
    client: &GitHubClient,
    token: &Pat,
    locator: &RepositoryLocator,
    limits: &RepositoryLimits,
    cancel: CancellationToken,
) -> Result<RepositoryDetails, RepositoryError> {
    fetch_repository_with_metadata(client, token, locator, limits, None, cancel)
        .await
        .map(|(details, _)| details.expect("unconditional read cannot be 304"))
}

pub async fn fetch_repository_with_metadata(
    client: &GitHubClient,
    token: &Pat,
    locator: &RepositoryLocator,
    limits: &RepositoryLimits,
    conditional: Option<&RepositoryResponseMetadata>,
    cancel: CancellationToken,
) -> Result<(Option<RepositoryDetails>, RepositoryResponseMetadata), RepositoryError> {
    let url = endpoint(client, locator, &[])?;
    let (response, metadata) = get_with_metadata(client, token, &url, conditional, cancel).await?;
    if response.status == 304 {
        return Ok((None, metadata));
    }
    check_single(&response)?;
    let raw: RepositoryResponse = decode(&response, limits)?;
    let visibility = match raw.visibility.as_deref() {
        Some("private") => RepoVisibility::Private,
        Some("internal") => RepoVisibility::Internal,
        Some("public") => RepoVisibility::Public,
        None if raw.private => RepoVisibility::Private,
        None => RepoVisibility::Public,
        Some(_) => return Err(invalid_response("unknown repository visibility")),
    };
    Ok((
        Some(RepositoryDetails {
            identity: RepositoryIdentity {
                id: raw.id,
                locator: RepositoryLocator::new(&raw.owner.login, raw.name)?,
            },
            owner: RepoOwner {
                id: raw.owner.id,
                login: raw.owner.login,
                avatar_url: raw.owner.avatar_url,
            },
            description: raw.description,
            visibility,
            default_branch: raw.default_branch.map(DynamicRef::new).transpose()?,
            fork: raw.fork,
            archived: raw.archived,
            stars: raw.stargazers_count,
            forks: raw.forks_count,
            open_issues_and_prs: raw.open_issues_count,
            subscribers: raw.subscribers_count,
            size_kb: raw.size,
            html_url: raw.html_url,
            clone_url: raw.clone_url,
            pushed_at: raw.pushed_at,
        }),
        metadata,
    ))
}

#[cfg(test)]
mod tests;
