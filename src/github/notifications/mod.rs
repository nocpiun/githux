//! One bounded Notifications page per explicit request. No subject URL fetching.
use crate::auth::token::Pat;
use crate::domain::error::GitHubError;
use crate::domain::notification::*;
use crate::domain::repository::{RepositoryIdentity, RepositoryLocator};
use crate::github::client::GitHubClient;
use crate::github::{error, headers};
use crate::network::cancel::CancellationToken;
use serde::Deserialize;
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub struct NotificationLimits {
    pub max_pages: u32,
    pub max_items: usize,
    pub max_body_bytes: usize,
}
impl Default for NotificationLimits {
    fn default() -> Self {
        Self {
            max_pages: 20,
            max_items: 1000,
            max_body_bytes: 2 * 1024 * 1024,
        }
    }
}

/// Opaque validated continuation; callers cannot supply arbitrary URLs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotificationCursor {
    url: String,
    pub(crate) page: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotificationPage {
    pub items: Vec<NotificationThread>,
    pub next: Option<NotificationCursor>,
    pub partial: Vec<NotificationPartial>,
    pub metadata: NotificationMetadata,
    pub not_modified: bool,
}

/// Includes response headers even for parsing/status failures, so a failing
/// response cannot discard a server-imposed poll/rate cooldown.
#[derive(Clone, Debug)]
pub struct PageFailure {
    pub error: NotificationError,
    pub metadata: NotificationMetadata,
}
impl From<NotificationError> for PageFailure {
    fn from(error: NotificationError) -> Self {
        Self {
            error,
            metadata: NotificationMetadata::default(),
        }
    }
}

pub async fn fetch_page(
    client: &GitHubClient,
    pat: &Pat,
    cursor: Option<&NotificationCursor>,
    last_modified: Option<&str>,
    limits: &NotificationLimits,
    cancel: CancellationToken,
) -> Result<NotificationPage, PageFailure> {
    let page = cursor.map_or(1, |cursor| cursor.page);
    if limits.max_pages == 0
        || limits.max_items == 0
        || limits.max_body_bytes == 0
        || page > limits.max_pages
    {
        return Err(NotificationError::InvalidResponse("invalid resource limits").into());
    }
    let first = format!(
        "{}notifications?all=false&participating=false&per_page=50&page=1",
        client.config().base_url
    );
    let url = cursor.map_or(first.as_str(), |cursor| cursor.url.as_str());
    if !valid_page_url(client, url, page) {
        return Err(NotificationError::InvalidResponse("untrusted continuation").into());
    }
    let mut request = client.build_get(url, pat);
    if page == 1
        && let Some(value) = last_modified
    {
        request = request.with_header("If-Modified-Since", value);
    }
    let response = client
        .transport()
        .execute_bounded(request, cancel, limits.max_body_bytes)
        .await
        .map_err(|err| {
            PageFailure::from(NotificationError::from(error::classify_transport(err)))
        })?;
    let metadata = NotificationMetadata {
        last_modified: response.header("last-modified").map(str::to_owned),
        poll_interval_seconds: response
            .header("x-poll-interval")
            .and_then(|s| s.parse().ok()),
        rate_limit: {
            let rate = response.rate_limit();
            NotificationRateLimit {
                limit: rate.limit,
                remaining: rate.remaining,
                reset: rate.reset,
                retry_after: rate.retry_after,
            }
        },
    };
    let fail = |error| PageFailure {
        error,
        metadata: metadata.clone(),
    };
    if response.status == 304 {
        if page != 1 || last_modified.is_none() {
            return Err(fail(NotificationError::InvalidResponse(
                "304 without a matching representation",
            )));
        }
        return Ok(NotificationPage {
            items: vec![],
            next: None,
            partial: vec![],
            metadata,
            not_modified: true,
        });
    }
    if let Some(err) = error::classify_status(&response) {
        return Err(fail(err.into()));
    }
    if response.status != 200 {
        return Err(fail(NotificationError::InvalidResponse(
            "expected HTTP 200",
        )));
    }
    let rows: Vec<ThreadDto> = serde_json::from_str(&response.body).map_err(|_| {
        fail(
            GitHubError::Parse {
                message: "invalid notification thread array".into(),
            }
            .into(),
        )
    })?;
    if rows.len() > 50 {
        return Err(fail(NotificationError::InvalidResponse(
            "more than 50 threads in one page",
        )));
    }
    let mut items = Vec::new();
    let mut ids = HashSet::new();
    for row in rows {
        let item = row.convert().map_err(&fail)?;
        if ids.insert(item.id.clone()) {
            items.push(item);
        }
    }
    let mut partial = Vec::new();
    if let Some(organizations) = headers::parse_sso_header(response.header("x-github-sso")) {
        partial.push(NotificationPartial::Sso { organizations });
    }
    let next = headers::parse_link_next(response.header("link")).and_then(|url| {
        if !valid_page_url(client, &url, page.saturating_add(1)) {
            partial.push(NotificationPartial::UnsafePagination);
            None
        } else if page >= limits.max_pages {
            partial.push(NotificationPartial::PageLimit);
            None
        } else {
            Some(NotificationCursor {
                url,
                page: page + 1,
            })
        }
    });
    Ok(NotificationPage {
        items,
        next,
        partial,
        metadata,
        not_modified: false,
    })
}

