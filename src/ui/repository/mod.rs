//! Production Repository page contract.
//!
//! `nsh-repository` owns this module and replaces the body with the real tree, overview,
//! reader and Star wiring. This contract implementation keeps the shared Shell compiling
//! against the documented factory until that delivery is cherry-picked.
use crate::app::pages::{NavigationSink, PageView};
use crate::app::runtime::{PageContext, RuntimeHandle};
use crate::ui::foundation::theme as t;
use crate::workspace::RepositoryNavigation;
use nsh_core::prelude::*;

pub struct RepositoryPage {
    context: PageContext,
    owner: String,
    name: String,
    intent: RepositoryNavigation,
    navigation: NavigationSink,
    active: bool,
}

impl RepositoryPage {
    pub fn new(
        context: PageContext,
        owner: String,
        name: String,
        intent: RepositoryNavigation,
        navigation: NavigationSink,
    ) -> Self {
        Self {
            context,
            owner,
            name,
            intent,
            navigation,
            active: true,
        }
    }

    pub fn intent(&self) -> &RepositoryNavigation {
        &self.intent
    }
}

impl PageView for RepositoryPage {
    fn element(&self) -> AnyElement {
        view()
            .flex_1()
            .bg(t::theme().background)
            .p(px(t::PAGE_GUTTER))
            .child(t::page_header(format!("{}/{}", self.owner, self.name)))
            .into_element()
    }

    fn poll(&mut self, _runtime: &RuntimeHandle) {}

    fn refresh(&mut self, _runtime: &RuntimeHandle) {}

    fn set_active(&mut self, active: bool) {
        self.active = active;
    }

    fn retire(&mut self) {}
}
