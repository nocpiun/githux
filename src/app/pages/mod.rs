//! Production page host contracts and lifecycle.
//!
//! Every page implementation connects to the background data services through PageContext
//! and dispatches route changes through NavigationSink.

pub mod home;
pub mod notifications;
pub mod profile;
pub mod work_items;

use crate::app::runtime::RuntimeHandle;
use crate::workspace::{RepositoryNavigation, Route, TabId};
use nsh_core::prelude::AnyElement;
use std::rc::Rc;

pub use home::HomePage;
pub use notifications::NotificationsPage;
pub use profile::ProfilePage;
pub use work_items::WorkItemsPage;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageNavigation {
    Open(Route),
    Repository { route: Route, intent: RepositoryNavigation },
    External(String),
    CanonicalProfile { tab: TabId, login: String },
}

pub type NavigationSink = Rc<dyn Fn(PageNavigation)>;

pub trait PageView {
    fn element(&self) -> AnyElement;
    fn poll(&mut self, runtime: &RuntimeHandle);
    fn refresh(&mut self, runtime: &RuntimeHandle);
    fn set_active(&mut self, active: bool);
    fn retire(&mut self);
}
