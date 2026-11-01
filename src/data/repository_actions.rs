//! Account-bound Star/Unstar actions. This service deliberately reuses the
//! injected RepositorySession authorization, concurrency and stale-response
//! gates rather than creating a second account service or credential path.

use crate::data::{AccountService, RepositoryReadContext, RepositorySession, Scoped};
use crate::domain::error::GitHubError;
use crate::domain::repository::{RepositoryDetails, RepositoryError, RepositoryIdentity};
use crate::domain::repository_actions::{
    RepositoryActionError, StarActionOutcome, StarConfirmation, StarObservation, StarredState,
};
use crate::github::{self, RepositoryLimits};
use crate::network::cancel::CancellationToken;
use std::sync::Arc;

/// The narrow write/read composition used by a repository page. It borrows the
/// identity of the caller's existing read session and the AccountService's
/// existing GitHub client; it owns no credential and opens no new connection.
#[derive(Clone)]
pub struct RepositoryActionService {
    session: RepositorySession,
    client: Arc<github::GitHubClient>,
}

impl RepositoryActionService {
    pub fn new(
        session: &RepositorySession,
        accounts: &AccountService,
    ) -> Result<Self, RepositoryActionError> {
        if !session.verify()
            || !accounts.repo_service().is_current(session.context())
            || accounts.active_context().as_ref() != Some(session.context().auth())
        {
            return Err(GitHubError::StaleContext.into());
        }
        Ok(Self {
            session: session.clone(),
            client: accounts.client.clone(),
        })
    }

    pub fn context(&self) -> &RepositoryReadContext {
        self.session.context()
    }

    pub fn verify(&self) -> bool {
        self.session.verify()
    }

    /// Read the account's actual starred state and never infer it from the
    /// repository's aggregate stargazer count.
    pub async fn starred(
        &self,
        repository: &RepositoryIdentity,
        cancel: CancellationToken,
    ) -> Result<Scoped<StarObservation>, RepositoryActionError> {
        let repository = repository.clone();
        let client = self.client.clone();
        self.session
            .read(cancel, async move |pat, cancel| {
                github::starring::fetch_starred(&client, &pat, &repository, cancel).await
            })
            .await
    }

    /// Apply one explicit desired state. A 204 is the only direct write success.
    /// Timeout/connection loss is reconciled with GET, and PUT/DELETE is never
    /// automatically replayed. The count is then re-read from the repository
    /// summary instead of being locally incremented or decremented.
    pub async fn set_starred(
        &self,
        repository: &RepositoryIdentity,
        desired: StarredState,
        limits: &RepositoryLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<StarActionOutcome>, RepositoryActionError> {
        let target = repository.clone();
        let client = self.client.clone();
        let write = self
            .session
            .read(cancel.clone(), async move |pat, cancel| {
                github::starring::set_starred(&client, &pat, &target, desired, cancel).await
            })
            .await;

        let (observation, confirmation) = match write {
            Ok(observation) => (observation.value, StarConfirmation::Written),
            Err(RepositoryActionError::GitHub(
                write @ (GitHubError::Timeout { .. } | GitHubError::Network { .. }),
            )) => match self.starred(repository, cancel.clone()).await {
                Ok(observation) => (observation.value, StarConfirmation::Reconciled),
                Err(RepositoryActionError::GitHub(
                    error @ (GitHubError::StaleContext | GitHubError::Cancelled),
                )) => return Err(error.into()),
                Err(error) => {
                    return Err(RepositoryActionError::UnknownOutcome {
                        write,
                        reconciliation: action_error_as_github(error),
                    });
                }
            },
            Err(error) => return Err(error),
        };

        let summary = self.summary_for_action(repository, limits, cancel).await?;
        if !self.verify() {
            return Err(GitHubError::StaleContext.into());
        }
        Ok(Scoped {
            context: self.context().clone(),
            value: StarActionOutcome {
                observation,
                summary,
                confirmation,
            },
        })
    }

    async fn summary_for_action(
        &self,
        repository: &RepositoryIdentity,
        limits: &RepositoryLimits,
        cancel: CancellationToken,
    ) -> Result<Result<RepositoryDetails, RepositoryError>, RepositoryActionError> {
        if cancel.is_cancelled() {
            return Err(GitHubError::Cancelled.into());
        }
        let summary = self
            .session
            .repository(repository.locator().clone(), limits, cancel)
            .await;
        match summary {
            Ok(summary) => {
                if summary.value.identity.id() != repository.id() {
                    return Ok(Err(RepositoryError::InvalidResponse {
                        message: format!(
                            "summary id {} does not match action target {}",
                            summary.value.identity.id(),
                            repository.id()
                        ),
                    }));
                }
                Ok(Ok(summary.value))
            }
            // Preserve account/authorization loss as a hard gate. Other summary
            // failures stay beside the confirmed starred observation.
            Err(RepositoryError::GitHub(
                error @ (GitHubError::StaleContext | GitHubError::Cancelled),
            )) => Err(error.into()),
            Err(error) => Ok(Err(error)),
        }
    }
}

fn action_error_as_github(error: RepositoryActionError) -> GitHubError {
    match error {
        RepositoryActionError::GitHub(error) => error,
        RepositoryActionError::Repository(RepositoryError::GitHub(error)) => error,
        other => GitHubError::Parse {
            message: other.to_string(),
        },
    }
}
