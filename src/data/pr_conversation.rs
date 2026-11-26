//! Account-scoped PR detail read using the shared credential and request lifetime.

use crate::domain::{
    error::GitHubError,
    pr_conversation::{Conversation, Target},
};
use crate::github::pr_conversation;
use crate::network::cancel::CancellationToken;

use super::{RepositorySession, Scoped};

impl RepositorySession {
    pub(crate) async fn pr_conversation(
        &self,
        target: Target,
        cancel: CancellationToken,
    ) -> Result<Scoped<Conversation>, GitHubError> {
        let result = self
            .read(cancel, |pat, cancel| async move {
                pr_conversation::fetch(self.client(), &pat, target, cancel).await
            })
            .await;
        if matches!(result, Err(GitHubError::Unauthorized)) {
            let _ = self.service().credential_rejected(self.context()).await;
        }
        result
    }
}
