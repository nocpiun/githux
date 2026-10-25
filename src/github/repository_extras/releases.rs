use super::*;
use crate::domain::repository_extras::{LatestReleaseOutcome, Release, ReleasesOutcome};

fn parse_page(raw: Vec<serde_json::Value>) -> Result<Vec<Release>, RepositoryExtrasError> {
    raw.into_iter().map(Release::from_json).collect()
}

/// Observe all visible releases within the configured page/entry/body limits.
/// Draft/prerelease values and visibility are supplied by GitHub; no local
/// filtering or asset download occurs.
pub async fn fetch_releases(
    client: &GitHubClient,
    token: &Pat,
    repository: &RepositoryIdentity,
    limits: &RepositoryExtrasLimits,
    cancel: CancellationToken,
) -> Result<ReleasesOutcome, RepositoryExtrasError> {
    limits.validate()?;
    let mut url = endpoint(client, repository.locator(), &["releases"])?;
    url.query_pairs_mut()
        .append_pair("per_page", &limits.per_page.to_string());
    let outcome = paginated(
        client,
        token,
        Pagination {
            url,
            limits,
            cancel,
            allow_no_content: false,
        },
        parse_page,
        |items, page| items.append(page),
    )
    .await?;
    let PageOutcome::Content(releases, completeness, page_metadata) = outcome else {
        return Err(RepositoryExtrasError::InvalidResponse {
            message: "unexpected no-content releases response",
        });
    };
    Ok(ReleasesOutcome {
        repository: repository.clone(),
        releases,
        completeness,
        page_metadata,
    })
}

/// Read GitHub's latest published full release. A 404 is deliberately ambiguous
/// between no qualifying visible release and an invisible release/repository.
pub async fn fetch_latest_release(
    client: &GitHubClient,
    token: &Pat,
    repository: &RepositoryIdentity,
    limits: &RepositoryExtrasLimits,
    conditional: Option<&ResponseMetadata>,
    cancel: CancellationToken,
) -> Result<LatestReleaseOutcome, RepositoryExtrasError> {
    limits.validate()?;
    let url = endpoint(client, repository.locator(), &["releases", "latest"])?;
    let request = request(client, token, &url, conditional)?;
    let response = execute(client, request, limits, cancel.clone()).await?;
    let metadata = metadata(&response);
    if response.status == 404 {
        return Ok(LatestReleaseOutcome {
            repository: repository.clone(),
            release: None,
            metadata,
        });
    }
    if response.status == 304 && conditional.is_some() {
        return Err(RepositoryExtrasError::InvalidResponse {
            message: "not-modified latest release requires a data-layer cached value",
        });
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
    if parse_sso_header(response.header("x-github-sso")).is_some() {
        return Err(RepositoryExtrasError::Repository(
            RepositoryError::SsoPartial {
                organizations: parse_sso_header(response.header("x-github-sso"))
                    .unwrap_or_default(),
            },
        ));
    }
    let release: Release = serde_json::from_str::<serde_json::Value>(&response.body)
        .map_err(|_| RepositoryExtrasError::InvalidResponse {
            message: "invalid latest release JSON",
        })
        .and_then(Release::from_json)?;
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }
    Ok(LatestReleaseOutcome {
        repository: repository.clone(),
        release: Some(release),
        metadata,
    })
}
