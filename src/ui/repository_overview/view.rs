//! Rendering and interaction for the repository TOP panel.
//!
//! The view is presentation-only: it reads the typed section states it was given, emits typed
//! actions, and owns only local popover focus, scroll and dismissal state. No request, credential
//! read, URL open or cache write happens here.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use ely_core::{
    buttons::{Button, ButtonVariant},
    data_display::Avatar,
    primitives::{Icon, IconName},
    theme::{ActiveTheme, AvatarSize, ControlSize, Elevation, IconSize, Radius, TextSize},
    typography::Ellipsis,
};
use gpui::{
    AnyElement, App, AppContext as _, Bounds, Context, FocusHandle, FontWeight, Hsla, IntoElement,
    MouseButton, ParentElement, Pixels, Point, Render, ScrollHandle, StatefulInteractiveElement,
    Styled, Window, div, prelude::*, px, relative,
};

use crate::domain::repository_extras::{
    Contributor, ContributorKind, ContributorsState as DomainContributorsState, ExtrasCompleteness,
    ExtrasIncompleteReason, LanguageStat, Release,
};
use crate::ui::scroll;

use super::{
    ContributorsState, LanguagesState, OnAction, OverviewAction, OverviewBinding, OverviewSection,
    OverviewUpdate, ReleasesData, ReleasesState, SectionState,
};

/// How many rows each section may draw. Narrow windows stack the sections, so they get a smaller
/// budget and the panel keeps room for the reader below.
#[derive(Clone, Copy)]
struct RowLimits {
    releases: usize,
    contributors: usize,
    languages: usize,
}

fn row_limits(viewport: gpui::Size<Pixels>) -> RowLimits {
    if viewport.width < px(720.0) {
        RowLimits {
            releases: 2,
            contributors: 2,
            languages: 2,
        }
    } else {
        RowLimits {
            releases: 3,
            contributors: 3,
            languages: 3,
        }
    }
}

/// The reusable repository TOP panel.
pub struct RepositoryOverview {
    binding: OverviewBinding,
    releases: ReleasesState,
    contributors: ContributorsState,
    languages: LanguagesState,
    on_action: Option<OnAction>,
    last_action: Option<OverviewAction>,
    focus: FocusHandle,
    notes_focus: FocusHandle,
    notes_scroll: ScrollHandle,
    notes_motion: scroll::Motion<ScrollHandle>,
    notes_bars: scroll::Bars,
    open_release: Option<u64>,
    /// Last painted bounds of the component, in window coordinates. The popover anchors to its
    /// bottom-left so it does not cover the section that opened it.
    bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    clock: Instant,
    #[cfg(test)]
    pinned_clock: Option<u64>,
}

impl RepositoryOverview {
    pub fn new(binding: OverviewBinding, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let notes_scroll = ScrollHandle::new();
        Self {
            binding,
            releases: SectionState::Idle,
            contributors: SectionState::Idle,
            languages: SectionState::Idle,
            on_action: None,
            last_action: None,
            focus: cx.focus_handle().tab_stop(true),
            notes_focus: cx.focus_handle().tab_stop(true),
            notes_scroll: notes_scroll.clone(),
            notes_motion: scroll::Motion::new(notes_scroll),
            notes_bars: scroll::Bars::new(),
            open_release: None,
            bounds: Rc::new(RefCell::new(None)),
            clock: Instant::now(),
            #[cfg(test)]
            pinned_clock: None,
        }
    }

    pub fn binding(&self) -> &OverviewBinding {
        &self.binding
    }

