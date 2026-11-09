//! Production Home/Feed page host.
use crate::app::pages::{NavigationSink, PageView};
use crate::app::runtime::{PageContext, RuntimeHandle};
use crate::ui::foundation::theme as t;
use nsh_core::prelude::*;

pub struct HomePage {
    context: PageContext,
    navigation: NavigationSink,
    active: bool,
}

impl HomePage {
    pub fn new(context: PageContext, navigation: NavigationSink) -> Self {
        Self {
            context,
            navigation,
            active: true,
        }
    }
}

impl PageView for HomePage {
    fn element(&self) -> AnyElement {
        view()
            .flex_1()
            .bg(t::theme().background)
            .child(
                view()
                    .flex_row()
                    .flex_1()
                    .p(px(t::PAGE_GUTTER))
                    .gap_4()
                    .child(
                        view()
                            .flex_1()
                            .child(t::page_header("Home"))
                    )
                    .child(
                        view()
                            .flex_1()
                            .child(t::page_header("Feed"))
                    )
            )
            .into_element()
    }

    fn poll(&mut self, _runtime: &RuntimeHandle) {}

    fn refresh(&mut self, _runtime: &RuntimeHandle) {}

    fn set_active(&mut self, active: bool) {
        self.active = active;
    }

    fn retire(&mut self) {}
}
