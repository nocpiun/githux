//! Production single-Issue detail host. It joins the account-scoped issue/comments reader to
//! [`IssueDetailView`] and routes repository, author profile and external links through Shell.

use std::rc::Rc;

use gpui::{AnyElement, AppContext, Context, Entity, IntoElement, Render, Subscription, Window};

use crate::app::accounts::AccountServices;
use crate::domain::issue_detail::IssueLocator;
use crate::ui::issue_detail::{IssueDetailAction, IssueDetailView};
use crate::workspace::Route;

use super::{NavigationSink, PageAvatars, PageNavigation};

pub struct IssueDetailPage {
    view: Option<Entity<IssueDetailView>>,
    _changed: Option<Subscription>,
}

impl IssueDetailPage {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        namespace: &str,
        name: &str,
        number: u64,
        services: Option<AccountServices>,
        navigation: NavigationSink,
        avatars: PageAvatars,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            view: None,
            _changed: None,
        };
        let Some(services) = services else {
            return this;
        };
        let Ok(locator) = IssueLocator::new(namespace, name, number) else {
            return this;
        };
        let repos = services.repositories();
        if repos.issue_detail_session().is_err() {
            return this;
        }
        let active_account = services.accounts().active_summary();
        let view = cx.new(|cx| {
            IssueDetailView::new(
                repos.clone(),
                services.executor().clone(),
                locator,
                avatars,
                window,
                cx,
            )
            .expect("the account session was verified before construction")
        });
        view.update(cx, |view, _| {
            let on_action: crate::ui::issue_detail::OnIssueDetailAction =
                Rc::new(move |action, window, cx| match action {
                    IssueDetailAction::Repository {
                        context,
                        repository,
                    } => {
                        if !repos.is_current(&context) {
                            return;
                        }
                        navigation(
                            PageNavigation::Open(Route::Repository {
                                namespace: repository.owner().to_string(),
                                name: repository.name().to_string(),
                            }),
                            window,
                            cx,
                        );
                    }
                    IssueDetailAction::Author { context, login } => {
                        if !repos.is_current(&context) {
                            return;
                        }
                        navigation(
                            PageNavigation::Open(Route::profile(login, active_account.as_ref())),
                            window,
                            cx,
                        );
                    }
                    IssueDetailAction::External(url) => {
                        navigation(PageNavigation::External(url), window, cx);
                    }
                });
            view.on_action(on_action);
        });
        this._changed = Some(cx.observe(&view, |_, _, cx| cx.notify()));
        this.view = Some(view);
        this
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if let Some(view) = &self.view {
            view.update(cx, |view, cx| view.set_active(active, cx));
        }
    }

    pub(crate) fn refresh_state(&self, cx: &gpui::App) -> crate::ui::shell_refresh::RefreshState {
        let Some(view) = &self.view else {
            return crate::ui::shell_refresh::RefreshState::disabled("Sign in to refresh Issue");
        };
        let view = view.read(cx);
        if !view.controller_is_current() {
            crate::ui::shell_refresh::RefreshState::disabled("Sign in to refresh Issue")
        } else if view.is_refreshing() {
            crate::ui::shell_refresh::RefreshState::refreshing("Refreshing Issue")
        } else {
            crate::ui::shell_refresh::RefreshState::ready("Refresh Issue")
        }
    }

    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(view) = &self.view else {
            return false;
        };
        view.update(cx, |view, cx| view.refresh(cx))
    }
}

impl Render for IssueDetailPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(view) = self.view.clone() {
            return view.into_any_element();
        }
        placeholder(
            "Sign in to read this issue. No saved issue content is available.",
            cx,
        )
    }
}

fn placeholder<E: 'static>(message: &str, cx: &mut Context<E>) -> AnyElement {
    use ely_core::theme::{ActiveTheme, TextSize};
    use gpui::{ParentElement, Styled, div, prelude::*};
    div()
        .debug_selector(|| "issue-detail-page-unavailable".to_string())
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .p_6()
        .text_align(gpui::TextAlign::Center)
        .text_size(cx.theme().text_size(TextSize::Sm))
        .text_color(cx.theme().colors.fg_subtle)
        .child(message.to_string())
        .into_any_element()
}