    pub fn releases(&self) -> &ReleasesState {
        &self.releases
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

    pub fn open_release(&self) -> Option<u64> {
        self.open_release
    }

    pub fn on_action(&mut self, handler: OnAction) {
        self.on_action = Some(handler);
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
        self.contributors = SectionState::Idle;
        self.languages = SectionState::Idle;
        self.last_action = None;
        self.close_notes(window, cx);
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
        if !self.binding.matches(binding) {
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
        if !self.binding.matches(binding) {
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
        if !self.binding.matches(binding) {
            return false;
        }
        self.languages = state;
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
            OverviewUpdate::Contributors(state) => self.apply_contributors(binding, state, cx),
            OverviewUpdate::Languages(state) => self.apply_languages(binding, state, cx),
        }
    }

    pub fn dismiss_notes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_notes(window, cx);
    }

    pub fn notes_offset(&self) -> Point<Pixels> {
        self.notes_scroll.offset()
    }

    fn emit(&mut self, action: OverviewAction, window: &mut Window, cx: &mut Context<Self>) {
        self.last_action = Some(action.clone());
        if let Some(handler) = self.on_action.clone() {
            handler(action, window, cx);
        }
        cx.notify();
    }

    fn open_notes(&mut self, release_id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.open_release = Some(release_id);
        self.notes_motion.set_offset(Point::default());
        window.focus(&self.notes_focus, cx);
        cx.notify();
    }

    fn close_notes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open_release.take().is_some() {
            // A closed popover must not keep easing in the background or compete with the
            // reader's own scroll target.
            scroll::cancel_region(&mut self.notes_bars, &self.notes_motion);
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    fn now_ms(&self) -> u64 {
        #[cfg(test)]
        if let Some(ms) = self.pinned_clock {
            return ms;
        }
        self.clock.elapsed().as_millis() as u64
    }

    #[cfg(test)]
    pub fn pin_clock(&mut self, ms: u64) {
        self.pinned_clock = Some(ms);
    }

    #[cfg(test)]
    pub fn show_notes_scrollbar(&mut self) {
        self.notes_bars.snap(true);
    }
}

impl Render for RepositoryOverview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let colors = theme.colors.clone();
        let viewport = window.viewport_size();
        let this = cx.entity().downgrade();
        let bounds_cell = self.bounds.clone();
        let now = self.now_ms();
        let reduce = cx.reduce_motion();

        if self.notes_motion.tick(Instant::now(), reduce) {
            window.request_animation_frame();
        }
        if self.notes_bars.is_animating() {
            window.request_animation_frame();
        }
        let notes_opacity = self.notes_bars.opacity(now);

        let refresh = |section: OverviewSection, disabled: bool, this: gpui::WeakEntity<Self>| {
            let label = format!("Refresh {}", section.title().to_lowercase());
            let id = match section {
                OverviewSection::Releases => "overview-refresh-releases",
                OverviewSection::Contributors => "overview-refresh-contributors",
                OverviewSection::Languages => "overview-refresh-languages",
            };
            let debug = format!("overview-refresh-{}", section.id());
            div()
                .debug_selector(move || debug.clone())
                .child(
                    Button::new(id, "")
                        .name(label)
                        .icon(IconName::RefreshCw)
                        .size(ControlSize::Sm)
                        .variant(ButtonVariant::Ghost)
                        .disabled(disabled)
                        .on_click(move |_, window, cx| {
                            let _ = this.update(cx, |view, cx| {
                                let binding = view.binding.clone();
                                view.emit(OverviewAction::Refresh { binding, section }, window, cx);
                            });
                        }),
                )
                .into_any_element()
        };

        let releases_loading = self.releases.is_loading();
        let contributors_loading = self.contributors.is_loading();
        let languages_loading = self.languages.is_loading();

        let limits = row_limits(viewport);
        let releases_body = releases_body(&self.releases, &this, limits.releases, cx);
        let contributors_body =
            contributors_body(&self.contributors, &this, limits.contributors, cx);
        let languages_body = languages_body(&self.languages, &this, limits.languages, cx);

        let releases_card = section_card(
            "overview-releases",
            OverviewSection::Releases.title(),
            IconName::Tag,
            refresh(OverviewSection::Releases, releases_loading, this.clone()),
            releases_body,
            cx,
        );
        let contributors_card = section_card(
            "overview-contributors",
            OverviewSection::Contributors.title(),
            IconName::Users,
            refresh(
                OverviewSection::Contributors,
                contributors_loading,
                this.clone(),
            ),
            contributors_body,
            cx,
        );
        let languages_card = section_card(
            "overview-languages",
            OverviewSection::Languages.title(),
            IconName::Languages,
            refresh(OverviewSection::Languages, languages_loading, this.clone()),
            languages_body,
            cx,
        );

        let notes_release = self.open_release.and_then(|id| {
            self.releases.value().and_then(|data| {
                data.list
                    .releases
                    .iter()
                    .find(|release| release.id == id)
                    .and_then(|release| {
                        release
                            .body
                            .as_ref()
                            .map(|body| (release.id, release.tag_name.clone(), body.clone()))
                    })
            })
        });

        let mut root = div()
            .id("repository-overview")
            .debug_selector(|| "repository-overview".to_string())
            .relative()
            .w_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_3()
            .border_b_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .track_focus(&self.focus)
            .child(
                gpui::canvas(
                    move |bounds, _, _| {
                        *bounds_cell.borrow_mut() = Some(bounds);
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .child(
                div()
                    .debug_selector(|| "overview-sections".to_string())
                    .flex()
                    .flex_wrap()
                    .items_start()
                    .gap_3()
                    .w_full()
                    .child(releases_card)
                    .child(contributors_card)
                    .child(languages_card),
            );

        if let Some((id, tag_name, body)) = notes_release {
            let panel = notes_panel(
                NotesPanel {
                    id,
                    tag_name: &tag_name,
                    body: &body,
                    notes_focus: &self.notes_focus,
                    owner: cx.entity_id(),
                    scroll_handle: &self.notes_scroll,
                    motion: &self.notes_motion,
                    opacity: notes_opacity,
                    this: &this,
                },
                cx,
            );
            let anchor = self
                .bounds
                .borrow()
                .map(|bounds| Point::new(bounds.left(), bounds.bottom()))
                .unwrap_or_default();
            let shield_this = this.clone();
            let shield = div()
                .id("overview-notes-dismiss")
                .debug_selector(|| "overview-notes-dismiss".to_string())
                .absolute()
                .top_0()
                .left_0()
                .w(viewport.width)
                .h(viewport.height)
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, window, _| {
                    window.prevent_default();
                })
                .on_click(move |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    let _ = shield_this.update(cx, |view, cx| view.close_notes(window, cx));
                });
            let overlay = div()
                .relative()
                .w(viewport.width)
                .h(viewport.height)
                .child(shield)
                .child(div().absolute().left(anchor.x).top(anchor.y).child(panel));
            root = root.child(
                gpui::deferred(gpui::anchored().position(Point::default()).child(overlay))
                    .with_priority(1),
            );
        }

        root
    }
}

fn section_card(
    selector: &'static str,
    title: &'static str,
    icon: IconName,
    refresh: AnyElement,
    body: Vec<AnyElement>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let colors = &theme.colors;
    div()
        .id(selector)
        .debug_selector(move || selector.to_string())
        .flex()
        .flex_col()
        .gap_2()
        .flex_1()
        .min_w(px(180.0))
        .p_3()
        .rounded(theme.radius(Radius::Lg))
        .border_1()
        .border_color(colors.border)
        .bg(colors.surface)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .min_w_0()
                        .child(Icon::new(icon).size(IconSize::Sm).color(colors.fg_muted))
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .child(title.to_string()),
                        ),
                )
                .child(refresh),
        )
        .children(body)
        .into_any_element()
}

