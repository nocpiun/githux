//! Sidebar policy adapter. Uses the existing list snapshot service and preserves typed failures.
use super::Failure;
use crate::data::page_cache::{PageCacheError, PageOrigin, PageReadMode, PageValue};
use crate::data::{RepoService, RepositoryReadContext};
use crate::domain::{error::GitHubError, repo::ReposOutcome};
use crate::network::cancel::CancellationToken;
use crate::storage::cache::CacheAuthorization;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepositoryListOrigin {
    Network,
    Historical { verified_at_ms: i64 },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepositoryList {
    Idle,
    Loading,
    Ready {
        outcome: ReposOutcome,
        origin: RepositoryListOrigin,
    },
    Failed(Failure),
}
pub(super) struct ListReply {
    pub context: RepositoryReadContext,
    pub authorization: Option<CacheAuthorization>,
    pub result: Result<(ReposOutcome, RepositoryListOrigin), Failure>,
}
fn transient(error: &GitHubError) -> bool {
    matches!(
        error,
        GitHubError::Network { .. } | GitHubError::Timeout { .. } | GitHubError::RateLimited { .. }
    ) || matches!(error, GitHubError::UnexpectedStatus { status } if (500..600).contains(status))
}
fn cache_error(error: PageCacheError<GitHubError>) -> Failure {
    match error {
        PageCacheError::Read(e) | PageCacheError::Context(e) => Failure::GitHub(e),
        PageCacheError::Cache(e) => Failure::Cache(e),
    }
}
pub(super) async fn read(
    repos: RepoService,
    context: RepositoryReadContext,
    cancel: CancellationToken,
) -> ListReply {
    // Capture before any async read. A revoke + revalidate cannot revive this token.
    let binding = repos.cache(&context);
    let authorization = binding
        .as_ref()
        .and_then(|c| c.store().authorization(c.scope(), None));
    let result = async {
        if !repos.is_current(&context) {
            return Err(Failure::GitHub(GitHubError::StaleContext));
        }
        match repos
            .list_repos_cached(PageReadMode::Online, cancel.clone())
            .await
        {
            Ok(scoped) => match scoped.value {
                PageValue::Value {
                    value,
                    origin,
                    metadata,
                } => Ok((
                    value,
                    match origin {
                        PageOrigin::Network => RepositoryListOrigin::Network,
                        PageOrigin::HistoricalSnapshot => RepositoryListOrigin::Historical {
                            verified_at_ms: metadata.verified_at_ms,
                        },
                    },
                )),
                PageValue::Miss(reason) => Err(Failure::NotCached(reason)),
            },
            Err(PageCacheError::Cache(_)) => {
                // A cache failure must not remove the original online capability.
                let result = repos.list_repos_cancellable(cancel).await;
                if matches!(result, Err(GitHubError::Unauthorized)) {
                    repos
                        .credential_rejected(&context)
                        .await
                        .map_err(Failure::GitHub)?;
                }
                result
                    .map(|value| (value, RepositoryListOrigin::Network))
                    .map_err(Failure::GitHub)
            }
            Err(error) => {
                let history_allowed = match &error {
                    PageCacheError::Read(e) | PageCacheError::Context(e) => transient(e),
                    _ => false,
                };
                if history_allowed
                    && let Ok(scoped) = repos.list_repos_cached(PageReadMode::Offline, cancel).await
                    && let PageValue::Value {
                        value,
                        origin: PageOrigin::HistoricalSnapshot,
                        metadata,
                    } = scoped.value
                {
                    return Ok((
                        value,
                        RepositoryListOrigin::Historical {
                            verified_at_ms: metadata.verified_at_ms,
                        },
                    ));
                }
                Err(cache_error(error))
            }
        }
    }
    .await;
    let result = if !repos.is_current(&context) {
        Err(Failure::GitHub(GitHubError::StaleContext))
    } else {
        result
    };
    ListReply {
        context,
        authorization,
        result,
    }
}
