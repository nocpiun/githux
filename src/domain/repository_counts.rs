//! Independent OPEN issue and pull-request counts. Neither is REST's combined count.
use crate::domain::{error::GitHubError, profile::ProfileError, repository::RepositoryIdentity};

/// Typed request/field failures, preserving existing GraphQL and transport categories.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CountsError {
    #[error(transparent)]
    Read(#[from] ProfileError),
    #[error("count response repository identity is unavailable or mismatched")]
    RepositoryIdentity,
    #[error("invalid count for {0}")]
    InvalidCount(&'static str),
}
impl From<GitHubError> for CountsError {
    fn from(error: GitHubError) -> Self {
        Self::Read(error.into())
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositoryCounts {
    pub repository: RepositoryIdentity,
    pub issues: Result<u64, CountsError>,
    pub pull_requests: Result<u64, CountsError>,
}
