//! Domain types for reading a single GitHub Issue and its paginated plain comments.
//!
//! No I/O, no credentials and no UI state live here. Pull-request responses from GitHub's
//! `/repos/{owner}/{repo}/issues/{number}` endpoint are rejected explicitly as [`IssueDetailError::NotAnIssue`]
//! so an Issue view never silently renders a Pull Request.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{error::GitHubError, repository::RepositoryLocator};

/// Validated identity of a single Issue inside a repository.
///
/// Normalized equality uses ASCII-lowercased `owner` and `name` plus `number`, matching GitHub's
/// case-insensitive repository identity and workspace routing rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueLocator {
    repository: RepositoryLocator,
    number: u64,
}

impl IssueLocator {
    pub fn new(
        owner: impl Into<String>,
        name: impl Into<String>,
        number: u64,
    ) -> Result<Self, IssueDetailError> {
        if number == 0 {
            return Err(IssueDetailError::InvalidInput {
                field: "issue number",
            });
        }
        let repository =
            RepositoryLocator::new(owner, name).map_err(|_| IssueDetailError::InvalidInput {
                field: "repository locator",
            })?;
        Ok(Self { repository, number })
    }

    pub fn from_repository(
        repository: RepositoryLocator,
        number: u64,
    ) -> Result<Self, IssueDetailError> {
        if number == 0 {
            return Err(IssueDetailError::InvalidInput {
                field: "issue number",
            });
        }
        Ok(Self { repository, number })
    }

    pub fn repository(&self) -> &RepositoryLocator {
        &self.repository
    }

    pub fn owner(&self) -> &str {
        self.repository.owner()
    }

    pub fn name(&self) -> &str {
        self.repository.name()
    }

    pub fn number(&self) -> u64 {
        self.number
    }

    /// Normalized `(owner_lower, name_lower, number)` identity for cache keys and deduplication.
    pub fn normalized_key(&self) -> (String, String, u64) {
        (
            self.owner().to_ascii_lowercase(),
            self.name().to_ascii_lowercase(),
            self.number,
        )
    }

    pub fn same_resource(&self, other: &Self) -> bool {
        self.number == other.number
            && self.owner().eq_ignore_ascii_case(other.owner())
            && self.name().eq_ignore_ascii_case(other.name())
    }

    pub fn display_slug(&self) -> String {
        format!("{}/{} #{}", self.owner(), self.name(), self.number)
    }
}

/// Issue open/closed status. Unrecognized API strings stay `Unknown` instead of silently
/// becoming `Open` or `Closed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IssueState {
    Open,
    Closed,
    Unknown(String),
}

impl IssueState {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "open" => Self::Open,
            "closed" => Self::Closed,
            other => Self::Unknown(other.to_string()),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Open => "Open",
            Self::Closed => "Closed",
            Self::Unknown(_) => "Unknown",
        }
    }
}

/// GitHub's `state_reason` field on an issue (`completed`, `not_planned`, `reopened`, `duplicate`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IssueStateReason {
    Completed,
    NotPlanned,
    Reopened,
    Duplicate,
    Other(String),
}

impl IssueStateReason {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "completed" => Self::Completed,
            "not_planned" => Self::NotPlanned,
            "reopened" => Self::Reopened,
            "duplicate" => Self::Duplicate,
            other => Self::Other(other.to_string()),
        }
    }

    pub fn label(&self) -> Option<&str> {
        match self {
            Self::Completed => Some("Completed"),
            Self::NotPlanned => Some("Not planned"),
            Self::Reopened => Some("Reopened"),
            Self::Duplicate => Some("Duplicate"),
            Self::Other(_) => None,
        }
    }
}

/// Author metadata attached to an issue or comment.
///
/// GitHub may return `user: null` (deleted/ghost user); callers represent that as `Option<IssueAuthor>`
/// rather than fabricating an id or login.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueAuthor {
    pub id: Option<u64>,
    pub login: String,
    pub avatar_url: Option<String>,
}

/// Label attached to an issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueLabel {
    pub name: String,
    pub color: Option<String>,
    pub description: Option<String>,
}

/// Single read-only GitHub Issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueDetail {
    pub id: u64,
    pub number: u64,
    pub repository: RepositoryLocator,
    pub title: String,
    pub state: IssueState,
    pub state_reason: Option<IssueStateReason>,
    pub author: Option<IssueAuthor>,
    /// Raw Markdown body, or `None` when GitHub returned `null` or whitespace-only text.
    pub body: Option<String>,
    pub comments_count: u64,
    pub labels: Vec<IssueLabel>,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
    pub html_url: Option<String>,
}

