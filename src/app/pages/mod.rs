//! Production page hosts. They join the existing online read services to the accepted page
//! components without introducing a second credential, HTTP, database or scrolling stack.
//!
//! Every result is published on the GPUI thread after checking its captured service context and
//! host generation. Shell additionally owns TabId/resource/incarnation checks before calling the
//! narrow update methods. No host performs I/O from `render`.

mod home;
mod profile;

pub use home::HomePage;
pub use profile::ProfilePage;

use gpui::{App, AppContext, Entity, IntoElement, Window};
use std::rc::Rc;

use crate::app::accounts::AccountServices;
use crate::app::avatar::{AvatarKey, AvatarState};
use crate::ui::avatar::{AvatarSlot, AvatarView, avatar};
use crate::ui::repository::{PageAction, RepositoryPage, RepositoryPageServices};
use crate::workspace::{Route, TabId};

/// One mounted page and its stable workspace identity. Closed pages are removed with their tab;
/// inactive pages retain only still-valid content and navigation state.
pub enum MountedPage {
    Home(Entity<HomePage>),
    Profile(Entity<ProfilePage>),
    Repository { page: Entity<RepositoryPage> },
}

impl MountedPage {
    pub fn render(&self, _window: &mut Window, _cx: &mut App) -> gpui::AnyElement {
        match self {
            Self::Home(page) => page.clone().into_any_element(),
            Self::Profile(page) => page.clone().into_any_element(),
            Self::Repository { page } => page.clone().into_any_element(),
        }
    }

    pub fn suspend(&self, cx: &mut App) {
        match self {
            Self::Home(page) => page.update(cx, |page, cx| page.set_active(false, cx)),
            Self::Profile(page) => page.update(cx, |page, cx| page.set_active(false, cx)),
            Self::Repository { page } => page.update(cx, |page, cx| page.suspend(cx)),
        }
    }

    pub fn resume(&self, cx: &mut App) {
        match self {
            Self::Home(page) => page.update(cx, |page, cx| page.set_active(true, cx)),
            Self::Profile(page) => page.update(cx, |page, cx| page.set_active(true, cx)),
            Self::Repository { .. } => {
                // A still-valid repository session is retained; its own receiver and render check
                // detect authorization loss. No re-read occurs for ordinary tab activation.
            }
        }
    }
}

/// Navigation emitted by a page. Shell is the only writer of workspace routes.
pub enum PageNavigation {
    Open(Route),
    External(String),
    CanonicalProfile { tab: TabId, login: String },
}

pub type NavigationSink = Rc<dyn Fn(PageNavigation, &mut Window, &mut App)>;

/// Shared avatar access. The account controller remains the only loader/cache owner; pages only
/// resolve decoded images for presentation.
#[derive(Clone)]
pub struct PageAvatars {
    accounts: Option<gpui::WeakEntity<crate::app::accounts::Accounts>>,
}

impl PageAvatars {
    pub fn new(accounts: Option<gpui::WeakEntity<crate::app::accounts::Accounts>>) -> Self {
        Self { accounts }
    }

    pub fn accounts(&self) -> Option<&gpui::WeakEntity<crate::app::accounts::Accounts>> {
        self.accounts.as_ref()
    }

    pub fn image(&self, key: AvatarKey, cx: &mut App) -> Option<gpui::ImageSource> {
        let accounts = self.accounts.as_ref()?.upgrade()?;
        match accounts.update(cx, |accounts, _| accounts.avatar(key)) {
            AvatarState::Ready(image) => Some(gpui::ImageSource::Image(image)),
            _ => None,
        }
    }

    pub fn element(
        &self,
        id: &str,
        selector: &str,
        name: &str,
        key: Option<AvatarKey>,
        slot: AvatarSlot,
        cx: &mut App,
    ) -> gpui::AnyElement {
        let image = key.and_then(|key| self.image(key, cx));
        avatar(
            AvatarView {
                id,
                selector,
                image,
                name,
                slot,
            },
            cx,
        )
    }
}

