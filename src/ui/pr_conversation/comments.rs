use ely_core::{
    buttons::{Button, ButtonVariant},
    theme::{ActiveTheme, TextSize},
};
use gpui::{App, Context, IntoElement, ParentElement, Styled, Window, div, prelude::*};
use std::{collections::HashSet, time::SystemTime};

use crate::data::pr_conversation::{LiveComments, Refresh};
use crate::domain::{error::GitHubError, issue_detail::IssueDetailError};
use crate::github::issue_detail::IssueDetailLimits;
use crate::ui::{compact_status, relative_time};

use crate::ui::issue_detail::controller::CommentsRequest;

use super::{CommentRead, CommentsRequestMode, ConversationView};

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
        let session = self.session.clone();
        let scope = self.cache_scope.clone();
        let collection = self.comment_collection;
        let persist = !self.cached_body;
        let work = self.executor.spawn(async move {
            session
                .refresh_comments(
                    scope,
                    collection,
                    online.cursor,
                    IssueDetailLimits::default(),
                    online.cancel,
                    persist,
                )
                .await
        });
        if let Ok(work) = work {
            self.comment_task = Some(CommentRead { request, work });
            self.ensure_drain_task(window, cx);
        } else {
            self.comments
                .finish_comments(&request, Err(super::request_interrupted().into()));
        }
        cx.notify();
    }

    pub(super) fn finish_comments(
        &mut self,
        request: &CommentsRequest,
        refresh: Refresh<LiveComments, IssueDetailError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_current() || !self.cache_is_authorized() {
            self.clear_conversation();
            self.error = Some(GitHubError::StaleContext);
        } else if matches!(&refresh.result, Err(error) if error.is_missing() || error.is_auth_or_permission())
        {
            self.clear_conversation();
            self.error = Some(match refresh.result {
                Err(IssueDetailError::GitHub(error)) => error,
                _ => GitHubError::NotFound,
            });
        } else {
            let result = refresh.result.map(|live| {
                self.comment_collection = live.collection;
                self.clear_comment_history();
                live.page
            });
            if self.cached_comments_partial.is_some()
                && matches!(&result, Err(error) if refresh.no_store || !error.is_transient())
            {
                self.comments.comments.clear();
                self.comments.comments_loaded = false;
                self.clear_comment_history();
            }
            if self.comments.finish_comments(request, result) {
                self.sync_comment_readers(window, cx);
            }
        }
        cx.notify();
    }

    fn clear_comment_history(&mut self) {
        self.cached_comments_partial = None;
        if !self.cached_body {
            self.cache_authorization = None;
        }
    }

    pub(super) fn sync_comment_readers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        if self.cached_comments_partial == Some(true) {
            content = content.child(
                div()
                    .debug_selector(|| "pr-comments-partial".into())
                    .text_color(color)
                    .child("Some comments are unavailable until refresh."),
            );
        }
        if self.pending.is_some() {
            return content;
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
            let label = if self.cached_comments_partial.is_some() {
                "Cached · Comments refresh failed"
            } else {
                "Could not read comments."
            };
            content = content.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_color(color)
                    .child(compact_status::failed(
                        "pr-comments-retry",
                        label,
                        error.to_string(),
                        Some(move |window: &mut Window, cx: &mut App| {
                            let _ =
                                view.update(cx, |view, cx| view.request_comments(mode, window, cx));
                        }),
                        Some("pr-comments-retry"),
                        cx,
                    ))
                    .child(label),
            );
        } else if self.cached_comments_partial != Some(true)
            && self.comments.comments_confirmed_empty()
            && self.comments.comments_next.is_none()
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
