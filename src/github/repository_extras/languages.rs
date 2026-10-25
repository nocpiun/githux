use super::*;
use crate::domain::repository_extras::LanguagesOutcome;

/// Observe language byte counts and derive proportions with a checked total.
/// An empty object or zero counts are successful observations, not failures.
pub async fn fetch_languages(
    client: &GitHubClient,
    token: &Pat,
    repository: &RepositoryIdentity,
    limits: &RepositoryExtrasLimits,
    conditional: Option<&ResponseMetadata>,
    cancel: CancellationToken,
) -> Result<LanguagesOutcome, RepositoryExtrasError> {
    limits.validate()?;
    let url = endpoint(client, repository.locator(), &["languages"])?;
    let request = request(client, token, &url, conditional)?;
    let response = execute(client, request, limits, cancel.clone()).await?;
    let metadata = metadata(&response);
    if response.status == 304 && conditional.is_some() {
        return Err(RepositoryExtrasError::InvalidResponse {
            message: "not-modified languages require a data-layer cached value",
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
    if let Some(organizations) = parse_sso_header(response.header("x-github-sso")) {
        return Err(RepositoryExtrasError::Repository(
            RepositoryError::SsoPartial { organizations },
        ));
    }
    let bytes = serde_json::from_str(&response.body).map_err(|_| {
        RepositoryExtrasError::InvalidResponse {
            message: "expected a language-to-byte-count object",
        }
    })?;
    let outcome = LanguagesOutcome::from_bytes(repository.clone(), bytes, metadata)?;
    if cancel.is_cancelled() {
        return Err(GitHubError::Cancelled.into());
    }
    Ok(outcome)
}