fn releases_body(
    state: &ReleasesState,
    this: &gpui::WeakEntity<RepositoryOverview>,
    limit: usize,
    cx: &App,
) -> Vec<AnyElement> {
    let theme = cx.theme();
    let colors = &theme.colors;
    let mut body = Vec::new();
    match state {
        SectionState::Idle => body.push(note("Not loaded yet.", colors.fg_subtle, None, cx)),
        SectionState::Loading => body.push(note(
            "Loading releases…",
            colors.fg_subtle,
            Some("overview-releases-loading"),
            cx,
        )),
        SectionState::Error { message } => body.push(note(
            message,
            colors.danger,
            Some("overview-releases-error"),
            cx,
        )),
        SectionState::Ready(data) => {
            releases_data_body(data, None, this, limit, cx, &mut body);
        }
        SectionState::Partial { value, reason } => {
            releases_data_body(value, Some(reason), this, limit, cx, &mut body);
        }
    }
    body
}

fn releases_data_body(
    data: &ReleasesData,
    partial: Option<&Vec<ExtrasIncompleteReason>>,
    this: &gpui::WeakEntity<RepositoryOverview>,
    limit: usize,
    cx: &App,
    body: &mut Vec<AnyElement>,
) {
    let theme = cx.theme();
    let colors = &theme.colors;

    let latest = match (&data.latest, &data.latest_error) {
        (Some(outcome), _) => match &outcome.release {
            Some(release) => {
                let mut summary = div()
                    .debug_selector(|| "overview-releases-latest".to_string())
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(theme.text_size(TextSize::Xs))
                            .text_color(colors.fg_subtle)
                            .child("GitHub latest"),
                    )
                    .child(release_heading(release, "latest", cx));
                if let Some(name) = release.name.as_deref().filter(|name| !name.is_empty()) {
                    summary = summary.child(
                        div()
                            .text_size(theme.text_size(TextSize::Sm))
                            .text_color(colors.fg_muted)
                            .child(Ellipsis::new(name.to_string())),
                    );
                }
                summary.into_any_element()
            }
            None => note(
                latest_none_text(),
                colors.fg_muted,
                Some("overview-releases-latest-none"),
                cx,
            ),
        },
        (None, Some(error)) => note(
            format!(
                "Latest release could not be loaded: {error}. The release list below is still the last successful observation."
            ),
            colors.danger,
            Some("overview-releases-latest-error"),
            cx,
        ),
        (None, None) => note(
            "Latest release was not requested.",
            colors.fg_subtle,
            None,
            cx,
        ),
    };
    body.push(latest);

    if data.list.releases.is_empty() {
        let text = match &data.list.completeness {
            ExtrasCompleteness::Complete => {
                "No release entries were returned in this complete observation."
            }
            ExtrasCompleteness::Partial { .. } => {
                "No release entries were returned before the partial result ended."
            }
        };
        body.push(note(
            text,
            colors.fg_subtle,
            Some("overview-releases-empty"),
            cx,
        ));
    } else {
        body.push(note(
            format!("{} observed release entries", data.list.releases.len()),
            colors.fg_subtle,
            None,
            cx,
        ));
        for (index, release) in data.list.releases.iter().take(limit).enumerate() {
            body.push(release_row(release, index, this, cx));
        }
        if data.list.releases.len() > limit {
            body.push(note(
                format!(
                    "+{} more release entries not shown",
                    data.list.releases.len() - limit
                ),
                colors.fg_subtle,
                None,
                cx,
            ));
        }
    }

    if let Some(reasons) = partial {
        body.push(note(
            partial_reason_text(reasons),
            colors.warning,
            Some("overview-releases-partial"),
            cx,
        ));
    }
}

