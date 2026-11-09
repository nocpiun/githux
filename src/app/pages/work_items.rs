//! Production global Issues / Pull Requests host. The shared global-list view owns the search
//! controller, filters, cursor and selection; this host maps the route to the list kind and
//! forwards the page lifecycle.
use std::rc::Rc;

use nsh_core::prelude::*;
use nsh_core::prelude::{AnyElement, IntoElement};

use crate::app::runtime::{PageContext, RuntimeHandle};
use crate::domain::work_item::WorkItemKind;
use crate::ui::global_work_items::{OnRepository, WorkItemsView};
use crate::workspace::Route;

use super::{NavigationSink, PageNavigation, PageView};

pub struct WorkItemsPage {
    view: Option<WorkItemsView>,
}

impl WorkItemsPage {
    pub fn new(context: PageContext, route: Route, navigation: NavigationSink) -> Self {
        // Only `Issues` and `PullRequests` mount this page; any other route is a Shell bug and
        // must not fabricate a list for it.
        let kind = match route {
            Route::Issues => Some(WorkItemKind::Issue),
            Route::PullRequests => Some(WorkItemKind::PullRequest),
            _ => None,
        };
        let Some(kind) = kind else {
            return Self { view: None };
        };
        let on_repository: OnRepository = Rc::new(move |repository| {
            // The view already re-checked the captured account/authorization. The Shell checks
            // its own tab and incarnation when it consumes this route.
            navigation(PageNavigation::Open(Route::Repository {
                namespace: repository.owner,
                name: repository.name,
            }));
        });
        Self {
            view: Some(WorkItemsView::new(context, kind, on_repository)),
        }
    }
}

impl WorkItemsPage {
    /// Fixed wall clock for the offline page tests; production keeps the live clock.
    pub fn set_now(&mut self, now: Option<u64>) {
        if let Some(view) = &mut self.view {
            view.set_now(now);
        }
    }

    pub fn loaded_rows(&self) -> usize {
        self.view.as_ref().map_or(0, |view| view.loaded_rows())
    }
}

impl PageView for WorkItemsPage {
    fn element(&self) -> AnyElement {
        match &self.view {
            Some(view) => view.element(),
            None => unavailable().into_element(),
        }
    }

    fn poll(&mut self, runtime: &RuntimeHandle) {
        if let Some(view) = &mut self.view {
            view.poll(runtime);
        }
    }

    fn refresh(&mut self, runtime: &RuntimeHandle) {
        if let Some(view) = &mut self.view {
            view.refresh(runtime);
        }
    }

    fn set_active(&mut self, active: bool) {
        if let Some(view) = &mut self.view {
            view.set_active(active);
        }
    }

    fn retire(&mut self) {
        if let Some(view) = &mut self.view {
            view.retire();
        }
    }
}

fn unavailable() -> impl IntoElement {
    view()
        .size_full()
        .items_center()
        .justify_center()
        .p_4()
        .child(
            text("This route does not show a global work list.")
                .text_size(px(13.0))
                .text_color(crate::ui::foundation::theme::theme().muted_foreground),
        )
}
