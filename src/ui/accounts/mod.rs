//! Reusable account presentation shared by the product Shell and the developer example.
//!
//! This module contains no service construction, credential access or I/O. Both callers own an
//! [`crate::app::accounts`] controller and pass it only display data: the account directory, the
//! current selection, one status message and the repository-list state. The masked PAT field is a
//! real `ely_core` text input owned by the caller, so the secret never enters this module's state.

mod form;

use crate::domain::repo::ReposOutcome;

pub(crate) use form::{AccountCallbacks, AccountsSection, TokenForm, accounts_section};

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

/// The current account's repository list, deliberately distinguishing loading, errors, partial
/// results and a genuinely empty complete result.
///
/// `Ready` keeps the whole `ReposOutcome`, so a partial list stays usable while its incompleteness
/// stays visible. Nothing here can turn a failure into a successful empty list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RepositoryPreview {
    /// No read is published. The message says what would be read and why nothing is shown.
    Idle(String),
    Loading,
    Ready(ReposOutcome),
    Failed(String),
}
