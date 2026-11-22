//! Shell-level active-page refresh coordination for the tab-strip `tab-refresh` control.
//!
//! This module owns the route-to-page refresh mapping, the toolbar's refresh state projection,
//! duplicate-click coalescing, and per-page observer wiring so `Shell` stays decoupled from each
//! page host's internal work slots. It never invents requests for routes without remote data
//! sources, never traverses unmounted tabs or unexpanded repository directories, and never triggers
//! remote write operations.

use gpui::{App, Context, Subscription};

use crate::app::pages::MountedPage;
use crate::workspace::{Route, TabId};

use super::Shell;

/// Visual and interactive state of the top-right `tab-refresh` toolbar control for the active tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RefreshState {
    /// No refreshable remote data source is available on the active tab (for example Settings,
    /// blank/undesigned detail routes, logged-out/unmounted pages, or an active server cooldown).
    Disabled { tooltip: &'static str },
    /// At least one mounted remote data source on the active tab can be revalidated right now.
    Ready { tooltip: &'static str },
    /// At least one mounted remote data source on the active tab is currently loading/refreshing.
    Refreshing { tooltip: &'static str },
}

impl RefreshState {
    pub(crate) const fn disabled(tooltip: &'static str) -> Self {
        Self::Disabled { tooltip }
    }

    pub(crate) const fn ready(tooltip: &'static str) -> Self {
        Self::Ready { tooltip }
    }

    pub(crate) const fn refreshing(tooltip: &'static str) -> Self {
        Self::Refreshing { tooltip }
    }

    pub(crate) const fn is_ready(self) -> bool {
        matches!(self, Self::Ready { .. })
    }

    #[cfg(test)]
    pub(crate) const fn is_refreshing(self) -> bool {
        matches!(self, Self::Refreshing { .. })
    }

    #[cfg(test)]
    pub(crate) const fn is_disabled(self) -> bool {
        matches!(self, Self::Disabled { .. })
    }

    pub(crate) const fn tooltip(self) -> &'static str {
        match self {
            Self::Disabled { tooltip } | Self::Ready { tooltip } | Self::Refreshing { tooltip } => {
                tooltip
            }
        }
    }
}

/// Resolves the `tab-refresh` state for the currently active route and its mounted page host.
///
/// Every `Route` variant is matched explicitly so newly added routes must declare their refresh
/// semantics rather than silently inheriting a fallback.
pub(crate) fn state_for_active_page(
    route: &Route,
    page: Option<&MountedPage>,
    cx: &App,
) -> RefreshState {
    match route {
        Route::Home => match page {
            Some(MountedPage::Home(page)) => page.read(cx).refresh_state(),
            _ => RefreshState::disabled("Sign in to refresh Home"),
        },
        Route::Profile | Route::UserProfile { .. } => match page {
            Some(MountedPage::Profile(page)) => page.read(cx).refresh_state(),
            _ => RefreshState::disabled("Sign in to refresh Profile"),
        },
        Route::Repository { .. } => match page {
            Some(MountedPage::Repository { page }) => {
                let page = page.read(cx);
                if !page.can_refresh() {
                    RefreshState::disabled("Sign in to refresh Repository")
                } else if page.is_refreshing() {
                    RefreshState::refreshing("Refreshing Repository")
                } else {
                    RefreshState::ready("Refresh Repository")
                }
            }
            _ => RefreshState::disabled("Sign in to refresh Repository"),
        },
        Route::Notifications => match page {
            Some(MountedPage::Notifications(page)) => page.read(cx).refresh_state(cx),
            _ => RefreshState::disabled("Sign in to refresh Notifications"),
        },
        Route::Issues => match page {
            Some(MountedPage::WorkItems(page)) => page.read(cx).refresh_state(cx),
            _ => RefreshState::disabled("Sign in to refresh Issues"),
        },
        Route::PullRequests => match page {
            Some(MountedPage::WorkItems(page)) => page.read(cx).refresh_state(cx),
            _ => RefreshState::disabled("Sign in to refresh Pull Requests"),
        },
        Route::Settings => RefreshState::disabled("Settings has no remote data to refresh"),
        Route::PullRequest { .. } => RefreshState::disabled("Nothing to refresh on this page"),
    }
}

/// Triggers a refresh of all currently mounted data sources on the active page.
///
/// Returns `true` only when a real refresh pass was started. Disabled routes, unmounted pages,
/// cooldown windows, and already-refreshing pages return `false` without issuing duplicate work.
pub(crate) fn refresh_active_page(route: &Route, page: Option<&MountedPage>, cx: &mut App) -> bool {
    if !state_for_active_page(route, page, cx).is_ready() {
        return false;
    }
    match (route, page) {
        (Route::Home, Some(MountedPage::Home(page))) => {
            page.update(cx, |page, cx| page.refresh_all(cx))
        }
        (Route::Profile | Route::UserProfile { .. }, Some(MountedPage::Profile(page))) => {
            page.update(cx, |page, cx| page.refresh_all(cx))
        }
        (Route::Repository { .. }, Some(MountedPage::Repository { page })) => {
            page.update(cx, |page, cx| page.refresh_if_idle(cx))
        }
        (Route::Notifications, Some(MountedPage::Notifications(page))) => {
            page.update(cx, |page, cx| page.refresh(cx))
        }
        (Route::Issues | Route::PullRequests, Some(MountedPage::WorkItems(page))) => {
            page.update(cx, |page, cx| page.refresh(cx))
        }
        _ => false,
    }
}

/// Observes a mounted page host so `Shell` repaints the tab-strip refresh control whenever the
/// currently active tab's refresh state transitions (starts, settles, fails, or exits cooldown).
/// Updates on inactive background tabs do not trigger unnecessary Shell repaints.
pub(crate) fn observe_page(
    tab: TabId,
    page: &MountedPage,
    cx: &mut Context<Shell>,
) -> Subscription {
    match page {
        MountedPage::Home(entity) => cx.observe(entity, move |shell, _, cx| {
            if shell.workspace.active_id() == tab {
                cx.notify();
            }
        }),
        MountedPage::Profile(entity) => cx.observe(entity, move |shell, _, cx| {
            if shell.workspace.active_id() == tab {
                cx.notify();
            }
        }),
        MountedPage::Repository { page: entity } => cx.observe(entity, move |shell, _, cx| {
            if shell.workspace.active_id() == tab {
                cx.notify();
            }
        }),
        MountedPage::Notifications(entity) => cx.observe(entity, move |shell, _, cx| {
            if shell.workspace.active_id() == tab {
                cx.notify();
            }
        }),
        MountedPage::WorkItems(entity) => cx.observe(entity, move |shell, _, cx| {
            if shell.workspace.active_id() == tab {
                cx.notify();
            }
        }),
    }
}
