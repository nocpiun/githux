//! Production Notifications host. The account-scoped inbox view owns the controller and its
//! request generation; this host only translates its typed repository navigation into the Shell's
//! internal route and forwards the page lifecycle.
use std::rc::Rc;

use nsh_core::prelude::AnyElement;

use crate::app::runtime::{PageContext, RuntimeHandle};
use crate::ui::notifications::{NotificationsView, OnRepository};
use crate::workspace::Route;

use super::{NavigationSink, PageNavigation, PageView};

pub struct NotificationsPage {
    view: NotificationsView,
}

impl NotificationsPage {
    pub fn new(context: PageContext, navigation: NavigationSink) -> Self {
        let on_repository: OnRepository = Rc::new(move |repository| {
            // The view already re-checked the captured account/authorization. The Shell checks
            // its own tab and incarnation when it consumes this route.
            let locator = repository.locator();
            navigation(PageNavigation::Open(Route::Repository {
                namespace: locator.owner().to_string(),
                name: locator.name().to_string(),
            }));
        });
        Self {
            view: NotificationsView::new(context, on_repository),
        }
    }
}

impl NotificationsPage {
    /// Fixed wall clock for the offline page tests; production keeps the live clock.
    pub fn set_now(&mut self, now: Option<u64>) {
        self.view.set_now(now);
    }

    pub fn loaded_rows(&self) -> usize {
        self.view.loaded_rows()
    }
}

impl PageView for NotificationsPage {
    fn element(&self) -> AnyElement {
        self.view.element()
    }

    fn poll(&mut self, runtime: &RuntimeHandle) {
        self.view.poll(runtime);
    }

    fn refresh(&mut self, runtime: &RuntimeHandle) {
        self.view.refresh(runtime);
    }

    fn set_active(&mut self, active: bool) {
        self.view.set_active(active);
    }

    fn retire(&mut self) {
        self.view.retire();
    }
}
