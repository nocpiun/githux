//! Bounded, low-level GitHub reads for Releases, Contributors and Languages.
//!
//! Callers supply PAT and cancellation explicitly. Account/data-session
//! integration is deliberately the next layer's responsibility. No assets,
//! external URLs, polling, automatic retries or write operations are performed.

use crate::auth::token::Pat;
use crate::domain::error::GitHubError;
use crate::domain::repository::{RepositoryError, RepositoryIdentity};
use crate::domain::repository_extras::{
    ExtrasCompleteness, ExtrasIncompleteReason, RepositoryExtrasError, ResponseMetadata,
};
use crate::github::client::GitHubClient;
use crate::github::error::{classify_status, classify_transport};
use crate::github::headers::{parse_link_next, parse_sso_header};
use crate::github::repository::endpoint;
use crate::network::cancel::CancellationToken;
use crate::network::transport::{HttpRequest, HttpResponse, TransportError};
use reqwest::Url;
use std::collections::HashSet;

mod contributors;
mod languages;
mod releases;

pub use contributors::fetch_contributors;
pub use languages::fetch_languages;
pub use releases::{fetch_latest_release, fetch_releases};

#[cfg(test)]
mod tests;

/// Per-page and aggregate defensive budgets. Complete means all Link pages were
/// observed within these limits, not that GitHub's contributor association or
/// statistics are exhaustive or current.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryExtrasLimits {
    pub per_page: usize,
    pub max_pages: usize,
    pub max_entries: usize,
    pub max_response_bytes: usize,
}

impl Default for RepositoryExtrasLimits {
    fn default() -> Self {
        Self {
            per_page: 100,
            max_pages: 10,
            max_entries: 1_000,
            max_response_bytes: 2 * 1024 * 1024,
        }
    }
}

impl RepositoryExtrasLimits {
    fn validate(&self) -> Result<(), RepositoryExtrasError> {
        if !(1..=100).contains(&self.per_page) {
            return Err(RepositoryExtrasError::InvalidLimits(
                "per_page must be in 1..=100",
            ));
        }
        if self.max_pages == 0 || self.max_entries == 0 || self.max_response_bytes == 0 {
            return Err(RepositoryExtrasError::InvalidLimits(
                "page, entry and response limits must be positive",
            ));
        }
        Ok(())
    }
}

pub(crate) fn metadata(response: &HttpResponse) -> ResponseMetadata {
    ResponseMetadata {
        etag: response.header("etag").map(str::to_owned),
        last_modified: response.header("last-modified").map(str::to_owned),
        cache_control: response.header("cache-control").map(str::to_owned),
        poll_interval_seconds: response
            .header("x-poll-interval")
            .and_then(|value| value.parse().ok()),
    }
}

pub(crate) fn request(
    client: &GitHubClient,
    token: &Pat,
    url: &Url,
    conditional: Option<&ResponseMetadata>,
) -> Result<HttpRequest, RepositoryExtrasError> {
    if !client.url_is_trusted(url.as_str()) {
        return Err(RepositoryExtrasError::Repository(
            RepositoryError::InvalidInput {
                field: "API origin",
            },
        ));
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
    Ok(request)
}

pub(crate) async fn execute(
    client: &GitHubClient,
    request: HttpRequest,
    limits: &RepositoryExtrasLimits,
    cancel: CancellationToken,
) -> Result<HttpResponse, RepositoryExtrasError> {
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }
    let response = client
        .transport()
        .execute_bounded(request, cancel.clone(), limits.max_response_bytes)
        .await
        .map_err(|error| match error {
            TransportError::ResponseTooLarge { limit } => {
                RepositoryExtrasError::Repository(RepositoryError::LimitExceeded {
                    resource: "repository extras response bytes",
                    limit,
                })
            }
            error => classify_transport(error).into(),
        })?;
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }
    // Custom transports get the same defense as ReqwestTransport's receive bound.
    if response.body.len() > limits.max_response_bytes {
        return Err(RepositoryExtrasError::Repository(
            RepositoryError::LimitExceeded {
                resource: "repository extras response bytes",
                limit: limits.max_response_bytes,
            },
        ));
    }
    Ok(response)
}

/// Execute a bounded Link-paginated array. The page parser is invoked before any
/// entry from that page is committed; prior pages remain explicitly Partial on
/// a later error. First-page errors, 204 and cancellation are never Partial.
pub(crate) enum PageOutcome<T> {
    NoContent(Vec<ResponseMetadata>),
    Content(Vec<T>, ExtrasCompleteness, Vec<ResponseMetadata>),
}

pub(crate) struct Pagination<'a> {
    pub url: Url,
    pub limits: &'a RepositoryExtrasLimits,
    pub cancel: CancellationToken,
    pub allow_no_content: bool,
}

