//! An account-scoped reader bound to a single pull request.
use crate::data::{RepositoryReadContext, RepositorySession, Scoped};
use crate::domain::repository::RepositoryError;
use crate::github::pr_files::{self, Page, Target};
use crate::network::cancel::CancellationToken;

#[derive(Clone)]
pub struct FilesSession {
    repository: RepositorySession,
    target: Target,
}

impl FilesSession {
    pub fn new(repository: RepositorySession, target: Target) -> Self {
        Self { repository, target }
    }

    pub fn context(&self) -> &RepositoryReadContext {
        self.repository.context()
    }

    pub fn target(&self) -> &Target {
        &self.target
    }

    pub fn verify(&self) -> bool {
        self.repository.verify()
    }

    pub async fn page(
        &self,
        number: u32,
        cancel: CancellationToken,
    ) -> Result<Scoped<Page>, RepositoryError> {
        self.repository
            .read(cancel, async move |pat, cancel| {
                pr_files::fetch_page(self.repository.client(), &pat, &self.target, number, cancel)
                    .await
            })
            .await
    }
}
