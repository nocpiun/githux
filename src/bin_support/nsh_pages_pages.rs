//! Test-only page contract adapter and the page hosts under test.
//!
//! The production `app::pages::mod` (with Home, Profile, Repository and the shared widgets) is the
//! integration author's file; a standalone page test target must not pull the whole application
//! tree in. This file repeats the documented minimal contract (`docs/native-shell-inplace.md`) and
//! includes the two real page hosts unchanged, so the tests still exercise the delivered code.
#![allow(dead_code)]
use std::rc::Rc;

use nsh_core::prelude::AnyElement;

use crate::app::runtime::RuntimeHandle;
use crate::workspace::{RepositoryNavigation, Route, TabId};

pub enum PageNavigation {
    Open(Route),
    Repository {
        route: Route,
        intent: RepositoryNavigation,
    },
    External(String),
    CanonicalProfile {
        tab: TabId,
        login: String,
    },
}
pub type NavigationSink = Rc<dyn Fn(PageNavigation)>;

pub trait PageView {
    fn element(&self) -> AnyElement;
    fn poll(&mut self, runtime: &RuntimeHandle);
    fn refresh(&mut self, runtime: &RuntimeHandle);
    fn set_active(&mut self, active: bool);
    fn retire(&mut self);
}

#[path = "../app/pages/notifications.rs"]
pub mod notifications;
#[path = "../app/pages/work_items.rs"]
pub mod work_items;
