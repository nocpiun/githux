//! Complete Link pagination of observed branch pointers, deduplicated by name.

use crate::auth::token::Pat;
use crate::domain::error::GitHubError;
use crate::domain::repository::{
    Branch, BranchesOutcome, Completeness, DynamicRef, IncompleteReason, ObjectSha,
    RepositoryError, RepositoryIdentity,
};
use crate::github::client::GitHubClient;
use crate::github::headers::{parse_link_next, parse_sso_header};
use crate::github::repository::{RepositoryLimits, check_status, decode, endpoint, get};
use crate::network::cancel::CancellationToken;
use reqwest::Url;
use serde::Deserialize;
use std::collections::HashSet;

#[derive(Deserialize)]
struct BranchResponse {
    name: String,
    commit: CommitPointer,
    protected: bool,
}

#[derive(Deserialize)]
struct CommitPointer {
    sha: String,
}

pub async fn fetch_branches(
    client: &GitHubClient,
    token: &Pat,
    repository: &RepositoryIdentity,
    limits: &RepositoryLimits,
    cancel: CancellationToken,
) -> Result<BranchesOutcome, RepositoryError> {
    let mut url = endpoint(client, repository.locator(), &["branches"])?;
    url.query_pairs_mut().append_pair("per_page", "100");
    let initial_path = url.path().to_owned();
    let mut visited = HashSet::new();
    let mut names = HashSet::new();
    let mut branches = Vec::new();
    let mut reasons = Vec::new();
    let mut successful_pages = 0;
    loop {
        if cancel.is_cancelled() {
            return Err(GitHubError::Cancelled.into());
        }
        if successful_pages >= limits.branch_pages {
            reasons.push(IncompleteReason::PageLimit);
            break;
        }
        if !visited.insert(url.to_string()) {
            reasons.push(IncompleteReason::PaginationCycle);
            break;
        }
        let page = async {
            let response = get(client, token, &url, cancel.clone()).await?;
            check_status(&response)?;
            let raw: Vec<BranchResponse> = decode(&response, limits)?;
            // Parse a whole page before committing it, so malformed data never
            // leaves an apparently successful fragment of that page.
            let parsed = raw
                .into_iter()
                .map(|b| {
                    Ok(Branch {
                        name: DynamicRef::new(b.name)?,
                        observed_commit: ObjectSha::new(b.commit.sha)?,
                        protected: b.protected,
                    })
                })
                .collect::<Result<Vec<_>, RepositoryError>>()?;
            Ok::<_, RepositoryError>((response, parsed))
        }
        .await;
        let (response, parsed) = match page {
            Ok(page) => page,
            Err(error) => {
                if error == RepositoryError::GitHub(GitHubError::Cancelled) || successful_pages == 0
                {
                    return Err(error);
                }
                reasons.push(IncompleteReason::Failed(error));
                break;
            }
        };
        successful_pages += 1;
        if let Some(organizations) = parse_sso_header(response.header("x-github-sso")) {
            reasons.push(IncompleteReason::SsoPartial { organizations });
        }
        for branch in parsed {
            if names.contains(branch.name.as_str()) {
                continue;
            }
            if branches.len() == limits.branch_entries {
                reasons.push(IncompleteReason::EntryLimit);
                break;
            }
            names.insert(branch.name.as_str().to_owned());
            branches.push(branch);
        }
        if reasons.contains(&IncompleteReason::EntryLimit) {
            break;
        }
        let link = response.header("link");
        match parse_link_next(link) {
            Some(next) => {
                if !client.url_is_trusted(&next) {
                    reasons.push(IncompleteReason::UntrustedPagination);
                    break;
                }
                let next = Url::parse(&next).map_err(|_| RepositoryError::InvalidInput {
                    field: "pagination URL",
                })?;
                // Same origin is necessary but pagination must also remain on
                // this repository's branch endpoint, with no userinfo/fragment.
                if next.path() != initial_path
                    || !next.username().is_empty()
                    || next.password().is_some()
                    || next.fragment().is_some()
                {
                    reasons.push(IncompleteReason::InvalidPagination);
                    break;
                }
                url = next;
            }
            None => {
                // A present, unparseable Link is not proof of completion.
                if link.is_some_and(|value| !value.trim().is_empty()) {
                    let valid_terminal = link.is_some_and(terminal_links_are_valid);
                    if !valid_terminal {
                        reasons.push(IncompleteReason::InvalidPagination);
                    }
                }
                break;
            }
        }
    }
    Ok(BranchesOutcome {
        repository: repository.clone(),
        branches,
        completeness: Completeness::from_reasons(reasons),
    })
}

// The shared next parser intentionally returns None both for no-next and for
// malformed input. Conservatively validate terminal entries so a broken next
// alongside a valid prev cannot masquerade as completion. This never decides
// credential trust or follows URLs; those remain the client's responsibility.
fn terminal_links_are_valid(value: &str) -> bool {
    let mut rest = value.trim();
    loop {
        let Some(target_start) = rest.strip_prefix('<') else {
            return false;
        };
        let Some((target, after)) = target_start.split_once('>') else {
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
            .filter(|p| !p.is_empty())
        {
            let Some((key, val)) = parameter.split_once('=') else {
                return false;
            };
            if key.trim().eq_ignore_ascii_case("rel") {
                for relation in val.trim().trim_matches('"').split_ascii_whitespace() {
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
