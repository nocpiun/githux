//! Reusable Home / Feed presentation over the existing domain read models.
//!
//! The host owns account-scoped reads and must verify authorization before it
//! supplies an outcome. `RequestBinding` and generation protect this view from
//! stale UI responses; they are not an authorization check. No network,
//! credentials, cache, routing or URL opening lives in this module.

// The independent preview and integration test include this module directly.
// Production Shell mounting intentionally remains a separate integration task.
#![allow(dead_code)]

mod presentation;
#[cfg(test)]
mod tests;

use std::rc::Rc;
use std::time::Instant;

use ely_core::{
    buttons::Button,
    theme::{ActiveTheme, ControlSize, Radius, TextSize},
};
use gpui::{
    AnyElement, App, Context, FocusHandle, Focusable, KeyDownEvent, Pixels, Point, Render,
    ScrollStrategy, UniformListScrollHandle, Window, div, prelude::*, px,
};

use crate::domain::home::{FeedOutcome, HomeError, HomeItem, HomeSearchOutcome};
use crate::ui::scroll::{self, Bars, Motion};

use presentation::{feed_row, home_row, home_summary};

/// An opaque, host-selected request scope. Include the stable account/page
/// identity; do not put a PAT or other credential in this value.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RequestBinding(String);

impl RequestBinding {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The two independently refreshed panels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Column {
    Home,
    Feed,
}

/// A manual refresh request. It carries the binding/generation observed by the
/// view so the host can reject an action belonging to an obsolete page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefreshRequest {
    pub column: Column,
    pub binding: RequestBinding,
    pub generation: u64,
}

/// Stable repository identity used for navigation callbacks.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RepositoryIdentity {
    pub id: Option<u64>,
    pub owner: String,
    pub name: String,
}

impl RepositoryIdentity {
    pub fn full_name(&self) -> String {
        if self.owner.is_empty() {
            self.name.clone()
        } else {
            format!("{}/{}", self.owner, self.name)
        }
    }
}

/// Stable Issue/PR identity; `id` is GitHub's global issue/PR id, not the
/// repository-local `number`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkItemIdentity {
    pub id: u64,
    pub kind: crate::domain::home::HomeItemKind,
    pub number: u64,
    pub repository: RepositoryIdentity,
}

/// Stable profile identity. The id is included when the typed model has one.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProfileIdentity {
    pub id: u64,
    pub login: String,
}

/// Stable Feed identity. No raw event payload or URL is exposed as an action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedIdentity {
    pub id: String,
    pub event_type: crate::domain::home::FeedEventType,
    pub repository: RepositoryIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HomeAction {
    Refresh(RefreshRequest),
    Repository(RepositoryIdentity),
    Profile(ProfileIdentity),
    ActivateWorkItem(WorkItemIdentity),
    ActivateFeed(FeedIdentity),
}

pub type OnAction = Rc<dyn Fn(HomeAction, &mut Window, &mut App)>;

/// Successful data, a hard failure, or a host-declared no-data state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelContent<T> {
    Ready(T),
    Error(HomeError),
    /// No current or previously supplied snapshot. This is deliberately not
    /// called "empty": offline/no-data must not become "no activity".
    Unavailable {
        message: String,
    },
}

/// State of one column. Refreshing retains previously supplied data while the
/// host works, but does not claim that data was freshly verified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelState<T> {
    Loading,
    Ready {
        content: PanelContent<T>,
        refreshing: bool,
    },
}

fn begin_refresh<T>(state: &mut PanelState<T>) {
    match state {
        PanelState::Loading => {}
        PanelState::Ready { refreshing, .. } => *refreshing = true,
    }
}

impl<T> PanelState<T> {
    fn loading(&mut self) {
        *self = Self::Loading;
    }

    fn refreshing(&self) -> bool {
        matches!(
            self,
            Self::Ready {
                refreshing: true,
                ..
            }
        )
    }

    /// A first load or a manual refresh already owns this column's request
    /// slot; another refresh must not be emitted until a result or error ends
    /// the wait.
    fn is_busy(&self) -> bool {
        matches!(self, Self::Loading) || self.refreshing()
    }

