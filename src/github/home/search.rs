//! `GET /search/issues` for a fixed `involves:` query.
//!
//! Query shape (parameterized, never a product default):
//!
//! ```text
//! q=involves:<login> [updated:>=<from>] [updated:<=<to>]
//! sort=updated&order=desc&per_page=<n>&page=<n>
//! ```
//!
//! The login is validated by [`Login`], so it cannot inject extra qualifiers;
//! the `updated` window is only emitted when the caller supplied one. The
//! adapter deliberately does **not** add review-requested relations, a default
//! time window, unread state, or browse history.

use crate::auth::token::Pat;
use crate::domain::error::GitHubError;
use crate::domain::home::{
    HomeError, HomeItem, HomeItemKind, HomePartialReason, HomeQuery, HomeReadiness,
    HomeRepositoryRef, HomeSearchOutcome,
};
use crate::github::client::GitHubClient;
use crate::github::error::{classify_status, classify_transport};
use crate::github::headers::{parse_link_next, parse_sso_header};
use crate::network::cancel::CancellationToken;
use crate::network::transport::TransportError;
use reqwest::Url;
use serde::Deserialize;
use std::collections::HashSet;

/// The REST search endpoint returns at most 1,000 results regardless of paging.
const SEARCH_RESULT_LIMIT: u64 = 1_000;

/// Defensive budgets for the search adapter. `per_page` is capped at the API
/// maximum and page/item budgets bound how much one call may accumulate.
#[derive(Debug, Clone)]
pub struct HomeSearchLimits {
    pub max_pages: usize,
    pub max_items: usize,
    pub response_bytes: usize,
    pub per_page: u32,
}

impl Default for HomeSearchLimits {
    fn default() -> Self {
        Self {
            max_pages: 10,
            max_items: SEARCH_RESULT_LIMIT as usize,
            response_bytes: 4 * 1024 * 1024,
            per_page: 100,
        }
    }
}

#[derive(Deserialize)]
struct SearchResponse {
    total_count: u64,
    #[serde(default)]
    incomplete_results: bool,
    #[serde(default)]
    items: Vec<SearchItem>,
}

#[derive(Deserialize)]
struct SearchItem {
    id: u64,
    number: u64,
    title: String,
    html_url: String,
    state: String,
    updated_at: String,
    repository_url: String,
    /// Present (as an object) only for pull requests.
    #[serde(default)]
    pull_request: Option<serde_json::Value>,
}

/// Search for issues and pull requests the caller's login participates in.
///
/// Returns a partial outcome when pagination is truncated by a local budget,
/// the 1,000-result service ceiling, `incomplete_results`, a later-page failure
/// or an untrusted/invalid `Link`. Cancellation is always a hard error.
pub async fn search_involving(
    client: &GitHubClient,
    token: &Pat,
    query: &HomeQuery,
    limits: &HomeSearchLimits,
    cancel: CancellationToken,
) -> Result<HomeSearchOutcome, HomeError> {
    validate_limits(limits)?;
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }

    let q = build_query(query);
    let mut page: u32 = 1;
    let mut items: Vec<HomeItem> = Vec::new();
    let mut seen: HashSet<u64> = HashSet::new();
    let mut reasons: Vec<HomePartialReason> = Vec::new();
    let mut total_count: u64 = 0;
    let mut incomplete_results = false;
    let mut pages_fetched = 0usize;

    loop {
        if page as usize > limits.max_pages {
            reasons.push(HomePartialReason::PageLimit);
            break;
        }

        let url = search_url(client, &q, limits.per_page, page)?;
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
                        resource: "search response bytes",
                        limit,
                    });
                }
                reasons.push(HomePartialReason::ResponseLimit);
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
                reasons.push(HomePartialReason::PageFailed {
                    message: error.to_string(),
                });
                break;
            }
        };
        // Cancellation is a hard stop even after earlier pages parsed.
        if cancel.is_cancelled() {
            return Err(GitHubError::Cancelled.into());
        }
        // Defense for custom transports that do not enforce the receive bound.
        if response.body.len() > limits.response_bytes {
            if pages_fetched == 0 {
                return Err(HomeError::LimitExceeded {
                    resource: "search response bytes",
                    limit: limits.response_bytes,
                });
            }
            reasons.push(HomePartialReason::ResponseLimit);
            break;
        }
        if let Some(organizations) = parse_sso_header(response.header("x-github-sso")) {
            reasons.push(HomePartialReason::SsoPartial { organizations });
        }
        if let Some(error) = classify_status(&response) {
            if pages_fetched == 0 {
                return Err(error.into());
            }
            reasons.push(HomePartialReason::PageFailed {
                message: error.to_string(),
            });
            break;
        }

        let parsed: SearchResponse = match serde_json::from_str(&response.body) {
            Ok(parsed) => parsed,
            Err(error) => {
                if pages_fetched == 0 {
                    return Err(GitHubError::Parse {
                        message: error.to_string(),
                    }
                    .into());
                }
                reasons.push(HomePartialReason::ParseFailed {
                    message: error.to_string(),
                });
                break;
            }
        };
        pages_fetched += 1;
        total_count = parsed.total_count;
        if parsed.incomplete_results && !incomplete_results {
            incomplete_results = true;
            reasons.push(HomePartialReason::IncompleteResults);
        }

        let mut truncated = false;
        for raw in parsed.items {
            if items.len() >= limits.max_items {
                truncated = true;
                break;
            }
            let item = convert_item(raw)?;
            if seen.insert(item.id) {
                items.push(item);
            }
        }
        if truncated {
            reasons.push(HomePartialReason::EntryLimit);
            break;
        }

        match next_page(client, response.header("link"), page)? {
            NextPage::None => {
                if total_count > SEARCH_RESULT_LIMIT {
                    reasons.push(HomePartialReason::ResultLimit);
                }
                break;
            }
            NextPage::Page(next) => page = next,
            NextPage::Untrusted => {
                reasons.push(HomePartialReason::UntrustedPagination);
                break;
            }
            NextPage::Invalid => {
                reasons.push(HomePartialReason::InvalidPagination);
                break;
            }
            NextPage::Cycle => {
                reasons.push(HomePartialReason::PaginationCycle);
                break;
            }
        }
    }

    Ok(HomeSearchOutcome {
        items,
        total_count,
        incomplete_results,
        readiness: HomeReadiness::from_reasons(reasons),
    })
}

