use super::PageReadSession;
use crate::data::Scoped;
use crate::domain::repository::{RepositoryError, RepositoryIdentity};
use crate::domain::repository_pulls::{PullFilter, PullListPage, PullPageCursor};
use crate::github::repository_pulls::{self, PullListLimits};
use crate::network::cancel::CancellationToken;

impl PageReadSession {
    /// One repository pull request list page under this captured account context.
    ///
    /// This is the page-cache entry point for the repository `Pull requests` tab; the
    /// tab's own session pins the repository identity, while this session supplies the
    /// credential, epoch, bounded network permit and cancellation checks.
    pub(crate) async fn repository_pulls(
        &self,
        repository: &RepositoryIdentity,
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
                    repository,
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
