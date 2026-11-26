use std::{
    rc::Rc,
    time::{Instant, SystemTime},
};

use ely_core::{
    buttons::{Button, ButtonVariant},
    forms::{Choice, Select},
    theme::{ActiveTheme, ControlSize},
};
use gpui::{
    AnyElement, App, Axis, Context, InteractiveElement, IntoElement, KeyDownEvent, ParentElement,
    Styled, Window, div, prelude::*, px, uniform_list,
};

use super::super::RepositoryPage;
use crate::github::repository_issues::{IssueFilter, IssueRow};
use crate::ui::{
    compact_status, page_layout, relative_time, scroll,
    work_item_row::{self, Glyph, RepositoryLink, Row, Selectors, Tone},
};

impl RepositoryPage {
    pub(in super::super) fn render_issues(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let filter = self.issues.filter;
        let busy = self.issues.busy();
        let controls = Select::new(
            "repository-issue-state",
            [Choice::new("open", "Open"), Choice::new("closed", "Closed")],
        )
        .selected(filter.value())
        .size(ControlSize::Sm)
        .on_change({
            let page = cx.weak_entity();
            move |value, _, cx| {
                let filter = if value == "closed" {
                    IssueFilter::Closed
                } else {
                    IssueFilter::Open
                };
                let _ = page.update(cx, |page, cx| page.filter_issues(filter, cx));
            }
        });
        let mut header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .pb_2()
            .child(div().w(px(104.0)).child(controls));
        if let Some((first, error)) = &self.issues.failure {
            let first = *first;
            if self.issues.historical && !busy {
                header = header.child(crate::ui::badge::Badge::new("Cached").outline());
            }
            header = header.child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(cx.theme().colors.fg_muted)
                    .child(error.clone()),
            );
            if !busy {
                header = header.child(
                    Button::new("repository-issues-retry", "Retry")
                        .size(ControlSize::Sm)
                        .on_click(
                            cx.listener(move |page, _, _, cx| page.request_issues(first, cx)),
                        ),
                );
            }
        }
        if busy
            || (!self.issues.loaded
                && self.issues.failure.is_none()
                && self.details().is_none()
                && self.error().is_none())
        {
            header = header.child(compact_status::loading(
                "repository-issues-loading",
                "Loading issues…",
                cx,
            ));
        }
        let mut body = div()
            .id("repository-issues")
            .debug_selector(|| "repository-issues".into())
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .px(px(page_layout::PAGE_GUTTER))
            .pt(px(page_layout::PAGE_TOP))
            .child(header);
        if self.details().is_none()
            && let Some(error) = self.error()
        {
            body = body
                .child(
                    div()
                        .text_color(cx.theme().colors.fg_muted)
                        .child(error.title.clone()),
                )
                .child(
                    div().flex().child(
                        Button::new("repository-issues-retry-repository", "Retry")
                            .size(ControlSize::Sm)
                            .on_click(cx.listener(|page, _, _, cx| page.refresh_repository(cx))),
                    ),
                );
        }
        if self.issues.rows.is_empty() {
            if self.issues.loaded
                && self.issues.next.is_none()
                && (self.issues.failure.is_none() || self.issues.historical)
            {
                let message = if self.issues.historical {
                    "No cached issues"
                } else {
                    "No issues"
                };
                body = body.child(
                    div()
                        .debug_selector(|| "repository-issues-empty".into())
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(cx.theme().colors.fg_subtle)
                        .child(message),
                );
            }
        } else {
            body = body.child(self.issue_list(window, cx));
        }
        if self.issues.next.is_some() && !self.issues.historical && self.issues.failure.is_none() {
            body = body.child(
                div()
                    .debug_selector(|| "repository-issues-more".into())
                    .flex()
                    .flex_none()
                    .justify_center()
                    .pb_2()
                    .child(
                        Button::new("repository-issues-more", "Load more")
                            .variant(ButtonVariant::Secondary)
                            .size(ControlSize::Sm)
                            .disabled(busy)
                            .on_click(cx.listener(|page, _, _, cx| page.request_issues(false, cx))),
                    ),
            );
        }
        body.into_any_element()
    }

    fn issue_list(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let last = self.issues.rows.len();
        let now = SystemTime::now();
        let list = uniform_list(
            "repository-issue-list",
            last,
            cx.processor(
                move |page: &mut RepositoryPage, range: std::ops::Range<usize>, _, cx| {
                    let end = range.end;
                    range
                        .map(|index| {
                            let row = issue_row(
                                &page.issues.rows[index],
                                page.issues.filter,
                                page.issues.selected == Some(index),
                                now,
                                cx,
                            );
                            let trailing = (index + 1 == end && end < last).then(|| {
                                issue_row(
                                    &page.issues.rows[end],
                                    page.issues.filter,
                                    page.issues.selected == Some(end),
                                    now,
                                    cx,
                                )
                            });
                            scroll::with_trailing_overdraw(row, trailing)
                        })
                        .collect::<Vec<_>>()
                },
            ),
        )
        .track_scroll(&self.issues.list_scroll)
        .size_full()
        .pb(px(page_layout::PAGE_BOTTOM));
        let timestamp = self.now_ms();
        let motion = self.issues.motion.clone();
        if self.issues.bars.dragging().is_some() && !cx.has_active_drag() {
            self.issues
                .bars
                .settle(&motion, timestamp, cx.reduce_motion());
        }
        let opacity = self.issues.bars.opacity(timestamp);
        if motion.tick(Instant::now(), cx.reduce_motion()) || self.issues.bars.is_animating() {
            window.request_animation_frame();
        }
        let weak = cx.weak_entity();
        let bar = scroll::Bar {
            target: &motion,
            id: scroll::BarId {
                owner: cx.entity_id(),
                name: "repository-issues-scrollbar",
                axis: Axis::Vertical,
            },
            placement: scroll::Placement::overlay(2.5, 8.0),
            style: scroll::Style::DEFAULT,
            opacity,
            on_drag: Rc::new(move |axis, dragging, _, cx| {
                let _ = weak.update(cx, |page, cx| {
                    let timestamp = page.now_ms();
                    if page.issues.bars.drag(
                        &page.issues.motion,
                        axis,
                        dragging,
                        timestamp,
                        cx.reduce_motion(),
                    ) {
                        cx.notify();
                    }
                });
            }),
        }
        .render(cx);
        div()
            .id("repository-issues-viewport")
            .debug_selector(|| "repository-issues-viewport".into())
            .relative()
            .w_full()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .track_focus(&self.issues.focus)
            .on_key_down(cx.listener(|page, event, window, cx| page.issue_key(event, window, cx)))
            .on_hover(cx.listener(|page, hovered, _, cx| {
                let now = page.now_ms();
                if page.issues.bars.hover(*hovered, now, cx.reduce_motion()) {
                    cx.notify();
                }
            }))
            .child(list)
            .child(scroll::wheel_overlay(
                "repository-issues",
                Axis::Vertical,
                motion,
            ))
            .child(bar)
            .into_any_element()
    }

    fn issue_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.issues.focus.is_focused(window) || event.keystroke.modifiers.modified() {
            return;
        }
        let Some(last) = self.issues.rows.len().checked_sub(1) else {
            return;
        };
        let index = match event.keystroke.key.as_str() {
            "up" => self.issues.selected.unwrap_or(0).saturating_sub(1),
            "down" => self
                .issues
                .selected
                .map_or(0, |index| (index + 1).min(last)),
            "home" => 0,
            "end" => last,
            "enter" if !event.is_held => {
                let number = self.issues.rows[self.issues.selected.unwrap_or(0)].number;
                cx.stop_propagation();
                self.open_issue(self.issues.filter, number, window, cx);
                return;
            }
            _ => return,
        };
        cx.stop_propagation();
        self.issues.selected = Some(index);
        self.issues.motion.cancel();
        self.issues
            .list_scroll
            .scroll_to_item(index, gpui::ScrollStrategy::Nearest);
        cx.notify();
    }
}

