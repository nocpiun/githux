//! Reusable repository TOP information component for Releases, Contributors and Languages.
//!
//! The component renders only supplied observations. It never reads credentials, calls HTTP,
//! downloads assets, opens URLs or persists a cache. The host binds account, repository and
//! request generation in [`OverviewBinding`], applies each section's typed outcome through
//! [`RepositoryOverview::apply`], and handles the typed [`OverviewAction`]s.
//!
//! This module is not declared by the product `ui::mod` yet; the standalone example and test
//! target include it through `src/bin_support/repository_overview_modules.rs`. The module-level
//! `dead_code` allowance is scoped to that not-yet-wired boundary, as in `ui::reader`.

#![allow(dead_code)]

pub mod fixtures;
mod view;

#[cfg(test)]
mod tests;

pub use view::RepositoryOverview;

use std::rc::Rc;

use crate::domain::account::AccountKey;
use crate::domain::repository::RepositoryIdentity;
use crate::domain::repository_extras::{
    ContributorsOutcome, ExtrasIncompleteReason, LanguagesOutcome, LatestReleaseOutcome,
    ReleasesOutcome,
};
use gpui::{App, Window};

/// One independently refreshing section of the TOP panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverviewSection {
    Releases,
    Contributors,
    Languages,
}

impl OverviewSection {
    pub fn id(self) -> &'static str {
        match self {
            Self::Releases => "releases",
            Self::Contributors => "contributors",
            Self::Languages => "languages",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Releases => "Releases",
            Self::Contributors => "Contributors",
            Self::Languages => "Languages",
        }
    }
}

/// Stable host binding for one repository overview request scope.
///
/// `generation` is the host's own request/page generation. The component rejects an update whose
/// binding does not exactly match the binding it is showing, so a response for another repository,
/// account or request generation cannot overwrite the current panel. The host must still verify
/// the service authorization context (`RepoService::is_current`) before applying a success or
/// error; this binding is not an authorization proof.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverviewBinding {
    pub account: AccountKey,
    pub repository: RepositoryIdentity,
    pub generation: u64,
}

impl OverviewBinding {
    pub fn new(account: AccountKey, repository: RepositoryIdentity, generation: u64) -> Self {
        Self {
            account,
            repository,
            generation,
        }
    }

    pub fn matches(&self, other: &Self) -> bool {
        self == other
    }
}

/// The two Releases observations, kept separate because a 404 latest result is not a list result.
///
/// `latest` is the exact GitHub latest response supplied by the host. The component never infers
/// latest by sorting the list. `latest_error` is set when the latest request failed while the list
/// observation is still usable; it does not erase the list.
#[derive(Clone, Debug, PartialEq)]
pub struct ReleasesData {
    pub list: ReleasesOutcome,
    pub latest: Option<LatestReleaseOutcome>,
    pub latest_error: Option<String>,
}

impl ReleasesData {
    pub fn new(list: ReleasesOutcome, latest: Option<LatestReleaseOutcome>) -> Self {
        Self {
            list,
            latest,
            latest_error: None,
        }
    }

    pub fn with_latest_error(list: ReleasesOutcome, latest_error: impl Into<String>) -> Self {
        Self {
            list,
            latest: None,
            latest_error: Some(latest_error.into()),
        }
    }
}

/// One section's independent state.
///
/// `T` is the typed domain observation (`ReleasesData`, `ContributorsOutcome`,
/// `LanguagesOutcome`). `P` is the typed partial reason carried alongside the still-usable value.
#[derive(Clone, Debug, PartialEq)]
pub enum SectionState<T, P> {
    Idle,
    Loading,
    Ready(T),
    Partial { value: T, reason: P },
    Error { message: String },
}

impl<T, P> SectionState<T, P> {
    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Ready(value) | Self::Partial { value, .. } => Some(value),
            _ => None,
        }
    }

    pub fn error(&self) -> Option<&str> {
        match self {
            Self::Error { message } => Some(message),
            _ => None,
        }
    }

    pub fn is_loading(&self) -> bool {
        matches!(self, Self::Loading)
    }

    pub fn is_partial(&self) -> bool {
        matches!(self, Self::Partial { .. })
    }
}

pub type ReleasesState = SectionState<ReleasesData, Vec<ExtrasIncompleteReason>>;
pub type ContributorsState = SectionState<ContributorsOutcome, Vec<ExtrasIncompleteReason>>;
pub type LanguagesState = SectionState<LanguagesOutcome, String>;

/// A typed section update. The host applies it with the captured request binding.
///
/// The Releases variant is naturally the largest because it carries both the list and the latest
/// observation. Boxing it would make the common host call noisier, so the size difference is
/// allowed here rather than hidden in the call sites.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum OverviewUpdate {
    Releases(ReleasesState),
    Contributors(ContributorsState),
    Languages(LanguagesState),
}

/// A typed user action. The component never opens a URL, downloads an asset or refreshes by itself.
#[derive(Clone, Debug, PartialEq)]
pub enum OverviewAction {
    Refresh {
        binding: OverviewBinding,
        section: OverviewSection,
    },
    OpenRelease {
        binding: OverviewBinding,
        release_id: u64,
        tag_name: String,
        html_url: Option<String>,
    },
    OpenContributor {
        binding: OverviewBinding,
        login: Option<String>,
        html_url: Option<String>,
    },
}

/// The component's action sink. The host decides whether a refresh is allowed and what an
/// external URL means.
pub type OnAction = Rc<dyn Fn(OverviewAction, &mut Window, &mut App) + 'static>;
