//! Standalone inbox view/controller adapter. Official Shell mounting is owned
//! by the coordinator. No route, browser redirect or notification write action.
#![allow(dead_code)]
#[cfg(test)]
mod tests;
use crate::data::notifications::{InboxSnapshot, Notifications, RequestKind};
use crate::domain::notification::{NotificationThread, SubjectKind};
use crate::domain::repository::RepositoryIdentity;
use crate::network::executor::BackgroundExecutor;
use crate::ui::scroll::{self, Bars, Motion};
use ely_core::{
    buttons::{Button, IconButton},
    motion::Spinner,
    primitives::{FocusRing, Icon, IconName, Tooltip},
    theme::{ActiveTheme, ControlSize, IconSize, TextSize},
    typography::Ellipsis,
};
use gpui::{
    AnyElement, App, Axis, Context, FocusHandle, Focusable, KeyDownEvent, ListAlignment, ListState,
    Render, Subscription, Window, div, prelude::*, px,
};
use std::{rc::Rc, sync::Arc, time::Instant};

/// Host checks this context and its tab/incarnation before opening Repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositoryNavigation {
    pub context: crate::data::RepositoryReadContext,
    pub repository: RepositoryIdentity,
}
pub type OnRepository = Rc<dyn Fn(RepositoryNavigation, &mut Window, &mut App)>;

pub struct NotificationsView {
    controller: Notifications,
    executor: Arc<BackgroundExecutor>,
    snapshot: InboxSnapshot,
    list: ListState,
    motion: Motion<ListState>,
    bars: Bars,
    focus: FocusHandle,
    selected: Option<usize>,
    clock: Instant,
    cooldown_until: Option<Instant>,
    cooldown_wake: Option<gpui::Task<()>>,
    on_repository: OnRepository,
    _activation: Subscription,
}
impl NotificationsView {
    pub fn new(
        controller: Notifications,
        executor: Arc<BackgroundExecutor>,
        on_repository: OnRepository,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let list = ListState::new(0, ListAlignment::Top, px(160.0));
        let activation = cx.observe_window_activation(window, |view: &mut Self, window, cx| {
            if !window.is_window_active() {
                scroll::cancel_region(&mut view.bars, &view.motion);
                cx.notify();
            }
        });
        let mut view = Self {
            controller,
            executor,
            snapshot: InboxSnapshot::default(),
            motion: Motion::new(list.clone()),
            list,
            bars: Bars::new(),
            focus: cx.focus_handle().tab_stop(true),
            selected: None,
            clock: Instant::now(),
            cooldown_until: None,
            cooldown_wake: None,
            on_repository,
            _activation: activation,
        };
        view.sync(cx);
        view.request(RequestKind::First, cx);
        view
    }

    /// Call when the page becomes hidden. Re-entering the retained entity does
    /// not issue another first load. Account changes require a fresh controller.
    pub fn cancel_interactions(&mut self, cx: &mut Context<Self>) {
        scroll::cancel_region(&mut self.bars, &self.motion);
        cx.notify();
    }

