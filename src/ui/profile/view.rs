use std::{rc::Rc, time::Instant};

use gpui::{
    App, AppContext, Context, Entity, FocusHandle, ScrollHandle, Subscription, Window, point, px,
};

use crate::{
    content::{PreparedBody, PreparedContent},
    domain::{ContributionCalendar, ContributionRange, ProfileBasic, ProfilePinned},
    ui::{
        reader::{DocumentKind, MarkdownLayout, ReaderAction, ReaderView, ResourceContext},
        scroll::{self, Bars, Motion},
    },
};

pub type OnProfileAction = Rc<dyn Fn(super::ProfileAction, &mut Window, &mut App)>;

use super::{
    ProfileAction, ProfileActionKind, ProfileBinding, ProfileRequest, ProfileTarget, SectionState,
    heatmap::HeatmapPresentation,
};

pub struct ProfileView {
    pub(super) target: ProfileTarget,
    pub(super) binding: ProfileBinding,
    pub(super) generation: u64,
    pub(super) range: ContributionRange,
    pub(super) basic: SectionState<ProfileBasic>,
    pub(super) repositories: SectionState<ProfilePinned>,
    pub(super) contributions: SectionState<ContributionCalendar>,
    pub(super) readme: SectionState<()>,
    pub(super) avatar: Option<gpui::ImageSource>,
    pub(super) reader: Option<Entity<ReaderView>>,
    reader_generation: u64,
    pub(super) repository_focus: FocusHandle,
    pub(super) heatmap_focus: FocusHandle,
    pub(super) selected_repository: usize,
    pub(super) content_width: f32,
    pub(super) repository_keyboard_focus: bool,
    pub(super) heatmap_keyboard_focus: bool,
    pub(super) selected_day: Option<usize>,
    pub(super) page_scroll: ScrollHandle,
    pub(super) heatmap_scroll: ScrollHandle,
    pub(super) page_motion: Motion<ScrollHandle>,
    pub(super) heatmap_motion: Motion<ScrollHandle>,
    pub(super) page_bars: Bars,
    pub(super) heatmap_bars: Bars,
    pub(super) clock: Instant,
    pub(super) on_action: Option<OnProfileAction>,
    pub(super) last_action: Option<ProfileAction>,
    _window_activation: Subscription,
}

impl ProfileView {
    pub fn new(
        target: ProfileTarget,
        binding: ProfileBinding,
        range: ContributionRange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let page_scroll = ScrollHandle::new();
        let heatmap_scroll = ScrollHandle::new();
        let page_motion = Motion::new(page_scroll.clone());
        let heatmap_motion = Motion::new(heatmap_scroll.clone());
        let activation_page = page_motion.clone();
        let activation_heatmap = heatmap_motion.clone();
        let activation = cx.observe_window_activation(window, move |view, window, cx| {
            if !window.is_window_active() {
                scroll::cancel_region(&mut view.page_bars, &activation_page);
                scroll::cancel_region(&mut view.heatmap_bars, &activation_heatmap);
                cx.notify();
            }
        });
        Self {
            target,
            binding,
            generation: 0,
            range,
            basic: SectionState::Loading,
            repositories: SectionState::Loading,
            contributions: SectionState::Loading,
            readme: SectionState::Loading,
            avatar: None,
            reader: None,
            reader_generation: 0,
            repository_focus: cx.focus_handle().tab_stop(true),
            heatmap_focus: cx.focus_handle().tab_stop(true),
            selected_repository: 0,
            content_width: 880.0,
            repository_keyboard_focus: false,
            heatmap_keyboard_focus: false,
            selected_day: None,
            page_scroll,
            heatmap_scroll,
            page_motion,
            heatmap_motion,
            page_bars: Bars::new(),
            heatmap_bars: Bars::new(),
            clock: Instant::now(),
            on_action: None,
            last_action: None,
            _window_activation: activation,
        }
    }

    /// Begin a new host binding. Every visible section, focused action and
    /// scroll position is invalidated; results carrying the previous request
    /// key are rejected even if the new profile has the same login.
    pub fn bind(
        &mut self,
        target: ProfileTarget,
        binding: ProfileBinding,
        range: ContributionRange,
        cx: &mut Context<Self>,
    ) -> ProfileRequest {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("profile request generation exhausted");
        self.target = target;
        self.binding = binding;
        self.range = range;
        self.basic = SectionState::Loading;
        self.repositories = SectionState::Loading;
        self.contributions = SectionState::Loading;
        self.readme = SectionState::Loading;
        self.avatar = None;
        self.cancel_reader(cx);
        self.reader = None;
        self.selected_repository = 0;
        self.selected_day = None;
        self.last_action = None;
        self.repository_focus = cx.focus_handle().tab_stop(true);
        self.heatmap_focus = cx.focus_handle().tab_stop(true);
        self.page_motion.set_offset(point(px(0.0), px(0.0)));
        self.heatmap_motion.set_offset(point(px(0.0), px(0.0)));
        scroll::cancel_region(&mut self.page_bars, &self.page_motion);
        scroll::cancel_region(&mut self.heatmap_bars, &self.heatmap_motion);
        cx.notify();
        self.request()
    }

