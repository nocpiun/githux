//! Typed in-app destinations derived from a notification's opaque subject URL.
//!
//! A subject URL is server-provided data, never a fetch target. This module only
//! compares its parsed origin and path against the thread's own repository and
//! subject type; non-matching values keep the previous "title has no action"
//! behavior.

use super::{NotificationSubject, SubjectKind};
use crate::domain::repository::RepositoryIdentity;

const API_HOST: &str = "api.github.com";

/// One Issue or Pull Request that a notification reliably points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotificationTarget {
    Issue {
        repository: RepositoryIdentity,
        number: u64,
    },
    PullRequest {
        repository: RepositoryIdentity,
        number: u64,
    },
}

impl NotificationTarget {
    /// Match `repository` + subject kind against the official REST URL shape.
    /// Anything missing, ambiguous or unsupported returns `None`; callers must
    /// then keep the current non-navigating behavior.
    pub fn from_subject(
        repository: &RepositoryIdentity,
        subject: &NotificationSubject,
    ) -> Option<Self> {
        let url = reqwest::Url::parse(subject.url.as_deref()?).ok()?;
        if url.scheme() != "https"
            || url.host_str() != Some(API_HOST)
            || url.port_or_known_default() != Some(443)
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return None;
        }
        let segments: Vec<_> = url.path_segments()?.collect();
        if segments.len() != 5 || segments[0] != "repos" {
            return None;
        }
        let locator = repository.locator();
        if segments[1] != locator.owner() || segments[2] != locator.name() {
            return None;
        }
        let number = positive_number(segments[4])?;
        match subject.kind {
            SubjectKind::Issue if segments[3] == "issues" => Some(Self::Issue {
                repository: repository.clone(),
                number,
            }),
            SubjectKind::PullRequest if segments[3] == "pulls" => Some(Self::PullRequest {
                repository: repository.clone(),
                number,
            }),
            _ => None,
        }
    }

    pub fn repository(&self) -> &RepositoryIdentity {
        match self {
            Self::Issue { repository, .. } | Self::PullRequest { repository, .. } => repository,
        }
    }

    pub fn number(&self) -> u64 {
        match self {
            Self::Issue { number, .. } | Self::PullRequest { number, .. } => *number,
        }
    }

    pub fn noun(&self) -> &'static str {
        match self {
            Self::Issue { .. } => "issue",
            Self::PullRequest { .. } => "pull request",
        }
    }
}

/// GitHub numbers are positive decimal integers. Reject `0`, signs, whitespace,
/// percent-encoded digits and leading zeros rather than silently normalizing a
/// value that the server does not produce.
fn positive_number(value: &str) -> Option<u64> {
    if value.is_empty()
        || value.starts_with('0')
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    value.parse().ok()
}

#[cfg(test)]
#[path = "target_tests.rs"]
mod tests;
