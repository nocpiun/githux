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
mod rows;
#[cfg(test)]
mod tests;
mod time;

use crate::ui::tooltip::{ui_tooltip, ui_tooltip_content};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

use ely_core::{
    buttons::IconButton,
    motion::Spinner,
    primitives::{IconName, Tooltip},
    theme::{ActiveTheme, ControlSize, IconSize, Radius, TextSize},
};
use gpui::{
    AnyElement, App, Context, FocusHandle, Focusable, KeyDownEvent, ListAlignment, ListState,
    Pixels, Point, Render, Subscription, Task, Window, div, prelude::*, px,
};

use crate::domain::home::{FeedOutcome, HomeError, HomeItem, HomeSearchOutcome};
use crate::ui::scroll::{self, Bars, Motion, ScrollTarget};

use presentation::home_summary;
use rows::{feed_row, home_row};

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

/// Stable profile identity. Actor ids remain real; repository-owner targets
/// carry only their canonical login (`id: None`) when the model has no id.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProfileIdentity {
    pub id: Option<u64>,
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
/// Optional host presentation hook. The component supplies a stable actor identity and receives
/// an already-resolved avatar element; no image request or cache belongs to HomeView.
pub type AvatarRenderer =
    Rc<dyn Fn(&crate::domain::home::FeedEvent, usize, &mut App) -> Option<AnyElement>>;

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
    scroll: ListState,
    motion: Motion<ListState>,
    bars: Bars,
    focus: FocusHandle,
    selected: Option<usize>,
    pending_reveal: Option<usize>,
    clock: Instant,
    #[cfg(test)]
    pinned_clock: Option<u64>,
}

impl PanelView {
    fn new(cx: &mut Context<HomeView>) -> Self {
        let scroll = ListState::new(0, ListAlignment::Top, px(200.0));
        let motion = Motion::new(scroll.clone());
        Self {
            scroll,
            motion,
            bars: Bars::new(),
            focus: cx.focus_handle().tab_stop(true),
            selected: None,
            pending_reveal: None,
            clock: Instant::now(),
            #[cfg(test)]
            pinned_clock: None,
        }
    }

    fn reset(&mut self, window: &mut Window, cx: &mut App) {
        scroll::cancel_region(&mut self.bars, &self.motion);
        if self.focus.contains_focused(window, cx) {
            window.blur(cx);
        }
        self.selected = None;
        self.pending_reveal = None;
        // Replacing the handle also drops native deferred reveal/drag state.
        // The new Motion wraps exactly the new list's real base handle.
        self.scroll = ListState::new(0, ListAlignment::Top, px(200.0));
        self.motion = Motion::new(self.scroll.clone());
    }

    fn now_ms(&self) -> u64 {
        #[cfg(test)]
        if let Some(ms) = self.pinned_clock {
            return ms;
        }
        self.clock.elapsed().as_millis() as u64
    }

    fn offset(&self) -> Point<Pixels> {
        self.motion.offset()
    }

    fn handles(&self) -> PanelHandles {
        PanelHandles {
            scroll: self.scroll.clone(),
            motion: self.motion.clone(),
            focus: self.focus.clone(),
        }
    }

    fn reveal(&mut self, index: usize) {
        self.motion.cancel();
        self.pending_reveal = Some(index);
    }
}

/// Cloneable real handles for one panel, sampled once per frame.
struct PanelHandles {
    scroll: ListState,
    motion: Motion<ListState>,
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
    avatar_renderer: Option<AvatarRenderer>,
    last_action: Option<HomeAction>,
    now: SystemTime,
    fixed_now: bool,
    active: bool,
    time_task: Option<Task<()>>,
    _activation: Subscription,
    _visibility: Subscription,
}

