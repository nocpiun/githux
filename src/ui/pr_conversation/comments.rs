use ely_core::{
    buttons::{Button, ButtonVariant},
    theme::{ActiveTheme, TextSize},
};
use gpui::{App, Context, IntoElement, ParentElement, Styled, Window, div, prelude::*};
use std::{collections::HashSet, time::SystemTime};

use crate::domain::{error::GitHubError, issue_detail::IssueDetailError};
use crate::github::issue_detail::IssueDetailLimits;
use crate::ui::{compact_status, relative_time};

use super::{CommentsRequestMode, ConversationView};

impl ConversationView {
    pub(super) fn request_comments(
        &mut self,
        mode: CommentsRequestMode,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self.summary().is_none() || self.is_refreshing() {
            return;
        }
        let Some(request) = self.comments.begin_comments(mode) else {
            return;
        };
        self.comments.comments_error = None;
        let online = request.clone();
        let work = self
            .executor
            .spawn(async move { online.execute_online(&IssueDetailLimits::default()).await });
        self.comment_task = Some(cx.spawn_in(window, async move |view, cx| {
            let result = match work {
                Ok(work) => work.await.unwrap_or_else(|_| Err(super::request_interrupted().into())),
                Err(_) => Err(super::request_interrupted().into()),
            };
            let _ = cx.update(|window, cx| {
                let _ = view.update(cx, |view, cx| {
                    view.comment_task = None;
                    if matches!(&result, Err(error) if error.is_missing() || error.is_auth_or_permission()) {
                        view.clear_conversation();
                        view.error = Some(match result {
                            Err(IssueDetailError::GitHub(error)) => error,
                            _ => GitHubError::NotFound,
                        });
                    } else if view.comments.finish_comments(&request, result) {
                        view.sync_comment_readers(window, cx);
                    }
                    cx.notify();
                });
            });
        }));
        cx.notify();
    }

    fn sync_comment_readers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids: HashSet<_> = self
            .comments
            .comments
            .iter()
            .map(|comment| comment.id)
            .collect();
        self.comment_readers.retain(|id, _| ids.contains(id));
        for comment in &self.comments.comments {
            let Some(body) = &comment.body else {
                self.comment_readers.remove(&comment.id);
                continue;
            };
            if self
                .comment_readers
                .get(&comment.id)
                .is_some_and(|(text, _)| text == body)
            {
                continue;
            }
            let reader = self.create_reader(body.clone(), window, cx);
            self.comment_readers
                .insert(comment.id, (body.clone(), reader));
        }
    }

    pub(super) fn render_comments(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let color = theme.colors.fg_subtle;
        let border = theme.colors.border;
        let mut content = div()
            .w_full()
            .flex()
            .flex_col()
            .gap_3()
            .border_t_1()
            .border_color(border)
            .pt_3()
            .child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Comments"),
            );
        let now = SystemTime::now();
        for comment in &self.comments.comments {
            let id = comment.id;
            let author = comment
                .author
                .as_ref()
                .map_or("Unknown author", |author| author.login.as_str());
            content = content.child(
                div()
                    .id(("pr-comment", id))
                    .debug_selector(move || format!("pr-comment-{id}"))
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_color(color)
                            .text_size(theme.text_size(TextSize::Sm))
                            .child(format!(
                                "{author} · {}",
                                relative_time::relative(Some(&comment.created_at), now)
                            )),
                    )
                    .child(if let Some((_, reader)) = self.comment_readers.get(&id) {
                        reader.clone().into_any_element()
                    } else {
                        div()
                            .text_color(color)
                            .child("No comment text.")
                            .into_any_element()
                    }),
            );
        }
        if let Some(mode) = self.comments.comments_busy {
            let label = match mode {
                CommentsRequestMode::Replace => "Loading comments",
                CommentsRequestMode::Append => "Loading more comments",
            };
            content = content.child(compact_status::loading("pr-comments-loading", label, cx));
        } else if let Some(error) = &self.comments.comments_error {
            let view = cx.entity().downgrade();
            let mode = self
                .comments
                .comments_failed_mode
                .unwrap_or(CommentsRequestMode::Replace);
            content = content.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_color(color)
                    .child(compact_status::failed(
                        "pr-comments-retry",
                        "Could not read comments",
                        error.to_string(),
                        Some(move |window: &mut Window, cx: &mut App| {
                            let _ =
                                view.update(cx, |view, cx| view.request_comments(mode, window, cx));
                        }),
                        Some("pr-comments-retry"),
                        cx,
                    ))
                    .child("Could not read comments."),
            );
        } else if self.comments.comments_confirmed_empty() && self.comments.comments_next.is_none()
        {
            content = content.child(
                div()
                    .debug_selector(|| "pr-comments-empty".into())
                    .text_color(color)
                    .child("No comments yet."),
            );
        } else if self.comments.comments_next.is_some() {
            let view = cx.entity().downgrade();
            content = content.child(
                div()
                    .debug_selector(|| "pr-comments-load-more".into())
                    .child(
                        Button::new("pr-comments-load-more", "Load more comments")
                            .variant(ButtonVariant::Secondary)
                            .on_click(move |_, window, cx| {
                                let _ = view.update(cx, |view, cx| {
                                    view.request_comments(CommentsRequestMode::Append, window, cx)
                                });
                            }),
                    ),
            );
        }
        content
    }
}
