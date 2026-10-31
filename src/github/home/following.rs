//! `GET /user/following` — the authenticated account's followed accounts.
//!
//! Feed only ever shows activities whose subject the active account currently
//! follows, so this adapter is the source of that relationship. It reads the
//! **authenticated** list (`/user/following`), not another user's list, so the
//! relationship it reports is the same one the Feed decision depends on.
//!
//! Pagination is followed to the end with the shared `Link` parser and an
//! explicit page/item budget. A failed, truncated or unreadable lookup is
//! reported as [`FollowingReadiness::Partial`] or as a hard error — never as
//! "this account follows nobody". Callers must treat anything other than
//! `Complete` as "unknown membership", not as a negative answer.

use crate::auth::token::Pat;
use crate::domain::error::GitHubError;
use crate::domain::home::{
    FollowedAccount, Following, FollowingPartialReason, FollowingReadiness, HomeError, Login,
};
use crate::github::client::GitHubClient;
use crate::github::error::{classify_status, classify_transport};
use crate::github::headers::{parse_link_next, parse_sso_header};
use crate::network::cancel::CancellationToken;
use crate::network::transport::TransportError;
use reqwest::Url;
use serde::Deserialize;
use std::collections::BTreeMap;

/// Defensive budgets for the following adapter. The documented endpoint caps
/// `per_page` at 100; the page/item budgets bound how much one call accumulates
/// even for accounts that follow a very large number of users.
#[derive(Debug, Clone)]
pub struct FollowingLimits {
    pub max_pages: usize,
    pub max_accounts: usize,
    pub response_bytes: usize,
    pub per_page: u32,
}

impl Default for FollowingLimits {
    fn default() -> Self {
        Self {
            max_pages: 10,
            max_accounts: 1_000,
            response_bytes: 4 * 1024 * 1024,
            per_page: 100,
        }
    }
}

#[derive(Deserialize)]
struct FollowedResponse {
    id: u64,
    login: String,
    #[serde(default)]
    avatar_url: Option<String>,
}

/// Read the set of accounts the authenticated login follows.
///
/// The `login` argument is the captured active-account login; authorization is
/// verified by the account-scoped caller. The authenticated `/user/following`
/// endpoint describes the credential's own account.
pub async fn fetch_following(
    client: &GitHubClient,
    token: &Pat,
    login: &Login,
    limits: &FollowingLimits,
    cancel: CancellationToken,
) -> Result<Following, HomeError> {
    validate_limits(limits)?;
    if login.as_str().trim().is_empty() {
        return Err(HomeError::InvalidInput { field: "login" });
    }
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }

    let mut page: u32 = 1;
    // Keyed by stable numeric id so a renamed login does not duplicate an account.
    let mut accounts: BTreeMap<u64, FollowedAccount> = BTreeMap::new();
    let mut reasons: Vec<FollowingPartialReason> = Vec::new();
    let mut pages_fetched = 0usize;

    loop {
        if page as usize > limits.max_pages {
            reasons.push(FollowingPartialReason::PageLimit);
            break;
        }
        let url = following_url(client, limits.per_page, page)?;
        let response = match client
            .transport()
            .execute_bounded(
                client.build_get(url.as_str(), token),
                cancel.clone(),
                limits.response_bytes,
            )
            .await
        {
            Ok(response) => response,
            Err(TransportError::ResponseTooLarge { limit }) => {
                if pages_fetched == 0 {
                    return Err(HomeError::LimitExceeded {
                        resource: "following response bytes",
                        limit,
                    });
                }
                reasons.push(FollowingPartialReason::ResponseLimit);
                break;
            }
            Err(error) => {
                let error = classify_transport(error);
                if matches!(error, GitHubError::Cancelled) {
                    return Err(error.into());
                }
                if pages_fetched == 0 {
                    return Err(error.into());
                }
                reasons.push(FollowingPartialReason::PageFailed {
                    message: error.to_string(),
                });
                break;
            }
        };
        if cancel.is_cancelled() {
            return Err(GitHubError::Cancelled.into());
        }
        // Defense for custom transports that do not enforce the receive bound.
        if response.body.len() > limits.response_bytes {
            if pages_fetched == 0 {
                return Err(HomeError::LimitExceeded {
                    resource: "following response bytes",
                    limit: limits.response_bytes,
                });
            }
            reasons.push(FollowingPartialReason::ResponseLimit);
            break;
        }
        if let Some(organizations) = parse_sso_header(response.header("x-github-sso")) {
            reasons.push(FollowingPartialReason::SsoPartial { organizations });
        }
        if let Some(error) = classify_status(&response) {
            if pages_fetched == 0 {
                return Err(error.into());
            }
            reasons.push(FollowingPartialReason::PageFailed {
                message: error.to_string(),
            });
            break;
        }

        let parsed: Vec<FollowedResponse> = match serde_json::from_str(&response.body) {
            Ok(parsed) => parsed,
            Err(error) => {
                if pages_fetched == 0 {
                    return Err(GitHubError::Parse {
                        message: error.to_string(),
                    }
                    .into());
                }
                reasons.push(FollowingPartialReason::ParseFailed {
                    message: error.to_string(),
                });
                break;
            }
        };
        pages_fetched += 1;

        let mut truncated = false;
        for raw in parsed {
            if accounts.len() >= limits.max_accounts {
                truncated = true;
                break;
            }
            if raw.id == 0 || raw.login.trim().is_empty() {
                // Invalid entries cannot certify membership. Preserve the other
                // positive relationships, but never call this set complete.
                reasons.push(FollowingPartialReason::ParseFailed {
                    message: "invalid followed-account identity".into(),
                });
                continue;
            }
            accounts.insert(
                raw.id,
                FollowedAccount {
                    id: raw.id,
                    login: raw.login,
                    avatar_url: raw
                        .avatar_url
                        .filter(|url| !url.trim().is_empty())
                        .map(|url| url.trim().to_string()),
                },
            );
        }
        if truncated {
            reasons.push(FollowingPartialReason::AccountLimit);
            break;
        }

        match next_page(client, response.header("link"), page)? {
            NextPage::None => break,
            NextPage::Page(next) => page = next,
            NextPage::Untrusted => {
                reasons.push(FollowingPartialReason::UntrustedPagination);
                break;
            }
            NextPage::Invalid => {
                reasons.push(FollowingPartialReason::InvalidPagination);
                break;
            }
            NextPage::Cycle => {
                reasons.push(FollowingPartialReason::PaginationCycle);
                break;
            }
        }
    }

    let readiness = FollowingReadiness::from_reasons(reasons);
    Ok(Following {
        accounts: accounts.into_values().collect(),
        readiness,
    })
}

