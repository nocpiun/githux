//! Presentation helpers for [`IssueDetailView`].

use std::rc::Rc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use ely_core::{
    buttons::{Button, IconButton},
    data_display::Tone,
    motion::Spinner,
    primitives::{Icon, IconName},
    theme::{ActiveTheme, AvatarSize, ControlSize, IconSize, Radius, TextSize},
};
use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, Role,
    SharedString, StatefulInteractiveElement, Styled, div, prelude::*, px,
};

use crate::domain::error::GitHubError;
use crate::domain::issue_detail::{
    IssueComment, IssueDetail, IssueDetailError, IssueState, IssueStateReason,
};
use crate::ui::avatar::AvatarSlot;
use crate::ui::badge::Badge;
use crate::ui::page_layout::{PAGE_BOTTOM, PAGE_GUTTER, PAGE_TOP};
use crate::ui::relative_time;
use crate::ui::scroll::{self, Bar, BarId, Placement, Style};
use crate::ui::tooltip::ui_tooltip;

use super::controller::CommentsRequestMode;
use super::{IssueDetailAction, IssueDetailView};

impl IssueDetailView {
    pub(super) fn render_page(
        &self,
        current: bool,
        opacity: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let text_sm = cx.theme().text_size(TextSize::Sm);
        let colors = cx.theme().colors.clone();

        if !current {
            return div()
                .debug_selector(|| "issue-detail-account-changed".into())
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(colors.surface)
                .text_color(colors.fg_subtle)
                .text_size(text_sm)
                .child("Account changed")
                .into_any_element();
        }

        let content: AnyElement = if let Some(issue) = &self.controller.issue {
            self.render_loaded_issue(issue, cx)
        } else if self.controller.issue_busy {
            div()
                .debug_selector(|| "issue-detail-loading".into())
                .w_full()
                .py_12()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .child(Spinner::new("issue-detail-spinner").size(IconSize::Md))
                .child(
                    div()
                        .text_size(text_sm)
                        .text_color(colors.fg_muted)
                        .child(format!(
                            "Loading {}…",
                            self.controller.locator.display_slug()
                        )),
                )
                .into_any_element()
        } else if let Some(error) = &self.controller.issue_error {
            self.render_issue_error(error, cx)
        } else {
            div()
                .debug_selector(|| "issue-detail-unavailable".into())
                .w_full()
                .py_12()
                .flex()
                .items_center()
                .justify_center()
                .text_color(colors.fg_subtle)
                .child("Issue could not be loaded.")
                .into_any_element()
        };

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
        let bar = Bar {
            target: &self.motion,
            id: BarId {
                owner: cx.entity_id(),
                name: "issue-detail-scrollbar",
                axis: gpui::Axis::Vertical,
            },
            placement: Placement::overlay(1.5, scroll::LIST_BAR_LANE),
            style: Style::DEFAULT,
            opacity,
            on_drag,
        }
        .render(cx);

        let weak = cx.entity().downgrade();
        let lease_key = format!("issue-detail-visible-{}", cx.entity_id().as_u64());
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
            .id("issue-detail-root")
            .debug_selector(|| "issue-detail".into())
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(colors.surface)
            .text_color(colors.fg)
            .text_size(text_sm)
            .child(
                div()
                    .id("issue-detail-viewport")
                    .debug_selector(|| "issue-detail-viewport".into())
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(
                        div()
                            .w_full()
                            .max_w(px(920.0))
                            .mx_auto()
                            .px(px(PAGE_GUTTER + 8.0))
                            .pt(px(PAGE_TOP))
                            .pb(px(PAGE_BOTTOM + 24.0))
                            .child(content),
                    ),
            )
            .child(scroll::wheel_overlay_with(
                "issue-detail-wheel",
                scroll::WheelAxes::Vertical,
                self.motion.clone(),
                Instant::now,
            ))
            .child(bar)
            .child(lease)
            .on_hover(cx.listener(|view, hovered, _, cx| {
                if view.bars.hover(
                    *hovered,
                    view.clock.elapsed().as_millis() as u64,
                    cx.reduce_motion(),
                ) {
                    cx.notify();
                }
            }))
            .into_any_element()
    }

