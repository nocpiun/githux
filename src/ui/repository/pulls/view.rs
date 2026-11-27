//! The Pull requests tab's own content area.
//!
//! The shared work-item row presents every entry, exactly like the repository Issues list, so
//! the two tabs cannot drift. The page owns querying, paging and delivery; this file only draws
//! the state the model already decided (loading, real empty, partial, failed with one retry) and
//! forwards row activation and keyboard selection to the page.

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
    StatefulInteractiveElement, Styled, Window, div, px, uniform_list,
};

use super::super::RepositoryPage;
use crate::domain::repository_pulls::{PullFilter, PullState, PullSummary};
use crate::ui::{
    compact_status, page_layout, scroll,
    work_item_row::{self, Glyph, RepositoryLink, Row, Selectors, Tone},
};

impl RepositoryPage {
    pub(in super::super) fn render_pulls(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let filter = self.pulls.filter();
        let busy = self.pulls.busy();
        let controls = Select::new(
            "repository-pull-state",
            [
                Choice::new("open", PullFilter::Open.label()),
                Choice::new("closed", PullFilter::Closed.label()),
            ],
        )
        .selected(filter.api_state())
        .size(ControlSize::Sm)
        .on_change({
            let page = cx.weak_entity();
            move |value, _, cx| {
                let filter = if value == "closed" {
                    PullFilter::Closed
                } else {
                    PullFilter::Open
                };
                let _ = page.update(cx, |page, cx| page.filter_pulls(filter, cx));
            }
        });
        let mut header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .pb_2()
            .child(div().w(px(104.0)).child(controls));

        let failed = self.pulls_failed();
        let failed_now = failed.is_some();
        if let Some((first, error)) = failed {
            // A stored prefix is only labelled "Cached" after the network read actually
            // failed; a cache-first paint that is still being verified is not a fallback yet.
            if self.pulls_historical() && !busy {
                header = header.child(
                    div()
                        .debug_selector(|| "repository-pulls-cached".into())
                        .child(crate::ui::badge::Badge::new("Cached").outline()),
                );
            }
            header = header.child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(cx.theme().colors.fg_muted)
                    .child(error),
            );
            if !busy {
                header = header.child(
                    div()
                        .debug_selector(|| "repository-pulls-retry".into())
                        .child(
                            Button::new("repository-pulls-retry", "Retry")
                                .size(ControlSize::Sm)
                                .on_click(cx.listener(move |page, _, _, cx| {
                                    if first {
                                        page.request_pulls(true, cx);
                                    } else {
                                        page.retry_pulls(cx);
                                    }
                                })),
                        ),
                );
            }
        }
        if self.pulls_partial_label().is_some() && !busy {
            header = header.child(crate::ui::badge::Badge::new("Partial").outline());
        }
        if busy
            || (self.pulls_model().is_none() && self.details().is_none() && self.error().is_none())
        {
            header = header.child(compact_status::loading(
                "repository-pulls-loading",
                "Loading pull requests…",
                cx,
            ));
        }

        let mut body = div()
            .id("repository-pulls")
            .debug_selector(|| "repository-pulls".into())
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_hidden()
            .px(px(page_layout::PAGE_GUTTER))
            .pt(px(page_layout::PAGE_TOP))
            .child(header);

