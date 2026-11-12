//! Reusable controller-backed global Issues / Pull Requests view.
//! Construction/activation starts the initial read; rendering never requests.
// Mounted by the Shell through `ui::global_work_items`; the standalone preview also compiles it.
// Preview/test-only APIs (`set_now`, `rebind`) remain for the developer preview.
#![allow(dead_code)]
pub mod controller;
mod presentation;
mod rows;
// Transitional consumer of Home's pure formatter: the same source file, no
// second parser. Notifications is extracting a shared helper; this path will
// be replaced by that committed module when it lands.
#[cfg(test)]
mod tests;
#[path = "../home/time.rs"]
mod time;

use crate::ui::scroll::{self, Bars, Motion};
use crate::{
    data::RepoService, domain::work_item::*, github::work_items::WorkItemLimits,
    network::executor::BackgroundExecutor,
};
use controller::{Controller, Request, RequestMode};
use ely_core::{
    buttons::{Button, IconButton},
    motion::Spinner,
    primitives::{Icon, IconName, Tooltip},
    theme::{ActiveTheme, ControlSize, IconSize, TextSize},
};
use gpui::{
    AnyElement, App, Context, FocusHandle, Focusable, KeyDownEvent, ListAlignment, ListState,
    Render, Subscription, Task, Window, div, prelude::*, px,
};
use std::{
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

/// The host checks its workspace/tab incarnation when consuming navigation.
/// Account ownership is captured at the time the repository link is rendered.
#[derive(Clone, Debug)]
pub struct RepositoryNavigation {
    pub context: crate::data::RepositoryReadContext,
    pub repository: WorkRepository,
}
pub type OnRepository = Rc<dyn Fn(RepositoryNavigation, &mut Window, &mut App)>;

pub struct WorkItemsView {
    pub controller: Controller,
    repos: RepoService,
    executor: Arc<BackgroundExecutor>,
    limits: WorkItemLimits,
    receiver: Option<Task<()>>,
    row_count: usize,
    content_insets: scroll::ListInsets,
    focus: FocusHandle,
    list: ListState,
    motion: Motion<scroll::InsetListTarget>,
    bars: Bars,
    selected: Option<u64>,
    pending_reveal: Option<usize>,
    clock: Instant,
    active: bool,
    now: SystemTime,
    fixed_now: bool,
    time_task: Option<Task<()>>,
    on_repository: Option<OnRepository>,
    _activation: Subscription,
    _visibility: Subscription,
}
impl WorkItemsView {
    pub fn new(
        repos: RepoService,
        executor: Arc<BackgroundExecutor>,
        kind: WorkItemKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Self, WorkItemError> {
        let session = repos.work_item_session()?;
        let content_insets = scroll::ListInsets::default();
        let list = ListState::new(0, ListAlignment::Top, px(rows::ROW_HEIGHT));
        let activation = cx.observe_window_activation(window, |view, _, cx| {
            scroll::cancel_region(&mut view.bars, &view.motion);
            cx.notify();
        });
        let visibility = cx.observe_window_visibility(window, |view, visibility, _, cx| {
            if !visibility.is_visible() {
                view.cancel_activity();
            }
            cx.notify();
        });
        let mut view = Self {
            controller: Controller::new(session, kind),
            repos,
            executor,
            limits: WorkItemLimits::default(),
            receiver: None,
            row_count: 0,
            content_insets: content_insets.clone(),
            focus: cx.focus_handle().tab_stop(true),
            motion: Motion::new(scroll::InsetListTarget::new(list.clone(), content_insets)),
            list,
            bars: Bars::new(),
            selected: None,
            pending_reveal: None,
            clock: Instant::now(),
            active: true,
            now: SystemTime::now(),
            fixed_now: false,
            time_task: None,
            on_repository: None,
            _activation: activation,
            _visibility: visibility,
        };
        view.request(RequestMode::Replace, cx);
        Ok(view)
    }
    /// Deterministic test/preview assembly with an already-owned controller.
    /// It deliberately performs no request until the caller applies one.
    #[cfg(test)]
    pub(crate) fn with_controller(
        controller: Controller,
        repos: RepoService,
        executor: Arc<BackgroundExecutor>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let count = controller.rows.len();
        let content_insets = scroll::ListInsets::default();
        let list = ListState::new(count, ListAlignment::Top, px(rows::ROW_HEIGHT));
        let activation = cx.observe_window_activation(window, |view, _, cx| {
            scroll::cancel_region(&mut view.bars, &view.motion);
            cx.notify();
        });
        let visibility = cx.observe_window_visibility(window, |view, visibility, _, cx| {
            if !visibility.is_visible() {
                view.cancel_activity();
            }
            cx.notify();
        });
        Self {
            controller,
            repos,
            executor,
            limits: WorkItemLimits::default(),
            receiver: None,
            row_count: count,
            content_insets: content_insets.clone(),
            focus: cx.focus_handle().tab_stop(true),
            motion: Motion::new(scroll::InsetListTarget::new(list.clone(), content_insets)),
            list,
            bars: Bars::new(),
            selected: None,
            pending_reveal: None,
            clock: Instant::now(),
            active: true,
            now: SystemTime::now(),
            fixed_now: false,
            time_task: None,
            on_repository: None,
            _activation: activation,
            _visibility: visibility,
        }
    }
    pub fn on_repository(&mut self, callback: OnRepository) {
        self.on_repository = Some(callback);
    }
    /// Call when retained tabs leave the foreground. Cancellation invalidates
    /// successes and errors; reentry loads only when no snapshot was obtained.
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.active = active;
        if !active {
            self.shutdown_activity();
        } else if !self.controller.loaded {
            self.request(RequestMode::Replace, cx);
        }
        cx.notify();
    }
    fn shutdown_activity(&mut self) {
        self.controller.cancel();
        self.receiver = None;
        self.time_task = None;
        scroll::cancel_region(&mut self.bars, &self.motion);
    }

    fn cancel_activity(&mut self) {
        self.time_task = None;
        scroll::cancel_region(&mut self.bars, &self.motion);
    }

    /// Inject a fixed wall clock for previews/tests. `None` resumes real time.
    pub fn set_now(&mut self, now: Option<SystemTime>, cx: &mut Context<Self>) {
        self.fixed_now = now.is_some();
        self.now = now.unwrap_or_else(SystemTime::now);
        self.time_task = None;
        cx.notify();
    }

    fn sync_time_task(&mut self, window: &Window, cx: &mut Context<Self>) {
        if !self.active || !window.is_window_active() || !window.is_visible() {
            self.cancel_activity();
            return;
        }
        if self.fixed_now || self.controller.rows.is_empty() {
            self.time_task = None;
            return;
        }
        if self.time_task.is_none() {
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

    /// Use on account/auth changes, including A -> B -> A. No old rows survive.
    pub fn rebind(&mut self, cx: &mut Context<Self>) -> Result<(), WorkItemError> {
        let session = self.repos.work_item_session();
        self.controller.cancel();
        self.receiver = None;
        self.controller.rows.clear();
        self.controller.next = None;
        self.controller.loaded = false;
        self.controller.partial.clear();
        self.controller.error = None;
        self.controller.failed_mode = None;
        self.controller.total_count = None;
        self.reset_list();
        cx.notify();
        let session = session?;
        self.controller.rebind(session);
        if self.active {
            self.request(RequestMode::Replace, cx);
        }
        cx.notify();
        Ok(())
    }
    pub fn filter(
        &mut self,
        kind: WorkItemKind,
        relation: Relation,
        state: StateFilter,
        cx: &mut Context<Self>,
    ) {
        if !self.active {
            return;
        }
        if let Some(request) = self.controller.filter(kind, relation, state) {
            self.reset_list();
            self.dispatch(request, cx);
        }
    }
    fn reset_list(&mut self) {
        scroll::cancel_region(&mut self.bars, &self.motion);
        self.selected = None;
        self.pending_reveal = None;
        self.row_count = 0;
        self.list = ListState::new(0, ListAlignment::Top, px(rows::ROW_HEIGHT));
        self.content_insets = scroll::ListInsets::default();
        self.motion = Motion::new(scroll::InsetListTarget::new(
            self.list.clone(),
            self.content_insets.clone(),
        ));
    }
    fn request(&mut self, mode: RequestMode, cx: &mut Context<Self>) {
        if !self.active {
            return;
        }
        if let Some(request) = self.controller.begin(mode) {
            self.dispatch(request, cx);
        }
    }
    fn dispatch(&mut self, request: Request, cx: &mut Context<Self>) {
        self.receiver = None;
        let worker_request = request.clone();
        let limits = self.limits.clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        let spawned = self.executor.handle().spawn(async move {
            let result = worker_request.execute(&limits).await;
            let _ = send.send(result);
        });
        if spawned.is_err() {
            self.controller.finish(
                &request,
                Err(crate::domain::error::GitHubError::Network {
                    message: "request executor unavailable".into(),
                }
                .into()),
            );
            cx.notify();
            return;
        }
        self.receiver = Some(cx.spawn(async move |view, cx| {
            let result = receive.await.unwrap_or_else(|_| {
                Err(crate::domain::error::GitHubError::Network {
                    message: "request worker closed".into(),
                }
                .into())
            });
            let _ = view.update(cx, |view, cx| {
                view.apply_result(&request, result, cx);
            });
        }));
        cx.notify();
    }
    fn apply_result(
        &mut self,
        request: &Request,
        result: Result<WorkItemPage, WorkItemError>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.controller.finish(request, result) {
            return false;
        }
        if request.mode == RequestMode::Replace && self.controller.error.is_none() {
            self.reset_list();
        }
        let count = self.controller.rows.len();
        if request.mode == RequestMode::Append && self.controller.error.is_none() {
            self.list
                .splice(self.row_count..self.row_count, count - self.row_count);
        } else if self.controller.error.is_none() {
            self.list
                .reset_with_uniform_height(count, px(rows::ROW_HEIGHT));
        }
        self.row_count = count;
        cx.notify();
        true
    }
    fn navigate(&self, navigation: RepositoryNavigation, window: &mut Window, cx: &mut App) {
        if self.active
            && self.repos.is_current(&navigation.context)
            && let Some(callback) = &self.on_repository
        {
            callback(navigation, window, cx);
        }
    }
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus.is_focused(window) || !self.controller.current() {
            return;
        }
        let len = self.controller.rows.len();
        let selected = self
            .selected
            .and_then(|id| self.controller.rows.iter().position(|row| row.id == id));
        let target = match event.keystroke.key.as_str() {
            "down" => Some(selected.map_or(0, |index| (index + 1).min(len.saturating_sub(1)))),
            "up" => Some(selected.unwrap_or(0).saturating_sub(1)),
            "home" => Some(0),
            "end" => Some(len.saturating_sub(1)),
            // Repository navigation has its own explicit keyboard target.
            // Enter on a title/row does not stand in for unimplemented details.
            _ => None,
        };
        if len > 0
            && let Some(index) = target
        {
            self.selected = Some(self.controller.rows[index].id);
            self.motion.cancel();
            self.pending_reveal = Some(index);
            cx.stop_propagation();
            cx.notify();
        }
    }
    fn controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let query = &self.controller.query;
        let kind = query.kind;
        let relation = query.relation;
        let state = query.state;
        let weak = cx.entity().downgrade();
        let relations = ely_core::forms::Select::new(
            "work-relation",
            [
                ely_core::forms::Choice::new("assigned", "Assigned"),
                ely_core::forms::Choice::new("created", "Created"),
            ],
        )
        .selected(if relation == Relation::Assigned {
            "assigned"
        } else {
            "created"
        })
        .size(ControlSize::Sm)
        .on_change(move |value, _, cx| {
            let relation = if value == "created" {
                Relation::Created
            } else {
                Relation::Assigned
            };
            let _ = weak.update(cx, |view, cx| view.filter(kind, relation, state, cx));
        });
        let weak = cx.entity().downgrade();
        let states = ely_core::forms::Select::new(
            "work-state",
            [
                ely_core::forms::Choice::new("open", "Open"),
                ely_core::forms::Choice::new("closed", "Closed"),
                ely_core::forms::Choice::new("all", "All"),
            ],
        )
        .selected(match state {
            StateFilter::Open => "open",
            StateFilter::Closed => "closed",
            StateFilter::All => "all",
        })
        .size(ControlSize::Sm)
        .on_change(move |value, _, cx| {
            let state = match value.as_str() {
                "closed" => StateFilter::Closed,
                "all" => StateFilter::All,
                _ => StateFilter::Open,
            };
            let _ = weak.update(cx, |view, cx| view.filter(kind, relation, state, cx));
        });
        div()
            .flex()
            .items_center()
            .gap_2()
            .flex_none()
            .child(div().w(px(124.0)).child(relations))
            .child(div().w(px(104.0)).child(states))
            .into_any_element()
    }
}
impl Focusable for WorkItemsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for WorkItemsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_time_task(window, cx);
        let current = self.controller.current();
        if !current {
            scroll::cancel_region(&mut self.bars, &self.motion);
        }
        if self.motion.tick(Instant::now(), cx.reduce_motion()) {
            window.request_animation_frame();
        }
        if let Some(index) = self.pending_reveal.take() {
            self.list.scroll_to(gpui::ListOffset {
                item_ix: index,
                offset_in_item: px(0.0),
            });
        }
        let now = self.clock.elapsed().as_millis() as u64;
        if self.bars.dragging().is_some() && !cx.has_active_drag() {
            self.bars.settle(&self.motion, now, cx.reduce_motion());
        }
        let opacity = self.bars.opacity(now);
        if self.bars.is_animating() {
            window.request_animation_frame();
        }
        let colors = cx.theme().colors.clone();
        let busy = self.controller.busy.is_some() && current;
        let refresh = if busy {
            div()
                .size(px(28.0))
                .flex()
                .items_center()
                .justify_center()
                .child(Spinner::new("work-loading").size(IconSize::Sm))
                .into_any_element()
        } else {
            IconButton::new("work-refresh", IconName::RotateCw)
                .size(ControlSize::Sm)
                .disabled(!current)
                .tooltip("Refresh")
                .on_click(cx.listener(|view, _, _, cx| view.request(RequestMode::Replace, cx)))
                .into_any_element()
        };
        let title = if self.controller.query.kind == WorkItemKind::Issue {
            "Issues"
        } else {
            "Pull Requests"
        };
        let header = div()
            .debug_selector(|| "work-header".into())
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            // Same title slot height and inner inset as Home's column header and Notifications,
            // so the four production pages put their heading on the same line and at the same x.
            .h(px(crate::ui::page_layout::PAGE_HEADER_HEIGHT))
            .px(px(crate::ui::page_layout::PAGE_HEADER_INSET))
            .child(
                div()
                    .debug_selector(|| "work-title".into())
                    .flex_1()
                    .min_w_0()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_size(cx.theme().text_size(TextSize::Lg))
                    .child(title),
            )
            .child(refresh);
        let controls = self.controls(cx);
        let mut status = div().flex_none().flex().items_center().gap_2().min_w_0();
        if current && let Some(error) = &self.controller.error {
            let retry_mode = self.controller.failed_mode.unwrap_or(RequestMode::Replace);
            status = status
                .child(
                    Icon::new(IconName::CircleAlert)
                        .size(IconSize::Sm)
                        .color(colors.danger),
                )
                .child(
                    IconButton::new("work-retry", IconName::RotateCw)
                        .size(ControlSize::Sm)
                        .disabled(busy)
                        .tooltip(presentation::error_message(error))
                        .on_click(cx.listener(move |view, _, _, cx| view.request(retry_mode, cx))),
                );
        }
        if current && !self.controller.partial.is_empty() {
            status = status.child(
                div()
                    .id("work-partial")
                    .debug_selector(|| "work-partial".into())
                    .tooltip(Tooltip::text(presentation::partial_message(
                        &self.controller.partial,
                    )))
                    .child(
                        Icon::new(IconName::CircleAlert)
                            .size(IconSize::Sm)
                            .color(colors.warning),
                    ),
            );
        }
        let fixed_header = div()
            .debug_selector(|| "work-fixed-header".into())
            .flex_none()
            .flex()
            .flex_col()
            .min_w_0()
            .gap_2()
            .child(header)
            .child(controls)
            .when(
                current && (self.controller.error.is_some() || !self.controller.partial.is_empty()),
                |header| header.child(status),
            );
        let insets = scroll::ContentInsets::new(
            px(
                if current
                    && (self.controller.error.is_some() || !self.controller.partial.is_empty())
                {
                    8.0
                } else {
                    16.0
                },
            ),
            px(crate::ui::page_layout::PAGE_BOTTOM),
        );
        insets.sync(
            &self.list,
            self.controller.rows.len(),
            &mut self.content_insets,
        );
        let body = if !current {
            div()
                .flex_1()
                .pt(insets.top)
                .pb(insets.bottom)
                .child("Account changed")
                .into_any_element()
        } else if self.controller.is_empty() {
            div()
                .debug_selector(|| "work-empty".into())
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .pt(insets.top)
                .pb(insets.bottom)
                .text_color(colors.fg_subtle)
                .child("No results")
                .into_any_element()
        } else {
            self.render_list(opacity, cx)
        };
        let more = self.controller.next.is_some() && current;
        let weak = cx.entity().downgrade();
        let lease_key = format!("work-items-visible-{}", cx.entity_id().as_u64());
        let lease = gpui::canvas(
            move |_, window, cx| {
                let weak = weak.clone();
                window.use_keyed_state(lease_key.clone(), cx, move |_, lease| {
                    lease
                        .on_release(move |_, cx| {
                            let _ = weak.update(cx, |view, _| {
                                scroll::cancel_region(&mut view.bars, &view.motion);
                            });
                        })
                        .detach();
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size(px(0.0));
        div()
            .debug_selector(|| "global-work-items".into())
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            // Keep heading/control spacing inside the fixed header. The native list
            // reaches the page bottom without a permanently masked padding band.
            .px(px(crate::ui::page_layout::PAGE_GUTTER))
            .pt(px(crate::ui::page_layout::PAGE_TOP))
            .bg(colors.surface)
            .text_color(colors.fg)
            .text_size(cx.theme().text_size(TextSize::Sm))
            .relative()
            .child(fixed_header)
            .child(body)
            .child(lease)
            .when(more, |view| {
                view.child(
                    div()
                        .debug_selector(|| "work-more-footer".into())
                        .flex_none()
                        .pb(px(crate::ui::page_layout::PAGE_BOTTOM))
                        .child(
                            Button::new("work-more", "Load more")
                                .size(ControlSize::Sm)
                                .disabled(busy)
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.request(RequestMode::Append, cx)
                                })),
                        ),
                )
            })
    }
}
impl WorkItemsView {
    fn render_list(&self, opacity: f32, cx: &mut Context<Self>) -> AnyElement {
        let insets = self.content_insets.get();
        let count = self.controller.rows.len();
        let list = gpui::list(
            self.list.clone(),
            cx.processor(move |view: &mut WorkItemsView, index: usize, _, cx| {
                if !view.controller.current() {
                    return div().into_any_element();
                }
                let item = view.controller.rows[index].clone();
                let context = Some(view.controller.context().clone());
                insets.item(
                    index,
                    count,
                    rows::row(&item, view.selected == Some(item.id), context, view.now, cx),
                )
            }),
        )
        .size_full()
        .pb(insets.bottom);
        let weak = cx.entity().downgrade();
        let on_drag: scroll::OnDragging = Rc::new(move |axis, active, _, cx| {
            let _ = weak.update(cx, |view, cx| {
                view.bars.drag(
                    &view.motion,
                    axis,
                    active,
                    view.clock.elapsed().as_millis() as u64,
                    cx.reduce_motion(),
                );
                cx.notify();
            });
        });
        let bar = scroll::Bar {
            target: &self.motion,
            id: scroll::BarId {
                owner: cx.entity_id(),
                name: "work-scrollbar",
                axis: gpui::Axis::Vertical,
            },
            placement: scroll::Placement::overlay(1.5, scroll::LIST_BAR_LANE)
                .with_track_insets(insets.top, insets.bottom),
            style: scroll::Style::DEFAULT,
            opacity,
            on_drag,
        }
        .render(cx);
        div()
            .id("work-rows")
            .debug_selector(|| "work-rows".into())
            .track_focus(&self.focus)
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left_0()
                    .right(px(scroll::LIST_BAR_LANE))
                    .overflow_hidden()
                    .debug_selector(|| "work-list-viewport".into())
                    .child(list),
            )
            .child(scroll::wheel_overlay_with(
                "work-wheel",
                scroll::WheelAxes::Vertical,
                self.motion.clone(),
                Instant::now,
            ))
            .child(bar)
            .on_hover(cx.listener(|view, hovered, _, cx| {
                if view.bars.hover(
                    *hovered,
                    view.clock.elapsed().as_millis() as u64,
                    cx.reduce_motion(),
                ) {
                    cx.notify();
                }
            }))
            .on_key_down(cx.listener(|view, event, window, cx| view.key_down(event, window, cx)))
            .into_any_element()
    }
}
