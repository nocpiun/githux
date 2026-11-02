//! Framework-independent contracts for the single supported repository write:
//! Star/Unstar. The stargazer count is a separate repository observation; it is
//! never derived from the current user's starred state.

use crate::domain::error::GitHubError;
use crate::domain::repository::{RepositoryDetails, RepositoryError, RepositoryIdentity};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StarredState {
    Starred,
    Unstarred,
}

impl StarredState {
    pub fn is_starred(self) -> bool {
        self == Self::Starred
    }

    pub fn toggled(self) -> Self {
        match self {
            Self::Starred => Self::Unstarred,
            Self::Unstarred => Self::Starred,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Starred => "Starred",
            Self::Unstarred => "Star",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StarObservation {
    pub repository: RepositoryIdentity,
    pub state: StarredState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StarConfirmation {
    /// A normal status read, followed by an authoritative repository summary.
    Observed,
    /// The server confirmed the requested PUT/DELETE with 204.
    Written,
    /// A timeout/connection failure was reconciled with GET, never by replaying
    /// the write. The observed state may or may not equal the requested one.
    Reconciled,
}

/// A usable starred observation plus the independently read repository summary.
/// Keeping a summary failure beside a confirmed state avoids pretending the
/// server write was undone merely because the subsequent count read failed.
#[derive(Debug)]
pub struct StarActionOutcome {
    pub observation: StarObservation,
    pub summary: Result<RepositoryDetails, RepositoryError>,
    pub confirmation: StarConfirmation,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RepositoryActionError {
    #[error(transparent)]
    GitHub(#[from] GitHubError),
    #[error(transparent)]
    Repository(#[from] RepositoryError),
    #[error("repository summary id {actual} does not match action target {expected}")]
    IdentityMismatch { expected: u64, actual: u64 },
    #[error(
        "star outcome was unknown ({write}); reconciliation also failed ({reconciliation}); the write was not replayed"
    )]
    UnknownOutcome {
        write: GitHubError,
        reconciliation: GitHubError,
    },
}
