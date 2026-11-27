//! PR Conversation read models. The base repository and number identify the resource.

use crate::domain::{error::GitHubError, repository::RepositoryLocator};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    pub repository: RepositoryLocator,
    number: i32,
}

impl Target {
    pub fn new(repository: RepositoryLocator, number: u64) -> Option<Self> {
        let number = i32::try_from(number).ok().filter(|number| *number > 0)?;
        Some(Self { repository, number })
    }

    pub fn number(&self) -> i32 {
        self.number
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum State {
    Open,
    Closed,
    Merged,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub(crate) enum SourceBranch {
    Present { repository: String },
    Deleted { repository: String },
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Conversation {
    pub target: Target,
    pub reading: ConversationReading,
}

impl std::ops::Deref for Conversation {
    type Target = ConversationReading;

    fn deref(&self) -> &Self::Target {
        &self.reading
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub(crate) struct ConversationReading {
    pub title: String,
    pub state: State,
    pub draft: bool,
    pub author: Option<String>,
    pub body: Option<String>,
    pub base_branch: String,
    pub head_branch: String,
    pub source_branch: SourceBranch,
}

pub(crate) fn access_denied(error: &GitHubError) -> bool {
    matches!(
        error,
        GitHubError::Unauthorized
            | GitHubError::Forbidden
            | GitHubError::NotFound
            | GitHubError::SsoRequired { .. }
            | GitHubError::StaleContext
    )
}
