//! Compact, natural-height repository summary strip. No I/O, scrolling or popovers.
use gpui::{Context, RenderImage, Window};
use std::collections::HashMap;

use super::{
    ContributorsState, LanguagesState, LatestReleaseState, OnAction, OverviewAction,
    OverviewBinding, OverviewSection, OverviewUpdate, ReleasesState, SectionState,
};

pub struct RepositoryOverview {
    binding: OverviewBinding,
    releases: ReleasesState,
    latest: Option<LatestReleaseState>,
    contributors: ContributorsState,
    languages: LanguagesState,
    on_action: Option<OnAction>,
    last_action: Option<OverviewAction>,
    avatar_images: HashMap<u64, std::sync::Arc<RenderImage>>,
    date_label: Option<(u64, String)>,
}
impl RepositoryOverview {
    pub fn new(binding: OverviewBinding, _: &mut Window, _: &mut Context<Self>) -> Self {
        Self {
            binding,
            releases: SectionState::Idle,
            latest: None,
            contributors: SectionState::Idle,
            languages: SectionState::Idle,
            on_action: None,
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
    pub fn on_action(&mut self, handler: OnAction) {
        self.on_action = Some(handler);
    }
    /// Host-supplied decoded image. Never pass a remote URL to the renderer.
    pub fn set_avatar_image(
        &mut self,
        binding: &OverviewBinding,
        user_id: u64,
        image: std::sync::Arc<RenderImage>,
        cx: &mut Context<Self>,
    ) -> bool {
        if binding != &self.binding {
            return false;
        }
        self.avatar_images.insert(user_id, image);
        cx.notify();
        true
    }
    /// Optional host-formatted relative date, bound to the exact release and current request.
    pub fn set_release_date_label(
        &mut self,
        binding: &OverviewBinding,
        release_id: u64,
        label: String,
        cx: &mut Context<Self>,
    ) -> bool {
        if binding != &self.binding {
            return false;
        }
        self.date_label = Some((release_id, label));
        cx.notify();
        true
    }
    /// Replaces the request scope. Existing section states are cleared rather than relabelled as
    /// belonging to the new account/repository/generation.
    pub fn set_binding(
        &mut self,
        binding: OverviewBinding,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        let _ = window;
        cx.notify();
    }

    /// Applies a Releases outcome captured for `binding`. Returns false, without changing the
    /// panel, when the binding no longer matches the current one.
    pub fn apply_releases(
        &mut self,
        binding: &OverviewBinding,
        state: ReleasesState,
        cx: &mut Context<Self>,
    ) -> bool {
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
        cx.notify();
        true
    }

    /// Applies a Contributors outcome captured for `binding`.
    pub fn apply_contributors(
        &mut self,
        binding: &OverviewBinding,
        state: ContributorsState,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.binding.matches(binding)
            || state
                .value()
                .is_some_and(|value| value.repository != binding.repository)
        {
            return false;
        }
        self.contributors = state;
        cx.notify();
        true
    }

    /// Applies a Languages outcome captured for `binding`.
    pub fn apply_languages(
        &mut self,
        binding: &OverviewBinding,
        state: LanguagesState,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.binding.matches(binding)
            || state
                .value()
                .is_some_and(|value| value.repository != binding.repository)
        {
            return false;
        }
        self.languages = state;
        cx.notify();
        true
    }

    /// Applies the latest endpoint independently of the release list request.
    pub fn apply_latest_release(
        &mut self,
        binding: &OverviewBinding,
        state: LatestReleaseState,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.binding.matches(binding)
            || state
                .value()
                .is_some_and(|value| value.repository != binding.repository)
        {
            return false;
        }
        self.latest = Some(state);
        cx.notify();
        true
    }

    /// Applies one typed update, rejecting stale bindings as the individual methods do.
    pub fn apply(
        &mut self,
        binding: &OverviewBinding,
        update: OverviewUpdate,
        cx: &mut Context<Self>,
    ) -> bool {
        match update {
            OverviewUpdate::Releases(state) => self.apply_releases(binding, state, cx),
            OverviewUpdate::LatestRelease(state) => self.apply_latest_release(binding, state, cx),
            OverviewUpdate::Contributors(state) => self.apply_contributors(binding, state, cx),
            OverviewUpdate::Languages(state) => self.apply_languages(binding, state, cx),
        }
    }

    fn emit(&mut self, action: OverviewAction, window: &mut Window, cx: &mut Context<Self>) {
        self.last_action = Some(action.clone());
        if let Some(handler) = self.on_action.clone() {
            handler(action, window, cx);
        }
        cx.notify();
    }
}

#[path = "summary.rs"]
pub(super) mod summary;
