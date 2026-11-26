//! Mounts one account-scoped Conversation in the Shell's tab viewport.

use std::sync::Arc;

use ely_core::theme::ActiveTheme;
use gpui::{
    App, AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, Subscription,
    Window, div, prelude::*,
};

use crate::app::accounts::AccountServices;
use crate::domain::{
    issue_detail::IssueLocator, pr_conversation::Target, repository::RepositoryLocator,
};
use crate::network::reader_images::ReqwestExternalImages;
use crate::ui::{
    issue_detail::controller::Controller, pr_conversation::ConversationView, reader::ImageProvider,
    shell_refresh::RefreshState,
};

use super::issue_detail::IssueImageProvider;

pub struct PullRequestPage {
    view: Option<Entity<ConversationView>>,
    _changed: Option<Subscription>,
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
        let view = (|| {
            let services = services?;
            let repository = RepositoryLocator::new(namespace, name).ok()?;
            let target = Target::new(repository.clone(), number)?;
            let locator = IssueLocator::from_repository(repository, number).ok()?;
            let repos = services.repositories();
            let session = repos.session().ok()?;
            let comments = Controller::new(repos.issue_detail_session().ok()?, locator);
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
            Some(cx.new(|cx| {
                ConversationView::new(
                    session,
                    comments,
                    target,
                    services.executor().handle(),
                    images,
                    window,
                    cx,
                )
            }))
        })();
        let changed = view
            .as_ref()
            .map(|view| cx.observe(view, |_, _, cx| cx.notify()));
        Self {
            view,
            _changed: changed,
        }
    }

    pub fn suspend(&mut self, cx: &mut Context<Self>) {
        if let Some(view) = &self.view {
            view.update(cx, |view, cx| view.suspend(cx));
        }
    }

    pub(crate) fn refresh_state(&self, cx: &App) -> RefreshState {
        match self.view.as_ref().map(|view| view.read(cx)) {
            Some(view) if view.is_current() && view.is_refreshing() => {
                RefreshState::refreshing("Refreshing pull request")
            }
            Some(view) if view.is_current() => RefreshState::ready("Refresh pull request"),
            _ => RefreshState::disabled("Sign in to refresh pull request"),
        }
    }

    pub(crate) fn refresh(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
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
        if let Some(view) = &self.view {
            return view.clone().into_any_element();
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
