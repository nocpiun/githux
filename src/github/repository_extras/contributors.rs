use super::*;
use crate::domain::repository_extras::{Contributor, ContributorsOutcome, ContributorsState};

fn parse_page(raw: Vec<serde_json::Value>) -> Result<Vec<Contributor>, RepositoryExtrasError> {
    raw.into_iter().map(Contributor::from_json).collect()
}

/// Include anonymous contributors because the top bar needs all visible
/// contribution entries. The result length is an observation, never a total.
pub async fn fetch_contributors(
    client: &GitHubClient,
    token: &Pat,
    repository: &RepositoryIdentity,
    limits: &RepositoryExtrasLimits,
    cancel: CancellationToken,
) -> Result<ContributorsOutcome, RepositoryExtrasError> {
    limits.validate()?;
    let mut url = endpoint(client, repository.locator(), &["contributors"])?;
    url.query_pairs_mut()
        .append_pair("anon", "true")
        .append_pair("per_page", &limits.per_page.to_string());
    let outcome: PageOutcome<Contributor> = paginated(
        client,
        token,
        Pagination {
            url,
            limits,
            cancel,
            allow_no_content: true,
        },
        parse_page,
        |items, page| items.append(page),
    )
    .await?;
    match outcome {
        PageOutcome::NoContent(page_metadata) => Ok(ContributorsOutcome {
            repository: repository.clone(),
            contributors: Vec::new(),
            state: ContributorsState::EmptyRepository,
            completeness: ExtrasCompleteness::Complete,
            page_metadata,
        }),
        PageOutcome::Content(contributors, completeness, page_metadata) => {
            Ok(ContributorsOutcome {
                repository: repository.clone(),
                contributors,
                state: ContributorsState::Content,
                completeness,
                page_metadata,
            })
        }
    }
}