impl IssueDetail {
    pub fn has_body(&self) -> bool {
        self.body
            .as_deref()
            .is_some_and(|text| !text.trim().is_empty())
    }
}

/// One plain issue comment from `GET /repos/{owner}/{repo}/issues/{number}/comments`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueComment {
    pub id: u64,
    pub author: Option<IssueAuthor>,
    /// Raw Markdown body, or `None` when GitHub returned `null` or whitespace-only text.
    pub body: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub author_association: Option<String>,
    pub html_url: Option<String>,
}

impl IssueComment {
    pub fn has_body(&self) -> bool {
        self.body
            .as_deref()
            .is_some_and(|text| !text.trim().is_empty())
    }
}

/// Validated pagination cursor for one issue's comments endpoint.
///
/// Bound to the exact issue locator, `per_page`, and endpoint origin so a cursor from one issue
/// or service configuration can never be replayed against another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueCommentsCursor {
    pub(crate) locator: IssueLocator,
    pub(crate) page: u32,
    pub(crate) per_page: u32,
    pub(crate) origin: String,
}

impl IssueCommentsCursor {
    pub fn page(&self) -> u32 {
        self.page
    }

    pub fn per_page(&self) -> u32 {
        self.per_page
    }
}

/// One bounded page of issue comments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueCommentsPage {
    pub comments: Vec<IssueComment>,
    pub next: Option<IssueCommentsCursor>,
    pub page: u32,
}

/// Typed errors for Issue detail and comment reads.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum IssueDetailError {
    #[error(transparent)]
    GitHub(#[from] GitHubError),
    #[error("item is a pull request, not an issue")]
    NotAnIssue,
    #[error("issue was deleted or issues are disabled in this repository")]
    Gone,
    #[error("invalid {field}")]
    InvalidInput { field: &'static str },
    #[error("unexpected GitHub issue response: {message}")]
    InvalidResponse { message: String },
}

impl IssueDetailError {
    /// Confirmed resource absence (404 or 410 Gone).
    pub fn is_missing(&self) -> bool {
        matches!(self, Self::GitHub(GitHubError::NotFound) | Self::Gone)
    }

    /// Authorization, permission or stale-context loss: cached private data must not be retained.
    pub fn is_auth_or_permission(&self) -> bool {
        matches!(
            self,
            Self::GitHub(
                GitHubError::Unauthorized
                    | GitHubError::Forbidden
                    | GitHubError::SsoRequired { .. }
                    | GitHubError::StaleContext
            )
        )
    }

    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::GitHub(GitHubError::Cancelled))
    }

    /// Errors where retaining an authorized cached snapshot with a retryable indicator is valid.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::GitHub(
                GitHubError::Network { .. }
                    | GitHubError::Timeout { .. }
                    | GitHubError::RateLimited { .. }
                    | GitHubError::UnexpectedStatus { .. }
            )
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locator_rejects_zero_and_normalizes_case() {
        assert!(matches!(
            IssueLocator::new("owner", "repo", 0),
            Err(IssueDetailError::InvalidInput {
                field: "issue number"
            })
        ));
        let a = IssueLocator::new("Owner", "Repo", 42).unwrap();
        let b = IssueLocator::new("owner", "repo", 42).unwrap();
        let c = IssueLocator::new("owner", "other", 42).unwrap();
        assert!(a.same_resource(&b));
        assert_eq!(a.normalized_key(), b.normalized_key());
        assert!(!a.same_resource(&c));
        assert_ne!(a.normalized_key(), c.normalized_key());
    }

    #[test]
    fn state_and_reason_preserve_unknown_honestly() {
        assert_eq!(IssueState::parse("open"), IssueState::Open);
        assert_eq!(IssueState::parse("CLOSED"), IssueState::Closed);
        assert_eq!(
            IssueState::parse("archived"),
            IssueState::Unknown("archived".into())
        );
        assert_eq!(IssueState::Unknown("x".into()).label(), "Unknown");
        assert_eq!(
            IssueStateReason::parse("not_planned"),
            IssueStateReason::NotPlanned
        );
        assert_eq!(IssueStateReason::NotPlanned.label(), Some("Not planned"));
        assert_eq!(IssueStateReason::Other("custom".into()).label(), None);
    }
}
