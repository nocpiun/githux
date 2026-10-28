//! Reusable account and repository-preview presentation.
//!
//! This module contains no service construction, credential access or I/O. The
//! app bridge owns operations and supplies real domain snapshots; no fixture
//! account or avatar is used here.

mod view;

use crate::domain::repo::ReposOutcome;

pub(crate) use view::{AccountCallbacks, AccountsView, render};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StatusTone {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatusMessage {
    pub tone: StatusTone,
    pub text: String,
}

impl StatusMessage {
    pub fn new(tone: StatusTone, text: impl Into<String>) -> Self {
        Self {
            tone,
            text: text.into(),
        }
    }
}

/// The repository area deliberately distinguishes loading, errors, partial
/// results and a genuinely empty complete result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RepositoryPreview {
    Idle(String),
    Loading,
    Ready(ReposOutcome),
    Failed(String),
}