        // A repository summary failure means nothing can be listed: the page-level retry
        // revalidates the repository rather than pretending the list is empty.
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
                        Button::new("repository-pulls-retry-repository", "Retry")
                            .size(ControlSize::Sm)
                            .on_click(cx.listener(|page, _, _, cx| page.refresh_repository(cx))),
                    ),
                );
        }

        let rows = self.pulls_model().map_or(0, |model| model.rows().len());
        if rows == 0 {
            // Only a confirmed complete and empty network result is drawn as "No pull
            // requests". A stored empty prefix, a failure, a partial page and a page still
            // loading each keep their own honest state.
            if let Some(model) = self.pulls_model()
                && (model.is_empty_result() || model.is_partial() || model.is_cached_empty())
            {
                let message = if model.is_cached_empty() && !model.is_partial() {
                    "No cached pull requests"
                } else if model.is_partial() {
                    "No pull requests could be confirmed"
                } else {
                    "No pull requests"
                };
                body = body.child(
                    div()
                        .debug_selector(|| "repository-pulls-empty".into())
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(cx.theme().colors.fg_subtle)
                        .child(message),
                );
            }
        } else {
            body = body.child(self.pull_list(window, cx));
        }

        // A stored first page has a bounded prefix: its real continuation is not cached, so
        // "Load more" would silently start a page from a different first-page generation.
        let more = self
            .pulls_model()
            .is_some_and(|model| model.next().is_some() && !model.is_historical())
            && !busy
            && !failed_now;
        if more {
            body = body.child(
                div()
                    .debug_selector(|| "repository-pulls-more".into())
                    .flex()
                    .flex_none()
                    .justify_center()
                    .pb_2()
                    .child(
                        Button::new("repository-pulls-more", "Load more")
                            .variant(ButtonVariant::Secondary)
                            .size(ControlSize::Sm)
                            .on_click(cx.listener(|page, _, _, cx| page.request_pulls(false, cx))),
                    ),
            );
        }
        body.into_any_element()
    }

    fn pull_list(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let last = self.pulls_model().map_or(0, |model| model.rows().len());
        let filter = self.pulls.filter();
        let selected = self.pulls.selected;
        let now = SystemTime::now();
        let list = uniform_list(
            "repository-pull-list",
            last,
            cx.processor(
                move |page: &mut RepositoryPage, range: std::ops::Range<usize>, _, cx| {
                    let end = range.end;
                    range
                        .map(|index| {
                            let row = pull_row(
                                page.pulls
                                    .model
                                    .as_ref()
                                    .expect("the list is drawn only with a model")
                                    .rows()
                                    .get(index)
                                    .expect("a visible index is a loaded row"),
                                filter,
                                selected == Some(index),
                                now,
                                cx,
                            );
                            let trailing = (index + 1 == end && end < last).then(|| {
                                pull_row(
                                    page.pulls.model.as_ref().unwrap().rows().get(end).unwrap(),
                                    filter,
                                    selected == Some(end),
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
        .track_scroll(&self.pulls.list_scroll)
        .size_full()
        .pb(px(page_layout::PAGE_BOTTOM));
        let timestamp = self.now_ms();
        let motion = self.pulls.motion.clone();
        if self.pulls.bars.dragging().is_some() && !cx.has_active_drag() {
            self.pulls
                .bars
                .settle(&motion, timestamp, cx.reduce_motion());
        }
        let opacity = self.pulls.bars.opacity(timestamp);
        if motion.tick(Instant::now(), cx.reduce_motion()) || self.pulls.bars.is_animating() {
            window.request_animation_frame();
        }
        let weak = cx.weak_entity();
        let bar = scroll::Bar {
            target: &motion,
            id: scroll::BarId {
                owner: cx.entity_id(),
                name: "repository-pulls-scrollbar",
                axis: Axis::Vertical,
            },
            placement: scroll::Placement::overlay(2.5, 8.0),
            style: scroll::Style::DEFAULT,
            opacity,
            on_drag: Rc::new(move |axis, dragging, _, cx| {
                let _ = weak.update(cx, |page, cx| {
                    let timestamp = page.now_ms();
                    if page.pulls.bars.drag(
                        &page.pulls.motion,
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
            .id("repository-pulls-viewport")
            .debug_selector(|| "repository-pulls-viewport".into())
            .relative()
            .w_full()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .track_focus(&self.pulls.focus)
            .on_key_down(cx.listener(|page, event, window, cx| page.pull_key(event, window, cx)))
            .on_hover(cx.listener(|page, hovered, _, cx| {
                let now = page.now_ms();
                if page.pulls.bars.hover(*hovered, now, cx.reduce_motion()) {
                    cx.notify();
                }
            }))
            .child(list)
            .child(scroll::wheel_overlay(
                "repository-pulls",
                Axis::Vertical,
                motion,
            ))
            .child(bar)
            .into_any_element()
    }

    fn pull_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.pulls.focus.is_focused(window) || event.keystroke.modifiers.modified() {
            return;
        }
        let Some(last) = self
            .pulls_model()
            .map(|model| model.rows().len())
            .filter(|len| *len > 0)
            .map(|len| len - 1)
        else {
            return;
        };
        // The stored index may predate a shorter result set (a filter change or a refresh).
        // Navigation clamps a stale index into the current rows; Enter rejects it instead of
        // opening whatever row now sits at that index.
        let clamped = self.pulls.selected.map(|index| index.min(last));
        let index = match event.keystroke.key.as_str() {
            "up" => clamped.unwrap_or(0).saturating_sub(1),
            "down" => clamped.map_or(0, |index| (index + 1).min(last)),
            "home" => 0,
            "end" => last,
            "enter" if !event.is_held => {
                // Enter needs the stored index itself: a clamped one would silently open
                // whatever row now sits at the end of a shorter list.
                let Some(index) = self.pulls.selected.filter(|index| *index <= last) else {
                    self.pulls.selected = None;
                    cx.stop_propagation();
                    cx.notify();
                    return;
                };
                let filter = self.pulls.filter();
                let Some(number) = self
                    .pulls_model()
                    .and_then(|model| model.rows().get(index))
                    .map(|row| row.number)
                else {
                    return;
                };
                cx.stop_propagation();
                self.open_pull(filter, number, window, cx);
                return;
            }
            _ => return,
        };
        cx.stop_propagation();
        self.pulls.selected = Some(index);
        self.pulls.motion.cancel();
        self.pulls
            .list_scroll
            .scroll_to_item(index, gpui::ScrollStrategy::Nearest);
        cx.notify();
    }
}

fn pull_row(
    item: &PullSummary,
    filter: PullFilter,
    selected: bool,
    now: SystemTime,
    cx: &Context<RepositoryPage>,
) -> AnyElement {
    let number = item.number;
    let tone = pull_tone(item.state);
    let weak = cx.weak_entity();
    let open = Rc::new(move |window: &mut Window, cx: &mut App| {
        let _ = weak.update(cx, |page, cx| page.open_pull(filter, number, window, cx));
    });
    let open_title = open.clone();
    let display = item.row(now);
    let branch = format!("{} ← {}", display.base, display.head);
    work_item_row::render(
        Row {
            selectors: Selectors {
                row: format!("repository-pull-{number}"),
                title: format!("repository-pull-title-{number}"),
                title_line: None,
                icon: None,
                metadata: None,
                number: Some(format!("repository-pull-number-{number}")),
                repository: None,
                owner: None,
                updated: None,
            },
            glyph: Glyph::PullRequest,
            tone,
            // A draft is open-coloured but must read "Draft" visibly, not only in aria.
            state_label: Some(display.state_label),
            number,
            title: display.title.clone(),
            title_tooltip: format!("{}\n{branch}", display.title),
            repository: RepositoryLink::Omitted,
            updated_label: if display.state == PullState::Merged {
                let who = display.author.as_deref().unwrap_or("unknown author");
                format!("{} · merged", who)
            } else {
                let who = display.author.as_deref().unwrap_or("unknown author");
                format!("{} · {}", who, display.updated)
            },
            updated_tooltip: format!("{branch}\nUpdated {}", display.updated_tooltip),
            aria: format!(
                "Pull request #{number} {}, {}, {branch}, updated {}",
                display.title, display.state_label, display.updated_tooltip
            ),
            selected,
            show_selected_bar: false,
            on_row_click: Some(open.clone()),
            // The leading icon and #number address the same pull request as the title, so a
            // click anywhere on those identity bits opens it; they stay out of the tab order.
            on_identity_click: Some(open),
            on_title_click: Some(Rc::new(move |_, window, cx| open_title(window, cx))),
        },
        cx,
    )
}

/// The row tone for a PR. Draft is an open change not yet ready for review, so it keeps the
/// open colour and states "Draft" in the metadata; merged and closed stay distinct.
fn pull_tone(state: PullState) -> Tone {
    match state {
        PullState::Draft | PullState::Open => Tone::Open,
        PullState::Merged => Tone::Merged,
        PullState::Closed => Tone::Closed,
    }
}