    fn data(&self) -> Option<&T> {
        match self {
            Self::Ready {
                content: PanelContent::Ready(data),
                ..
            } => Some(data),
            _ => None,
        }
    }
}

/// Host-computed refresh availability. The view does not count down cooldowns
/// or schedule polling; the host supplies changes when its state changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefreshAvailability {
    pub available: bool,
    pub reason: Option<String>,
}

impl RefreshAvailability {
    pub fn available() -> Self {
        Self {
            available: true,
            reason: None,
        }
    }

    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            available: false,
            reason: Some(reason.into()),
        }
    }
}

impl Default for RefreshAvailability {
    fn default() -> Self {
        Self::available()
    }
}

struct PanelModel<T> {
    state: PanelState<T>,
    refresh: RefreshAvailability,
}

impl<T> PanelModel<T> {
    fn new() -> Self {
        Self {
            state: PanelState::Ready {
                content: PanelContent::Unavailable {
                    message: "The host has not supplied a result for this binding.".to_string(),
                },
                refreshing: false,
            },
            refresh: RefreshAvailability::default(),
        }
    }

    fn reset(&mut self) {
        self.state = PanelState::Ready {
            content: PanelContent::Unavailable {
                message: "The host has not supplied a result for this binding.".to_string(),
            },
            refreshing: false,
        };
        self.refresh = RefreshAvailability::default();
    }
}

/// One panel's virtualized rows, selection and isolated scroll/focus state.
struct PanelView {
    scroll: UniformListScrollHandle,
    motion: Motion<gpui::ScrollHandle>,
    bars: Bars,
    focus: FocusHandle,
    selected: Option<usize>,
    clock: Instant,
    #[cfg(test)]
    pinned_clock: Option<u64>,
}

impl PanelView {
    fn new(cx: &mut Context<HomeView>) -> Self {
        let scroll = UniformListScrollHandle::new();
        let motion = Motion::new(scroll.0.borrow().base_handle.clone());
        Self {
            scroll,
            motion,
            bars: Bars::new(),
            focus: cx.focus_handle().tab_stop(true),
            selected: None,
            clock: Instant::now(),
            #[cfg(test)]
            pinned_clock: None,
        }
    }

    fn reset(&mut self, window: &mut Window, cx: &mut App) {
        scroll::cancel_region(&mut self.bars, &self.motion);
        if self.focus.is_focused(window) {
            window.blur(cx);
        }
        self.selected = None;
        // Replacing the handle also drops native deferred reveal/drag state.
        // The new Motion wraps exactly the new list's real base handle.
        self.scroll = UniformListScrollHandle::new();
        self.motion = Motion::new(self.scroll.0.borrow().base_handle.clone());
    }

    fn now_ms(&self) -> u64 {
        #[cfg(test)]
        if let Some(ms) = self.pinned_clock {
            return ms;
        }
        self.clock.elapsed().as_millis() as u64
    }

    fn offset(&self) -> Point<Pixels> {
        self.scroll.0.borrow().base_handle.offset()
    }

    fn handles(&self) -> PanelHandles {
        PanelHandles {
            scroll: self.scroll.clone(),
            motion: self.motion.clone(),
            focus: self.focus.clone(),
        }
    }

    fn reveal(&self, index: usize) {
        self.motion.cancel();
        self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
    }
}

/// Cloneable real handles for one panel, sampled once per frame.
struct PanelHandles {
    scroll: UniformListScrollHandle,
    motion: Motion<gpui::ScrollHandle>,
    focus: FocusHandle,
}

/// A reusable two-column Home/Feed view. Construct one per page session and
/// call [`HomeView::bind`] whenever the account or page binding changes.
pub struct HomeView {
    binding: RequestBinding,
    generation: u64,
    home: PanelModel<HomeSearchOutcome>,
    feed: PanelModel<FeedOutcome>,
    home_view: PanelView,
    feed_view: PanelView,
    on_action: Option<OnAction>,
    last_action: Option<HomeAction>,
}

