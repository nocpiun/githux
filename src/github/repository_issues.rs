//! Bounded repository Issues reads. GitHub includes pull requests in this endpoint.
use reqwest::Url;
use serde::Deserialize;

use crate::auth::token::Pat;
use crate::domain::{
    error::GitHubError,
    repository::{RepositoryError, RepositoryIdentity},
};
use crate::github::{
    client::GitHubClient,
    error::classify_transport,
    headers::parse_link_next,
    repository::{check_single, endpoint, invalid_response},
};
use crate::network::cancel::CancellationToken;

const PER_PAGE: &str = "30";
const RESPONSE_BYTES: usize = 8 * 1024 * 1024;
// Bound the cost of one request and one cached response set even if every source page is all PRs.
const PAGES_PER_BATCH: usize = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum IssueFilter {
    #[default]
    Open,
    Closed,
}

impl IssueFilter {
    pub(crate) fn value(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct IssueCursor {
    repository_id: u64,
    source: Url,
    next: Url,
    page: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct IssueRow {
    pub number: u64,
    pub title: String,
    pub updated_at: String,
}

#[derive(Debug)]
pub(crate) struct IssuePage {
    pub rows: Vec<IssueRow>,
    pub next: Option<IssueCursor>,
}

// Untagged decoding discards a PR without requiring any issue display fields on it.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawRow {
    PullRequest {
        #[serde(rename = "pull_request")]
        _marker: serde::de::IgnoredAny,
    },
    Issue {
        state: String,
        #[serde(flatten)]
        row: IssueRow,
    },
}

pub(crate) async fn fetch(
    client: &GitHubClient,
    token: &Pat,
    repository: &RepositoryIdentity,
    filter: IssueFilter,
    cursor: Option<&IssueCursor>,
    cancel: CancellationToken,
) -> Result<IssuePage, RepositoryError> {
    let mut source = endpoint(client, repository.locator(), &["issues"])?;
    let parameters = [
        ("state", filter.value()),
        ("sort", "created"),
        ("direction", "desc"),
        ("per_page", PER_PAGE),
    ];
    source
        .query_pairs_mut()
        .extend_pairs(parameters)
        .append_pair("page", "1");
    let (mut url, mut page) = match cursor {
        Some(cursor) if cursor.source == source && cursor.repository_id == repository.id() => {
            (cursor.next.clone(), cursor.page)
        }
        Some(_) => {
            return Err(RepositoryError::InvalidInput {
                field: "issue cursor identity",
            });
        }
        None => (source.clone(), 1),
    };
    // GitHub commonly switches the continuation to /repositories/{id}/issues.
    let base =
        Url::parse(&client.config().base_url).map_err(|_| RepositoryError::InvalidInput {
            field: "API base URL",
        })?;
    let by_id = format!(
        "{}/repositories/{}/issues",
        base.path().trim_end_matches('/'),
        repository.id()
    );
    for index in 0..PAGES_PER_BATCH {
        if cancel.is_cancelled() {
            return Err(GitHubError::Cancelled.into());
        }
        let response = client
            .transport()
            .execute_bounded(
                client.build_get(url.as_str(), token),
                cancel.clone(),
                RESPONSE_BYTES,
            )
            .await
            .map_err(classify_transport)?;
        if cancel.is_cancelled() {
            return Err(GitHubError::Cancelled.into());
        }
        check_single(&response)?;
        if response.body.len() > RESPONSE_BYTES {
            return Err(GitHubError::ResponseTooLarge {
                limit: RESPONSE_BYTES,
            }
            .into());
        }
        let rows: Vec<RawRow> =
            serde_json::from_str(&response.body).map_err(|error| GitHubError::Parse {
                message: error.to_string(),
            })?;
        let mut issues = Vec::new();
        for raw in rows {
            if let RawRow::Issue { state, row } = raw {
                if row.number == 0 || state != filter.value() {
                    return Err(invalid_response("invalid repository issue number or state"));
                }
                issues.push(row);
            }
        }
        let link = response.header("link");
        let next = match parse_link_next(link) {
            Some(link) => {
                let target = Url::parse(&link)
                    .map_err(|_| invalid_response("invalid issue pagination URL"))?;
                let next_page = page
                    .checked_add(1)
                    .ok_or_else(|| invalid_response("issue pagination overflow"))?;
                let next_number = next_page.to_string();
                let pairs = target.query_pairs().collect::<Vec<_>>();
                let mut after = pairs.iter().filter(|(key, _)| key == "after");
                let invalid_after = after.next().is_some_and(|(_, value)| value.is_empty())
                    || after.next().is_some();
                if !client.url_is_trusted(target.as_str())
                    || invalid_after
                    || pairs.iter().any(|(key, _)| {
                        !matches!(
                            key.as_ref(),
                            "state" | "sort" | "direction" | "per_page" | "page" | "after"
                        )
                    })
                    || (target.path() != source.path() && target.path() != by_id)
                    || !target.username().is_empty()
                    || target.password().is_some()
                    || target.fragment().is_some()
                    || parameters
                        .into_iter()
                        .chain([("page", next_number.as_str())])
                        .any(|(key, value)| {
                            let mut matches = pairs.iter().filter(|(name, _)| name == key);
                            matches.next().is_none_or(|(_, actual)| actual != value)
                                || matches.next().is_some()
                        })
                {
                    return Err(invalid_response(
                        "issue continuation differs from the current repository or filter",
                    ));
                }
                Some(IssueCursor {
                    repository_id: repository.id(),
                    source: source.clone(),
                    next: target,
                    page: next_page,
                })
            }
            None if link.is_some_and(|header| {
                header.contains("rel=\"next\"") || header.contains("rel=next")
            }) =>
            {
                return Err(invalid_response("invalid issue pagination link"));
            }
            None => None,
        };
        match next {
            Some(cursor) if issues.is_empty() && index + 1 < PAGES_PER_BATCH => {
                url = cursor.next;
                page = cursor.page;
            }
            next => return Ok(IssuePage { rows: issues, next }),
        }
    }
    unreachable!("the final iteration returns the continuation")
}

#[cfg(test)]
mod tests;
