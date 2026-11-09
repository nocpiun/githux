//! Accounts / Settings view contract.
//!
//! `nsh-accounts` owns this module and replaces the body with the real account list,
//! add/replace/select/revalidate forms calling the runtime commands. This contract
//! implementation keeps the shared Shell compiling until that delivery is cherry-picked.
use crate::app::pages::NavigationSink;
use crate::app::runtime::RuntimeHandle;
use crate::ui::foundation::theme as t;
use nsh_core::prelude::*;

pub struct AccountsView {
    navigation: NavigationSink,
    active: bool,
}

impl AccountsView {
    pub fn new(navigation: NavigationSink) -> Self {
        Self {
            navigation,
            active: true,
        }
    }

    pub fn element(&self) -> AnyElement {
        view()
            .flex_1()
            .bg(t::theme().background)
            .p(px(t::PAGE_GUTTER))
            .child(t::page_header("Settings"))
            .into_element()
    }

    pub fn poll(&mut self, _runtime: &mut RuntimeHandle) {}

    pub fn retire(&mut self) {
        self.active = false;
    }
}