fn release_row(
    release: &Release,
    index: usize,
    this: &gpui::WeakEntity<RepositoryOverview>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let colors = &theme.colors;
    let id = release.id;
    let tag = release.tag_name.clone();
    let tag_for_action = tag.clone();
    let html_url = release.html_url.clone();
    let body = release.body.clone();
    let has_notes = body.is_some();

    let mut row = div()
        .id(("overview-release", id))
        .debug_selector(move || format!("overview-release-{id}"))
        .flex()
        .flex_col()
        .gap_1()
        .py_1()
        .child(release_heading(release, &format!("row-{index}"), cx));
    if let Some(name) = release.name.as_deref().filter(|name| !name.is_empty()) {
        row = row.child(
            div()
                .text_size(theme.text_size(TextSize::Sm))
                .text_color(colors.fg_muted)
                .min_w_0()
                .child(Ellipsis::new(name.to_string())),
        );
    }

    let mut actions = div().flex().items_center().gap_2();
    if has_notes {
        let notes_this = this.clone();
        actions = actions.child(
            div()
                .debug_selector(move || format!("overview-release-notes-{id}"))
                .child(
                    Button::new(("overview-release-notes", id), "Notes")
                        .size(ControlSize::Sm)
                        .variant(ButtonVariant::Secondary)
                        .on_click(move |_, window, cx| {
                            let _ =
                                notes_this.update(cx, |view, cx| view.open_notes(id, window, cx));
                        }),
                ),
        );
    }
    if html_url.is_some() {
        let open_this = this.clone();
        actions = actions.child(
            div()
                .debug_selector(move || format!("overview-release-open-{id}"))
                .child(
                    Button::new(("overview-release-open", id), "Open")
                        .size(ControlSize::Sm)
                        .variant(ButtonVariant::Link)
                        .on_click(move |_, window, cx| {
                            let _ = open_this.update(cx, |view, cx| {
                                let binding = view.binding.clone();
                                view.emit(
                                    OverviewAction::OpenRelease {
                                        binding,
                                        release_id: id,
                                        tag_name: tag_for_action.clone(),
                                        html_url: html_url.clone(),
                                    },
                                    window,
                                    cx,
                                );
                            });
                        }),
                ),
        );
    }
    row = row.child(actions);
    row.into_any_element()
}