    fn render_loaded_issue(&self, issue: &IssueDetail, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors.clone();
        let radius = cx.theme().radius(Radius::Md);
        let mono = cx.theme().mono_family.clone();
        let text_sm = cx.theme().text_size(TextSize::Sm);
        let text_xl = cx.theme().text_size(TextSize::Xl);
        let context = self.controller.context().clone();

        // Top bar: repository link + #number + state badge + refresh/provenance controls.
        let repo_label = format!("{}/{}", issue.repository.owner(), issue.repository.name());
        let repo_locator = issue.repository.clone();
        let repo_context = context.clone();
        let repo_link = div()
            .id("issue-detail-repo-link")
            .debug_selector(|| "issue-detail-repo-link".into())
            .role(Role::Link)
            .tab_index(0)
            .cursor_pointer()
            .text_size(text_sm)
            .text_color(colors.fg_muted)
            .hover(|style| style.underline().text_color(colors.fg))
            .on_click(cx.listener(move |view, _, window, cx| {
                view.emit(
                    IssueDetailAction::Repository {
                        context: repo_context.clone(),
                        repository: repo_locator.clone(),
                    },
                    window,
                    cx,
                );
            }))
            .child(repo_label);

        let number_badge = div()
            .debug_selector(|| "issue-detail-number".into())
            .font_family(mono)
            .text_size(text_sm)
            .text_color(colors.fg_subtle)
            .child(format!("#{}", issue.number));

        let (state_tone, state_label) = match (&issue.state, &issue.state_reason) {
            (IssueState::Open, _) => (Tone::Success, "Open".to_string()),
            (IssueState::Closed, Some(IssueStateReason::NotPlanned)) => {
                (Tone::Neutral, "Closed · Not planned".to_string())
            }
            (IssueState::Closed, Some(IssueStateReason::Duplicate)) => {
                (Tone::Neutral, "Closed · Duplicate".to_string())
            }
            (IssueState::Closed, _) => (Tone::Danger, "Closed".to_string()),
            (IssueState::Unknown(raw), _) => (
                Tone::Neutral,
                if raw.is_empty() {
                    "Unknown".to_string()
                } else {
                    raw.clone()
                },
            ),
        };

        let state_badge = div()
            .debug_selector(|| "issue-detail-state".into())
            .child(Badge::new(state_label).tone(state_tone));

        let busy = self.controller.is_busy();
        let refresh_control: AnyElement = if busy {
            div()
                .debug_selector(|| "issue-detail-refreshing".into())
                .size(px(28.0))
                .flex()
                .items_center()
                .justify_center()
                .child(Spinner::new("issue-detail-refresh-spinner").size(IconSize::Sm))
                .into_any_element()
        } else {
            IconButton::new("issue-detail-refresh", IconName::RotateCw)
                .size(ControlSize::Sm)
                .tooltip("Refresh issue")
                .on_click(cx.listener(|view, _, _, cx| {
                    view.refresh(cx);
                }))
                .into_any_element()
        };

        let mut meta_row = div().flex().items_center().justify_between().gap_2().child(
            div()
                .flex()
                .items_center()
                .flex_wrap()
                .gap_2()
                .child(state_badge)
                .child(repo_link)
                .child(number_badge),
        );

        let mut right_controls = div().flex().items_center().gap_2();
        if self.controller.issue_origin.revalidation_failed() {
            let label = self
                .controller
                .issue_origin
                .verified_at_ms()
                .and_then(|ms| UNIX_EPOCH.checked_add(Duration::from_millis(ms.max(0) as u64)))
                .and_then(|verified| self.now.duration_since(verified).ok())
                .map(|elapsed| {
                    let secs = elapsed.as_secs();
                    let rel = if secs < 60 {
                        "Just now".to_string()
                    } else if secs < 3600 {
                        let m = secs / 60;
                        format!("{m} minute{} ago", if m == 1 { "" } else { "s" })
                    } else if secs < 86400 {
                        let h = secs / 3600;
                        format!("{h} hour{} ago", if h == 1 { "" } else { "s" })
                    } else {
                        let d = secs / 86400;
                        format!("{d} day{} ago", if d == 1 { "" } else { "s" })
                    };
                    format!("Cached · {rel}")
                })
                .unwrap_or_else(|| "Cached".to_string());
            right_controls = right_controls
                .child(
                    div()
                        .id("issue-detail-cached-badge")
                        .debug_selector(|| "issue-detail-cached-badge".into())
                        .tooltip(ui_tooltip(
                            "Showing cached snapshot because online refresh failed.",
                        ))
                        .child(Badge::new(label).tone(Tone::Neutral)),
                )
                .child(
                    IconButton::new("issue-detail-cache-retry", IconName::RotateCw)
                        .size(ControlSize::Sm)
                        .tooltip("Retry refresh")
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.request_issue(false, cx);
                        })),
                );
        } else if let Some(error) = &self.controller.issue_error {
            right_controls = right_controls.child(
                div()
                    .id("issue-detail-inline-error")
                    .debug_selector(|| "issue-detail-inline-error".into())
                    .flex()
                    .items_center()
                    .gap_1()
                    .tooltip(ui_tooltip(error_summary(error)))
                    .child(
                        Icon::new(IconName::TriangleAlert)
                            .size(IconSize::Sm)
                            .color(colors.warning),
                    ),
            );
        }
        right_controls = right_controls.child(refresh_control);
        meta_row = meta_row.child(right_controls);

        // Title
        let title = div()
            .debug_selector(|| "issue-detail-title".into())
            .text_size(text_xl)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(colors.fg)
            .child(issue.title.clone());

        // Author + opened timestamp + comments count
        let author_line = self.render_author_line(
            "issue-detail-author",
            issue.author.as_ref(),
            "opened",
            &issue.created_at,
            Some(issue.comments_count),
            cx,
        );

        // Labels (when present)
        let labels_row = (!issue.labels.is_empty()).then(|| {
            let mut row = div()
                .debug_selector(|| "issue-detail-labels".into())
                .flex()
                .items_center()
                .flex_wrap()
                .gap_1p5();
            for label in &issue.labels {
                row = row.child(Badge::new(label.name.clone()).tone(Tone::Neutral));
            }
            row
        });

        // Issue Body (Markdown Flow or explicit "No description provided.")
        let body_section: AnyElement = if let Some(reader) = &self.body_reader {
            div()
                .debug_selector(|| "issue-detail-body".into())
                .w_full()
                .p_4()
                .rounded(radius)
                .border_1()
                .border_color(colors.border)
                .child(reader.view.clone())
                .into_any_element()
        } else {
            div()
                .debug_selector(|| "issue-detail-empty-body".into())
                .w_full()
                .px_4()
                .py_3()
                .rounded(radius)
                .border_1()
                .border_color(colors.border)
                .text_size(text_sm)
                .text_color(colors.fg_subtle)
                .italic()
                .child("No description provided.")
                .into_any_element()
        };

        // Comments section (independent from issue body)
        let comments_section = self.render_comments_section(issue, cx);

        div()
            .flex()
            .flex_col()
            .gap_4()
            .min_w_0()
            .child(
                div()
                    .debug_selector(|| "issue-detail-header".into())
                    .flex()
                    .flex_col()
                    .gap_2()
                    .pb_3()
                    .border_b_1()
                    .border_color(colors.border)
                    .child(meta_row)
                    .child(title)
                    .child(author_line)
                    .children(labels_row),
            )
            .child(body_section)
            .child(comments_section)
            .into_any_element()
    }

    fn render_author_line(
        &self,
        id_prefix: &str,
        author: Option<&crate::domain::issue_detail::IssueAuthor>,
        verb: &str,
        timestamp: &str,
        comments_count: Option<u64>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let colors = theme.colors.clone();
        let rel = relative_time::relative(Some(timestamp), self.now);
        let exact = timestamp.to_string();

        let mut row = div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap_2()
            .text_size(theme.text_size(TextSize::Xs))
            .text_color(colors.fg_muted);

        if let Some(author) = author {
            let avatar_el = self.avatars.element(
                &format!("{id_prefix}-avatar"),
                &format!("{id_prefix}-avatar"),
                &author.login,
                Self::avatar_key(author),
                AvatarSlot::Size(AvatarSize::Xs),
                cx,
            );
            let login = author.login.clone();
            let login_text = author.login.clone();
            let context = self.controller.context().clone();
            let selector = format!("{id_prefix}-login");
            let author_link = div()
                .id(SharedString::from(selector.clone()))
                .debug_selector(move || selector.clone())
                .role(Role::Link)
                .tab_index(0)
                .cursor_pointer()
                .font_weight(FontWeight::MEDIUM)
                .text_color(colors.fg)
                .hover(|style| style.underline())
                .on_click(cx.listener(move |view, _, window, cx| {
                    view.emit(
                        IssueDetailAction::Author {
                            context: context.clone(),
                            login: login.clone(),
                        },
                        window,
                        cx,
                    );
                }))
                .child(login_text);
            row = row.child(avatar_el).child(author_link);
        } else {
            row = row.child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.fg_subtle)
                    .child("ghost"),
            );
        }

        let time_id = format!("{id_prefix}-time");
        row = row.child(verb.to_string()).child(
            div()
                .id(SharedString::from(time_id))
                .text_color(colors.fg_subtle)
                .tooltip(ui_tooltip(exact))
                .child(rel),
        );

        if let Some(count) = comments_count {
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .pl_2()
                    .text_color(colors.fg_subtle)
                    .child(
                        Icon::new(IconName::MessageSquare)
                            .size(IconSize::Xs)
                            .color(colors.fg_subtle),
                    )
                    .child(if count == 1 {
                        "1 comment".to_string()
                    } else {
                        format!("{count} comments")
                    }),
            );
        }

        row.into_any_element()
    }

    fn render_comments_section(&self, issue: &IssueDetail, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors.clone();
        let radius = cx.theme().radius(Radius::Md);
        let text_base = cx.theme().text_size(TextSize::Base);
        let text_sm = cx.theme().text_size(TextSize::Sm);

        let loaded_count = self.controller.comments.len();
        let heading_text = if self.controller.comments_loaded {
            if loaded_count as u64 == issue.comments_count || issue.comments_count == 0 {
                format!("Comments ({loaded_count})")
            } else {
                format!("Comments ({loaded_count} of {})", issue.comments_count)
            }
        } else {
            format!("Comments ({})", issue.comments_count)
        };

        let mut heading = div().flex().items_center().gap_2().child(
            div()
                .debug_selector(|| "issue-detail-comments-heading".into())
                .text_size(text_base)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.fg)
                .child(heading_text),
        );
        if self.controller.comments_busy == Some(CommentsRequestMode::Replace) {
            heading = heading.child(Spinner::new("issue-comments-loading").size(IconSize::Xs));
        }

        let mut section = div()
            .debug_selector(|| "issue-detail-comments".into())
            .flex()
            .flex_col()
            .gap_3()
            .pt_2()
            .child(heading);

        // First-page error when no comments are currently displayed.
        if self.controller.comments.is_empty()
            && let Some(error) = &self.controller.comments_error
        {
            let retry_mode = self
                .controller
                .comments_failed_mode
                .unwrap_or(CommentsRequestMode::Replace);
            section = section.child(
                div()
                    .debug_selector(|| "issue-detail-comments-error".into())
                    .w_full()
                    .px_4()
                    .py_3()
                    .rounded(radius)
                    .border_1()
                    .border_color(colors.border)
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .child(
                                Icon::new(IconName::CircleAlert)
                                    .size(IconSize::Sm)
                                    .color(colors.danger),
                            )
                            .child(div().text_size(text_sm).text_color(colors.fg_muted).child(
                                format!("Failed to load comments: {}", error_summary(error)),
                            )),
                    )
                    .child(
                        Button::new("issue-detail-comments-retry", "Retry")
                            .size(ControlSize::Sm)
                            .disabled(self.controller.comments_busy.is_some())
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.request_comments(retry_mode, false, cx);
                            })),
                    ),
            );
            return section.into_any_element();
        }

        // Initial loading state (no cached comments yet).
        if !self.controller.comments_loaded && self.controller.comments_busy.is_some() {
            section = section.child(
                div()
                    .debug_selector(|| "issue-detail-comments-loading".into())
                    .py_4()
                    .text_size(text_sm)
                    .text_color(colors.fg_subtle)
                    .child("Loading comments…"),
            );
            return section.into_any_element();
        }

        // Confirmed empty comments (distinct from loading and error).
        if self.controller.comments_confirmed_empty() {
            section = section.child(
                div()
                    .debug_selector(|| "issue-detail-empty-comments".into())
                    .w_full()
                    .px_4()
                    .py_3()
                    .rounded(radius)
                    .border_1()
                    .border_color(colors.border)
                    .text_size(text_sm)
                    .text_color(colors.fg_subtle)
                    .child("No comments yet."),
            );
            return section.into_any_element();
        }

        // Render loaded comments.
        for comment in &self.controller.comments {
            section = section.child(self.render_comment_card(comment, cx));
        }

        // Append/refresh error while keeping already-loaded comments visible.
        if let Some(error) = &self.controller.comments_error {
            let retry_mode = self
                .controller
                .comments_failed_mode
                .unwrap_or(CommentsRequestMode::Append);
            section = section.child(
                div()
                    .debug_selector(|| "issue-detail-comments-append-error".into())
                    .w_full()
                    .px_4()
                    .py_3()
                    .rounded(radius)
                    .border_1()
                    .border_color(colors.border)
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .child(
                                Icon::new(IconName::CircleAlert)
                                    .size(IconSize::Sm)
                                    .color(colors.danger),
                            )
                            .child(div().text_size(text_sm).text_color(colors.fg_muted).child(
                                format!("Failed to load more comments: {}", error_summary(error)),
                            )),
                    )
                    .child(
                        Button::new("issue-detail-comments-append-retry", "Retry")
                            .size(ControlSize::Sm)
                            .disabled(self.controller.comments_busy.is_some())
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.request_comments(retry_mode, false, cx);
                            })),
                    ),
            );
        }

        // Load more button when a next page cursor exists.
        if self.controller.comments_next.is_some() {
            let appending = self.controller.comments_busy == Some(CommentsRequestMode::Append);
            section = section.child(
                div()
                    .debug_selector(|| "issue-detail-comments-footer".into())
                    .pt_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new(
                            "issue-detail-load-more",
                            if appending {
                                "Loading more…"
                            } else {
                                "Load more comments"
                            },
                        )
                        .size(ControlSize::Sm)
                        .disabled(self.controller.comments_busy.is_some())
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.request_comments(CommentsRequestMode::Append, false, cx);
                        })),
                    )
                    .when(appending, |footer| {
                        footer
                            .child(Spinner::new("issue-comments-append-spinner").size(IconSize::Xs))
                    }),
            );
        }

        section.into_any_element()
    }

    fn render_comment_card(&self, comment: &IssueComment, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors.clone();
        let radius = cx.theme().radius(Radius::Md);
        let text_sm = cx.theme().text_size(TextSize::Sm);
        let id = comment.id;

        let mut header = div()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(colors.border)
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .child(self.render_author_line(
                &format!("issue-comment-{id}"),
                comment.author.as_ref(),
                "commented",
                &comment.created_at,
                None,
                cx,
            ));

        if let Some(assoc) = &comment.author_association {
            header = header.child(Badge::new(format_association(assoc)).tone(Tone::Neutral));
        }

        let body: AnyElement = if let Some(reader) = self.comment_readers.get(&id) {
            div()
                .px_4()
                .py_3()
                .child(reader.view.clone())
                .into_any_element()
        } else {
            div()
                .px_4()
                .py_3()
                .text_size(text_sm)
                .text_color(colors.fg_subtle)
                .italic()
                .child("Empty comment.")
                .into_any_element()
        };

        div()
            .debug_selector(move || format!("issue-comment-{id}"))
            .w_full()
            .rounded(radius)
            .border_1()
            .border_color(colors.border)
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .into_any_element()
    }

    fn render_issue_error(&self, error: &IssueDetailError, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let colors = theme.colors.clone();
        let (selector, heading, allow_retry) = if error.is_missing() {
            (
                "issue-detail-error-missing",
                "Issue not found or unavailable",
                true,
            )
        } else if matches!(error, IssueDetailError::NotAnIssue) {
            (
                "issue-detail-error-not-an-issue",
                "This item is a pull request, not an issue",
                false,
            )
        } else if error.is_auth_or_permission() {
            (
                "issue-detail-error-permission",
                "Access denied for this issue",
                true,
            )
        } else {
            ("issue-detail-error-failed", "Could not load issue", true)
        };

        div()
            .debug_selector(move || selector.into())
            .w_full()
            .py_12()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                Icon::new(IconName::CircleAlert)
                    .size(IconSize::Lg)
                    .color(colors.danger),
            )
            .child(
                div()
                    .text_size(theme.text_size(TextSize::Base))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.fg)
                    .child(heading),
            )
            .child(
                div()
                    .max_w(px(480.0))
                    .text_align(gpui::TextAlign::Center)
                    .text_size(theme.text_size(TextSize::Sm))
                    .text_color(colors.fg_muted)
                    .child(error_summary(error)),
            )
            .when(allow_retry, |panel| {
                panel.child(
                    Button::new("issue-detail-retry", "Retry")
                        .size(ControlSize::Sm)
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.request_issue(false, cx);
                            view.request_comments(CommentsRequestMode::Replace, false, cx);
                        })),
                )
            })
            .into_any_element()
    }
}