pub(crate) async fn paginated<T, F>(
    client: &GitHubClient,
    token: &Pat,
    pagination: Pagination<'_>,
    mut parse: F,
    mut append: impl FnMut(&mut Vec<T>, &mut Vec<T>),
) -> Result<PageOutcome<T>, RepositoryExtrasError>
where
    F: FnMut(Vec<serde_json::Value>) -> Result<Vec<T>, RepositoryExtrasError>,
{
    let Pagination {
        url: initial_url,
        limits,
        cancel,
        allow_no_content,
    } = pagination;
    enum Fetched<T> {
        NoContent(HttpResponse),
        Content(HttpResponse, Vec<T>),
    }

    limits.validate()?;
    let initial_path = initial_url.path().to_owned();
    let mut next_url = Some(initial_url);
    let mut visited = HashSet::new();
    let mut items = Vec::new();
    let mut reasons = Vec::new();
    let mut page_metadata = Vec::new();
    let mut page_count = 0usize;

    while let Some(url) = next_url.take() {
        if cancel.is_cancelled() {
            return Err(GitHubError::Cancelled.into());
        }
        if page_count >= limits.max_pages {
            reasons.push(ExtrasIncompleteReason::PageLimit {
                limit: limits.max_pages,
            });
            break;
        }
        if !visited.insert(url.to_string()) {
            reasons.push(ExtrasIncompleteReason::PaginationCycle);
            break;
        }
        let result = async {
            let request = request(client, token, &url, None)?;
            let response = execute(client, request, limits, cancel.clone()).await?;
            if response.status == 204 && allow_no_content && page_count == 0 {
                return Ok(Fetched::NoContent(response));
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
            let raw: Vec<serde_json::Value> =
                serde_json::from_str(&response.body).map_err(|_| {
                    RepositoryExtrasError::InvalidResponse {
                        message: "expected a JSON array",
                    }
                })?;
            let parsed = parse(raw)?;
            if cancel.is_cancelled() {
                return Err(GitHubError::Cancelled.into());
            }
            Ok::<_, RepositoryExtrasError>(Fetched::Content(response, parsed))
        }
        .await;

        let (response, mut parsed) = match result {
            Ok(Fetched::NoContent(response)) => {
                return Ok(PageOutcome::NoContent(vec![metadata(&response)]));
            }
            Ok(Fetched::Content(response, parsed)) => (response, parsed),
            Err(error) => {
                if error == RepositoryExtrasError::GitHub(GitHubError::Cancelled) || page_count == 0
                {
                    return Err(error);
                }
                if let RepositoryExtrasError::Repository(RepositoryError::LimitExceeded {
                    limit,
                    ..
                }) = error
                {
                    reasons.push(ExtrasIncompleteReason::ResponseLimit { limit });
                } else {
                    reasons.push(ExtrasIncompleteReason::Failed(error));
                }
                break;
            }
        };
        page_count += 1;
        page_metadata.push(metadata(&response));
        if let Some(organizations) = parse_sso_header(response.header("x-github-sso")) {
            reasons.push(ExtrasIncompleteReason::SsoPartial { organizations });
        }
        append(&mut items, &mut parsed);
        if items.len() >= limits.max_entries {
            let truncated = items.len() > limits.max_entries;
            items.truncate(limits.max_entries);
            if truncated || parse_link_next(response.header("link")).is_some() {
                reasons.push(ExtrasIncompleteReason::EntryLimit {
                    limit: limits.max_entries,
                });
            }
            break;
        }
        let link = response.header("link");
        match parse_link_next(link) {
            Some(next) => {
                let Ok(next) = Url::parse(&next) else {
                    reasons.push(ExtrasIncompleteReason::InvalidPagination);
                    break;
                };
                // Same origin alone would allow a Link to jump to an unrelated
                // API endpoint. Keep pagination on this resource path as well.
                if !client.url_is_trusted(next.as_str()) {
                    reasons.push(ExtrasIncompleteReason::UntrustedPagination);
                    break;
                }
                if next.path() != initial_path
                    || !next.username().is_empty()
                    || next.password().is_some()
                    || next.fragment().is_some()
                {
                    reasons.push(ExtrasIncompleteReason::InvalidPagination);
                    break;
                }
                next_url = Some(next);
            }
            None => {
                // No next is not proof of completion when Link itself is invalid.
                if link
                    .is_some_and(|value| !value.trim().is_empty() && !valid_terminal_links(value))
                {
                    reasons.push(ExtrasIncompleteReason::InvalidPagination);
                }
            }
        }
    }

    Ok(PageOutcome::Content(
        items,
        ExtrasCompleteness::from_reasons(reasons),
        page_metadata,
    ))
}

// Conservative terminal-link validation without changing the shared parser.
fn valid_terminal_links(value: &str) -> bool {
    let mut rest = value.trim();
    loop {
        let Some(target) = rest.strip_prefix('<') else {
            return false;
        };
        let Some((target, after)) = target.split_once('>') else {
            return false;
        };
        if Url::parse(target).is_err() {
            return false;
        }
        let mut quoted = false;
        let mut escaped = false;
        let mut end = after.len();
        for (index, ch) in after.char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' if quoted => escaped = true,
                '"' => quoted = !quoted,
                ',' if !quoted => {
                    end = index;
                    break;
                }
                _ => {}
            }
        }
        if quoted || escaped {
            return false;
        }
        let mut terminal = false;
        for parameter in after[..end]
            .split(';')
            .map(str::trim)
            .filter(|part| !part.is_empty())
        {
            let Some((key, value)) = parameter.split_once('=') else {
                return false;
            };
            if key.trim().eq_ignore_ascii_case("rel") {
                for relation in value.trim().trim_matches('"').split_ascii_whitespace() {
                    if relation.eq_ignore_ascii_case("next") {
                        return false;
                    }
                    terminal |= ["prev", "first", "last"]
                        .iter()
                        .any(|known| relation.eq_ignore_ascii_case(known));
                }
            }
        }
        if !terminal {
            return false;
        }
        if end == after.len() {
            return true;
        }
        rest = after[end + 1..].trim();
    }
}
