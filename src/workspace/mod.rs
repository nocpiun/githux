//! Navigation and session memory, pure Rust without framework handles.
//!
//! One window owns one [`Sessions`] container, holding one [`Workspace`] per account
//! plus a separate logged-out workspace. Only the active account's workspace is visible.
//! Switching accounts shows that account's own tabs and keeps every other account's tabs untouched.

use crate::domain::{AccountKey, AccountSummary, HostId, RepoSummary};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(pub(crate) u64);

impl TabId {
    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Route {
    #[default]
    Home,
    Settings,
    Profile,
    UserProfile {
        host: HostId,
        login: String,
    },
    Notifications,
    Issues,
    PullRequests,
    PullRequest {
        namespace: String,
        name: String,
        number: u64,
    },
    Repository {
        namespace: String,
        name: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RouteKey {
    Home,
    Settings,
    Profile,
    Notifications,
    Issues,
    PullRequests,
    UserProfile {
        host: HostId,
        login: String,
    },
    PullRequest {
        namespace: String,
        name: String,
        number: u64,
    },
    Repository {
        namespace: String,
        name: String,
    },
}

impl Route {
    pub fn repository(repo: &RepoSummary) -> Self {
        Self::Repository {
            namespace: repo.owner.login.clone(),
            name: repo.name.clone(),
        }
    }

    pub fn profile(login: impl Into<String>, account: Option<&AccountSummary>) -> Self {
        let login = login.into();
        if account.is_some_and(|a| a.identity.login.eq_ignore_ascii_case(&login)) {
            Self::Profile
        } else {
            Self::UserProfile {
                host: HostId::github(),
                login,
            }
        }
    }

    pub fn profile_login(&self, account: Option<&AccountSummary>) -> Option<String> {
        match self {
            Self::Profile => account.map(|a| a.identity.login.clone()),
            Self::UserProfile { login, .. } => Some(login.clone()),
            _ => None,
        }
    }

    pub fn key(&self) -> RouteKey {
        match self {
            Self::Home => RouteKey::Home,
            Self::Settings => RouteKey::Settings,
            Self::Profile => RouteKey::Profile,
            Self::Notifications => RouteKey::Notifications,
            Self::Issues => RouteKey::Issues,
            Self::PullRequests => RouteKey::PullRequests,
            Self::UserProfile { host, login } => RouteKey::UserProfile {
                host: host.clone(),
                login: login.to_ascii_lowercase(),
            },
            Self::Repository { namespace, name } => RouteKey::Repository {
                namespace: namespace.to_ascii_lowercase(),
                name: name.to_ascii_lowercase(),
            },
            Self::PullRequest {
                namespace,
                name,
                number,
            } => RouteKey::PullRequest {
                namespace: namespace.to_ascii_lowercase(),
                name: name.to_ascii_lowercase(),
                number: *number,
            },
        }
    }

    pub fn title(&self) -> String {
        match self {
            Self::Home => "Home".into(),
            Self::Settings => "Settings".into(),
            Self::Profile => "Profile".into(),
            Self::Notifications => "Notifications".into(),
            Self::Issues => "Issues".into(),
            Self::PullRequests => "Pull requests".into(),
            Self::UserProfile { login, .. } => login.clone(),
            Self::Repository { namespace, name } => format!("{namespace}/{name}"),
            Self::PullRequest {
                namespace,
                name,
                number,
            } => format!("{namespace}/{name} #{number}"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepositoryNavigation {
    pub branch: Option<String>,
    pub file: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub id: TabId,
    pub route: Route,
    pub repository: Option<RepositoryNavigation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    sessions: Vec<Session>,
    active: TabId,
    incarnation: u64,
}

impl Workspace {
    fn new(next: &mut u64) -> Self {
        let home = Self::allocate(next, Route::Home);
        Self {
            active: home.id,
            sessions: vec![home],
            incarnation: 0,
        }
    }

    fn allocate(next: &mut u64, route: Route) -> Session {
        let id = TabId(*next);
        *next = next.checked_add(1).expect("tab identity exhausted");
        let repository =
            matches!(route, Route::Repository { .. }).then(RepositoryNavigation::default);
        Session {
            id,
            route,
            repository,
        }
    }

    pub fn sessions(&self) -> &[Session] {
        &self.sessions
    }

    pub fn active_id(&self) -> TabId {
        self.active
    }

    pub fn incarnation(&self) -> u64 {
        self.incarnation
    }

    pub fn session(&self, id: TabId) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }

    pub fn active(&self) -> &Session {
        self.session(self.active).expect("active tab exists")
    }
}

/// Stable account identity plus a fresh activation generation protects A → B → A.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageIdentity {
    pub account: Option<AccountKey>,
    pub incarnation: u64,
    pub tab: TabId,
    pub resource: RouteKey,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sessions {
    logged_out: Workspace,
    remembered: HashMap<AccountKey, Workspace>,
    visible: Option<AccountKey>,
    next: u64,
}

impl Default for Sessions {
    fn default() -> Self {
        Self::new()
    }
}

impl Sessions {
    pub fn new() -> Self {
        let mut next = 0;
        Self {
            logged_out: Workspace::new(&mut next),
            remembered: HashMap::new(),
            visible: None,
            next,
        }
    }

    pub fn visible_account(&self) -> Option<&AccountKey> {
        self.visible.as_ref()
    }

    pub fn workspace(&self) -> &Workspace {
        match &self.visible {
            Some(key) => &self.remembered[key],
            None => &self.logged_out,
        }
    }

    fn workspace_mut(&mut self) -> &mut Workspace {
        match &self.visible {
            Some(key) => self.remembered.get_mut(key).expect("visible workspace"),
            None => &mut self.logged_out,
        }
    }

    pub fn show(&mut self, account: Option<AccountKey>) -> bool {
        if self.visible == account {
            return false;
        }
        if let Some(key) = &account {
            self.remembered
                .entry(key.clone())
                .or_insert_with(|| Workspace::new(&mut self.next));
        }
        self.visible = account;
        let workspace = self.workspace_mut();
        workspace.incarnation = workspace
            .incarnation
            .checked_add(1)
            .expect("workspace generation exhausted");
        true
    }

    /// Removed accounts lose their navigation; re-adding starts at Home with fresh ids.
    pub fn retain_accounts(&mut self, accounts: &[AccountSummary]) {
        if self
            .visible
            .as_ref()
            .is_some_and(|key| !accounts.iter().any(|a| &a.identity.key == key))
        {
            self.show(None);
        }
        self.remembered
            .retain(|key, _| accounts.iter().any(|a| &a.identity.key == key));
    }

    pub fn open(&mut self, route: Route) -> TabId {
        if let Some(id) = self
            .workspace()
            .sessions
            .iter()
            .find(|s| s.route.key() == route.key())
            .map(|s| s.id)
        {
            self.activate(id);
            return id;
        }
        let session = Workspace::allocate(&mut self.next, route);
        let id = session.id;
        let workspace = self.workspace_mut();
        workspace.sessions.push(session);
        workspace.active = id;
        id
    }

    pub fn activate(&mut self, id: TabId) -> bool {
        let workspace = self.workspace_mut();
        if workspace.active == id || workspace.session(id).is_none() {
            return false;
        }
        workspace.active = id;
        true
    }

    pub fn reorder(&mut self, id: TabId, target: usize) -> bool {
        let workspace = self.workspace_mut();
        let Some(from) = workspace.sessions.iter().position(|s| s.id == id) else {
            return false;
        };
        let target = target.min(workspace.sessions.len().saturating_sub(1));
        if from == target {
            return false;
        }
        let session = workspace.sessions.remove(from);
        workspace.sessions.insert(target, session);
        true
    }

    pub fn cycle(&mut self, by: isize) -> Option<TabId> {
        let workspace = self.workspace_mut();
        if workspace.sessions.len() < 2 {
            return None;
        }
        let at = workspace
            .sessions
            .iter()
            .position(|s| s.id == workspace.active)?;
        let next = (at as isize + by).rem_euclid(workspace.sessions.len() as isize) as usize;
        workspace.active = workspace.sessions[next].id;
        Some(workspace.active)
    }

    pub fn close(&mut self, id: TabId) -> bool {
        let Some(at) = self.workspace().sessions.iter().position(|s| s.id == id) else {
            return false;
        };
        let workspace = self.workspace_mut();
        workspace.sessions.remove(at);
        if workspace.sessions.is_empty() {
            let home = Workspace::allocate(&mut self.next, Route::Home);
            let workspace = self.workspace_mut();
            workspace.active = home.id;
            workspace.sessions.push(home);
        } else if workspace.active == id {
            workspace.active = workspace.sessions[at.saturating_sub(1)].id;
        }
        true
    }

    pub fn repository_mut(&mut self, id: TabId) -> Option<&mut RepositoryNavigation> {
        self.workspace_mut()
            .sessions
            .iter_mut()
            .find(|s| s.id == id)?
            .repository
            .as_mut()
    }

    pub fn identity(&self, id: TabId) -> Option<PageIdentity> {
        let workspace = self.workspace();
        Some(PageIdentity {
            account: self.visible.clone(),
            incarnation: workspace.incarnation,
            tab: id,
            resource: workspace.session(id)?.route.key(),
        })
    }

    pub fn is_current(&self, identity: &PageIdentity) -> bool {
        self.identity(identity.tab).as_ref() == Some(identity)
    }

    /// Call only after checking the original page ticket; canonicalization may not retarget a tab.
    pub fn canonicalize_profile(&mut self, identity: &PageIdentity, login: String) -> bool {
        if !self.is_current(identity) {
            return false;
        }
        let Some(session) = self
            .workspace_mut()
            .sessions
            .iter_mut()
            .find(|s| s.id == identity.tab)
        else {
            return false;
        };
        if let Route::UserProfile { login: current, .. } = &mut session.route {
            if !current.eq_ignore_ascii_case(&login) {
                return false;
            }
            *current = login;
            true
        } else {
            false
        }
    }
}

impl std::ops::Deref for Sessions {
    type Target = Workspace;

    fn deref(&self) -> &Workspace {
        self.workspace()
    }
}

impl std::ops::DerefMut for Sessions {
    fn deref_mut(&mut self) -> &mut Workspace {
        self.workspace_mut()
    }
}
