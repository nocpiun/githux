//! Read-only notification observations. Thread IDs are not Issue/PR IDs.
use crate::domain::error::GitHubError;
use crate::domain::repository::RepositoryIdentity;
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubjectKind {
    Issue,
    PullRequest,
    Commit,
    Release,
    Discussion,
    Unknown(String),
}

impl SubjectKind {
    pub fn from_api(value: String) -> Self {
        match value.as_str() {
            "Issue" => Self::Issue,
            "PullRequest" => Self::PullRequest,
            "Commit" => Self::Commit,
            "Release" => Self::Release,
            "Discussion" => Self::Discussion,
            _ => Self::Unknown(value),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotificationSubject {
    pub title: String,
    pub kind: SubjectKind,
    /// Opaque server data; never an authenticated fetch or navigation target.
    pub url: Option<String>,
    pub latest_comment_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotificationThread {
    pub id: String,
    pub repository: RepositoryIdentity,
    pub subject: NotificationSubject,
    /// Preserve future reasons without inventing an action or classification.
    pub reason: String,
    pub unread: bool,
    pub updated_at: String,
    pub last_read_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotificationPartial {
    Sso { organizations: Vec<String> },
    UnsafePagination,
    PageLimit,
    ItemLimit,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NotificationRateLimit {
    pub limit: Option<u32>,
    pub remaining: Option<u32>,
    pub reset: Option<u64>,
    pub retry_after: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NotificationMetadata {
    pub last_modified: Option<String>,
    pub poll_interval_seconds: Option<u64>,
    pub rate_limit: NotificationRateLimit,
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum NotificationError {
    #[error(transparent)]
    GitHub(#[from] GitHubError),
    #[error("notification response is invalid: {0}")]
    InvalidResponse(&'static str),
    #[error("wait {seconds} seconds before trying again")]
    Cooldown { seconds: u64 },
    #[error("a notification request is already running")]
    Busy,
}

impl NotificationError {
    pub fn recovery(&self) -> String {
        match self {
            Self::GitHub(GitHubError::Unauthorized) => {
                "Replace the account's Classic PAT in Settings, then refresh.".into()
            }
            Self::GitHub(GitHubError::Forbidden) => {
                "Check Classic PAT notifications/repo scope and organization policy, then refresh."
                    .into()
            }
            Self::GitHub(GitHubError::SsoRequired { .. }) => {
                "Authorize the Classic PAT for the organization in GitHub settings, then refresh."
                    .into()
            }
            Self::GitHub(GitHubError::RateLimited { .. }) | Self::Cooldown { .. } => {
                "Wait for the request cooldown, then retry.".into()
            }
            Self::GitHub(GitHubError::StaleContext) => "Select the current account again.".into(),
            _ => "Check your connection and retry.".into(),
        }
    }
}