impl HomeView {
    /// Starts an unrequested binding (no current data). The host must call
    /// [`HomeView::set_loading`] and then supply each result with the same
    /// binding and generation.
    pub fn new(
        binding: RequestBinding,
        generation: u64,
        window: &mut Window,
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
            avatar_renderer: None,
            last_action: None,
            now: SystemTime::now(),
            fixed_now: false,
            active: true,
            time_task: None,
            // Observers only cancel. A retained but unmounted view must not
            // restart its timer on window reactivation; startup belongs to render.
            _activation: cx.observe_window_activation(window, |view, window, cx| {
                if !window.is_window_active() {
                    view.cancel_activity();
                }
                cx.notify();
            }),
            _visibility: cx.observe_window_visibility(window, |view, visibility, _, cx| {
                if !visibility.is_visible() {
                    view.cancel_activity();
                }
                cx.notify();
            }),
        }
    }

    /// Inject a fixed wall clock for previews/tests. `None` resumes real time.
    pub fn set_now(&mut self, now: Option<SystemTime>, cx: &mut Context<Self>) {
        self.fixed_now = now.is_some();
        self.now = now.unwrap_or_else(SystemTime::now);
        self.time_task = None;
        cx.notify();
    }

    /// A retained page's host calls this when switching tabs. No elapsed-time
    /// timer or scroll animation runs for an inactive page.
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.active = active;
        if !active {
            self.cancel_activity();
        }
        cx.notify();
    }

    fn cancel_activity(&mut self) {
        self.time_task = None;
        for panel in [&mut self.home_view, &mut self.feed_view] {
            scroll::cancel_region(&mut panel.bars, &panel.motion);
        }
    }

    fn sync_time_task(&mut self, window: &Window, cx: &mut Context<Self>) {
        if !self.active || !window.is_window_active() || !window.is_visible() {
            self.cancel_activity();
            return;
        }
        if self.fixed_now || self.item_count(Column::Home) + self.item_count(Column::Feed) == 0 {
            self.time_task = None;
            return;
        }
        if self.time_task.is_none()
            && self.item_count(Column::Home) + self.item_count(Column::Feed) > 0
        {
            self.now = SystemTime::now();
            self.time_task = Some(cx.spawn(async |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_secs(30))
                        .await;
                    if this
                        .update(cx, |view, cx| {
                            view.now = SystemTime::now();
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }));
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
        self.cancel_activity();
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

    pub fn set_avatar_renderer(&mut self, renderer: AvatarRenderer) {
        self.avatar_renderer = Some(renderer);
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
        self.home_view.motion.cancel();
        self.home_view.pending_reveal = None;
        self.home_view
            .scroll
            .reset_with_uniform_height(outcome.items.len(), px(rows::HOME_ROW_HEIGHT));
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
        self.feed_view.motion.cancel();
        self.feed_view.pending_reveal = None;
        self.feed_view
            .scroll
            .reset_with_uniform_height(outcome.events.len(), px(180.0));
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

    #[cfg(test)]
    pub(crate) fn feed_refresh_available(&self) -> bool {
        !self.feed.state.is_busy() && self.feed.refresh.available
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
        if !self.panel_mut(column).focus.is_focused(window) {
            return;
        }
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
        self.sync_time_task(window, cx);
        let (colors, body_size) = {
            let theme = cx.theme();
            (theme.colors.clone(), theme.text_size(TextSize::Sm))
        };
        for panel in [&mut self.home_view, &mut self.feed_view] {
            let animating = panel.motion.tick(Instant::now(), cx.reduce_motion());
            if animating {
                window.request_animation_frame();
            }
            if let Some(index) = panel.pending_reveal.take() {
                panel.scroll.scroll_to(gpui::ListOffset {
                    item_ix: index,
                    offset_in_item: px(0.0),
                });
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
            .map(|columns| {
                if narrow {
                    columns.flex_col().gap_4().children([home, feed])
                } else {
                    columns.flex_row().gap_8().children([home, feed])
                }
            });

        div()
            .debug_selector(|| "home-view".to_string())
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .min_h_0()
            .relative()
            .px_4()
            .pt_4()
            .pb_2()
            .bg(colors.surface)
            .text_size(body_size)
            .text_color(colors.fg)
            .child(columns)
            .child(visible_lifetime(cx))
    }
}

fn render_panel<T: PanelRows + 'static>(
    column: Column,
    model: &PanelModel<T>,
    opacity: f32,
    handles: &PanelHandles,
    cx: &mut Context<HomeView>,
) -> AnyElement {
    let (heading_size, side, icon_size) = {
        let theme = cx.theme();
        (
            theme.text_size(TextSize::Lg),
            theme.control_height(ControlSize::Sm),
            IconSize::Sm,
        )
    };
    let title = match column {
        Column::Home => "Home",
        Column::Feed => "Feed",
    };
    let body = panel_body(column, model, opacity, handles, cx);
    let busy = model.state.is_busy();
    let disabled = !model.refresh.available || busy;
    let tooltip = if busy {
        format!("Refreshing {title}")
    } else if let Some(reason) = model
        .refresh
        .reason
        .as_ref()
        .filter(|_| !model.refresh.available)
    {
        format!("Refresh {title}: {reason}")
    } else {
        format!("Refresh {title}")
    };
    let panel = cx.entity().downgrade();
    let control = if busy {
        div()
            .id(format!("{}-refresh-busy", column_name(column)))
            .debug_selector(move || format!("{}-refresh-spinner", column_name(column)))
            .role(gpui::Role::ProgressIndicator)
            .aria_label(tooltip.clone())
            .tooltip(ui_tooltip(tooltip))
            .flex()
            .items_center()
            .justify_center()
            .size(side)
            .child(Spinner::new(format!("{}-spinner", column_name(column))).size(icon_size))
            .into_any_element()
    } else {
        // The wrapper is exactly the button's slot; the tip itself must set the UI family because
        // GPUI prepaints it as a separate root (Ely's own `IconButton` tip would use the default
        // system face).
        div()
            .id(format!("{}-refresh-slot", column_name(column)))
            .flex_none()
            .tooltip(ui_tooltip(tooltip.clone()))
            .child(
                IconButton::new(
                    format!("{}-refresh", column_name(column)),
                    IconName::RotateCw,
                )
                .size(ControlSize::Sm)
                .disabled(disabled)
                .on_click(move |_, window, cx| {
                    let _ = panel.update(cx, |view, cx| view.refresh(column, window, cx));
                }),
            )
            .into_any_element()
    };
    div()
        .debug_selector(move || format!("{}-panel", column_name(column)))
        .flex()
        .flex_col()
        .flex_1()
        .flex_basis(px(0.0))
        .min_w_0()
        .min_h_0()
        .overflow_hidden()
        .gap_2()
        .child(
            div()
                .debug_selector(move || format!("{}-header", column_name(column)))
                .flex_none()
                .flex()
                .items_center()
                .gap_2()
                .h(px(40.0))
                .px_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(heading_size)
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(
                    div()
                        .debug_selector(move || format!("{}-refresh-slot", column_name(column)))
                        .flex_none()
                        .size(side)
                        .child(control),
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
        home_row(
            &item,
            index,
            view.home_view.selected == Some(index),
            view.now,
            cx,
        )
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
        feed_row(
            &event,
            index,
            view.feed_view.selected == Some(index),
            view.now,
            view.avatar_renderer
                .as_ref()
                .and_then(|render| render(&event, index, cx)),
            cx,
        )
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
    // A hard failure is a compact warning with a readable reason in its tooltip. A first load
    // or an empty/unavailable state stays a short written status: it is not an error mark, and a
    // spinner alone would leave "no data" and "not signed in" indistinguishable.
    if let PanelState::Ready {
        content: PanelContent::Error(error),
        ..
    } = &model.state
    {
        let reason = presentation::error_message(error);
        return div()
            .debug_selector(move || format!("{}-status", column_name(column)))
            .flex_1()
            .min_h_0()
            .flex()
            .items_center()
            .justify_center()
            .p_5()
            .text_center()
            .child(crate::ui::compact_status::failed(
                format!("{}-error-mark", column_name(column)),
                "This column could not be loaded",
                reason,
                None::<fn(&mut gpui::Window, &mut gpui::App)>,
                None,
                cx,
            ))
            .into_any_element();
    }
    let status = match &model.state {
        PanelState::Loading => Some((
            "Loading…",
            colors.fg_subtle,
            None,
            crate::ui::compact_status::loading(
                format!("{}-loading-mark", column_name(column)),
                "Loading this column…",
                cx,
            ),
        )),
        PanelState::Ready {
            content: PanelContent::Unavailable { message },
            ..
        } => Some((
            "No current data.",
            colors.fg_subtle,
            Some(message.clone()),
            { crate::ui::compact_status::empty("No current data", cx) },
        )),
        PanelState::Ready {
            content: PanelContent::Ready(_),
            ..
        }
        | PanelState::Ready {
            content: PanelContent::Error(_),
            ..
        } => None,
    };
    if let Some((title, color, detail, mark)) = status {
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
            .child(mark)
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
    let partial = summary.notice;
    let list = list_rows::<T>(column, opacity, handles, cx);
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

fn list_rows<T: PanelRows + 'static>(
    column: Column,
    opacity: f32,
    handles: &PanelHandles,
    cx: &mut Context<HomeView>,
) -> AnyElement {
    let scroll = handles.scroll.clone();
    let motion = handles.motion.clone();
    let focus = handles.focus.clone();
    let list = gpui::list(
        scroll,
        cx.processor(move |view: &mut HomeView, index: usize, window, cx| {
            T::row(view, index, window, cx)
        }),
    )
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
        placement: scroll::Placement::overlay(1.5, 8.0),
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
        .child(scroll::wheel_overlay_with(
            match column {
                Column::Home => "home-rows",
                Column::Feed => "feed-rows",
            },
            scroll::WheelAxes::Vertical,
            motion,
            Instant::now,
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

// Keyed state is released when a retained page leaves the rendered tree.
// That cancels its one timer even if the host keeps the entity alive.
fn visible_lifetime(cx: &Context<HomeView>) -> AnyElement {
    let owner = cx.entity().downgrade();
    let key = format!("home-visible-lifetime-{}", cx.entity_id().as_u64());
    gpui::canvas(
        move |_, window, cx| {
            let owner = owner.clone();
            window.use_keyed_state(key.clone(), cx, move |_, lease| {
                lease
                    .on_release(move |_, cx| {
                        let _ = owner.update(cx, |view, _| view.cancel_activity());
                    })
                    .detach();
            });
        },
        |_, _, _, _| {},
    )
    // Keep the render lease alive without adding a flex item or a gap below
    // the two viewports. The canvas remains part of every rendered frame.
    .absolute()
    .top_0()
    .left_0()
    .size(px(0.0))
    .into_any_element()
}
