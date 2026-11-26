//! PR Conversation content, including the shared ordinary-comment read controller.

mod comments;

use std::{rc::Rc, sync::Arc};

use ely_core::{
    primitives::{Icon, IconName},
    theme::{ActiveTheme, IconSize, TextSize},
};
use gpui::{
    App, AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, Task, Window,
    div, prelude::*,
};

use crate::content::PreparedBody;
use crate::data::RepositorySession;
use crate::domain::{
    error::GitHubError,
    pr_conversation::{Conversation, SourceBranch, State, Target, access_denied},
};
use crate::network::{
    cancel::{CancellationToken, CancellationTokenSource},
    executor::BackgroundExecutorHandle,
};
use crate::ui::{
    compact_status,
    issue_detail::{
        controller::{CommentsRequestMode, Controller},
        resolve_relative_issue_link,
    },
    page_layout::{PAGE_BOTTOM, PAGE_GUTTER, PAGE_TOP},
    reader::{
        DocumentKind, ImageProvider, LinkTarget, MarkdownLayout, ReaderAction, ReaderView,
        ResourceContext,
    },
};

pub(crate) struct ConversationView {
    session: RepositorySession,
    target: Target,
    executor: BackgroundExecutorHandle,
    images: Option<Arc<dyn ImageProvider>>,
    summary: Option<Conversation>,
    reader: Option<Entity<ReaderView>>,
    error: Option<GitHubError>,
    pending: Option<(CancellationTokenSource, Task<()>)>,
    comments: Controller,
    comment_task: Option<Task<()>>,
    comment_readers: std::collections::HashMap<u64, (String, Entity<ReaderView>)>,
}

impl ConversationView {
    pub fn new(
        session: RepositorySession,
        comments: Controller,
        target: Target,
        executor: BackgroundExecutorHandle,
        images: Option<Arc<dyn ImageProvider>>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self {
            session,
            target,
            executor,
            images,
            summary: None,
            reader: None,
            error: None,
            pending: None,
            comments,
            comment_task: None,
            comment_readers: Default::default(),
        };
        view.refresh(window, cx);
        view
    }

    pub fn summary(&self) -> Option<&Conversation> {
        if self.session.verify() {
            self.summary.as_ref()
        } else {
            None
        }
    }

    pub fn is_current(&self) -> bool {
        self.session.verify()
    }

    pub fn is_refreshing(&self) -> bool {
        self.pending.is_some() || self.comments.comments_busy.is_some()
    }

    pub fn refresh(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        if !self.is_current() || self.is_refreshing() {
            return false;
        }
        self.error = None;
        let (cancel, source) = CancellationToken::new();
        let session = self.session.clone();
        let target = self.target.clone();
        let Ok(request) = self.executor.spawn(async move {
            session
                .pr_conversation(target, cancel)
                .await
                .map(|reply| reply.value)
        }) else {
            self.error = Some(request_interrupted());
            cx.notify();
            return false;
        };
        let task = cx.spawn_in(window, async move |view, cx| {
            let result = request.await.unwrap_or_else(|_| Err(request_interrupted()));
            let _ = cx.update(|window, cx| {
                let _ = view.update(cx, |view, cx| view.finish(result, window, cx));
            });
        });
        self.pending = Some((source, task));
        cx.notify();
        true
    }

    fn finish(
        &mut self,
        result: Result<Conversation, GitHubError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending = None;
        let result = if self.is_current() {
            result
        } else {
            Err(GitHubError::StaleContext)
        };
        match result {
            Ok(summary) => {
                if self
                    .summary
                    .as_ref()
                    .and_then(|summary| summary.body.as_ref())
                    != summary.body.as_ref()
                {
                    self.reader = summary
                        .body
                        .as_ref()
                        .map(|body| self.create_reader(body.clone(), window, cx));
                }
                self.summary = Some(summary);
            }
            Err(error) => {
                if access_denied(&error) {
                    self.clear_conversation();
                }
                self.error = Some(error);
            }
        }
        if self.summary().is_some() {
            self.request_comments(CommentsRequestMode::Replace, window, cx);
        }
        cx.notify();
    }

    fn clear_conversation(&mut self) {
        self.summary = None;
        self.reader = None;
        self.comments.cancel_all();
        self.comment_task = None;
        self.comments.comments.clear();
        self.comments.comments_loaded = false;
        self.comments.comments_next = None;
        self.comments.comments_error = None;
        self.comments.comments_failed_mode = None;
        self.comment_readers.clear();
    }