impl HomeView {
    /// Starts an unrequested binding (no current data). The host must call
    /// [`HomeView::set_loading`] and then supply each result with the same
    /// binding and generation.
    pub fn new(
        binding: RequestBinding,
        generation: u64,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            binding,
            generation,
            home: PanelModel::new(),
            feed: PanelModel::new(),
            home_view: PanelView::new(cx),
            feed_view: PanelView::new(cx),
            on_action: None,
            last_action: None,
        }
    }

    /// Clears both columns and all selection/focus/scroll state before a new
    /// binding can receive input. Even A→B→A never revives an old response.
    pub fn bind(
        &mut self,
        binding: RequestBinding,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.binding = binding;
        self.generation = generation;
        self.home.reset();
        self.feed.reset();
        self.home_view.reset(window, cx);
        self.feed_view.reset(window, cx);
        self.last_action = None;
        cx.notify();
    }

    pub fn binding(&self) -> &RequestBinding {
        &self.binding
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn home_state(&self) -> &PanelState<HomeSearchOutcome> {
        &self.home.state
    }

    pub fn feed_state(&self) -> &PanelState<FeedOutcome> {
        &self.feed.state
    }

    pub fn selected(&self, column: Column) -> Option<usize> {
        match column {
            Column::Home => self.home_view.selected,
            Column::Feed => self.feed_view.selected,
        }
    }

    pub fn offset(&self, column: Column) -> Point<Pixels> {
        match column {
            Column::Home => self.home_view.offset(),
            Column::Feed => self.feed_view.offset(),
        }
    }

    pub fn on_action(&mut self, handler: OnAction) {
        self.on_action = Some(handler);
    }

    /// The last emitted action, useful for a host status line and preview.
    pub fn last_action(&self) -> Option<&HomeAction> {
        self.last_action.as_ref()
    }

    fn accepts(&self, binding: &RequestBinding, generation: u64) -> bool {
        binding == &self.binding && generation == self.generation
    }

    pub fn work_item_action(&self, index: usize) -> Option<HomeAction> {
        let item: &HomeItem = self.home.state.data()?.items.get(index)?;
        Some(HomeAction::ActivateWorkItem(WorkItemIdentity {
            id: item.id,
            kind: item.kind,
            number: item.number,
            repository: RepositoryIdentity {
                id: None,
                owner: item.repository.owner.clone(),
                name: item.repository.name.clone(),
            },
        }))
    }

    fn begin_refresh(&mut self, column: Column) {
        match column {
            Column::Home => begin_refresh(&mut self.home.state),
            Column::Feed => begin_refresh(&mut self.feed.state),
        }
    }

    /// Starts loading one column without disturbing the other. A host normally
    /// calls this before its first read; refreshing an existing snapshot can be
    /// left to the refresh action itself.
    pub fn set_loading(
        &mut self,
        column: Column,
        binding: &RequestBinding,
        generation: u64,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.accepts(binding, generation) {
            return false;
        }
        match column {
            Column::Home => {
                self.home.state.loading();
                self.home_view.selected = None;
            }
            Column::Feed => {
                self.feed.state.loading();
                self.feed_view.selected = None;
            }
        }
        cx.notify();
        true
    }

    /// Applies a typed result only if it belongs to the current binding and
    /// generation. The keyboard selection is cleared so it can never keep
    /// pointing at an entry from an older snapshot.
    pub fn apply_home(
        &mut self,
        binding: &RequestBinding,
        generation: u64,
        outcome: HomeSearchOutcome,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.accepts(binding, generation) {
            return false;
        }
        self.home.state = PanelState::Ready {
            content: PanelContent::Ready(outcome),
            refreshing: false,
        };
        self.home_view.selected = None;
        cx.notify();
        true
    }

    pub fn apply_feed(
        &mut self,
        binding: &RequestBinding,
        generation: u64,
        outcome: FeedOutcome,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.accepts(binding, generation) {
            return false;
        }
        self.feed.state = PanelState::Ready {
            content: PanelContent::Ready(outcome),
            refreshing: false,
        };
        self.feed_view.selected = None;
        cx.notify();
        true
    }

    pub fn apply_error(
        &mut self,
        column: Column,
        binding: &RequestBinding,
        generation: u64,
        error: HomeError,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.accepts(binding, generation) {
            return false;
        }
        match column {
            Column::Home => {
                self.home.state = PanelState::Ready {
                    content: PanelContent::Error(error),
                    refreshing: false,
                }
            }
            Column::Feed => {
                self.feed.state = PanelState::Ready {
                    content: PanelContent::Error(error),
                    refreshing: false,
                }
            }
        }
        match column {
            Column::Home => self.home_view.selected = None,
            Column::Feed => self.feed_view.selected = None,
        }
        cx.notify();
        true
    }

    pub fn set_unavailable(
        &mut self,
        column: Column,
        binding: &RequestBinding,
        generation: u64,
        message: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.accepts(binding, generation) {
            return false;
        }
        let message = message.into();
        match column {
            Column::Home => {
                self.home.state = PanelState::Ready {
                    content: PanelContent::Unavailable { message },
                    refreshing: false,
                }
            }
            Column::Feed => {
                self.feed.state = PanelState::Ready {
                    content: PanelContent::Unavailable { message },
                    refreshing: false,
                }
            }
        }
        match column {
            Column::Home => self.home_view.selected = None,
            Column::Feed => self.feed_view.selected = None,
        }
        cx.notify();
        true
    }

    /// Host-provided cooldown, in-flight, rate-limit or polling availability.
    pub fn set_refresh_availability(
        &mut self,
        column: Column,
        binding: &RequestBinding,
        generation: u64,
        availability: RefreshAvailability,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.accepts(binding, generation) {
            return false;
        }
        match column {
            Column::Home => self.home.refresh = availability,
            Column::Feed => self.feed.refresh = availability,
        }
        cx.notify();
        true
    }

    fn panel_mut(&mut self, column: Column) -> &mut PanelView {
        match column {
            Column::Home => &mut self.home_view,
            Column::Feed => &mut self.feed_view,
        }
    }

    fn item_count(&self, column: Column) -> usize {
        match column {
            Column::Home => self
                .home
                .state
                .data()
                .map_or(0, |outcome| outcome.items.len()),
            Column::Feed => self
                .feed
                .state
                .data()
                .map_or(0, |outcome| outcome.events.len()),
        }
    }

    fn can_refresh(&self, column: Column) -> bool {
        match column {
            Column::Home => self.home.refresh.available && !self.home.state.is_busy(),
            Column::Feed => self.feed.refresh.available && !self.feed.state.is_busy(),
        }
    }

    fn emit(&mut self, action: HomeAction, window: &mut Window, cx: &mut Context<Self>) {
        self.last_action = Some(action.clone());
        if let Some(handler) = self.on_action.clone() {
            handler(action, window, cx);
        }
        cx.notify();
    }

    /// Public for deterministic host/preview tests; pointer and keyboard paths
    /// call this same method exactly once per activation.
    pub fn refresh(&mut self, column: Column, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_refresh(column) {
            return;
        }
        self.begin_refresh(column);
        self.emit(
            HomeAction::Refresh(RefreshRequest {
                column,
                binding: self.binding.clone(),
                generation: self.generation,
            }),
            window,
            cx,
        );
    }

    pub fn select(
        &mut self,
        column: Column,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.item_count(column) {
            return;
        }
        let panel = self.panel_mut(column);
        panel.selected = Some(index);
        panel.focus.focus(window, cx);
        panel.reveal(index);
        cx.notify();
    }

    fn move_selection(
        &mut self,
        column: Column,
        delta: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let count = self.item_count(column);
        if count == 0 {
            return;
        }
        let current = self.selected(column);
        let next = match (current, delta) {
            (None, d) if d >= 0 => 0,
            (None, _) => count - 1,
            (Some(index), d) => (index as isize)
                .saturating_add(d)
                .clamp(0, count as isize - 1) as usize,
        };
        self.select(column, next, window, cx);
    }

    fn feed_action(&self, index: usize) -> Option<HomeAction> {
        let event = self.feed.state.data()?.events.get(index)?;
        let (owner, name) = event
            .repository
            .name
            .split_once('/')
            .map_or(("", event.repository.name.as_str()), |(owner, name)| {
                (owner, name)
            });
        Some(HomeAction::ActivateFeed(FeedIdentity {
            id: event.id.clone(),
            event_type: event.event_type.clone(),
            repository: RepositoryIdentity {
                id: Some(event.repository.id),
                owner: owner.to_string(),
                name: name.to_string(),
            },
        }))
    }

    fn activate(
        &mut self,
        column: Column,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let action = match column {
            Column::Home => self.work_item_action(index),
            Column::Feed => self.feed_action(index),
        };
        if let Some(action) = action {
            self.emit(action, window, cx);
        }
    }

    pub fn open_repository(
        &mut self,
        repository: RepositoryIdentity,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.emit(HomeAction::Repository(repository), window, cx);
    }

    pub fn open_profile(
        &mut self,
        profile: ProfileIdentity,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.emit(HomeAction::Profile(profile), window, cx);
    }

    fn key_down(
        &mut self,
        column: Column,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.modifiers.secondary(cx)
            || event.keystroke.modifiers.alt
            || event.keystroke.modifiers.control
        {
            return;
        }
        match event.keystroke.key.as_str() {
            "down" => self.move_selection(column, 1, window, cx),
            "up" => self.move_selection(column, -1, window, cx),
            "home" => {
                if self.item_count(column) > 0 {
                    self.select(column, 0, window, cx);
                }
            }
            "end" => {
                let count = self.item_count(column);
                if count > 0 {
                    self.select(column, count - 1, window, cx);
                }
            }
            "enter" | "space" => {
                if let Some(index) = self.selected(column) {
                    self.activate(column, index, window, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    #[cfg(test)]
    fn show_scrollbar(&mut self, column: Column) {
        self.panel_mut(column).bars.snap(true);
    }
}

impl Focusable for HomeView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.home_view.focus.clone()
    }
}

impl Render for HomeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (colors, body_size) = {
            let theme = cx.theme();
            (theme.colors.clone(), theme.text_size(TextSize::Sm))
        };
        for panel in [&self.home_view, &self.feed_view] {
            let animating = panel.motion.tick(Instant::now(), cx.reduce_motion());
            if animating {
                window.request_animation_frame();
            }
        }
        for (column, panel) in [
            (Column::Home, &mut self.home_view),
            (Column::Feed, &mut self.feed_view),
        ] {
            if panel.bars.dragging().is_some() && !cx.has_active_drag() {
                panel
                    .bars
                    .settle(&panel.motion, panel.now_ms(), cx.reduce_motion());
            }
            if panel.bars.is_animating() {
                window.request_animation_frame();
            }
            let _ = column;
        }

        let narrow = window.viewport_size().width < px(760.0);
        let home_opacity = self.home_view.bars.opacity(self.home_view.now_ms());
        let feed_opacity = self.feed_view.bars.opacity(self.feed_view.now_ms());
        let home_handles = self.home_view.handles();
        let feed_handles = self.feed_view.handles();
        let home = render_panel(Column::Home, &self.home, home_opacity, &home_handles, cx);
        let feed = render_panel(Column::Feed, &self.feed, feed_opacity, &feed_handles, cx);
        let columns = div()
            .debug_selector(|| "home-columns".to_string())
            .flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .min_w_0()
            .gap_4()
            .map(|columns| {
                if narrow {
                    columns.flex_col().children([home, feed])
                } else {
                    columns.flex_row().children([home, feed])
                }
            });

        div()
            .debug_selector(|| "home-view".to_string())
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .min_h_0()
            .p_4()
            .gap_4()
            .bg(colors.surface)
            .text_size(body_size)
            .text_color(colors.fg)
            .child(columns)
    }
}

fn render_panel<T: PanelRows + 'static>(
    column: Column,
    model: &PanelModel<T>,
    opacity: f32,
    handles: &PanelHandles,
    cx: &mut Context<HomeView>,
) -> AnyElement {
    let (colors, radius, heading_size, tiny, refreshing_size) = {
        let theme = cx.theme();
        (
            theme.colors.clone(),
            theme.radius(Radius::Md),
            theme.text_size(TextSize::Lg),
            theme.text_size(TextSize::Xs),
            theme.text_size(TextSize::Xs),
        )
    };
    let title = match column {
        Column::Home => "Home",
        Column::Feed => "Feed",
    };
    let root = match column {
        Column::Home => "home-panel",
        Column::Feed => "feed-panel",
    };
    let body = panel_body(column, model, opacity, handles, cx);
    let refreshing = model.state.refreshing();
    let disabled = !model.refresh.available || model.state.is_busy();
    let unavailable_reason = (!model.refresh.available)
        .then(|| model.refresh.reason.clone())
        .flatten();
    let panel = cx.entity().downgrade();
    div()
        .debug_selector(move || root.to_string())
        .flex()
        .flex_col()
        .flex_1()
        .flex_basis(px(0.0))
        .min_w_0()
        .min_h_0()
        .overflow_hidden()
        .rounded(radius)
        .border_1()
        .border_color(colors.border)
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap_2()
                .h(px(48.0))
                .px_3()
                .border_b_1()
                .border_color(colors.border)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(heading_size)
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(title),
                )
                .when(refreshing, |header| {
                    header.child(
                        div()
                            .text_size(refreshing_size)
                            .text_color(colors.fg_subtle)
                            .child("Refreshing…"),
                    )
                })
                .children(unavailable_reason.map(|reason| {
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(tiny)
                        .text_color(colors.fg_subtle)
                        .child(reason)
                }))
                .child(
                    Button::new(
                        match column {
                            Column::Home => "home-refresh",
                            Column::Feed => "feed-refresh",
                        },
                        "Refresh",
                    )
                    .size(ControlSize::Sm)
                    .disabled(disabled)
                    .on_click(move |_, window, cx| {
                        let _ = panel.update(cx, |view, cx| view.refresh(column, window, cx));
                    }),
                ),
        )
        .child(body)
        .into_any_element()
}