    /// Host calls this when the retained page becomes hidden: stop both the
    /// interaction animation and the request (its result can no longer publish).
    pub fn leave(&mut self, cx: &mut Context<Self>) {
        self.controller.cancel();
        self.cooldown_wake = None;
        self.cancel_interactions(cx);
        self.sync(cx);
    }
    fn sync(&mut self, cx: &mut Context<Self>) {
        let previous = std::mem::take(&mut self.snapshot);
        self.snapshot = self.controller.snapshot();
        let old_len = previous.items.len();
        let len = self.snapshot.items.len();
        if self.snapshot.items.starts_with(&previous.items) {
            if len > old_len {
                self.list.splice(old_len..old_len, len - old_len);
            }
        } else {
            self.list.splice(0..old_len, len);
        }
        self.selected = self.selected.filter(|index| *index < len);
        self.cooldown_until = Instant::now().checked_add(std::time::Duration::from_secs(
            self.snapshot.cooldown_seconds,
        ));
        self.cooldown_wake = None;
        if self.snapshot.cooldown_seconds > 0 {
            let timer = cx
                .background_executor()
                .timer(std::time::Duration::from_secs(
                    self.snapshot.cooldown_seconds,
                ));
            self.cooldown_wake = Some(cx.spawn(async move |weak, cx| {
                timer.await;
                let _ = weak.update(cx, |_, cx| cx.notify());
            }));
        }
        cx.notify();
    }
    fn request(&mut self, kind: RequestKind, cx: &mut Context<Self>) {
        let request = match self.controller.begin(kind) {
            Ok(Some(request)) => request,
            Ok(None) => return,
            Err(error) => {
                self.snapshot = self.controller.snapshot();
                self.snapshot.error = Some(error);
                cx.notify();
                return;
            }
        };
        self.sync(cx);
        let controller = self.controller.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        if self
            .executor
            .handle()
            .spawn(async move {
                controller.execute(request).await;
                let _ = tx.send(());
            })
            .is_err()
        {
            self.controller.cancel();
            self.sync(cx);
            self.snapshot.error = Some(
                crate::domain::error::GitHubError::Network {
                    message: "request executor closed".into(),
                }
                .into(),
            );
            return;
        }
        cx.spawn(async move |weak, cx| {
            if rx.await.is_ok() {
                let _ = weak.update(cx, |view, cx| view.sync(cx));
            }
        })
        .detach();
    }
    fn navigate(&self, index: usize, window: &mut Window, cx: &mut App) {
        if self.controller.verify()
            && let Some(item) = self.snapshot.items.get(index)
        {
            (self.on_repository)(
                RepositoryNavigation {
                    context: self.controller.context().clone(),
                    repository: item.repository.clone(),
                },
                window,
                cx,
            );
        }
    }
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus.is_focused(window) {
            return;
        }
        let len = self.snapshot.items.len();
        if len == 0 {
            return;
        }
        let current = self.selected.unwrap_or(0);
        let index = match event.keystroke.key.as_str() {
            "down" => Some(if self.selected.is_none() {
                0
            } else {
                (current + 1).min(len - 1)
            }),
            "up" => Some(current.saturating_sub(1)),
            "home" => Some(0),
            "end" => Some(len - 1),
            "pagedown" => Some((current + 6).min(len - 1)),
            "pageup" => Some(current.saturating_sub(6)),
            // Enter activates only the selected repository, explicitly labelled
            // by the list's accessibility description. No subject action exists.
            "enter" => {
                self.navigate(current, window, cx);
                cx.stop_propagation();
                None
            }
            "escape" => {
                self.cancel_interactions(cx);
                None
            }
            _ => None,
        };
        if let Some(index) = index {
            self.selected = Some(index);
            self.motion.cancel();
            self.list.scroll_to_reveal_item(index);
            cx.stop_propagation();
            cx.notify();
        }
    }
    fn row(&self, item: &NotificationThread, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors.clone();
        let small = cx.theme().text_size(TextSize::Xs);
        let icon = match item.subject.kind {
            SubjectKind::Issue => IconName::CircleDot,
            SubjectKind::PullRequest => IconName::GitPullRequest,
            _ => IconName::Bell,
        };
        let repository = format!(
            "{}/{}",
            item.repository.locator().owner(),
            item.repository.locator().name()
        );
        let weak = cx.entity().downgrade();
        let link = div()
            .id(("notification-repository", index))
            .debug_selector(move || format!("notification-repository-{index}"))
            .role(gpui::Role::Link)
            .aria_label(format!("Open repository {repository}"))
            .tab_index(0)
            .focus_ring(cx)
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .cursor_pointer()
            .hover(|style| style.underline())
            .tooltip(Tooltip::text(repository.clone()))
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                let _ = weak.update(cx, |view, cx| view.navigate(index, window, cx));
            })
            .child(Ellipsis::new(repository));
        div()
            .id(("notification-row", index))
            .debug_selector(move || format!("notification-row-{index}"))
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .gap_3()
            .px_3()
            .py_3()
            .border_b_1()
            .border_color(colors.border)
            .bg(if self.selected == Some(index) {
                colors.surface
            } else {
                colors.bg
            })
            .child(
                div()
                    .flex_none()
                    .pt_1()
                    .child(Icon::new(icon).size(IconSize::Sm)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .min_w_0()
                            .gap_2()
                            .child(
                                div()
                                    .id(("notification-title", index))
                                    .debug_selector(move || format!("notification-title-{index}"))
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .tooltip(Tooltip::text(item.subject.title.clone()))
                                    .child(Ellipsis::new(item.subject.title.clone())),
                            )
                            .child(div().flex_none().child(if item.unread {
                                "Unread"
                            } else {
                                "Read"
                            })),
                    )
                    .child(
                        div()
                            .id(("notification-repository-line", index))
                            .w_full()
                            .flex()
                            .items_center()
                            .min_w_0()
                            .gap_2()
                            .text_size(small)
                            .text_color(colors.fg_subtle)
                            .child(link),
                    )
                    .child(
                        div()
                            .id(("notification-meta", index))
                            .w_full()
                            .text_size(small)
                            .text_color(colors.fg_subtle)
                            .min_w_0()
                            .overflow_hidden()
                            .tooltip(Tooltip::text(format!(
                                "{} · {}",
                                item.reason, item.updated_at
                            )))
                            .child(Ellipsis::new(format!(
                                "{} · {}",
                                reason_label(&item.reason),
                                item.updated_at
                            ))),
                    ),
            )
            .into_any_element()
    }
}
impl Focusable for NotificationsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for NotificationsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Only local animation state is sampled here. Requests originate in
        // construction or explicit controls; no request is issued by render.
        if !self.controller.verify() && (self.snapshot.loaded || self.snapshot.busy.is_some()) {
            self.controller.cancel();
            self.sync(cx);
        }
        let now = self.clock.elapsed().as_millis() as u64;
        let reduced = cx.reduce_motion();
        if self.motion.tick(Instant::now(), reduced) {
            window.request_animation_frame();
        }
        if !cx.has_active_drag() {
            self.bars.settle(&self.motion, now, reduced);
        }
        let opacity = self.bars.opacity(now);
        if self.bars.is_animating() {
            window.request_animation_frame();
        }
        let colors = cx.theme().colors.clone();
        let snapshot = &self.snapshot;
        let busy = snapshot.busy.is_some();
        let summary = if snapshot.loaded {
            format!("{} loaded", snapshot.items.len())
        } else {
            String::new()
        };
        let cooling = self
            .cooldown_until
            .is_some_and(|deadline| deadline > Instant::now());
        let refresh = IconButton::new("notifications-refresh", IconName::RotateCw)
            .size(ControlSize::Sm)
            .disabled(busy || cooling)
            .tooltip(if cooling {
                "Refresh available after the server cooldown"
            } else {
                "Refresh unread notifications"
            })
            .on_click({
                let weak = cx.entity().downgrade();
                move |_, _, cx| {
                    let _ = weak.update(cx, |view, cx| view.request(RequestKind::Refresh, cx));
                }
            });
        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(cx.theme().text_size(TextSize::Lg))
                    .child("Notifications"),
            )
            .child(
                div()
                    .text_size(cx.theme().text_size(TextSize::Xs))
                    .text_color(colors.fg_subtle)
                    .child(summary),
            )
            .child(if busy {
                div()
                    .child(Spinner::new("notifications-loading").size(IconSize::Sm))
                    .into_any_element()
            } else {
                refresh.into_any_element()
            });
        let error = snapshot.error.as_ref().map(|error| {
            let tooltip = format!("{} {}", error, error.recovery());
            let kind = if snapshot.failed_request == Some(RequestKind::More) {
                RequestKind::More
            } else {
                RequestKind::Refresh
            };
            IconButton::new("notifications-error", IconName::CircleAlert)
                .size(ControlSize::Sm)
                .disabled(busy)
                .tooltip(tooltip)
                .on_click({
                    let weak = cx.entity().downgrade();
                    move |_, _, cx| {
                        let _ = weak.update(cx, |view, cx| view.request(kind, cx));
                    }
                })
                .into_any_element()
        });
        let partial = !snapshot.partial.is_empty();
        let empty = snapshot.is_empty();
        let no_rows = snapshot.items.is_empty();
        let has_more = snapshot.has_more;
        let list = gpui::list(
            self.list.clone(),
            cx.processor(|view, index, _, cx| {
                view.snapshot
                    .items
                    .get(index)
                    .cloned()
                    .map(|item| view.row(&item, index, cx))
                    .unwrap_or_else(|| div().into_any_element())
            }),
        )
        .size_full();
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
                name: "notifications-y",
                axis: Axis::Vertical,
            },
            placement: scroll::Placement::overlay(4.0, 8.0),
            style: scroll::Style::DEFAULT,
            opacity,
            on_drag,
        }
        .render(cx);
        let viewport = div()
            .id("notifications-rows")
            .debug_selector(|| "notifications-rows".into())
            .track_focus(&self.focus)
            .role(gpui::Role::List)
            .aria_label(
                "Unread notifications. Arrow keys select; Enter opens the selected repository.",
            )
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .child(if empty {
                div()
                    .p_3()
                    .text_color(colors.fg_subtle)
                    .child("No unread notifications")
                    .into_any_element()
            } else if no_rows {
                div().into_any_element()
            } else {
                list.into_any_element()
            })
            .child(scroll::wheel_overlay_with(
                "notifications-wheel",
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
            .on_key_down(cx.listener(Self::key_down));
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .bg(colors.bg)
            .text_color(colors.fg)
            .child(visible_lifetime(cx))
            .child(header)
            .when(error.is_some() || partial, |element| {
                element.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .children(error)
                        .when(partial, |row| {
                            row.child(
                                div()
                                    .id("notifications-partial")
                                    .debug_selector(|| "notifications-partial".into())
                                    .text_size(cx.theme().text_size(TextSize::Xs))
                                    .text_color(colors.fg_subtle)
                                    .tooltip(Tooltip::text(format!(
                                        "Some results could not be loaded: {:?}. Refresh to retry.",
                                        self.snapshot.partial
                                    )))
                                    .child("Partial results"),
                            )
                        }),
                )
            })
            .child(viewport)
            .when(has_more, |element| {
                element.child(
                    div().flex_none().px_3().py_2().child(
                        Button::new("notifications-more", "Load more")
                            .size(ControlSize::Sm)
                            .disabled(busy)
                            .on_click({
                                let weak = cx.entity().downgrade();
                                move |_, _, cx| {
                                    let _ = weak
                                        .update(cx, |view, cx| view.request(RequestKind::More, cx));
                                }
                            }),
                    ),
                )
            })
    }
}
fn visible_lifetime(cx: &Context<NotificationsView>) -> AnyElement {
    let weak = cx.entity().downgrade();
    let key = format!("notifications-visible-{}", cx.entity_id().as_u64());
    gpui::canvas(
        move |_, window, cx| {
            let weak = weak.clone();
            window.use_keyed_state(key.clone(), cx, move |_, lease| {
                lease
                    .on_release(move |_, cx| {
                        let _ = weak.update(cx, |view, cx| view.leave(cx));
                    })
                    .detach();
            });
        },
        |_, _, _, _| {},
    )
    .size_0()
    .into_any_element()
}
fn reason_label(reason: &str) -> &str {
    match reason {
        "subscribed" => "Watching",
        "mention" => "Mentioned",
        "team_mention" => "Team mentioned",
        "assign" => "Assigned",
        "review_requested" => "Review requested",
        "author" => "Author",
        "comment" => "Commented",
        "ci_activity" => "CI activity",
        "manual" => "Subscribed",
        "state_change" => "State changed",
        _ => "Other notification",
    }
}
