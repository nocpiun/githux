use super::PageReadSession;
use crate::data::Scoped;
use crate::domain::repository::{RepositoryError, RepositoryIdentity};
use crate::github::repository_issues::{self, IssueCursor, IssueFilter, IssuePage};
use crate::network::cancel::CancellationToken;

impl PageReadSession {
    pub(crate) async fn repository_issues(
        &self,
        repository: &RepositoryIdentity,
        filter: IssueFilter,
        cursor: Option<&IssueCursor>,
        cancel: CancellationToken,
    ) -> Result<Scoped<IssuePage>, RepositoryError> {
        self.repository
            .read(cancel, async move |pat, cancel| {
                repository_issues::fetch(&self.client, &pat, repository, filter, cursor, cancel)
                    .await
            })
            .await
    }
}