fn release_heading(release: &Release, key: &str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let colors = &theme.colors;
    let id = release.id;
    let draft_selector = format!("overview-release-draft-{key}-{id}");
    let prerelease_selector = format!("overview-release-prerelease-{key}-{id}");
    let mut heading = div().flex().items_center().gap_2().min_w_0().child(
        div()
            .min_w_0()
            .font_weight(FontWeight::MEDIUM)
            .child(Ellipsis::new(release.tag_name.clone())),
    );
    if release.draft {
        heading = heading.child(badge(
            "Draft",
            colors.warning,
            colors.warning_subtle,
            draft_selector,
            cx,
        ));
    }
    if release.prerelease {
        heading = heading.child(badge(
            "Pre-release",
            colors.info,
            colors.info_subtle,
            prerelease_selector,
            cx,
        ));
    }
    heading.into_any_element()
}

fn contributors_body(
    state: &ContributorsState,
    this: &gpui::WeakEntity<RepositoryOverview>,
    limit: usize,
    cx: &App,
) -> Vec<AnyElement> {
    let theme = cx.theme();
    let colors = &theme.colors;
    let mut body = Vec::new();
    match state {
        SectionState::Idle => body.push(note("Not loaded yet.", colors.fg_subtle, None, cx)),
        SectionState::Loading => body.push(note(
            "Loading contributors…",
            colors.fg_subtle,
            Some("overview-contributors-loading"),
            cx,
        )),
        SectionState::Error { message } => body.push(note(
            message,
            colors.danger,
            Some("overview-contributors-error"),
            cx,
        )),
        SectionState::Ready(outcome) => {
            contributors_outcome_body(outcome, None, this, limit, cx, &mut body);
        }
        SectionState::Partial { value, reason } => {
            contributors_outcome_body(value, Some(reason), this, limit, cx, &mut body);
        }
    }
    body
}

fn contributors_outcome_body(
    outcome: &crate::domain::repository_extras::ContributorsOutcome,
    partial: Option<&Vec<ExtrasIncompleteReason>>,
    this: &gpui::WeakEntity<RepositoryOverview>,
    limit: usize,
    cx: &App,
    body: &mut Vec<AnyElement>,
) {
    let theme = cx.theme();
    let colors = &theme.colors;
    match outcome.state {
        DomainContributorsState::EmptyRepository => body.push(note(
            "GitHub reports this repository is empty (HTTP 204).",
            colors.fg_muted,
            Some("overview-contributors-empty-repository"),
            cx,
        )),
        DomainContributorsState::Content if outcome.contributors.is_empty() => body.push(note(
            "No contributor entries were returned in this successful observation.",
            colors.fg_subtle,
            Some("overview-contributors-empty-content"),
            cx,
        )),
        DomainContributorsState::Content => {
            body.push(note(
                format!(
                    "{} observed contributor entries (not a guaranteed total; GitHub may associate only the first 500 author emails)",
                    outcome.observed_entries()
                ),
                colors.fg_subtle,
                None,
                cx,
            ));
            for (index, contributor) in outcome.contributors.iter().take(limit).enumerate() {
                body.push(contributor_row(contributor, index, this, cx));
            }
            if outcome.contributors.len() > limit {
                body.push(note(
                    format!(
                        "+{} more contributor entries not shown",
                        outcome.contributors.len() - limit
                    ),
                    colors.fg_subtle,
                    None,
                    cx,
                ));
            }
        }
    }
    if let Some(reasons) = partial {
        body.push(note(
            partial_reason_text(reasons),
            colors.warning,
            Some("overview-contributors-partial"),
            cx,
        ));
    }
}