trait PanelRows {
    fn len(&self) -> usize;
    fn summary(&self) -> presentation::Summary;
    fn row(
        view: &HomeView,
        index: usize,
        window: &mut Window,
        cx: &mut Context<HomeView>,
    ) -> AnyElement;
}

impl PanelRows for HomeSearchOutcome {
    fn len(&self) -> usize {
        self.items.len()
    }

    fn summary(&self) -> presentation::Summary {
        home_summary(self)
    }

    fn row(
        view: &HomeView,
        index: usize,
        _window: &mut Window,
        cx: &mut Context<HomeView>,
    ) -> AnyElement {
        let item = view.home.state.data().unwrap().items[index].clone();
        home_row(&item, index, view.home_view.selected == Some(index), cx)
    }
}

impl PanelRows for FeedOutcome {
    fn len(&self) -> usize {
        self.events.len()
    }

    fn summary(&self) -> presentation::Summary {
        presentation::feed_summary(self)
    }

    fn row(
        view: &HomeView,
        index: usize,
        _window: &mut Window,
        cx: &mut Context<HomeView>,
    ) -> AnyElement {
        let event = view.feed.state.data().unwrap().events[index].clone();
        feed_row(&event, index, view.feed_view.selected == Some(index), cx)
    }
}

fn panel_body<T: PanelRows + 'static>(
    column: Column,
    model: &PanelModel<T>,
    opacity: f32,
    handles: &PanelHandles,
    cx: &mut Context<HomeView>,
) -> AnyElement {
    let (colors, tiny, radius) = {
        let theme = cx.theme();
        (
            theme.colors.clone(),
            theme.text_size(TextSize::Xs),
            theme.radius(Radius::Md),
        )
    };
    let status = match &model.state {
        PanelState::Loading => Some(("Loading…", colors.fg_subtle, None)),
        PanelState::Ready {
            content: PanelContent::Error(error),
            ..
        } => Some((
            "This column could not be loaded.",
            colors.danger,
            Some(presentation::error_message(error)),
        )),
        PanelState::Ready {
            content: PanelContent::Unavailable { message },
            ..
        } => Some(("No current data.", colors.fg_subtle, Some(message.clone()))),
        PanelState::Ready {
            content: PanelContent::Ready(_),
            ..
        } => None,
    };
    if let Some((title, color, detail)) = status {
        return div()
            .debug_selector(move || format!("{}-status", column_name(column)))
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .p_5()
            .text_center()
            .child(div().text_color(color).child(title))
            .children(detail.map(|detail| {
                div()
                    .text_size(tiny)
                    .text_color(colors.fg_subtle)
                    .child(detail)
            }))
            .into_any_element();
    }

    let Some(data) = model.state.data() else {
        unreachable!("all non-ready panel states returned above")
    };
    let empty = data.len() == 0;
    let summary = T::summary(data);
    let partial = Some(summary.notice);
    let header = Some(summary.count);
    let list = uniform_rows::<T>(column, data.len(), opacity, handles, cx);
    let empty_panel = div()
        .debug_selector(move || format!("{}-empty", column_name(column)))
        .flex_1()
        .min_h_0()
        .flex()
        .items_center()
        .justify_center()
        .p_5()
        .text_center()
        .text_color(colors.fg_subtle)
        .child(match column {
            Column::Home => "No Issues or pull requests matched this search.",
            Column::Feed => "No events were returned in this Feed snapshot.",
        })
        .into_any_element();
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .children(header.map(|text| {
            div()
                .debug_selector(move || format!("{}-summary", column_name(column)))
                .flex_none()
                .px_3()
                .py_2()
                .text_size(tiny)
                .text_color(colors.fg_subtle)
                .child(text)
        }))
        .children(partial.map(|text| {
            div()
                .debug_selector(move || format!("{}-partial", column_name(column)))
                .flex_none()
                .mx_3()
                .mb_2()
                .p_2()
                .rounded(radius)
                .bg(colors.warning_subtle)
                .text_size(tiny)
                .text_color(colors.warning)
                .child(text)
        }))
        .child(if empty { empty_panel } else { list })
        .into_any_element()
}

