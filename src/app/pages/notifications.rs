//! Production Notifications host. It joins the account-scoped inbox controller to the accepted
//! Notifications component and routes its single typed repository navigation through the Shell.
use std::rc::Rc;

use gpui::{AnyElement, AppContext, Context, Entity, IntoElement, Render, Subscription, Window};

use crate::app::accounts::AccountServices;
use crate::ui::notifications::NotificationsView;
use crate::workspace::Route;

use super::{NavigationSink, PageNavigation};

pub struct NotificationsPage {
    view: Option<Entity<NotificationsView>>,
    _changed: Option<Subscription>,
}

impl NotificationsPage {
    pub fn new(
        services: Option<AccountServices>,
        navigation: NavigationSink,
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
        let repos = services.repositories();
        let Ok(controller) = repos.notifications() else {
            return this;
        };
        let on_repository: crate::ui::notifications::OnRepository =
            Rc::new(move |nav, window, cx| {
                // The component already re-checks its captured context, but the host repeats the
                // current-account check so an old account's link can never open under a new one.
                if !repos.is_current(&nav.context) {
                    return;
                }
                let locator = nav.repository.locator();
                navigation(
                    PageNavigation::Open(Route::Repository {
                        namespace: locator.owner().to_string(),
                        name: locator.name().to_string(),
                    }),
                    window,
                    cx,
                );
            });
        let view = cx.new(|cx| {
            NotificationsView::new(
                controller,
                services.executor().clone(),
                on_repository,
                window,
                cx,
            )
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
            return crate::ui::shell_refresh::RefreshState::disabled(
                "Sign in to refresh Notifications",
            );
        };
        let view = view.read(cx);
        if !view.controller_is_current() {
            crate::ui::shell_refresh::RefreshState::disabled("Sign in to refresh Notifications")
        } else if view.is_refreshing() {
            crate::ui::shell_refresh::RefreshState::refreshing("Refreshing Notifications")
        } else if view.is_cooling() {
            crate::ui::shell_refresh::RefreshState::disabled(
                "Refresh available after the server cooldown",
            )
        } else {
            crate::ui::shell_refresh::RefreshState::ready("Refresh Notifications")
        }
    }

    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(view) = &self.view else {
            return false;
        };
        view.update(cx, |view, cx| view.refresh(cx))
    }
}

impl Render for NotificationsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(view) = self.view.clone() {
            return view.into_any_element();
        }
        placeholder(
            "Sign in to read notifications. There is no offline page cache.",
            cx,
        )
    }
}

fn placeholder<E: 'static>(message: &str, cx: &mut Context<E>) -> AnyElement {
    use ely_core::theme::{ActiveTheme, TextSize};
    use gpui::{ParentElement, Styled, div, prelude::*};
    div()
        .debug_selector(|| "notifications-page-unavailable".to_string())
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