fn valid_page_url(client: &GitHubClient, value: &str, page: u32) -> bool {
    if !client.url_is_trusted(value) {
        return false;
    }
    let (Ok(url), Ok(base)) = (
        reqwest::Url::parse(value),
        reqwest::Url::parse(&client.config().base_url),
    ) else {
        return false;
    };
    if !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.path() != format!("{}notifications", base.path())
    {
        return false;
    }
    let mut all = false;
    let mut participating = false;
    let mut per_page = false;
    let mut found_page = false;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "all" if !all && value == "false" => all = true,
            "participating" if !participating && value == "false" => participating = true,
            "per_page" if !per_page && value == "50" => per_page = true,
            "page" if !found_page && value.parse::<u32>().ok() == Some(page) => found_page = true,
            _ => return false,
        }
    }
    all && participating && per_page && found_page
}

#[derive(Deserialize)]
struct ThreadDto {
    id: String,
    repository: RepositoryDto,
    subject: SubjectDto,
    reason: String,
    unread: bool,
    updated_at: String,
    last_read_at: Option<String>,
}
#[derive(Deserialize)]
struct RepositoryDto {
    id: u64,
    name: String,
    owner: OwnerDto,
}
#[derive(Deserialize)]
struct OwnerDto {
    login: String,
}
#[derive(Deserialize)]
struct SubjectDto {
    title: String,
    #[serde(rename = "type")]
    kind: String,
    url: Option<String>,
    latest_comment_url: Option<String>,
}
impl ThreadDto {
    fn convert(self) -> Result<NotificationThread, NotificationError> {
        if self.id.is_empty()
            || self.id.len() > 128
            || self.id.chars().any(char::is_control)
            || self.repository.id == 0
        {
            return Err(NotificationError::InvalidResponse(
                "invalid thread/repository identity",
            ));
        }
        let locator = RepositoryLocator::new(self.repository.owner.login, self.repository.name)
            .map_err(|_| NotificationError::InvalidResponse("invalid repository name"))?;
        Ok(NotificationThread {
            id: self.id,
            repository: RepositoryIdentity {
                id: self.repository.id,
                locator,
            },
            subject: NotificationSubject {
                title: self.subject.title,
                kind: SubjectKind::from_api(self.subject.kind),
                url: self.subject.url,
                latest_comment_url: self.subject.latest_comment_url,
            },
            reason: self.reason,
            unread: self.unread,
            updated_at: self.updated_at,
            last_read_at: self.last_read_at,
        })
    }
}

#[cfg(test)]
mod tests;