fn validate_limits(limits: &HomeSearchLimits) -> Result<(), HomeError> {
    if limits.per_page == 0 || limits.per_page > 100 {
        return Err(HomeError::InvalidInput { field: "per_page" });
    }
    if limits.max_pages == 0 {
        return Err(HomeError::InvalidInput { field: "max_pages" });
    }
    if limits.max_items == 0 {
        return Err(HomeError::InvalidInput { field: "max_items" });
    }
    Ok(())
}

fn build_query(query: &HomeQuery) -> String {
    let mut q = format!("involves:{}", query.login.as_str());
    if let Some(window) = &query.updated {
        if let Some(from) = window.from() {
            q.push_str(" updated:>=");
            q.push_str(from.as_str());
        }
        if let Some(to) = window.to() {
            q.push_str(" updated:<=");
            q.push_str(to.as_str());
        }
    }
    q
}

/// Build the request URL from our own query identity. `query_pairs_mut` performs
/// the percent-encoding, so qualifiers cannot break out of the `q` value.
fn search_url(client: &GitHubClient, q: &str, per_page: u32, page: u32) -> Result<Url, HomeError> {
    let base = format!("{}search/issues", client.config().base_url);
    let mut url = Url::parse(&base).map_err(|_| HomeError::InvalidInput {
        field: "API base URL",
    })?;
    url.query_pairs_mut()
        .append_pair("q", q)
        .append_pair("sort", "updated")
        .append_pair("order", "desc")
        .append_pair("per_page", &per_page.to_string())
        .append_pair("page", &page.to_string());
    if !client.url_is_trusted(url.as_str()) {
        return Err(HomeError::InvalidInput {
            field: "API origin",
        });
    }
    Ok(url)
}

fn convert_item(raw: SearchItem) -> Result<HomeItem, HomeError> {
    let kind = if raw.pull_request.is_some() {
        HomeItemKind::PullRequest
    } else {
        HomeItemKind::Issue
    };
    Ok(HomeItem {
        id: raw.id,
        kind,
        number: raw.number,
        title: raw.title,
        html_url: raw.html_url,
        state: raw.state,
        updated_at: raw.updated_at,
        repository: parse_repository_url(&raw.repository_url)?,
    })
}

/// Extract `{owner}/{name}` from `…/repos/{owner}/{name}`. Parsing is done with
/// the standards-compliant URL parser; the URL is never fetched.
fn parse_repository_url(value: &str) -> Result<HomeRepositoryRef, HomeError> {
    let url = Url::parse(value).map_err(|_| HomeError::InvalidResponse {
        message: "invalid repository_url".to_string(),
    })?;
    let segments: Vec<&str> = url
        .path_segments()
        .map(|segments| segments.collect())
        .unwrap_or_default();
    if segments.len() != 3
        || segments[0] != "repos"
        || segments[1].is_empty()
        || segments[2].is_empty()
    {
        return Err(HomeError::InvalidResponse {
            message: "unexpected repository_url shape".to_string(),
        });
    }
    Ok(HomeRepositoryRef {
        owner: segments[1].to_string(),
        name: segments[2].to_string(),
    })
}

enum NextPage {
    None,
    Page(u32),
    Untrusted,
    Invalid,
    Cycle,
}

/// Validate the `Link: rel="next"` target and return only its page number.
///
/// The target must stay on the trusted origin and be the search endpoint. We
/// then rebuild the request from **our** query identity, so a hostile or
/// stale link cannot swap the search expression. A non-advancing page is a
/// cycle; a missing/garbage page is invalid.
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
    if !url.path().trim_end_matches('/').ends_with("/search/issues") {
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
