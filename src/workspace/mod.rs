//! Window workspace and page-session ownership: routing and the current session's identity.
//!
//! This round has one active page per window and no page-session collection yet: opening,
//! reusing and closing application sessions belong to the next R1 task. The route type is the
//! stable boundary those sessions will point at, so the island's session strip can be filled
//! with a current title without pretending multi-tab support already exists.

use crate::ui::fixtures::RepoFixture;

/// One addressable page in the workspace.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Route {
    /// Home: relevant recent Issues/PRs and the following feed.
    #[default]
    Home,
    /// Global settings. Options are not designed yet; the page is intentionally empty.
    Settings,
    /// The signed-in profile. Overview content is not implemented this round.
    Profile,
    /// Notification center. Content is not designed yet; the page is intentionally empty.
    Notifications,
    /// Cross-repository personal issues. Content is not designed yet.
    Issues,
    /// Cross-repository personal pull requests. Content is not designed yet.
    PullRequests,
    /// A repository identified by its namespace and name; its reader is a later task.
    Repository { namespace: String, name: String },
}

impl Route {
    pub fn repository(repo: &RepoFixture) -> Self {
        Self::Repository {
            namespace: repo.namespace.to_string(),
            name: repo.name.to_string(),
        }
    }

    /// The title the session strip shows for this page.
    pub fn title(&self) -> String {
        match self {
            Self::Home => "Home".to_string(),
            Self::Settings => "Settings".to_string(),
            Self::Profile => "Profile".to_string(),
            Self::Notifications => "Notifications".to_string(),
            Self::Issues => "Issues".to_string(),
            Self::PullRequests => "Pull requests".to_string(),
            Self::Repository { namespace, name } => format!("{namespace}/{name}"),
        }
    }

    pub fn is_repository(&self, repo: &RepoFixture) -> bool {
        matches!(
            self,
            Self::Repository { namespace, name }
                if *namespace == repo.namespace && *name == repo.name
        )
    }
}

/// The window's current route and, later, its page sessions.
pub struct Workspace {
    route: Route,
}

impl Workspace {
    /// A workspace always starts on Home; restoring the last page is not part of the first release.
    pub fn new() -> Self {
        Self { route: Route::Home }
    }

    pub fn route(&self) -> &Route {
        &self.route
    }

    /// Moves to `route`; true when the active page changed.
    pub fn navigate(&mut self, route: Route) -> bool {
        if self.route == route {
            return false;
        }
        self.route = route;
        true
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}
