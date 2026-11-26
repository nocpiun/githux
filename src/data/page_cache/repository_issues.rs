use super::{CachedPageReadSession, PageCacheError, PageReadMode, PageValue};
use crate::data::Scoped;
use crate::domain::error::GitHubError;
use crate::domain::{
    cache::CacheCompleteness,
    repository::{RepositoryError, RepositoryIdentity},
};
use crate::github::repository_issues::{IssueCursor, IssueFilter, IssuePage};
use crate::network::cancel::CancellationToken;
use crate::storage::cache::{AccessEvent, CacheError};

#[cfg(test)]
mod tests;

impl CachedPageReadSession {
    pub(crate) async fn repository_issues_online(
        &self,
        repository: &RepositoryIdentity,
        filter: IssueFilter,
        cursor: Option<&IssueCursor>,
        cancel: CancellationToken,
    ) -> Result<IssuePage, PageCacheError<RepositoryError>> {
        if cursor.is_none() {
            match self
                .repository_issues(PageReadMode::Online, repository, filter, cancel.clone())
                .await
            {
                Ok(Scoped {
                    value: PageValue::Value { value, .. },
                    ..
                }) => return Ok(value),
                Ok(Scoped {
                    value: PageValue::Miss(_),
                    ..
                }) => {}
                Err(PageCacheError::Cache(error))
                    if !matches!(error, CacheError::AccessDenied | CacheError::Stale) => {}
                Err(error) => return Err(error),
            }
        }
        self.guard(&cancel)?;
        let result = self
            .session
            .repository_issues(repository, filter, cursor, cancel)
            .await;
        if matches!(
            &result,
            Err(RepositoryError::GitHub(GitHubError::Unauthorized))
        ) {
            self.blocking(|store, scope| {
                store.access_event(&scope, AccessEvent::CredentialRejected)
            })
            .await?;
        }
        result
            .map(|value| value.value)
            .map_err(PageCacheError::Read)
    }

    // Persist the first batch for each filter. Continuations belong to the live list and are
    // retained in memory with its workspace tab and scroll position.
    pub(crate) async fn repository_issues(
        &self,
        mode: PageReadMode,
        repository: &RepositoryIdentity,
        filter: IssueFilter,
        cancel: CancellationToken,
    ) -> Result<Scoped<PageValue<IssuePage>>, PageCacheError<RepositoryError>> {
        let request_cancel = cancel.clone();
        self.read(
            mode,
            Some(repository.id()),
            "repository-issues",
            serde_json::json!([
                repository.locator().owner(),
                repository.locator().name(),
                filter.value()
            ]),
            cancel,
            |session| async move {
                session
                    .repository_issues(repository, filter, None, request_cancel)
                    .await
            },
            |_| CacheCompleteness::Complete,
            None,
        )
        .await
    }
}
