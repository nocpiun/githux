//! Reusable repository TOP information component for Releases, Contributors and Languages.
//!
//! The component renders only supplied observations. It never reads credentials, calls HTTP,
//! downloads assets, opens URLs or persists a cache. The host binds account, repository and
//! request generation in [`OverviewBinding`], applies each section's typed outcome through
//! [`RepositoryOverview::apply`], and handles the typed [`OverviewAction`]s.
//!
//! The state and the NSH presentation are framework state only: no I/O and no services are
//! reached from this module. The repository page owns every read and authorizes it with the
//! captured `PageContext`.
pub mod glyphs;
mod summary;
mod view;
pub mod widgets;

#[cfg(test)]
pub mod fixtures;

#[cfg(test)]
mod tests;

pub use view::{language_bar, overview};

use std::collections::HashMap;

use crate::domain::account::AccountKey;
use crate::domain::repository::RepositoryIdentity;
use crate::domain::repository_extras::{
    ContributorsOutcome, ExtrasIncompleteReason, LanguagesOutcome, LatestReleaseOutcome,
    ReleasesOutcome,
};

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
    pub historical: bool,
    pub latest_historical: bool,
    pub sync_label: Option<String>,
}

impl ReleasesData {
    pub fn new(list: ReleasesOutcome, latest: Option<LatestReleaseOutcome>) -> Self {
        Self {
            list,
            latest,
            latest_error: None,
            historical: false,
            latest_historical: false,
            sync_label: None,
        }
    }

    pub fn with_latest_error(list: ReleasesOutcome, latest_error: impl Into<String>) -> Self {
        Self {
            list,
            latest: None,
            latest_error: Some(latest_error.into()),
            historical: false,
            latest_historical: false,
            sync_label: None,
        }
    }

    pub fn with_provenance(
        mut self,
        historical: bool,
        latest_historical: bool,
        sync_label: Option<String>,
    ) -> Self {
        self.historical = historical;
        self.latest_historical = latest_historical;
        self.sync_label = sync_label;
        self
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
    Historical {
        value: T,
        sync_label: Option<String>,
        reason: Option<P>,
    },
    Partial {
        value: T,
        reason: P,
    },
    Error {
        message: String,
    },
}

impl<T, P> SectionState<T, P> {
    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Ready(value) | Self::Historical { value, .. } | Self::Partial { value, .. } => {
                Some(value)
            }
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
        matches!(
            self,
            Self::Partial { .. }
                | Self::Historical {
                    reason: Some(_),
                    ..
                }
        )
    }

    pub fn is_historical(&self) -> bool {
        matches!(self, Self::Historical { .. })
    }

    pub fn sync_label(&self) -> Option<&str> {
        match self {
            Self::Historical { sync_label, .. } => sync_label.as_deref(),
            _ => None,
        }
    }
}

pub type ReleasesState = SectionState<ReleasesData, Vec<ExtrasIncompleteReason>>;
pub type LatestReleaseState = SectionState<LatestReleaseOutcome, String>;
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
    LatestRelease(LatestReleaseState),
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

/// One decoded contributor picture supplied by the host's single avatar loader.
#[derive(Clone, Debug, PartialEq)]
pub struct AvatarImage {
    /// Internally generated, provider-validated bytes or pixels. Never a remote URL.
    pub source: String,
    pub width: u32,
    pub height: u32,
}

/// The component's own state. It only stores what the host supplied.
#[derive(Clone, Debug, PartialEq)]
pub struct RepositoryOverview {
    binding: OverviewBinding,
    releases: ReleasesState,
    latest: Option<LatestReleaseState>,
    contributors: ContributorsState,
    languages: LanguagesState,
    last_action: Option<OverviewAction>,
    avatar_images: HashMap<u64, AvatarImage>,
    date_label: Option<(u64, String)>,
}

impl RepositoryOverview {
    pub fn new(binding: OverviewBinding) -> Self {
        Self {
            binding,
            releases: SectionState::Idle,
            latest: None,
            contributors: SectionState::Idle,
            languages: SectionState::Idle,
            last_action: None,
            avatar_images: HashMap::new(),
            date_label: None,
        }
    }

    pub fn binding(&self) -> &OverviewBinding {
        &self.binding
    }

    pub fn releases(&self) -> &ReleasesState {
        &self.releases
    }

    pub fn latest_release(&self) -> Option<&LatestReleaseState> {
        self.latest.as_ref()
    }

    pub fn contributors(&self) -> &ContributorsState {
        &self.contributors
    }