fn contributor_row(
    contributor: &Contributor,
    index: usize,
    this: &gpui::WeakEntity<RepositoryOverview>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let colors = &theme.colors;
    let anonymous = contributor.kind() == ContributorKind::Anonymous;
    let display = contributor_display(contributor);
    let secondary = contributor_secondary(contributor);
    let selector = if anonymous {
        format!("overview-contributor-anonymous-{index}")
    } else {
        format!("overview-contributor-{index}")
    };
    let mut row = div()
        .id(("overview-contributor", index as u64))
        .debug_selector(move || selector)
        .flex()
        .items_center()
        .gap_2()
        .py_1()
        .min_w_0()
        .child(
            Avatar::new(
                ("overview-contributor-avatar", index as u64),
                display.clone(),
            )
            .size(AvatarSize::Sm),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .min_w_0()
                        .font_weight(FontWeight::MEDIUM)
                        .child(Ellipsis::new(display)),
                )
                .child(
                    div()
                        .text_size(theme.text_size(TextSize::Xs))
                        .text_color(colors.fg_subtle)
                        .min_w_0()
                        .child(Ellipsis::new(secondary)),
                ),
        );
    if let Some(url) = contributor.html_url.clone() {
        let login = contributor.login.clone();
        let open_this = this.clone();
        row = row.child(
            div()
                .debug_selector(move || format!("overview-contributor-open-{index}"))
                .child(
                    Button::new(("overview-contributor-open", index as u64), "Open")
                        .size(ControlSize::Sm)
                        .variant(ButtonVariant::Link)
                        .on_click(move |_, window, cx| {
                            let _ = open_this.update(cx, |view, cx| {
                                let binding = view.binding.clone();
                                view.emit(
                                    OverviewAction::OpenContributor {
                                        binding,
                                        login: login.clone(),
                                        html_url: Some(url.clone()),
                                    },
                                    window,
                                    cx,
                                );
                            });
                        }),
                ),
        );
    }
    row.into_any_element()
}

fn languages_body(
    state: &LanguagesState,
    _this: &gpui::WeakEntity<RepositoryOverview>,
    limit: usize,
    cx: &App,
) -> Vec<AnyElement> {
    let theme = cx.theme();
    let colors = &theme.colors;
    let mut body = Vec::new();
    match state {
        SectionState::Idle => body.push(note("Not loaded yet.", colors.fg_subtle, None, cx)),
        SectionState::Loading => body.push(note(
            "Loading languages…",
            colors.fg_subtle,
            Some("overview-languages-loading"),
            cx,
        )),
        SectionState::Error { message } => body.push(note(
            message,
            colors.danger,
            Some("overview-languages-error"),
            cx,
        )),
        SectionState::Ready(outcome) => {
            languages_outcome_body(outcome, None, limit, cx, &mut body);
        }
        SectionState::Partial { value, reason } => {
            languages_outcome_body(value, Some(reason), limit, cx, &mut body);
        }
    }
    body
}

fn languages_outcome_body(
    outcome: &crate::domain::repository_extras::LanguagesOutcome,
    partial: Option<&String>,
    limit: usize,
    cx: &App,
    body: &mut Vec<AnyElement>,
) {
    let theme = cx.theme();
    let colors = &theme.colors;
    if outcome.total_bytes == 0 {
        body.push(note(
            "No language bytes were observed. No percentage distribution is inferred.",
            colors.fg_muted,
            Some("overview-languages-zero"),
            cx,
        ));
    } else {
        body.push(note(
            format!(
                "{} total bytes across {} languages",
                human_bytes(outcome.total_bytes),
                outcome.languages.len()
            ),
            colors.fg_subtle,
            None,
            cx,
        ));
        for (index, language) in outcome.languages.iter().take(limit).enumerate() {
            body.push(language_row(language, index, cx));
        }
        if outcome.languages.len() > limit {
            body.push(note(
                format!(
                    "+{} more languages not shown",
                    outcome.languages.len() - limit
                ),
                colors.fg_subtle,
                None,
                cx,
            ));
        }
        body.push(note(
            "Shares are derived from byte counts and may not sum to exactly 100% after rounding.",
            colors.fg_subtle,
            None,
            cx,
        ));
    }
    if let Some(reason) = partial {
        body.push(note(
            reason.clone(),
            colors.warning,
            Some("overview-languages-partial"),
            cx,
        ));
    }
}