    pub fn request(&self) -> ProfileRequest {
        ProfileRequest {
            target: self.target.clone(),
            binding: self.binding.clone(),
            generation: self.generation,
        }
    }

    pub fn target(&self) -> &ProfileTarget {
        &self.target
    }

    pub fn binding(&self) -> &ProfileBinding {
        &self.binding
    }

    pub fn range(&self) -> &ContributionRange {
        &self.range
    }

    pub fn basic(&self) -> &SectionState<ProfileBasic> {
        &self.basic
    }

    pub fn repositories(&self) -> &SectionState<ProfilePinned> {
        &self.repositories
    }

    pub fn contributions(&self) -> &SectionState<ContributionCalendar> {
        &self.contributions
    }

    pub fn readme_state(&self) -> &SectionState<()> {
        &self.readme
    }

    /// Publish a confirmed non-error absence (no eligible public root README).
    pub fn set_readme_unavailable(
        &mut self,
        request: &ProfileRequest,
        message: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.accepts(request) {
            return false;
        }
        self.cancel_reader(cx);
        self.reader = None;
        self.readme = SectionState::Unavailable(message.into());
        cx.notify();
        true
    }

    /// Replace only the decoded header image; no loader or URL enters this component.
    pub fn set_avatar(
        &mut self,
        request: &ProfileRequest,
        image: Option<gpui::ImageSource>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.accepts(request) {
            return false;
        }
        // The host may sample the shared avatar cache on each repaint. A Ready image is terminal
        // for this binding, so re-applying it must not schedule another frame forever.
        if self.avatar.is_some() && image.is_some() {
            return true;
        }
        self.avatar = image;
        cx.notify();
        true
    }

    pub fn reader(&self) -> Option<&Entity<ReaderView>> {
        self.reader.as_ref()
    }

    pub fn accepts(&self, request: &ProfileRequest) -> bool {
        *request == self.request()
    }

    pub fn update_basic(
        &mut self,
        request: &ProfileRequest,
        state: SectionState<ProfileBasic>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.accepts(request) {
            return false;
        }
        self.basic = state;
        cx.notify();
        true
    }

    pub fn update_repositories(
        &mut self,
        request: &ProfileRequest,
        state: SectionState<ProfilePinned>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.accepts(request) {
            return false;
        }
        self.selected_repository = 0;
        self.repositories = state;
        cx.notify();
        true
    }

    pub fn update_contributions(
        &mut self,
        request: &ProfileRequest,
        range: &ContributionRange,
        state: SectionState<ContributionCalendar>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.accepts(request) || range != &self.range {
            return false;
        }
        self.selected_day = match &state {
            SectionState::Ready(calendar) if !calendar.weeks.is_empty() => Some(0),
            _ => None,
        };
        self.contributions = state;
        self.heatmap_motion.set_offset(point(px(0.0), px(0.0)));
        cx.notify();
        true
    }

    pub fn update_readme(
        &mut self,
        request: &ProfileRequest,
        state: SectionState<PreparedContent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.accepts(request) {
            return false;
        }
        match state {
            SectionState::Ready(content) => {
                let context = ResourceContext::from_prepared(&content);
                self.update_readme_with_context(
                    request,
                    SectionState::Ready(content.body),
                    context,
                    window,
                    cx,
                )
            }
            SectionState::Loading => self.update_readme_with_context(
                request,
                SectionState::Loading,
                ResourceContext::new("", "", "", "README.md"),
                window,
                cx,
            ),
            SectionState::Error(error) => self.update_readme_with_context(
                request,
                SectionState::Error(error),
                ResourceContext::new("", "", "", "README.md"),
                window,
                cx,
            ),
            SectionState::Unavailable(message) => self.update_readme_with_context(
                request,
                SectionState::Unavailable(message),
                ResourceContext::new("", "", "", "README.md"),
                window,
                cx,
            ),
        }
    }

