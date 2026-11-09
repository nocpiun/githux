//! Production Profile page host.
use crate::app::pages::{NavigationSink, PageView};
use crate::app::runtime::{PageContext, RuntimeHandle};
use crate::ui::foundation::theme as t;
use nsh_core::prelude::*;

pub struct ProfilePage {
    context: PageContext,
    login: String,
    navigation: NavigationSink,
    active: bool,
}

impl ProfilePage {
    pub fn new(context: PageContext, login: String, navigation: NavigationSink) -> Self {
        Self {
            context,
            login,
            navigation,
            active: true,
        }
    }
}

impl PageView for ProfilePage {
    fn element(&self) -> AnyElement {
        view()
            .flex_1()
            .bg(t::theme().background)
            .p(px(t::PAGE_GUTTER))
            .child(t::page_header(self.login.clone()))
            .into_element()
    }

    fn poll(&mut self, _runtime: &RuntimeHandle) {}

    fn refresh(&mut self, _runtime: &RuntimeHandle) {}

    fn set_active(&mut self, active: bool) {
        self.active = active;
    }

    fn retire(&mut self) {}
}
