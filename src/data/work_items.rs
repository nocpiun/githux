//! Global lists reuse RepositorySession's credential, epoch and semaphore path.
#[cfg(test)]
#[path = "work_items_tests.rs"]
mod tests;
use super::{RepoService, RepositoryReadContext, RepositorySession, Scoped};
use crate::domain::{error::GitHubError, home::Login, work_item::*};
use crate::github::{
    GitHubClient,
    work_items::{self, WorkItemLimits},
};
use crate::network::cancel::CancellationToken;
use std::sync::Arc;

#[derive(Clone)]
pub struct WorkItemSession {
    repository: RepositorySession,
    client: Arc<GitHubClient>,
    login: Login,
}
impl std::fmt::Debug for WorkItemSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkItemSession")
            .field("context", self.context())
            .finish_non_exhaustive()
    }
}
impl RepoService {
    pub fn work_item_session(&self) -> Result<WorkItemSession, WorkItemError> {
        // session() only fails for an unavailable authorization context.
        let repository = self.session().map_err(|_| GitHubError::StaleContext)?;
        let login = Login::new(repository.context().active_login())
            .map_err(|_| GitHubError::StaleContext)?;
        Ok(WorkItemSession {
            repository,
            client: self.client.clone(),
            login,
        })
    }
}
impl WorkItemSession {
    pub fn context(&self) -> &RepositoryReadContext {
        self.repository.context()
    }
    pub fn verify(&self) -> bool {
        self.repository.verify()
    }
    pub fn query(
        &self,
        kind: WorkItemKind,
        relation: Relation,
        state: StateFilter,
    ) -> WorkItemQuery {
        let mut query = WorkItemQuery::new(kind, self.login.clone());
        query.relation = relation;
        query.state = state;
        query
    }
    pub async fn page(
        &self,
        query: &WorkItemQuery,
        cursor: Option<&WorkItemCursor>,
        limits: &WorkItemLimits,
        cancel: CancellationToken,
    ) -> Result<Scoped<WorkItemPage>, WorkItemError> {
        self.repository
            .read(cancel, async move |pat, cancel| {
                if query.login() != &self.login {
                    return Err(WorkItemError::InvalidInput {
                        field: "session login",
                    });
                }
                work_items::fetch_page(&self.client, &pat, query, cursor, limits, cancel).await
            })
            .await
    }
}