fn column_name(column: Column) -> &'static str {
    match column {
        Column::Home => "home",
        Column::Feed => "feed",
    }
}

fn uniform_rows<T: PanelRows + 'static>(
    column: Column,
    count: usize,
    opacity: f32,
    handles: &PanelHandles,
    cx: &mut Context<HomeView>,
) -> AnyElement {
    let scroll = handles.scroll.clone();
    let motion = handles.motion.clone();
    let focus = handles.focus.clone();
    let list = gpui::uniform_list(
        match column {
            Column::Home => "home-list",
            Column::Feed => "feed-list",
        },
        count,
        cx.processor(
            move |view: &mut HomeView, range: std::ops::Range<usize>, window, cx| {
                range
                    .map(|index| T::row(view, index, window, cx))
                    .collect::<Vec<_>>()
            },
        ),
    )
    .track_scroll(&scroll)
    .size_full();
    let this = cx.entity().downgrade();
    let on_drag: scroll::OnDragging = Rc::new(move |axis, dragging, _window, cx| {
        let _ = this.update(cx, |view, cx| {
            let panel = view.panel_mut(column);
            panel.bars.drag(
                &panel.motion,
                axis,
                dragging,
                panel.now_ms(),
                cx.reduce_motion(),
            );
            cx.notify();
        });
    });
    let bar = scroll::Bar {
        target: &motion,
        id: scroll::BarId {
            owner: cx.entity_id(),
            name: match column {
                Column::Home => "home-scrollbar-y",
                Column::Feed => "feed-scrollbar-y",
            },
            axis: gpui::Axis::Vertical,
        },
        placement: scroll::Placement::overlay(4.0, 8.0),
        style: scroll::Style::DEFAULT,
        opacity,
        on_drag,
    }
    .render(cx);
    div()
        .id(match column {
            Column::Home => "home-rows",
            Column::Feed => "feed-rows",
        })
        .debug_selector(move || format!("{}-rows", column_name(column)))
        .track_focus(&focus)
        .relative()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .overflow_hidden()
        .child(list)
        .child(scroll::wheel_overlay(
            match column {
                Column::Home => "home-rows",
                Column::Feed => "feed-rows",
            },
            gpui::Axis::Vertical,
            motion,
        ))
        .child(bar)
        .on_hover(cx.listener(move |view, hovered, _, cx| {
            let panel = view.panel_mut(column);
            if panel
                .bars
                .hover(*hovered, panel.now_ms(), cx.reduce_motion())
            {
                cx.notify();
            }
        }))
        .on_key_down(cx.listener(move |view, event: &KeyDownEvent, window, cx| {
            view.key_down(column, event, window, cx);
        }))
        .into_any_element()
}