    /// Alternate raw-body entry point using ReaderView's existing stable API.
    /// Fixed-content hosts should prefer [`Self::update_readme`]; this is useful
    /// to standalone previews that do not have a service-produced blob identity.
    pub fn update_readme_with_context(
        &mut self,
        request: &ProfileRequest,
        state: SectionState<PreparedBody>,
        context: ResourceContext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.accepts(request) {
            return false;
        }
        self.reader_generation = self
            .reader_generation
            .checked_add(1)
            .expect("profile README generation exhausted");
        let reader_generation = self.reader_generation;
        self.cancel_reader(cx);
        self.reader = None;
        self.readme = match state {
            SectionState::Loading => SectionState::Loading,
            SectionState::Error(error) => SectionState::Error(error),
            SectionState::Unavailable(message) => SectionState::Unavailable(message),
            SectionState::Ready(body) => {
                let reader_request = self.request();
                let this = cx.entity().downgrade();
                let reader = cx.new(|cx| {
                    let mut reader =
                        ReaderView::new(body, DocumentKind::Markdown, context, window, cx);
                    // The README is one section of the Profile page's own scrolling flow: the
                    // reader must report its natural height instead of owning a bounded viewport.
                    reader.set_markdown_layout(MarkdownLayout::Flow, cx);
                    let layout_view = cx.entity().downgrade();
                    reader.on_layout_changed(Rc::new(move |_, cx| {
                        if let Some(view) = layout_view.upgrade() {
                            view.update(cx, |_, cx| cx.notify());
                        }
                    }));
                    reader.on_action(Rc::new(move |action, window, cx| {
                        if let Some(this) = this.upgrade() {
                            this.update(cx, |view, cx| {
                                if view.accepts(&reader_request)
                                    && view.reader_generation == reader_generation
                                    && view.reader.is_some()
                                {
                                    view.emit(ProfileActionKind::Readme(action), window, cx);
                                }
                            });
                        }
                    }));
                    reader
                });
                self.reader = Some(reader);
                SectionState::Ready(())
            }
        };
        cx.notify();
        true
    }

    fn cancel_reader(&self, cx: &mut Context<Self>) {
        if let Some(reader) = &self.reader {
            reader.update(cx, |reader, cx| reader.cancel_interactions(cx));
        }
    }

    /// Stop background UI work while this page remains mounted but inactive. Valid content and
    /// navigation stay intact; the host separately clears them when authorization changes.
    pub fn suspend(&mut self, cx: &mut Context<Self>) {
        scroll::cancel_region(&mut self.page_bars, &self.page_motion);
        scroll::cancel_region(&mut self.heatmap_bars, &self.heatmap_motion);
        self.cancel_reader(cx);
        cx.notify();
    }

    pub fn on_action(&mut self, handler: OnProfileAction) {
        self.on_action = Some(handler);
    }

    pub fn last_action(&self) -> Option<&ProfileAction> {
        self.last_action.as_ref()
    }

    pub(super) fn emit(
        &mut self,
        kind: ProfileActionKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let action = ProfileAction {
            request: self.request(),
            kind,
        };
        self.last_action = Some(action.clone());
        if let Some(handler) = self.on_action.clone() {
            handler(action, window, cx);
        }
        cx.notify();
    }

    pub(super) fn heatmap_model(&self) -> Option<HeatmapPresentation> {
        match &self.contributions {
            SectionState::Ready(calendar) => {
                Some(super::heatmap::heatmap_presentation(&self.range, calendar))
            }
            _ => None,
        }
    }

    pub fn selected_day_detail(&self) -> Option<String> {
        let selected = self.selected_day?;
        self.heatmap_model().and_then(|model| {
            model
                .weeks
                .iter()
                .flat_map(|week| week.days.iter())
                .nth(selected)
                .map(|day| day.detail())
        })
    }

    pub(super) fn repository_columns(&self) -> usize {
        if self.content_width < 700.0 { 1 } else { 2 }
    }

    pub(super) fn now_ms(&self) -> u64 {
        self.clock.elapsed().as_millis() as u64
    }

    pub(super) fn tick_scroll(&self, window: &mut Window, reduce: bool) {
        if self.advance_scroll(Instant::now(), reduce) {
            window.request_animation_frame();
        }
    }

    pub(super) fn advance_scroll(&self, now: Instant, reduce: bool) -> bool {
        let page_moving = self.page_motion.tick(now, reduce);
        let heatmap_moving = self.heatmap_motion.tick(now, reduce);
        page_moving || heatmap_moving
    }

    #[cfg(test)]
    pub fn repository_focus_for_test(&self) -> FocusHandle {
        self.repository_focus.clone()
    }

    #[cfg(test)]
    pub fn heatmap_focus_for_test(&self) -> FocusHandle {
        self.heatmap_focus.clone()
    }

    #[cfg(test)]
    pub fn show_page_scrollbar_for_test(&mut self) {
        self.page_bars.snap(true);
    }

    #[cfg(test)]
    pub fn page_dragging_for_test(&self) -> Option<gpui::Axis> {
        self.page_bars.dragging()
    }
}

impl Drop for ProfileView {
    fn drop(&mut self) {
        scroll::cancel_region(&mut self.page_bars, &self.page_motion);
        scroll::cancel_region(&mut self.heatmap_bars, &self.heatmap_motion);
    }
}

/// Keep the public action types beside the component API.
impl From<ReaderAction> for ProfileActionKind {
    fn from(action: ReaderAction) -> Self {
        Self::Readme(action)
    }
}
