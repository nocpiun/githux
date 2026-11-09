//! Production Notifications page host.
use crate::app::pages::{NavigationSink, PageView};
use crate::app::runtime::{PageContext, RuntimeHandle};
use crate::ui::foundation::theme as t;
use nsh_core::prelude::*;

pub struct NotificationsPage {
    context: PageContext,
    navigation: NavigationSink,
    active: bool,
}

impl NotificationsPage {
    pub fn new(context: PageContext, navigation: NavigationSink) -> Self {
        Self {
            context,
            navigation,
            active: true,
        }
    }
}

impl PageView for NotificationsPage {
    fn element(&self) -> AnyElement {
        view()
            .flex_1()
            .bg(t::theme().background)
            .p(px(t::PAGE_GUTTER))
            .child(t::page_header("Notifications"))
            .into_element()
    }

    fn poll(&mut self, _runtime: &RuntimeHandle) {}

    fn refresh(&mut self, _runtime: &RuntimeHandle) {}

    fn set_active(&mut self, active: bool) {
        self.active = active;
    }

    fn retire(&mut self) {}
}
