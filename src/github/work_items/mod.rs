//! One bounded Search page per call, with shared network/error/header primitives.
use crate::auth::token::Pat;
use crate::domain::error::GitHubError;
use crate::domain::work_item::*;
use crate::github::{
    client::GitHubClient,
    error::{classify_status, classify_transport},
    headers::{parse_link_next, parse_sso_header},
};
use crate::network::cancel::CancellationToken;
use reqwest::Url;
use serde::Deserialize;
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct WorkItemLimits {
    pub per_page: u32,
    pub response_bytes: usize,
}
impl Default for WorkItemLimits {
    fn default() -> Self {
        Self {
            per_page: 50,
            response_bytes: 4 * 1024 * 1024,
        }
    }
}
#[derive(Deserialize)]
struct Response {
    total_count: u64,
    incomplete_results: bool,
    items: Vec<RawItem>,
}
#[derive(Deserialize)]
struct RawItem {
    id: u64,
    number: u64,
    title: String,
    state: String,
    updated_at: String,
    repository_url: String,
    pull_request: Option<PullRequest>,
}
#[derive(Deserialize)]
struct PullRequest {
    #[serde(default, deserialize_with = "merge_status")]
    merged_at: MergeStatus,
}
#[derive(Default)]
enum MergeStatus {
    #[default]
    Unknown,
    Unmerged,
    Merged,
}
fn merge_status<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<MergeStatus, D::Error> {
    Ok(match Option::<String>::deserialize(deserializer)? {
        Some(value) if !value.trim().is_empty() => MergeStatus::Merged,
        Some(_) => MergeStatus::Unknown,
        None => MergeStatus::Unmerged,
    })
}

