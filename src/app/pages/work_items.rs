//! Production global Issues / Pull Requests page host.
use crate::app::pages::{NavigationSink, PageView};
use crate::app::runtime::{PageContext, RuntimeHandle};
use crate::ui::foundation::theme as t;
use crate::workspace::Route;
use nsh_core::prelude::*;

pub struct WorkItemsPage {
    context: PageContext,
    route: Route,
    navigation: NavigationSink,
    active: bool,
}

impl WorkItemsPage {
    pub fn new(context: PageContext, route: Route, navigation: NavigationSink) -> Self {
        Self {
            context,
            route,
            navigation,
            active: true,
        }
    }
}

impl PageView for WorkItemsPage {
    fn element(&self) -> AnyElement {
        let title = match self.route {
            Route::Issues => "Issues",
            Route::PullRequests => "Pull requests",
            _ => "Work Items",
        };
        view()
            .flex_1()
            .bg(t::theme().background)
            .p(px(t::PAGE_GUTTER))
            .child(t::page_header(title))
            .into_element()
    }

    fn poll(&mut self, _runtime: &RuntimeHandle) {}

    fn refresh(&mut self, _runtime: &RuntimeHandle) {}

    fn set_active(&mut self, active: bool) {
        self.active = active;
    }

    fn retire(&mut self) {}
}