fn language_row(language: &LanguageStat, index: usize, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let colors = &theme.colors;
    let share = language.share.unwrap_or(0.0).clamp(0.0, 1.0);
    let fill = colors.chart[index % colors.chart.len()];
    div()
        .id(("overview-language", index as u64))
        .debug_selector(move || format!("overview-language-{index}"))
        .flex()
        .flex_col()
        .gap_1()
        .py_1()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .min_w_0()
                .child(
                    div()
                        .min_w_0()
                        .font_weight(FontWeight::MEDIUM)
                        .child(Ellipsis::new(language.name.clone())),
                )
                .child(
                    div()
                        .debug_selector(move || format!("overview-language-share-{index}"))
                        .flex_none()
                        .text_size(theme.text_size(TextSize::Sm))
                        .text_color(colors.fg_muted)
                        .child(format!(
                            "{} · {} bytes",
                            format_share(language.share),
                            language.bytes
                        )),
                ),
        )
        .child(
            div()
                .h(px(6.0))
                .w_full()
                .rounded_full()
                .bg(colors.sunken)
                .child(
                    div()
                        .h_full()
                        .w(relative(share as f32))
                        .rounded_full()
                        .bg(fill),
                ),
        )
        .into_any_element()
}

struct NotesPanel<'a> {
    id: u64,
    tag_name: &'a str,
    body: &'a str,
    notes_focus: &'a FocusHandle,
    owner: gpui::EntityId,
    scroll_handle: &'a ScrollHandle,
    motion: &'a scroll::Motion<ScrollHandle>,
    opacity: f32,
    this: &'a gpui::WeakEntity<RepositoryOverview>,
}

fn notes_panel(panel: NotesPanel<'_>, cx: &mut Context<RepositoryOverview>) -> AnyElement {
    let NotesPanel {
        id,
        tag_name,
        body,
        notes_focus,
        owner,
        scroll_handle,
        motion,
        opacity,
        this,
    } = panel;
    let theme = cx.theme();
    let colors = &theme.colors;

    let on_drag: scroll::OnDragging = {
        let this = this.clone();
        Rc::new(move |axis, dragging, _window, cx| {
            let _ = this.update(cx, |view, cx| {
                let motion = view.notes_motion.clone();
                if view
                    .notes_bars
                    .drag(&motion, axis, dragging, view.now_ms(), cx.reduce_motion())
                {
                    cx.notify();
                }
            });
        })
    };
    let bar = scroll::Bar {
        target: motion,
        id: scroll::BarId {
            owner,
            name: "overview-notes-scrollbar",
            axis: gpui::Axis::Vertical,
        },
        placement: scroll::Placement::overlay(4.0, 8.0),
        style: scroll::Style::DEFAULT,
        opacity,
        on_drag,
    }
    .render(cx);

    let header = div()
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .child(
            div()
                .min_w_0()
                .font_weight(FontWeight::MEDIUM)
                .child(Ellipsis::new(tag_name.to_string())),
        )
        .child({
            let this = this.clone();
            div()
                .debug_selector(move || format!("overview-notes-close-{id}"))
                .child(
                    Button::new(("overview-notes-close", id), "Close")
                        .size(ControlSize::Sm)
                        .variant(ButtonVariant::Secondary)
                        .on_click(move |_, window, cx| {
                            let _ = this.update(cx, |view, cx| view.close_notes(window, cx));
                        }),
                )
        });

    let body = div()
        .id("overview-notes-body")
        .debug_selector(|| "overview-notes-body".to_string())
        .relative()
        .h(px(160.0))
        .w_full()
        .overflow_y_scroll()
        .track_scroll(scroll_handle)
        .child(
            div()
                .text_size(theme.text_size(TextSize::Base))
                .text_color(colors.fg_muted)
                .child(body.to_string()),
        )
        .child(scroll::wheel_overlay(
            "overview-notes",
            gpui::Axis::Vertical,
            motion.clone(),
        ))
        .child(bar)
        .on_hover(cx.listener(move |view, hovered, _, cx| {
            if view
                .notes_bars
                .hover(*hovered, view.now_ms(), cx.reduce_motion())
            {
                cx.notify();
            }
        }));

    div()
        .id("overview-notes-panel")
        .debug_selector(|| "overview-notes-panel".to_string())
        .track_focus(notes_focus)
        .relative()
        .w(px(380.0))
        .max_h(px(260.0))
        .p_3()
        .flex()
        .flex_col()
        .gap_2()
        .rounded(theme.radius(Radius::Lg))
        .bg(colors.overlay)
        .border_1()
        .border_color(colors.border)
        .shadow(theme.elevation(Elevation::Floating))
        .text_size(theme.text_size(TextSize::Base))
        .text_color(colors.fg)
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
        .on_key_down({
            let this = this.clone();
            move |event, window, cx| {
                if event.keystroke.key == "escape" {
                    let _ = this.update(cx, |view, cx| view.close_notes(window, cx));
                    cx.stop_propagation();
                }
            }
        })
        .child(header)
        .child(body)
        .into_any_element()
}