pub async fn fetch_page(
    client: &GitHubClient,
    pat: &Pat,
    query: &WorkItemQuery,
    cursor: Option<&WorkItemCursor>,
    limits: &WorkItemLimits,
    cancel: CancellationToken,
) -> Result<WorkItemPage, WorkItemError> {
    if !(1..=100).contains(&limits.per_page) || limits.response_bytes == 0 {
        return Err(WorkItemError::InvalidInput {
            field: "search limits",
        });
    }
    let endpoint = format!("{}search/issues", client.config().base_url);
    let mut url =
        Url::parse(&endpoint).map_err(|_| WorkItemError::InvalidInput { field: "API base" })?;
    if !client.url_is_trusted(url.as_str()) {
        return Err(WorkItemError::InvalidInput {
            field: "API origin",
        });
    }
    let page = if let Some(cursor) = cursor {
        if cursor.query != *query || cursor.per_page != limits.per_page || cursor.origin != endpoint
        {
            return Err(WorkItemError::InvalidInput {
                field: "cursor identity",
            });
        }
        cursor.page
    } else {
        1
    };
    if page == 0 || u64::from(page - 1) * u64::from(limits.per_page) >= 1000 {
        return Err(WorkItemError::InvalidInput {
            field: "search result limit",
        });
    }
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }
    url.query_pairs_mut()
        .append_pair("q", &query.expression())
        .append_pair("sort", "updated")
        .append_pair("order", "desc")
        .append_pair("per_page", &limits.per_page.to_string())
        .append_pair("page", &page.to_string());
    let response = client
        .transport()
        .execute_bounded(
            client.build_get(url.as_str(), pat),
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
    let raw: Response =
        serde_json::from_str(&response.body).map_err(|error| GitHubError::Parse {
            message: error.to_string(),
        })?;
    if raw.items.len() > limits.per_page as usize {
        return Err(WorkItemError::InvalidResponse {
            message: "oversized search page".into(),
        });
    }
    let mut partial = Vec::new();
    if raw.incomplete_results {
        partial.push(PartialReason::IncompleteResults);
    }
    if raw.total_count > 1000 {
        partial.push(PartialReason::ResultLimit);
    }
    if let Some(organizations) = parse_sso_header(response.header("x-github-sso")) {
        partial.push(PartialReason::SsoPartial { organizations });
    }
    let mut next = None;
    if let Some(target) = parse_link_next(response.header("link")) {
        let parsed = Url::parse(&target).ok();
        if !client.url_is_trusted(&target) {
            partial.push(PartialReason::UntrustedPagination);
        } else if let Some(target) = parsed.filter(|target| {
            target.path() == url.path()
                && target.username().is_empty()
                && target.password().is_none()
                && target.fragment().is_none()
        }) {
            let pages: Vec<_> = target
                .query_pairs()
                .filter(|(key, _)| key == "page")
                .collect();
            match pages
                .first()
                .and_then(|(_, value)| value.parse::<u32>().ok())
                .filter(|_| pages.len() == 1)
            {
                Some(n) if n <= page => partial.push(PartialReason::PaginationCycle),
                Some(n) if n != page + 1 => partial.push(PartialReason::InvalidPagination),
                Some(n) if u64::from(n.saturating_sub(1)) * u64::from(limits.per_page) >= 1000 => {
                    if !partial.contains(&PartialReason::ResultLimit) {
                        partial.push(PartialReason::ResultLimit);
                    }
                }
                Some(n) => {
                    next = Some(WorkItemCursor {
                        query: query.clone(),
                        page: n,
                        per_page: limits.per_page,
                        origin: endpoint,
                    })
                }
                None => partial.push(PartialReason::InvalidPagination),
            }
        } else {
            partial.push(PartialReason::InvalidPagination);
        }
    } else if response
        .header("link")
        .is_some_and(|link| link.contains("next"))
    {
        partial.push(PartialReason::InvalidPagination);
    }
    if next.is_none()
        && ((raw.items.is_empty() && raw.total_count > 0)
            || (raw.total_count.min(1000) > u64::from(page) * u64::from(limits.per_page)))
        && !partial.contains(&PartialReason::ResultLimit)
        && !partial.contains(&PartialReason::IncompleteResults)
    {
        partial.push(PartialReason::InvalidPagination);
    }
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    let remaining = (1000 - (page - 1) * limits.per_page) as usize;
    if raw.items.len() > remaining && !partial.contains(&PartialReason::ResultLimit) {
        partial.push(PartialReason::ResultLimit);
    }
    for raw in raw.items.into_iter().take(remaining) {
        let kind = if raw.pull_request.is_some() {
            WorkItemKind::PullRequest
        } else {
            WorkItemKind::Issue
        };
        if kind != query.kind {
            return Err(WorkItemError::InvalidResponse {
                message: "search item kind differs from query".into(),
            });
        }
        let repo = Url::parse(&raw.repository_url).map_err(|_| WorkItemError::InvalidResponse {
            message: "repository URL".into(),
        })?;
        let base = Url::parse(&client.config().base_url)
            .map_err(|_| WorkItemError::InvalidInput { field: "API base" })?;
        let prefix = format!("{}repos/", base.path());
        let parts: Vec<_> = repo
            .path()
            .strip_prefix(&prefix)
            .unwrap_or("")
            .split('/')
            .collect();
        if !client.url_is_trusted(repo.as_str())
            || !repo.username().is_empty()
            || repo.password().is_some()
            || repo.query().is_some()
            || repo.fragment().is_some()
            || parts.len() != 2
            || parts.iter().any(|part| {
                part.is_empty()
                    || !part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
                    || *part == "."
                    || *part == ".."
            })
        {
            return Err(WorkItemError::InvalidResponse {
                message: "untrusted repository identity".into(),
            });
        }
        let state = match raw.state.as_str() {
            "open" => ItemState::Open,
            "closed" => match raw.pull_request.as_ref().map(|pr| &pr.merged_at) {
                Some(MergeStatus::Merged) => ItemState::Merged,
                Some(MergeStatus::Unknown) => ItemState::Unknown,
                Some(MergeStatus::Unmerged) | None => ItemState::Closed,
            },
            _ => ItemState::Unknown,
        };
        if seen.insert(raw.id) {
            items.push(WorkItem {
                id: raw.id,
                kind,
                number: raw.number,
                title: raw.title,
                repository: WorkRepository {
                    host: match base.host_str() {
                        Some("api.github.com") => "github.com",
                        other => other.unwrap_or_default(),
                    }
                    .into(),
                    owner: parts[0].into(),
                    name: parts[1].into(),
                },
                state,
                updated_at: raw.updated_at,
            });
        }
    }
    Ok(WorkItemPage {
        query: query.clone(),
        page,
        items,
        total_count: raw.total_count,
        next,
        partial,
    })
}

#[cfg(test)]
mod tests;
