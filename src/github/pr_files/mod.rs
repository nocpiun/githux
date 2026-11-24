//! Read one bounded page of GitHub's pull request files representation.
mod patch;
pub use patch::{Patch, RowKind};

use crate::auth::token::Pat;
use crate::domain::{
    error::GitHubError,
    repository::{RepositoryError, RepositoryLocator},
};
use crate::github::{
    GitHubClient,
    error::{classify_status, classify_transport},
    headers::{parse_link_next, parse_sso_header},
    repository::{endpoint, invalid_response},
};
use crate::network::cancel::CancellationToken;
use reqwest::Url;
use serde::Deserialize;
use std::num::NonZeroU64;

pub const FILE_LIMIT: u32 = 3_000;
const PAGE_SIZE: u32 = 30;
const RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub repository: RepositoryLocator,
    pub number: NonZeroU64,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Added,
    Removed,
    Renamed,
    Modified,
    Copied,
    Changed,
    Unchanged,
    #[serde(other)]
    Other,
}

#[derive(Debug, PartialEq, Eq)]
pub struct File {
    pub filename: String,
    pub previous_filename: Option<String>,
    pub status: Status,
    pub additions: u64,
    pub deletions: u64,
    pub patch: Patch,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PageEnd {
    More,
    Complete,
    FileLimit,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Page {
    pub number: u32,
    pub files: Vec<File>,
    pub end: PageEnd,
}

impl Page {
    pub fn offset(&self) -> u32 {
        (self.number - 1) * PAGE_SIZE
    }
}

#[derive(Deserialize)]
struct RawFile {
    filename: String,
    previous_filename: Option<String>,
    status: Status,
    additions: u64,
    deletions: u64,
    patch: Option<String>,
}

pub async fn fetch_page(
    client: &GitHubClient,
    pat: &Pat,
    target: &Target,
    page: u32,
    cancel: CancellationToken,
) -> Result<Page, RepositoryError> {
    if !(1..=FILE_LIMIT / PAGE_SIZE).contains(&page) {
        return Err(RepositoryError::InvalidInput {
            field: "files page",
        });
    }
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }
    let number = target.number.to_string();
    let suffix = ["pulls", &number, "files"];
    let mut url = endpoint(client, &target.repository, &suffix)?;
    url.query_pairs_mut()
        .append_pair("per_page", &PAGE_SIZE.to_string())
        .append_pair("page", &page.to_string());
    let response = client
        .transport()
        .execute_bounded(
            client.build_get(url.as_str(), pat),
            cancel.clone(),
            RESPONSE_BYTES,
        )
        .await
        .map_err(classify_transport)?;
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }
    if response.body.len() > RESPONSE_BYTES {
        return Err(GitHubError::ResponseTooLarge {
            limit: RESPONSE_BYTES,
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
    if let Some(organizations) = parse_sso_header(response.header("x-github-sso")) {
        return Err(RepositoryError::SsoPartial { organizations });
    }
    let raw: Vec<RawFile> =
        serde_json::from_str(&response.body).map_err(|error| GitHubError::Parse {
            message: error.to_string(),
        })?;
    if raw.len() > PAGE_SIZE as usize || raw.iter().any(|file| file.filename.is_empty()) {
        return Err(invalid_response("invalid pull request files list"));
    }
    let cap_reached = (page - 1) * PAGE_SIZE + raw.len() as u32 == FILE_LIMIT;
    let end = if cap_reached {
        PageEnd::FileLimit
    } else if let Some(link) = parse_link_next(response.header("link")) {
        if page == FILE_LIMIT / PAGE_SIZE {
            // GitHub can advertise a next page at its fixed result boundary.
            return Err(invalid_response(
                "files pagination exceeded the GitHub result limit",
            ));
        }
        validate_next(client, &url, &suffix, &link, page + 1)?;
        if raw.len() != PAGE_SIZE as usize {
            return Err(invalid_response(
                "short pull request files page with a next link",
            ));
        }
        PageEnd::More
    } else {
        PageEnd::Complete
    };
    let files = raw
        .into_iter()
        .map(|file| File {
            filename: file.filename,
            previous_filename: file.previous_filename.filter(|name| !name.is_empty()),
            status: file.status,
            additions: file.additions,
            deletions: file.deletions,
            patch: Patch::from_api(file.patch.as_deref(), file.additions, file.deletions),
        })
        .collect();
    Ok(Page {
        number: page,
        files,
        end,
    })
}

fn validate_next(
    client: &GitHubClient,
    request: &Url,
    suffix: &[&str],
    advertised: &str,
    next: u32,
) -> Result<(), RepositoryError> {
    let invalid = || invalid_response("invalid pull request files pagination link");
    let link = Url::parse(advertised).map_err(|_| invalid())?;
    if !client.url_is_trusted(advertised)
        || !link.username().is_empty()
        || link.password().is_some()
        || link.fragment().is_some()
    {
        return Err(invalid());
    }
    // GitHub emits canonical /repositories/{id}/ links for /repos/{owner}/{name}
    // requests. Use the link only as a page signal. The next GET is rebuilt from
    // the original target, so nothing in this link can change the request resource.
    let config = Url::parse(&client.config().base_url).map_err(|_| invalid())?;
    let prefix = config.path().trim_end_matches('/');
    let canonical = link
        .path()
        .strip_prefix(prefix)
        .and_then(|path| path.strip_prefix("/repositories/"))
        .and_then(|path| path.split_once('/'))
        .is_some_and(|(id, path)| id.parse::<NonZeroU64>().is_ok() && path == suffix.join("/"));
    if link.path() != request.path() && !canonical {
        return Err(invalid());
    }
    let mut matched_page = false;
    let mut matched_size = false;
    for (key, value) in link.query_pairs() {
        match key.as_ref() {
            "page" if !matched_page && value.parse() == Ok(next) => matched_page = true,
            "per_page" if !matched_size && value.parse() == Ok(PAGE_SIZE) => matched_size = true,
            _ => return Err(invalid()),
        }
    }
    if !matched_page {
        return Err(invalid());
    }
    Ok(())
}