fn format_association(raw: &str) -> String {
    match raw.trim().to_ascii_uppercase().as_str() {
        "OWNER" => "Owner".into(),
        "MEMBER" => "Member".into(),
        "COLLABORATOR" => "Collaborator".into(),
        "CONTRIBUTOR" => "Contributor".into(),
        "FIRST_TIME_CONTRIBUTOR" => "First-time contributor".into(),
        "FIRST_TIMER" => "First-timer".into(),
        "MANNEQUIN" => "Mannequin".into(),
        other => other.to_string(),
    }
}

pub(super) fn error_summary(error: &IssueDetailError) -> String {
    match error {
        IssueDetailError::NotAnIssue => {
            "GitHub returned a pull request for this number instead of an issue.".into()
        }
        IssueDetailError::Gone => {
            "This issue was deleted or issues are disabled in this repository.".into()
        }
        IssueDetailError::GitHub(GitHubError::NotFound) => {
            "The issue does not exist or your current account does not have access to it.".into()
        }
        IssueDetailError::GitHub(GitHubError::Unauthorized) => {
            "Your token was rejected. Re-authenticate or replace the PAT in Settings.".into()
        }
        IssueDetailError::GitHub(GitHubError::Forbidden) => {
            "Your current token does not have permission to read this issue.".into()
        }
        IssueDetailError::GitHub(GitHubError::SsoRequired { .. }) => {
            "Organization SSO authorization is required to read this issue.".into()
        }
        IssueDetailError::GitHub(GitHubError::RateLimited { retry }) => match retry.seconds {
            Some(seconds) => format!("GitHub rate limit reached. Retry after {seconds}s."),
            None => "GitHub rate limit reached. Try again later.".into(),
        },
        IssueDetailError::GitHub(GitHubError::Timeout { seconds }) => {
            format!("Request timed out after {seconds}s.")
        }
        IssueDetailError::GitHub(GitHubError::Network { message }) => {
            format!("Network error: {message}")
        }
        IssueDetailError::GitHub(GitHubError::UnexpectedStatus { status }) => {
            format!("GitHub returned HTTP {status}.")
        }
        other => other.to_string(),
    }
}