/// Create the real RepositoryPage with Shell's navigation sink and no Star/Fork mutation handler.
#[allow(clippy::too_many_arguments)]
pub fn repository_page(
    _tab: TabId,
    route: &Route,
    services: AccountServices,
    intent: crate::ui::repository::RepositoryNavigation,
    navigation: NavigationSink,
    avatars: PageAvatars,
    window: &mut Window,
    cx: &mut App,
) -> Option<MountedPage> {
    let Route::Repository { namespace, name } = route else {
        return None;
    };
    let locator = crate::domain::repository::RepositoryLocator::new(namespace, name).ok()?;
    let page_services =
        RepositoryPageServices::new(services.accounts().clone(), services.executor().clone());
    // Captured once for the contributor route, so a contributor link resolves like every other
    // profile link without reaching back into the service from the action closure.
    let active_account = services.accounts().active_summary();
    let page = cx.new(|cx| {
        let mut page = RepositoryPage::new(page_services, locator, intent, window, cx);
        page.on_action(Rc::new(move |action, window, cx| match action {
            PageAction::OpenExternal(url) => navigation(PageNavigation::External(url), window, cx),
            PageAction::Overview(
                crate::ui::repository_overview::OverviewAction::OpenContributor {
                    login: Some(login),
                    ..
                },
            ) => {
                // Contributors are hosted on the same unified profile route as the sidebar and
                // Home: signing in as that user must activate the existing own-profile tab rather
                // than creating a second page under the UserProfile key.
                navigation(
                    PageNavigation::Open(Route::profile(login, active_account.as_ref())),
                    window,
                    cx,
                );
            }
            PageAction::Overview(crate::ui::repository_overview::OverviewAction::OpenRelease {
                html_url: Some(url),
                ..
            }) => navigation(PageNavigation::External(url), window, cx),
            // These are intentionally not registered as successful actions: first release has no
            // Star/Fork write operation. The existing page leaves their controls disabled.
            PageAction::StarRequested { .. }
            | PageAction::ForkRequested { .. }
            | PageAction::LinkNotFollowed { .. }
            | PageAction::Overview(_) => {}
        }));
        page.activate(cx);
        page
    });
    // Contributor images still go through the single account avatar loader. The Shell repeats this
    // on later frames, so it only decodes pictures the overview does not already hold.
    sync_repository_avatars(&page, &avatars, cx);
    Some(MountedPage::Repository { page })
}

pub fn sync_repository_avatars(page: &Entity<RepositoryPage>, avatars: &PageAvatars, cx: &mut App) {
    let Some((binding, contributors)) = page.read(cx).overview().map(|overview| {
        let overview = overview.read(cx);
        (
            overview.binding().clone(),
            overview
                .contributors()
                .value()
                .map(|value| value.contributors.clone())
                .unwrap_or_default(),
        )
    }) else {
        return;
    };
    // Only decode a contributor picture the overview does not already hold. This runs from the
    // Shell's render path, so re-decoding unchanged avatars on every repaint (resize, tab scroll)
    // would stall interaction for no visual change.
    let mut ready = Vec::new();
    for contributor in contributors {
        let (Some(id), Some(url)) = (contributor.id, contributor.avatar_url) else {
            continue;
        };
        let already_attached = page
            .read(cx)
            .overview()
            .is_some_and(|overview| overview.read(cx).has_avatar_image(&binding, id));
        if already_attached {
            continue;
        }
        let key = AvatarKey::new("github.com", id, url);
        if let Some(gpui::ImageSource::Image(image)) = avatars.image(key, cx)
            && let Ok(render) = image.to_image_data(cx.svg_renderer())
        {
            ready.push((id, render));
        }
    }
    page.update(cx, |page, cx| {
        if let Some(overview) = page.overview() {
            overview.update(cx, |overview, cx| {
                for (id, image) in ready {
                    overview.set_avatar_image(&binding, id, image, cx);
                }
            });
        }
    });
}

#[cfg(test)]
mod tests;
