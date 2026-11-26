//! Repository-scoped `GET /repos/{owner}/{repo}/pulls` reads.
//!
//! This adapter answers "which pull requests does this repository have". It
//! never issues the personal search query used by the global Issues / Pull
//! Requests pages, and it never treats an issue number or a notification thread
//! as a pull request.
//!
//! GitHub's list endpoint has no total count and sorts by `created` descending by
//! default. The request makes that order explicit. Observed live on 2026-11-26,
//! the `Link: rel="next"` continuation is **not** an echo of this request: it is
//! rewritten to `/repositories/{id}/pulls` and drops `sort`/`direction`. The
//! adapter therefore never follows a link verbatim; it validates that the link
//! still describes this exact list (same repository, no unexpected parameters,
//! values it does keep unchanged, page + 1) and then rebuilds the request from
//! the original path and canonical query. SSO and rejected pagination are
//! reported as completeness problems rather than dropping already valid rows.

use crate::auth::token::Pat;
use crate::domain::error::GitHubError;
use crate::domain::repository::{
    Completeness, DynamicRef, IncompleteReason, RepositoryError, RepositoryIdentity,
};
use crate::domain::repository_pulls::{
    PullFilter, PullListPage, PullListQuery, PullPageCursor, PullState, PullSummary,
};
use crate::github::client::GitHubClient;
use crate::github::error::{classify_status, classify_transport};
use crate::github::headers::{parse_link_next, parse_sso_header};
use crate::github::repository::endpoint;
use crate::network::cancel::CancellationToken;
use reqwest::Url;
use serde::Deserialize;
use std::collections::HashSet;

/// Defensive limits for one list request. `per_page` is sent to the endpoint
/// (which caps at 100) and also bounds the accepted page size locally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullListLimits {
    pub per_page: u32,
    pub response_bytes: usize,
}

impl Default for PullListLimits {
    fn default() -> Self {
        Self {
            per_page: 30,
            response_bytes: 4 * 1024 * 1024,
        }
    }
}

const SORT: &str = "created";
const DIRECTION: &str = "desc";

#[derive(Deserialize)]
struct PullResponse {
    id: u64,
    number: u64,
    title: String,
    state: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    merged_at: Option<String>,
    #[serde(default)]
    user: Option<AuthorResponse>,
    head: BranchResponse,
    base: BranchResponse,
    created_at: String,
    updated_at: String,
}

#[derive(Deserialize)]
struct AuthorResponse {
    login: String,
}

#[derive(Deserialize)]
struct BranchResponse {
    #[serde(rename = "ref")]
    branch: String,
}

fn endpoint_url(
    client: &GitHubClient,
    repository: &RepositoryIdentity,
    filter: PullFilter,
    per_page: u32,
    page: u32,
) -> Result<Url, RepositoryError> {
    let mut url = endpoint(client, repository.locator(), &["pulls"])?;
    // Always rebuilt here: neither a caller nor a continuation link can add,
    // remove or reorder the query that defines this list.
    url.query_pairs_mut()
        .append_pair("state", filter.api_state())
        .append_pair("sort", SORT)
        .append_pair("direction", DIRECTION)
        .append_pair("per_page", &per_page.to_string())
        .append_pair("page", &page.to_string());
    Ok(url)
}

/// Whether `url` addresses a pull request collection rather than one pull request.
fn pull_collection_path(url: &Url) -> bool {
    let segments: Vec<_> = url
        .path_segments()
        .map(|segments| segments.filter(|part| !part.is_empty()).collect::<Vec<_>>())
        .unwrap_or_default();
    match segments.as_slice() {
        ["repos", owner, name, "pulls"] => !owner.is_empty() && !name.is_empty(),
        ["repositories", id, "pulls"] => is_repository_id(id),
        _ => false,
    }
}

fn is_repository_id(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// The numeric id a `/repositories/{id}/...` URL names, if that is its shape.
fn by_id(url: &Url) -> Option<&str> {
    let mut segments = url.path_segments()?;
    (segments.next() == Some("repositories"))
        .then(|| segments.next())
        .flatten()
        .filter(|id| is_repository_id(id))
}

/// Validate one continuation link and return only its page number.
///
/// The link is never taken as a URL to fetch. It has to prove it still describes
/// this list: trusted origin, a pull collection for *this* repository (named or
/// by the matching id), no unexpected query parameter, any identity parameter it
/// keeps agreeing with the request, and exactly the next page. The next request
/// is rebuilt by [`endpoint_url`], so a link cannot change the filter, sort,
/// page size or repository.
fn next_page(
    client: &GitHubClient,
    target: &str,
    source: &Url,
    repository: &RepositoryIdentity,
    query: &PullListQuery,
    limits: &PullListLimits,
    page: u32,
) -> Result<u32, IncompleteReason> {
    if !client.url_is_trusted(target) {
        return Err(IncompleteReason::UntrustedPagination);
    }
    let Ok(parsed) = Url::parse(target) else {
        return Err(IncompleteReason::InvalidPagination);
    };
    if !pull_collection_path(&parsed)
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        return Err(IncompleteReason::InvalidPagination);
    }
    let same_repository = match by_id(&parsed) {
        // The id form is only this repository when the id matches the identity the
        // list was read from; another id is another repository, not a continuation.
        Some(id) => id == repository.id().to_string(),
        None => parsed.path() == source.path(),
    };
    if !same_repository {
        return Err(IncompleteReason::InvalidPagination);
    }
    let mut next = None;
    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            "page" => {
                if next.is_some() {
                    return Err(IncompleteReason::InvalidPagination);
                }
                next = value.parse::<u32>().ok();
            }
            // A parameter the continuation keeps must still describe this request.
            "state" if value.as_ref() != query.filter().api_state() => {
                return Err(IncompleteReason::InvalidPagination);
            }
            "sort" if value.as_ref() != SORT => {
                return Err(IncompleteReason::InvalidPagination);
            }
            "direction" if value.as_ref() != DIRECTION => {
                return Err(IncompleteReason::InvalidPagination);
            }
            "per_page" if value.parse::<u32>().ok() != Some(limits.per_page) => {
                return Err(IncompleteReason::InvalidPagination);
            }
            "state" | "sort" | "direction" | "per_page" => {}
            _ => return Err(IncompleteReason::InvalidPagination),
        }
    }
    let Some(next) = next else {
        return Err(IncompleteReason::InvalidPagination);
    };
    if next <= page {
        return Err(IncompleteReason::PaginationCycle);
    }
    if next != page + 1 {
        return Err(IncompleteReason::InvalidPagination);
    }
    Ok(next)
}

