//! Mounts one account-scoped pull request and its in-page tabs.

use std::sync::Arc;

use ely_core::{
    buttons::{Button, ButtonVariant},
    theme::{ActiveTheme, ControlSize},
};
use gpui::{
    App, AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, Subscription,
    Window, div, prelude::*,
};

use crate::app::accounts::AccountServices;
use crate::data::pr_conversation::ConversationSession;
use crate::data::pr_files::FilesSession;
use crate::domain::{
    issue_detail::IssueLocator, pr_conversation::Target, repository::RepositoryLocator,
};
use crate::github::pr_files;
use crate::network::reader_images::ReqwestExternalImages;
use crate::ui::{
    issue_detail::controller::Controller, pr_conversation::ConversationView, pr_files::FilesView,
    reader::ImageProvider, shell_refresh::RefreshState,
};

use super::issue_detail::IssueImageProvider;

pub struct PullRequestPage {
    view: Option<Entity<ConversationView>>,
    files: Option<Entity<FilesView>>,
    files_selected: bool,
    _changed: Vec<Subscription>,
}

impl PullRequestPage {
    pub fn new(
        namespace: &str,
        name: &str,
        number: u64,
        services: Option<AccountServices>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mounted = (|| {
            let services = services?;
            let repository = RepositoryLocator::new(namespace, name).ok()?;
            let target = Target::new(repository.clone(), number)?;
            let locator = IssueLocator::from_repository(repository, number).ok()?;
            let repos = services.repositories();
            let session = repos.session().ok()?;
            let comments = Controller::new(repos.issue_detail_session().ok()?, locator.clone());
            if session.context() != comments.context() {
                return None;
            }
            let images = ReqwestExternalImages::new().ok().map(|external| {
                Arc::new(IssueImageProvider::new(
                    repos,
                    session.clone(),
                    target.repository.clone(),
                    services.executor().clone(),
                    Arc::new(external),
                )) as Arc<dyn ImageProvider>
            });
            let files_session = FilesSession::new(
                session.clone(),
                pr_files::Target {
                    repository: target.repository.clone(),
                    number: number.try_into().ok()?,
                },
            );
            let files = cx.new(|_| FilesView::new(files_session, services.executor().clone()));
            let view = cx.new(|cx| {
                ConversationView::new(
                    ConversationSession::new(session, target.clone(), locator),
                    comments,
                    target,
                    services.executor().handle(),
                    images,
                    window,
                    cx,
                )
            });
            Some((view, files))
        })();
        let Some((view, files)) = mounted else {
            return Self {
                view: None,
                files: None,
                files_selected: false,
                _changed: Vec::new(),
            };
        };
        let changed = vec![
            cx.observe(&view, |_, _, cx| cx.notify()),
            cx.observe(&files, |_, _, cx| cx.notify()),
        ];
        Self {
            view: Some(view),
            files: Some(files),
            files_selected: false,
            _changed: changed,
        }
    }

    pub fn suspend(&mut self, cx: &mut Context<Self>) {
        if let Some(view) = &self.view {
            view.update(cx, |view, cx| view.suspend(cx));
        }
        if let Some(files) = &self.files {
            files.update(cx, |files, cx| files.set_active(false, cx));
        }
    }

    pub fn resume(&mut self, cx: &mut Context<Self>) {
        if self.files_selected
            && let Some(files) = &self.files
        {
            files.update(cx, |files, cx| files.set_active(true, cx));
        }
    }

    pub(crate) fn files_selected(&self) -> bool {
        self.files_selected
    }

    fn select_files(&mut self, selected: bool, cx: &mut Context<Self>) {
        if selected == self.files_selected {
            return;
        }
        self.files_selected = selected;
        if selected && let Some(view) = &self.view {
            view.update(cx, |view, cx| view.suspend(cx));
        }
        if let Some(files) = &self.files {
            files.update(cx, |files, cx| files.set_active(selected, cx));
        }
        cx.notify();
    }

    pub(crate) fn refresh_state(&self, cx: &App) -> RefreshState {
        if self.files_selected {
            return match self.files.as_ref().map(|files| files.read(cx)) {
                Some(files) if files.is_loading() => {
                    RefreshState::refreshing("Refreshing changed files")
                }
                Some(files) if files.is_current() => RefreshState::ready("Refresh changed files"),
                _ => RefreshState::disabled("Sign in to refresh changed files"),
            };
        }
        match self.view.as_ref().map(|view| view.read(cx)) {
            Some(view) if view.is_current() && view.is_refreshing() => {
                RefreshState::refreshing("Refreshing pull request")
            }
            Some(view) if view.is_current() => RefreshState::ready("Refresh pull request"),
            _ => RefreshState::disabled("Sign in to refresh pull request"),
        }
    }

    pub(crate) fn refresh(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        if self.files_selected {
            return self
                .files
                .as_ref()
                .is_some_and(|files| files.update(cx, |files, cx| files.refresh(cx)));
        }
        self.view
            .as_ref()
            .is_some_and(|view| view.update(cx, |view, cx| view.refresh(window, cx)))
    }

    #[cfg(test)]
    pub(crate) fn summary_for_test<'a>(
        &self,
        cx: &'a App,
    ) -> Option<&'a crate::domain::pr_conversation::Conversation> {
        self.view.as_ref()?.read(cx).summary()
    }
}

impl Render for PullRequestPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let (Some(view), Some(files)) = (&self.view, &self.files) {
            let tabs = div()
                .flex()
                .gap_2()
                .px(gpui::px(crate::ui::page_layout::PAGE_GUTTER))
                .py_2()
                .flex_shrink_0()
                .children(
                    [
                        ("pr-tab-conversation", "Conversation", false),
                        ("pr-tab-files-changed", "Files changed", true),
                    ]
                    .map(|(id, label, selected)| {
                        div().debug_selector(move || id.to_owned()).child(
                            Button::new(id, label)
                                .size(ControlSize::Sm)
                                .variant(if self.files_selected == selected {
                                    ButtonVariant::Secondary
                                } else {
                                    ButtonVariant::Ghost
                                })
                                .on_click(cx.listener(move |page, _, _, cx| {
                                    page.select_files(selected, cx)
                                })),
                        )
                    }),
                );
            let page = div().w_full().flex().flex_col().child(tabs);
            return if self.files_selected {
                page.size_full()
                    .min_h_0()
                    .child(div().flex_1().min_h_0().child(files.clone()))
            } else {
                page.child(view.clone())
            }
            .into_any_element();
        }
        div()
            .debug_selector(|| "pr-conversation-unavailable".into())
            .w_full()
            .p_4()
            .text_color(cx.theme().colors.fg_subtle)
            .child("This pull request needs an active account with access.")
            .into_any_element()
    }
}