fn badge(label: &str, fg: Hsla, bg: Hsla, selector: String, cx: &App) -> AnyElement {
    let theme = cx.theme();
    div()
        .debug_selector(move || selector)
        .flex_none()
        .px_1()
        .py(px(1.0))
        .rounded(theme.radius(Radius::Sm))
        .bg(bg)
        .text_color(fg)
        .text_size(theme.text_size(TextSize::Xs))
        .child(label.to_string())
        .into_any_element()
}

fn note(
    text: impl Into<String>,
    color: Hsla,
    selector: Option<&'static str>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    div()
        .when_some(selector, |el, selector| {
            el.debug_selector(move || selector.to_string())
        })
        .text_size(theme.text_size(TextSize::Sm))
        .text_color(color)
        .child(text.into())
        .into_any_element()
}

/// The ambiguous latest-404 wording, kept in one place so the UI and its tests agree.
pub(crate) fn latest_none_text() -> &'static str {
    "No qualifying latest release is visible. A 404 can also mean the repository or release is not visible to this authorization; it does not prove that the repository has no releases."
}

/// The label used for a contributor row. Email and raw JSON are never shown.
pub(crate) fn contributor_display(contributor: &Contributor) -> String {
    if contributor.kind() == ContributorKind::Anonymous {
        return "Anonymous contributor".to_string();
    }
    contributor
        .name
        .as_deref()
        .filter(|name| !name.is_empty())
        .or(contributor.login.as_deref())
        .unwrap_or("GitHub user")
        .to_string()
}

/// The secondary line for a contributor row: identity and observed contribution count.
pub(crate) fn contributor_secondary(contributor: &Contributor) -> String {
    match contributor.kind() {
        ContributorKind::Anonymous => {
            format!("{} contributions · anonymous", contributor.contributions)
        }
        ContributorKind::GitHubUser => {
            let login = contributor.login.as_deref().unwrap_or("unknown login");
            format!("@{login} · {} contributions", contributor.contributions)
        }
    }
}

/// A byte-derived share as a textual percentage. `None` stays an em dash, never `0%`.
pub(crate) fn format_share(share: Option<f64>) -> String {
    match share {
        Some(share) if share.is_finite() => {
            format!("{:.1}%", share.clamp(0.0, 1.0) * 100.0)
        }
        _ => "—".to_string(),
    }
}

pub(crate) fn human_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let value = bytes as f64;
    if value >= GIB {
        format!("{:.1} GiB", value / GIB)
    } else if value >= MIB {
        format!("{:.1} MiB", value / MIB)
    } else if value >= KIB {
        format!("{:.1} KiB", value / KIB)
    } else {
        format!("{bytes} bytes")
    }
}

pub(crate) fn partial_reason_text(reasons: &[ExtrasIncompleteReason]) -> String {
    let parts = reasons
        .iter()
        .map(|reason| match reason {
            ExtrasIncompleteReason::Failed(_) => "a later page failed".to_string(),
            ExtrasIncompleteReason::PageLimit { limit } => {
                format!("page limit {limit} reached")
            }
            ExtrasIncompleteReason::EntryLimit { limit } => {
                format!("entry limit {limit} reached")
            }
            ExtrasIncompleteReason::ResponseLimit { limit } => {
                format!("response limit {limit} reached")
            }
            ExtrasIncompleteReason::UntrustedPagination => {
                "an untrusted pagination link was not followed".to_string()
            }
            ExtrasIncompleteReason::InvalidPagination => {
                "an invalid pagination link was not followed".to_string()
            }
            ExtrasIncompleteReason::PaginationCycle => {
                "a pagination cycle was detected".to_string()
            }
            ExtrasIncompleteReason::SsoPartial { organizations } => format!(
                "SSO authorization is required for {}",
                organizations.join(", ")
            ),
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        "Partial result.".to_string()
    } else {
        format!("Partial result: {}.", parts.join("; "))
    }
}