fn summary(raw: PullResponse) -> Result<PullSummary, RepositoryError> {
    if raw.id == 0 || raw.number == 0 {
        return Err(RepositoryError::InvalidResponse {
            message: "pull request identity".into(),
        });
    }
    let state = PullState::classify(&raw.state, raw.draft, raw.merged_at.as_deref())?;
    let author = raw
        .user
        .map(|user| user.login.trim().to_string())
        .filter(|login| !login.is_empty());
    Ok(PullSummary {
        id: raw.id,
        number: raw.number,
        title: raw.title,
        state,
        author,
        head: DynamicRef::new(raw.head.branch)?,
        base: DynamicRef::new(raw.base.branch)?,
        created_at: raw.created_at,
        updated_at: raw.updated_at,
    })
}

/// Read one page of the repository's pull requests.
///
/// A genuinely empty page is `Ok` with no rows and no next cursor. Only a failed
/// response is an error; a failed later page keeps the rows the caller already
/// has.
pub async fn fetch_page(
    client: &GitHubClient,
    token: &Pat,
    repository: &RepositoryIdentity,
    filter: PullFilter,
    cursor: Option<&PullPageCursor>,
    limits: &PullListLimits,
    cancel: CancellationToken,
) -> Result<PullListPage, RepositoryError> {
    if !(1..=100).contains(&limits.per_page) || limits.response_bytes == 0 {
        return Err(RepositoryError::InvalidInput {
            field: "pull list limits",
        });
    }
    let query = PullListQuery::new(repository.locator().clone(), filter);
    let page = match cursor {
        Some(cursor) => {
            if cursor.query() != &query
                || cursor.per_page() != limits.per_page
                || cursor.repository_id() != repository.id()
                || cursor.page() < 2
            {
                return Err(RepositoryError::InvalidInput {
                    field: "pull cursor identity",
                });
            }
            cursor.page()
        }
        None => 1,
    };
    let url = endpoint_url(client, repository, filter, limits.per_page, page)?;
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }
    let response = client
        .transport()
        .execute_bounded(
            client.build_get(url.as_str(), token),
            cancel.clone(),
            limits.response_bytes,
        )
        .await
        .map_err(classify_transport)?;
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }
    if response.body.len() > limits.response_bytes {
        return Err(GitHubError::ResponseTooLarge {
            limit: limits.response_bytes,
        }
        .into());
    }
    if let Some(error) = classify_status(&response) {
        return Err(error.into());
    }
    if response.status != 200 {
        return Err(GitHubError::UnexpectedStatus {
            status: response.status,
        }
        .into());
    }
    let raw: Vec<PullResponse> =
        serde_json::from_str(&response.body).map_err(|error| GitHubError::Parse {
            message: error.to_string(),
        })?;
    if raw.len() > limits.per_page as usize {
        return Err(RepositoryError::InvalidResponse {
            message: "oversized pull page".into(),
        });
    }
    let mut reasons = Vec::new();
    if let Some(organizations) = parse_sso_header(response.header("x-github-sso")) {
        reasons.push(IncompleteReason::SsoPartial { organizations });
    }
    let mut next = None;
    if let Some(target) = parse_link_next(response.header("link")) {
        match next_page(client, &target, &url, repository, &query, limits, page) {
            Ok(next_page) => {
                next = Some(PullPageCursor::new(
                    repository.id(),
                    query.clone(),
                    next_page,
                    limits.per_page,
                ));
            }
            Err(reason) => reasons.push(reason),
        }
    } else if response
        .header("link")
        .is_some_and(|link| link.contains("next"))
    {
        reasons.push(IncompleteReason::InvalidPagination);
    }
    let mut seen = HashSet::new();
    let mut pulls = Vec::with_capacity(raw.len());
    for raw in raw {
        let pull = summary(raw)?;
        if !seen.insert(pull.id) {
            return Err(RepositoryError::InvalidResponse {
                message: "duplicate pull request in one page".into(),
            });
        }
        pulls.push(pull);
    }
    Ok(PullListPage {
        query,
        page,
        pulls,
        next,
        completeness: Completeness::from_reasons(reasons),
    })
}

#[cfg(test)]
#[path = "repository_pulls/tests.rs"]
mod tests;