    pub fn languages(&self) -> &LanguagesState {
        &self.languages
    }

    pub fn last_action(&self) -> Option<&OverviewAction> {
        self.last_action.as_ref()
    }

    /// Whether a decoded picture is already attached for this contributor, so a host can skip
    /// re-decoding it. `binding` is the host's request scope, not an authorization proof; the host
    /// still verifies the service context before publishing anything.
    pub fn has_avatar_image(&self, binding: &OverviewBinding, user_id: u64) -> bool {
        binding == &self.binding && self.avatar_images.contains_key(&user_id)
    }

    /// Host-supplied decoded image. Never pass a remote URL to the renderer.
    pub fn set_avatar_image(
        &mut self,
        binding: &OverviewBinding,
        user_id: u64,
        image: AvatarImage,
    ) -> bool {
        if binding != &self.binding {
            return false;
        }
        if self.avatar_images.contains_key(&user_id) {
            return true;
        }
        self.avatar_images.insert(user_id, image);
        true
    }

    /// Optional host-formatted relative date, bound to the exact release and current request.
    pub fn set_release_date_label(
        &mut self,
        binding: &OverviewBinding,
        release_id: u64,
        label: String,
    ) -> bool {
        if binding != &self.binding {
            return false;
        }
        self.date_label = Some((release_id, label));
        true
    }

    /// Replaces the request scope. Existing section states are cleared rather than relabelled as
    /// belonging to the new account/repository/generation.
    pub fn set_binding(&mut self, binding: OverviewBinding) {
        if self.binding == binding {
            return;
        }
        self.binding = binding;
        self.releases = SectionState::Idle;
        self.latest = None;
        self.contributors = SectionState::Idle;
        self.languages = SectionState::Idle;
        self.last_action = None;
        self.avatar_images.clear();
        self.date_label = None;
    }

    /// Applies a Releases outcome captured for `binding`. Returns false, without changing the
    /// panel, when the binding no longer matches the current one.
    pub fn apply_releases(&mut self, binding: &OverviewBinding, state: ReleasesState) -> bool {
        if !self.binding.matches(binding)
            || state.value().is_some_and(|data| {
                data.list.repository != binding.repository
                    || data
                        .latest
                        .as_ref()
                        .is_some_and(|latest| latest.repository != binding.repository)
            })
        {
            return false;
        }
        self.releases = state;
        true
    }

    /// Applies a Contributors outcome captured for `binding`.
    pub fn apply_contributors(
        &mut self,
        binding: &OverviewBinding,
        state: ContributorsState,
    ) -> bool {
        if !self.binding.matches(binding)
            || state
                .value()
                .is_some_and(|value| value.repository != binding.repository)
        {
            return false;
        }
        self.contributors = state;
        true
    }

    /// Applies a Languages outcome captured for `binding`.
    pub fn apply_languages(&mut self, binding: &OverviewBinding, state: LanguagesState) -> bool {
        if !self.binding.matches(binding)
            || state
                .value()
                .is_some_and(|value| value.repository != binding.repository)
        {
            return false;
        }
        self.languages = state;
        true
    }

    /// Applies the latest endpoint independently of the release list request.
    pub fn apply_latest_release(
        &mut self,
        binding: &OverviewBinding,
        state: LatestReleaseState,
    ) -> bool {
        if !self.binding.matches(binding)
            || state
                .value()
                .is_some_and(|value| value.repository != binding.repository)
        {
            return false;
        }
        self.latest = Some(state);
        true
    }

    /// Applies one typed update, rejecting stale bindings as the individual methods do.
    pub fn apply(&mut self, binding: &OverviewBinding, update: OverviewUpdate) -> bool {
        match update {
            OverviewUpdate::Releases(state) => self.apply_releases(binding, state),
            OverviewUpdate::LatestRelease(state) => self.apply_latest_release(binding, state),
            OverviewUpdate::Contributors(state) => self.apply_contributors(binding, state),
            OverviewUpdate::Languages(state) => self.apply_languages(binding, state),
        }
    }

    /// Records a typed action for the host. The view passes its own sink; nothing is opened here.
    pub fn emit(&mut self, action: OverviewAction) {
        self.last_action = Some(action);
    }

    pub fn avatar_image(&self, user_id: u64) -> Option<&AvatarImage> {
        self.avatar_images.get(&user_id)
    }

    pub fn release_date_label(&self, release_id: u64) -> Option<&str> {
        self.date_label
            .as_ref()
            .filter(|(id, _)| *id == release_id)
            .map(|(_, label)| label.as_str())
    }
}