    fn create_reader(
        &self,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ReaderView> {
        let view = cx.entity().downgrade();
        let link_view = view.clone();
        let repository = &self.target.repository;
        let context =
            ResourceContext::new(repository.owner(), repository.name(), "", "conversation.md");
        let images = self.images.clone();
        cx.new(|cx| {
            let mut reader = ReaderView::new(
                PreparedBody::Utf8 {
                    has_bom: text.starts_with('\u{feff}'),
                    text,
                },
                DocumentKind::Markdown,
                context,
                window,
                cx,
            );
            reader.set_markdown_layout(MarkdownLayout::Flow, cx);
            reader.set_image_provider(images, cx);
            reader.on_layout_changed(Rc::new(move |_, cx| {
                let _ = view.update(cx, |_, cx| cx.notify());
            }));
            reader.on_action(Rc::new(move |ReaderAction::Link(target), _, cx| {
                let url = match target {
                    LinkTarget::External { url } => Some(url),
                    LinkTarget::Repository(link) => resolve_relative_issue_link(
                        &link.owner,
                        &link.name,
                        &link.path,
                        link.anchor.as_deref(),
                    ),
                    _ => None,
                };
                if let Some(url) = url
                    && link_view
                        .upgrade()
                        .is_some_and(|view| view.read(cx).summary().is_some())
                {
                    cx.open_url(&url);
                }
            }));
            reader
        })
    }

    pub fn suspend(&self, cx: &mut Context<Self>) {
        for reader in self
            .reader
            .iter()
            .chain(self.comment_readers.values().map(|(_, reader)| reader))
        {
            reader.update(cx, |reader, cx| reader.cancel_interactions(cx));
        }
    }
}

impl Drop for ConversationView {
    fn drop(&mut self) {
        self.comments.cancel_all();
        if let Some((cancel, _)) = self.pending.take() {
            cancel.cancel();
        }
    }
}

impl Render for ConversationView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let color = cx.theme().colors.fg_subtle;
        let mut content = div()
            .w_full()
            .flex()
            .flex_col()
            .flex_none()
            .gap_3()
            .px(gpui::px(PAGE_GUTTER))
            .pt(gpui::px(PAGE_TOP))
            .pb(gpui::px(PAGE_BOTTOM));
        if !self.is_current() {
            return content
                .text_color(color)
                .child("Sign in to read this pull request.");
        }
        if let Some(summary) = self.summary() {
            content = content.child(header(summary, cx));
            if let Some(reader) = self.reader.clone() {
                content = content.child(
                    div()
                        .debug_selector(|| "pr-conversation-body".into())
                        .child(reader),
                );
            } else {
                content = content.child(
                    div()
                        .debug_selector(|| "pr-conversation-empty-body".into())
                        .text_color(color)
                        .child("No description provided."),
                );
            }
            content = content.child(self.render_comments(cx));
        }
        if let Some(error) = &self.error {
            let view = cx.entity().downgrade();
            let label = if self.summary.is_some() {
                "Pull request refresh failed"
            } else {
                "Could not read this pull request"
            };
            content = content.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_color(color)
                    .child(compact_status::failed(
                        "pr-conversation-error",
                        label,
                        error.to_string(),
                        Some(move |window: &mut Window, cx: &mut App| {
                            let _ = view.update(cx, |view, cx| view.refresh(window, cx));
                        }),
                        None,
                        cx,
                    ))
                    .child(label),
            );
        } else if self.is_refreshing() && self.summary.is_none() {
            content = content.child(compact_status::loading(
                "pr-conversation-loading",
                "Loading pull request",
                cx,
            ));
        }
        content
    }
}

fn request_interrupted() -> GitHubError {
    GitHubError::Network {
        message: "pull request request interrupted".into(),
    }
}

fn header(summary: &Conversation, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let colors = &theme.colors;
    let (label, color) = match summary.state {
        State::Open if summary.draft => ("Draft", colors.fg_muted),
        State::Open => ("Open", colors.success),
        State::Closed => ("Closed", colors.fg_subtle),
        State::Merged => ("Merged", colors.accent),
    };
    let base = &summary.target.repository;
    let (head_repository, branch_status) = match &summary.source_branch {
        SourceBranch::Present { repository } => (Some(repository.as_str()), None),
        SourceBranch::Deleted { repository } => (Some(repository.as_str()), Some("Branch deleted")),
        SourceBranch::Unavailable => (None, Some("Source branch unavailable")),
    };
    let head = head_repository.map_or_else(
        || summary.head_branch.clone(),
        |repo| format!("{repo}:{}", summary.head_branch),
    );
    div()
        .debug_selector(|| "pr-conversation-summary".into())
        .w_full()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_size(theme.text_size(TextSize::Lg))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(summary.title.clone()),
        )
        .child(
            div()
                .flex()
                .items_center()
                .flex_wrap()
                .gap_2()
                .text_size(theme.text_size(TextSize::Sm))
                .text_color(colors.fg_subtle)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_color(color)
                        .child(
                            Icon::new(IconName::GitPullRequest)
                                .size(IconSize::Xs)
                                .color(color),
                        )
                        .child(label),
                )
                .child(format!("#{}", summary.target.number()))
                .child(summary.author.as_ref().map_or_else(
                    || "Unknown author".to_owned(),
                    |login| format!("Opened by {login}"),
                )),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .text_size(theme.text_size(TextSize::Sm))
                .child(branch_row(
                    "Base",
                    format!("{}/{}:{}", base.owner(), base.name(), summary.base_branch),
                    None,
                    cx,
                ))
                .child(branch_row("Head", head, branch_status, cx)),
        )
}

fn branch_row(
    label: &'static str,
    branch: String,
    status: Option<&'static str>,
    cx: &App,
) -> impl IntoElement {
    let colors = &cx.theme().colors;
    div()
        .flex()
        .items_center()
        .flex_wrap()
        .gap_2()
        .child(
            div()
                .w(gpui::px(32.))
                .flex_none()
                .text_color(colors.fg_subtle)
                .child(label),
        )
        .child(div().min_w_0().max_w_full().child(branch))
        .children(status.map(|text| div().text_color(colors.fg_subtle).child(text)))
}