fn issue_row(
    item: &IssueRow,
    filter: IssueFilter,
    selected: bool,
    now: SystemTime,
    cx: &Context<RepositoryPage>,
) -> AnyElement {
    let number = item.number;
    let tone = Tone::from_state(filter.value());
    let weak = cx.weak_entity();
    let open = Rc::new(move |window: &mut Window, cx: &mut App| {
        let _ = weak.update(cx, |page, cx| page.open_issue(filter, number, window, cx));
    });
    let open_title = open.clone();
    work_item_row::render(
        Row {
            selectors: Selectors {
                row: format!("repository-issue-{number}"),
                title: format!("repository-issue-title-{number}"),
                title_line: None,
                icon: None,
                metadata: None,
                number: None,
                repository: None,
                owner: None,
                updated: None,
            },
            glyph: Glyph::Issue,
            tone,
            number,
            title: item.title.clone(),
            title_tooltip: item.title.clone(),
            repository: RepositoryLink::Omitted,
            updated_label: relative_time::relative(Some(&item.updated_at), now),
            updated_tooltip: item.updated_at.clone(),
            aria: format!(
                "Issue #{number} {}, {}, {}",
                item.title,
                tone.label(),
                item.updated_at
            ),
            selected,
            show_selected_bar: false,
            on_row_click: Some(open),
            on_title_click: Some(Rc::new(move |_, window, cx| open_title(window, cx))),
            // The repository Issues list keeps the title as its single activation target; the
            // icon and number stay decorative here (the Home column opts in separately).
            on_identity_click: None,
        },
        cx,
    )
}
