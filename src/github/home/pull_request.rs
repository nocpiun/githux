//! `GET /repos/{owner}/{repo}/pulls/{number}` — author resolution for a merged
//! pull request.
//!
//! A live `PullRequestEvent` with `action=merged` carries only a slim
//! `pull_request` object (`url`, `id`, `number`, `head`, `base`) with no `user`
//! and, depending on the source, no `merged` flag. The contribution card needs
//! the **actual author**, never the maintainer actor who clicked merge, so the
//! author is read from the documented pull-request detail endpoint.
//!
//! Security: the repository and number are taken from the parsed event, and the
//! URL is rebuilt from the trusted API base. The payload's own `url` is never
//! fetched, so a crafted event cannot redirect the credential to another host
//! (see LESSONS L007).

use crate::auth::token::Pat;
use crate::domain::error::GitHubError;
use crate::domain::home::{
    FeedAuthor, HomeError, PullRequestDetail, PullRequestDetailOutcome, ResolvedPullRequest,
};
use crate::github::client::GitHubClient;
use crate::github::error::{classify_status, classify_transport};
use crate::network::cancel::CancellationToken;
use crate::network::transport::TransportError;
use reqwest::Url;
use serde::Deserialize;

/// One pull-request detail read is small; the budget is only a defense against
/// a pathological body.
#[derive(Debug, Clone)]
pub struct PullRequestDetailLimits {
    pub response_bytes: usize,
}

impl Default for PullRequestDetailLimits {
    fn default() -> Self {
        Self {
            response_bytes: 1024 * 1024,
        }
    }
}

#[derive(Deserialize)]
struct PullRequestResponse {
    id: u64,
    number: u64,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    merged: Option<bool>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    user: Option<UserResponse>,
}

#[derive(Deserialize)]
struct UserResponse {
    id: u64,
    login: String,
    #[serde(default)]
    avatar_url: Option<String>,
}

/// Read one pull request's detail to resolve its author.
///
/// Returns a typed error for a failed read. A successful read that cannot
/// identify an author is [`PullRequestDetailOutcome::Unattributable`]: the caller
/// must not credit the merger instead.
pub async fn fetch_pull_request_detail(
    client: &GitHubClient,
    token: &Pat,
    owner: &str,
    repository: &str,
    number: u64,
    limits: &PullRequestDetailLimits,
    cancel: CancellationToken,
) -> Result<PullRequestDetail, HomeError> {
    if limits.response_bytes == 0 {
        return Err(HomeError::InvalidInput {
            field: "response_bytes",
        });
    }
    let url = detail_url(client, owner, repository, number)?;
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }
    let response = match client
        .transport()
        .execute_bounded(
            client.build_get(url.as_str(), token),
            cancel,
            limits.response_bytes,
        )
        .await
    {
        Ok(response) => response,
        Err(TransportError::ResponseTooLarge { limit }) => {
            return Err(HomeError::LimitExceeded {
                resource: "pull request detail response bytes",
                limit,
            });
        }
        Err(error) => return Err(classify_transport(error).into()),
    };
    // Defense for custom transports that do not enforce the receive bound.
    if response.body.len() > limits.response_bytes {
        return Err(HomeError::LimitExceeded {
            resource: "pull request detail response bytes",
            limit: limits.response_bytes,
        });
    }
    if let Some(error) = classify_status(&response) {
        return Err(error.into());
    }
    let raw: PullRequestResponse =
        serde_json::from_str(&response.body).map_err(|error| GitHubError::Parse {
            message: error.to_string(),
        })?;
    if raw.number != number {
        // Answering a different pull request is not usable evidence for this
        // event's target.
        return Err(GitHubError::Parse {
            message: "pull request detail number did not match the requested number".to_string(),
        }
        .into());
    }
    let outcome = match raw.user {
        Some(user) if !user.login.trim().is_empty() => {
            PullRequestDetailOutcome::Attributed(ResolvedPullRequest {
                author: FeedAuthor {
                    id: user.id,
                    login: user.login.trim().to_string(),
                    avatar_url: user
                        .avatar_url
                        .filter(|url| !url.trim().is_empty())
                        .map(|url| url.trim().to_string()),
                },
                merged: raw.merged.unwrap_or(false),
                title: raw
                    .title
                    .filter(|title| !title.trim().is_empty())
                    .map(|title| title.trim().to_string()),
                html_url: raw.html_url.filter(|url| !url.trim().is_empty()),
            })
        }
        _ => PullRequestDetailOutcome::Unattributable,
    };
    Ok(PullRequestDetail {
        id: raw.id,
        number: raw.number,
        state: raw.state,
        merged: raw.merged.unwrap_or(false),
        outcome,
    })
}

fn detail_url(
    client: &GitHubClient,
    owner: &str,
    repository: &str,
    number: u64,
) -> Result<Url, HomeError> {
    // Segments are validated separately so nothing can escape the path; the
    // whole URL is then origin-checked by the shared client rule.
    for segment in [owner, repository] {
        if segment.trim().is_empty() || segment.len() > 100 {
            return Err(HomeError::InvalidInput {
                field: "repository identity",
            });
        }
        // Dot segments are collapsed by the URL parser, which could move the
        // request to a different path than the one this layer intended.
        if segment == "." || segment == ".." {
            return Err(HomeError::InvalidInput {
                field: "repository identity",
            });
        }
        if segment
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\u{5c}' | '?' | '#' | '@'))
        {
            return Err(HomeError::InvalidInput {
                field: "repository identity",
            });
        }
    }
    let mut url = Url::parse(&client.config().base_url).map_err(|_| HomeError::InvalidInput {
        field: "API base URL",
    })?;
    url.set_query(None);
    url.set_fragment(None);
    url.path_segments_mut()
        .map_err(|_| HomeError::InvalidInput {
            field: "API base URL",
        })?
        .pop_if_empty()
        .extend(["repos", owner, repository, "pulls", &number.to_string()]);
    if !client.url_is_trusted(url.as_str()) {
        return Err(HomeError::InvalidInput {
            field: "API origin",
        });
    }
    Ok(url)
}

#[cfg(test)]
mod tests;