fn validate_limits(limits: &FollowingLimits) -> Result<(), HomeError> {
    if limits.per_page == 0 || limits.per_page > 100 {
        return Err(HomeError::InvalidInput { field: "per_page" });
    }
    if limits.max_pages == 0 {
        return Err(HomeError::InvalidInput { field: "max_pages" });
    }
    if limits.max_accounts == 0 {
        return Err(HomeError::InvalidInput {
            field: "max_accounts",
        });
    }
    Ok(())
}

/// Build the `/user/following` URL from our own endpoint identity. The page
/// number always comes from this layer, never from a payload-supplied link.
fn following_url(client: &GitHubClient, per_page: u32, page: u32) -> Result<Url, HomeError> {
    let mut url =
        Url::parse(&format!("{}user/following", client.config().base_url)).map_err(|_| {
            HomeError::InvalidInput {
                field: "API base URL",
            }
        })?;
    url.query_pairs_mut()
        .append_pair("per_page", &per_page.to_string())
        .append_pair("page", &page.to_string());
    if !client.url_is_trusted(url.as_str()) {
        return Err(HomeError::InvalidInput {
            field: "API origin",
        });
    }
    Ok(url)
}

enum NextPage {
    None,
    Page(u32),
    Untrusted,
    Invalid,
    Cycle,
}

/// Validate a `Link: rel="next"` target and return only its page number. The
/// request itself is rebuilt from our own endpoint, so a hostile or stale link
/// cannot change the query.
fn next_page(
    client: &GitHubClient,
    link: Option<&str>,
    current: u32,
) -> Result<NextPage, HomeError> {
    let Some(target) = parse_link_next(link) else {
        return Ok(NextPage::None);
    };
    if !client.url_is_trusted(&target) {
        return Ok(NextPage::Untrusted);
    }
    let Ok(url) = Url::parse(&target) else {
        return Ok(NextPage::Invalid);
    };
    if !url.path().trim_end_matches('/').ends_with("following") {
        return Ok(NextPage::Invalid);
    }
    let page = url
        .query_pairs()
        .find(|(key, _)| key.as_ref() == "page")
        .and_then(|(_, value)| value.parse::<u32>().ok());
    let Some(page) = page else {
        return Ok(NextPage::Invalid);
    };
    if page <= current {
        return Ok(NextPage::Cycle);
    }
    Ok(NextPage::Page(page))
}

#[cfg(test)]
mod tests;
