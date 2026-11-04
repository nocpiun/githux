//! Production global Issues / Pull Requests host. It joins the account-scoped search controller to
//! the accepted WorkItems component and routes repository links through the Shell.
use std::rc::Rc;

use gpui::{AnyElement, AppContext, Context, Entity, IntoElement, Render, Subscription, Window};

use crate::app::accounts::AccountServices;
use crate::domain::work_item::WorkItemKind;
use crate::ui::global_work_items::WorkItemsView;
use crate::workspace::Route;

use super::{NavigationSink, PageNavigation};

pub struct WorkItemsPage {
    view: Option<Entity<WorkItemsView>>,
    _changed: Option<Subscription>,
}

impl WorkItemsPage {
    pub fn new(
        kind: WorkItemKind,
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
        // The view's constructor derives its own session. Pre-check so a logged-out or unverified
        // account reports the unavailable state instead of panicking on construction.
        if repos.work_item_session().is_err() {
            return this;
        }
        let view = cx.new(|cx| {
            WorkItemsView::new(repos.clone(), services.executor().clone(), kind, window, cx)
                .expect("the account session was verified before construction")
        });
        view.update(cx, |view, _| {
            let on_repository: crate::ui::global_work_items::OnRepository =
                Rc::new(move |nav, window, cx| {
                    // The component already re-checks its captured context; the host repeats the
                    // current-account check so an old account's link can never open under a new one.
                    if !repos.is_current(&nav.context) {
                        return;
                    }
                    navigation(
                        PageNavigation::Open(Route::Repository {
                            namespace: nav.repository.owner.clone(),
                            name: nav.repository.name.clone(),
                        }),
                        window,
                        cx,
                    );
                });
            view.on_repository(on_repository);
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
}

impl Render for WorkItemsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(view) = self.view.clone() {
            return view.into_any_element();
        }
        placeholder(
            "Sign in to read global issues and pull requests. There is no offline page cache.",
            cx,
        )
    }
}

fn placeholder<E: 'static>(message: &str, cx: &mut Context<E>) -> AnyElement {
    use ely_core::theme::{ActiveTheme, TextSize};
    use gpui::{ParentElement, Styled, div, prelude::*};
    div()
        .debug_selector(|| "work-items-page-unavailable".to_string())
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
