//! Account-bound repository pull request list reads.
//!
//! The session pins the repository identity and the account/authorization context
//! captured when the tab starts reading. Every page request runs through the
//! shared `RepositorySession` credential, epoch and cancellation path, so an
//! account switch makes both success and failure carry `StaleContext` instead of
//! entering the new page.

#[cfg(test)]
#[path = "repository_pulls_tests.rs"]
mod tests;

use super::{RepoService, RepositoryReadContext, RepositorySession, Scoped};
use crate::domain::repository::{RepositoryError, RepositoryIdentity};
use crate::domain::repository_pulls::{PullFilter, PullListPage, PullListQuery, PullPageCursor};
use crate::github::{
    GitHubClient,
    repository_pulls::{self, PullListLimits},
};
use crate::network::cancel::CancellationToken;
use std::sync::Arc;

#[derive(Clone)]
pub struct PullListSession {
    repository: RepositorySession,
    client: Arc<GitHubClient>,
    identity: RepositoryIdentity,
}

impl std::fmt::Debug for PullListSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PullListSession")
            .field("context", self.context())
            .field("repository", &self.identity.locator())
            .finish_non_exhaustive()
    }
}

impl RepoService {
    /// Begin a repository pull request list. The identity is fixed for the life of
    /// the session; a different repository needs a new session.
    pub fn pull_list_session(
        &self,
        identity: RepositoryIdentity,
    ) -> Result<PullListSession, RepositoryError> {
        let repository = self.session()?;
        Ok(PullListSession {
            repository,
            client: self.client.clone(),
            identity,
        })
    }
}

impl PullListSession {
    pub fn identity(&self) -> &RepositoryIdentity {
        &self.identity
    }

    pub fn context(&self) -> &RepositoryReadContext {
        self.repository.context()
    }

    pub fn verify(&self) -> bool {
        self.repository.verify()
    }

    /// The query for one filter of this session's repository.
    pub fn query(&self, filter: PullFilter) -> PullListQuery {
        PullListQuery::new(self.identity.locator().clone(), filter)
    }

    pub async fn page(
        &self,
        filter: PullFilter,
        cursor: Option<&PullPageCursor>,
        limits: &PullListLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<PullListPage>, RepositoryError> {
        self.repository
            .read(cancel, async move |pat, cancel| {
                repository_pulls::fetch_page(
                    &self.client,
                    &pat,
                    &self.identity,
                    filter,
                    cursor,
                    limits,
                    cancel,
                )
                .await
            })
            .await
    }
}
