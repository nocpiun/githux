//! Window workspace and page sessions: routing, tab identity, activation and page-local state.
//!
//! One window owns one [`Workspace`]. It keeps an ordered collection of [`Session`]s — the
//! application-level tabs drawn at the top of the island — and exactly one of them is active.
//! Opening a page activates the session that already addresses that resource, or creates a new
//! one; every resource is a single session, so clicking an entry twice never duplicates a tab.
//! Closing the last session creates a fresh Home session. [`TabId`]s increase monotonically and
//! are never reused, so page state keyed by an old id can never attach to a later session.
//!
//! Each session owns the page-local state that has to survive switching: a real GPUI
//! [`ScrollHandle`] and [`FocusHandle`], plus the repository browsing selection the reader will
//! fill in during R3. The shell drops a session's state when the session closes.
//!
//! There is no ceiling on open sessions. Once the strip runs out of width the shell scrolls it
//! horizontally instead of shrinking tabs further or refusing to open another page.

use gpui::{App, FocusHandle, ScrollHandle};

use crate::ui::fixtures::RepoFixture;

/// Stable identity of one session. Ids increase monotonically and a closed id is never handed out
/// again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(u64);

impl TabId {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// The comparable identity of a route. GitHub treats owner and repository names
/// case-insensitively, so two spellings of one repository resolve to one session; the route keeps
/// the first spelling it was opened with for display.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RouteKey {
    Home,
    Settings,
    Profile,
    Notifications,
    Issues,
    PullRequests,
    Repository { namespace: String, name: String },
}

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

    pub fn is_repository(&self, repo: &RepoFixture) -> bool {
        self.key() == Self::repository(repo).key()
    }

    /// The comparable resource identity; see [`RouteKey`].
    pub fn key(&self) -> RouteKey {
        match self {
            Self::Home => RouteKey::Home,
            Self::Settings => RouteKey::Settings,
            Self::Profile => RouteKey::Profile,
            Self::Notifications => RouteKey::Notifications,
            Self::Issues => RouteKey::Issues,
            Self::PullRequests => RouteKey::PullRequests,
            Self::Repository { namespace, name } => RouteKey::Repository {
                namespace: namespace.to_ascii_lowercase(),
                name: name.to_ascii_lowercase(),
            },
        }
    }

    /// The label the session tab shows. Repositories show their full name; the tab strips the
    /// text with an ellipsis and keeps the full name in a tooltip.
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
}

/// Repository browsing state a session carries for the reader that arrives in R3. It stays empty
/// until that reader sets it; the session-isolation tests drive it through the shell. This is
/// session state, not cached API data.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepositorySession {
    pub branch: Option<String>,
    pub file: Option<String>,
}

/// Page-local state owned by one session.
///
/// `scroll` and `focus` are the real GPUI handles the active page tracks, so switching sessions
/// restores that page's position and focus; closing the session drops them. `strip_focus` is the
/// tab's own tab stop in the strip; it sits at the front of the tab order (index -1) so keyboard
/// navigation reaches the strip before the long repository list.
pub struct PageState {
    pub scroll: ScrollHandle,
    /// Wheel smoothing for this page's scroll region. It belongs to the session so switching tabs
    /// never carries one page's easing into another.
    pub smooth: crate::ui::scrolling::SmoothScroll,
    pub focus: FocusHandle,
    pub strip_focus: FocusHandle,
    pub repository: Option<RepositorySession>,
}

impl PageState {
    fn new(route: &Route, cx: &mut App) -> Self {
        let scroll = ScrollHandle::new();
        Self {
            smooth: crate::ui::scrolling::SmoothScroll::new(scroll.clone()),
            scroll,
            focus: cx.focus_handle(),
            strip_focus: cx.focus_handle().tab_stop(true).tab_index(-1),
            repository: matches!(route, Route::Repository { .. }).then(RepositorySession::default),
        }
    }
}

/// One open page session.
pub struct Session {
    id: TabId,
    route: Route,
    state: PageState,
}

impl Session {
    pub fn id(&self) -> TabId {
        self.id
    }

    pub fn route(&self) -> &Route {
        &self.route
    }

    pub fn state(&self) -> &PageState {
        &self.state
    }

    /// The reader that arrives in R3 mutates this; tests drive it until then.
    #[cfg(test)]
    pub fn state_mut(&mut self) -> &mut PageState {
        &mut self.state
    }
}

/// What one [`Workspace::open`] call did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenResult {
    /// The resource already had a session; that session became active.
    Activated(TabId),
    /// A new session was created and became active.
    Opened(TabId),
}

/// What one [`Workspace::close`] call did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CloseResult {
    pub closed: TabId,
    pub active: TabId,
}

/// The window's session collection and active session.
pub struct Workspace {
    sessions: Vec<Session>,
    active: TabId,
    next_id: u64,
}

impl Workspace {
    /// A workspace always starts on Home; restoring the last page is not part of the first
    /// release.
    pub fn new(cx: &mut App) -> Self {
        let mut workspace = Self {
            sessions: Vec::new(),
            active: TabId(0),
            next_id: 0,
        };
        let home = workspace.push(Route::Home, cx);
        workspace.active = home;
        workspace
    }

    fn push(&mut self, route: Route, cx: &mut App) -> TabId {
        let id = TabId(self.next_id);
        self.next_id += 1;
        let state = PageState::new(&route, cx);
        self.sessions.push(Session { id, route, state });
        id
    }

    pub fn sessions(&self) -> &[Session] {
        &self.sessions
    }

    pub fn active_id(&self) -> TabId {
        self.active
    }

    pub fn active(&self) -> &Session {
        self.session(self.active)
            .expect("the active session is always present")
    }

    pub fn route(&self) -> &Route {
        self.active().route()
    }

    pub fn session(&self, id: TabId) -> Option<&Session> {
        self.sessions.iter().find(|session| session.id == id)
    }

    #[cfg(test)]
    pub fn session_mut(&mut self, id: TabId) -> Option<&mut Session> {
        self.sessions.iter_mut().find(|session| session.id == id)
    }

    pub fn position(&self, id: TabId) -> Option<usize> {
        self.sessions.iter().position(|session| session.id == id)
    }

    /// Opens or activates the session for `route`. A resource that is already open is activated
    /// instead of duplicated; a resource that is not open always gets a new session, however many
    /// are already open.
    pub fn open(&mut self, route: Route, cx: &mut App) -> OpenResult {
        let key = route.key();
        if let Some(session) = self
            .sessions
            .iter()
            .find(|session| session.route.key() == key)
        {
            let id = session.id;
            self.active = id;
            return OpenResult::Activated(id);
        }
        let id = self.push(route, cx);
        self.active = id;
        OpenResult::Opened(id)
    }

    /// Makes `id` active; false when it is unknown or already active.
    pub fn activate(&mut self, id: TabId) -> bool {
        if self.active == id || self.session(id).is_none() {
            return false;
        }
        self.active = id;
        true
    }

    /// Moves the session `id` to `target`, an index into the visual order. Nothing is closed or
    /// recreated: the same [`TabId`] keeps its route, scroll, focus and repository selection, and
    /// the active session does not change. `false` when `id` is unknown or already in place.
    pub fn reorder(&mut self, id: TabId, target: usize) -> bool {
        let Some(from) = self.position(id) else {
            return false;
        };
        let target = target.min(self.sessions.len().saturating_sub(1));
        if from == target {
            return false;
        }
        let session = self.sessions.remove(from);
        self.sessions.insert(target, session);
        true
    }

    /// Moves the active session by `by` positions, wrapping around; `None` when there is nothing
    /// to switch to.
    pub fn cycle(&mut self, by: isize) -> Option<TabId> {
        if self.sessions.len() < 2 {
            return None;
        }
        let index = self.position(self.active)?;
        let len = self.sessions.len() as isize;
        let next = (index as isize + by).rem_euclid(len) as usize;
        self.active = self.sessions[next].id;
        Some(self.active)
    }

    /// Closes `id`. Closing a non-active session keeps the current one active; closing the active
    /// session activates its left neighbour, or the right one when it has no left neighbour.
    /// Closing the last session creates a fresh Home session.
    pub fn close(&mut self, id: TabId, cx: &mut App) -> Option<CloseResult> {
        let index = self.position(id)?;
        let was_active = self.active == id;
        self.sessions.remove(index);
        if was_active {
            if self.sessions.is_empty() {
                let home = self.push(Route::Home, cx);
                self.active = home;
            } else {
                self.active = self.sessions[index.saturating_sub(1)].id;
            }
        }
        Some(CloseResult {
            closed: id,
            active: self.active,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn workspace(cx: &mut TestAppContext) -> Workspace {
        cx.update(Workspace::new)
    }

    fn repo(namespace: &str, name: &str) -> Route {
        Route::Repository {
            namespace: namespace.to_string(),
            name: name.to_string(),
        }
    }

    #[gpui::test]
    fn a_workspace_starts_with_one_active_home_session(cx: &mut TestAppContext) {
        let workspace = workspace(cx);
        assert_eq!(workspace.sessions().len(), 1);
        assert_eq!(workspace.route(), &Route::Home);
    }

    #[gpui::test]
    fn opening_a_resource_twice_reuses_its_session(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let mut workspace = Workspace::new(cx);
            let opened = workspace.open(repo("githux-dev", "githux"), cx);
            let first = match opened {
                OpenResult::Opened(id) => id,
                other => panic!("expected a new session, got {other:?}"),
            };
            assert_eq!(
                workspace.open(repo("githux-dev", "githux"), cx),
                OpenResult::Activated(first)
            );
            assert_eq!(workspace.sessions().len(), 2);
        });
    }

    #[gpui::test]
    fn github_case_aliases_resolve_to_one_session(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let mut workspace = Workspace::new(cx);
            let opened = match workspace.open(repo("Githux-Dev", "GitHux"), cx) {
                OpenResult::Opened(id) => id,
                other => panic!("expected a new session, got {other:?}"),
            };
            assert_eq!(
                workspace.open(repo("githux-dev", "githux"), cx),
                OpenResult::Activated(opened)
            );
            assert_eq!(workspace.sessions().len(), 2);
            assert_eq!(workspace.route(), &repo("Githux-Dev", "GitHux"));
        });
    }

    #[gpui::test]
    fn any_number_of_sessions_can_open(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let mut workspace = Workspace::new(cx);
            for index in 0..48 {
                assert!(matches!(
                    workspace.open(repo("owner", &format!("repo-{index}")), cx),
                    OpenResult::Opened(_)
                ));
            }
            assert_eq!(workspace.sessions().len(), 49, "Home plus 48 repositories");
            // Re-opening an existing resource still activates instead of adding one.
            assert!(matches!(
                workspace.open(Route::Home, cx),
                OpenResult::Activated(_)
            ));
            assert_eq!(workspace.sessions().len(), 49);
        });
    }

    #[gpui::test]
    fn closing_picks_the_left_neighbour_then_the_right_one(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let mut workspace = Workspace::new(cx);
            let a = match workspace.open(repo("owner", "a"), cx) {
                OpenResult::Opened(id) => id,
                other => panic!("expected a new session, got {other:?}"),
            };
            let b = match workspace.open(repo("owner", "b"), cx) {
                OpenResult::Opened(id) => id,
                other => panic!("expected a new session, got {other:?}"),
            };
            let c = match workspace.open(repo("owner", "c"), cx) {
                OpenResult::Opened(id) => id,
                other => panic!("expected a new session, got {other:?}"),
            };
            assert_eq!(workspace.route(), &repo("owner", "c"));

            // Closing a non-active session keeps the current one.
            let closed = workspace.close(a, cx).expect("a is open");
            assert_eq!(closed.active, c);
            assert_eq!(workspace.route(), &repo("owner", "c"));

            // Closing the active session moves left.
            let closed = workspace.close(c, cx).expect("c is open");
            assert_eq!(closed.active, b);
            assert_eq!(workspace.route(), &repo("owner", "b"));

            // The only session has no left neighbour: closing it opens Home.
            let closed = workspace.close(b, cx).expect("b is open");
            assert_ne!(closed.active, b, "Home gets a fresh id");
            assert_eq!(workspace.sessions().len(), 1);
            assert_eq!(workspace.route(), &Route::Home);
        });
    }

    #[gpui::test]
    fn reordering_keeps_every_tab_identity_and_the_active_session(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let mut workspace = Workspace::new(cx);
            let ids: Vec<TabId> = ["a", "b", "c"]
                .into_iter()
                .map(|name| match workspace.open(repo("owner", name), cx) {
                    OpenResult::Opened(id) => id,
                    other => panic!("expected a new session, got {other:?}"),
                })
                .collect();
            let active = workspace.active_id();
            let routes: Vec<Route> = workspace
                .sessions()
                .iter()
                .map(|session| session.route().clone())
                .collect();

            let home = workspace.sessions()[0].id();
            let order = |workspace: &Workspace| {
                workspace
                    .sessions()
                    .iter()
                    .map(Session::id)
                    .collect::<Vec<_>>()
            };

            // Forward: the first repository tab goes last.
            assert!(workspace.reorder(ids[0], 3));
            assert_eq!(order(&workspace), vec![home, ids[1], ids[2], ids[0]]);

            // Backward: the last tab goes first.
            assert!(workspace.reorder(ids[0], 0));
            assert_eq!(order(&workspace), vec![ids[0], home, ids[1], ids[2]]);

            // No-op targets and an unknown id change nothing.
            assert!(!workspace.reorder(ids[1], 2));
            assert!(!workspace.reorder(TabId(9_999), 0));
            assert_eq!(order(&workspace), vec![ids[0], home, ids[1], ids[2]]);

            // The active session and every route follow their own id, not the position.
            assert_eq!(workspace.active_id(), active);
            let mut by_id: Vec<Route> = workspace
                .sessions()
                .iter()
                .map(|session| session.route().clone())
                .collect();
            let mut expected = routes;
            by_id.sort_by_key(|route| route.title());
            expected.sort_by_key(|route| route.title());
            assert_eq!(by_id, expected);
        });
    }

    #[gpui::test]
    fn reordering_changes_the_cycle_and_close_neighbours(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let mut workspace = Workspace::new(cx);
            let home = workspace.active_id();
            let ids: Vec<TabId> = ["a", "b"]
                .into_iter()
                .map(|name| match workspace.open(repo("owner", name), cx) {
                    OpenResult::Opened(id) => id,
                    other => panic!("expected a new session, got {other:?}"),
                })
                .collect();

            // Home, a, b -> b, a, Home: the visual order is now what cycling follows.
            assert!(workspace.reorder(home, 2));
            assert_eq!(
                workspace
                    .sessions()
                    .iter()
                    .map(Session::id)
                    .collect::<Vec<_>>(),
                vec![ids[0], ids[1], home]
            );
            assert!(workspace.activate(ids[0]));
            assert_eq!(workspace.cycle(1), Some(ids[1]));
            assert_eq!(workspace.cycle(1), Some(home), "cycles after the reorder");
            assert_eq!(workspace.cycle(1), Some(ids[0]));

            // Closing the active tab now picks its visual left neighbour.
            assert!(workspace.activate(ids[1]));
            let closed = workspace.close(ids[1], cx).expect("b is open");
            assert_eq!(closed.active, ids[0], "left neighbour after reordering");

            // A bare reorder never closes, recreates or re-ids anything.
            assert_eq!(workspace.sessions().len(), 2);
        });
    }

    #[gpui::test]
    fn a_closed_id_is_never_reused(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let mut workspace = Workspace::new(cx);
            let opened = match workspace.open(repo("owner", "a"), cx) {
                OpenResult::Opened(id) => id,
                other => panic!("expected a new session, got {other:?}"),
            };
            workspace.close(opened, cx).expect("a is open");
            let reopened = match workspace.open(repo("owner", "a"), cx) {
                OpenResult::Opened(id) => id,
                other => panic!("expected a new session, got {other:?}"),
            };
            assert!(reopened > opened, "{reopened:?} must not reuse {opened:?}");
        });
    }

    #[gpui::test]
    fn cycling_wraps_both_ways(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let mut workspace = Workspace::new(cx);
            workspace.open(repo("owner", "a"), cx);
            workspace.open(repo("owner", "b"), cx);
            let ids: Vec<TabId> = workspace.sessions().iter().map(Session::id).collect();
            assert_eq!(ids.len(), 3);

            workspace.cycle(1);
            assert_eq!(workspace.active_id(), ids[0]);
            workspace.cycle(-1);
            assert_eq!(workspace.active_id(), ids[2]);
        });
    }

    #[gpui::test]
    fn sessions_own_separate_scroll_focus_and_repository_state(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let mut workspace = Workspace::new(cx);
            let a = match workspace.open(repo("owner", "a"), cx) {
                OpenResult::Opened(id) => id,
                other => panic!("expected a new session, got {other:?}"),
            };
            let b = match workspace.open(repo("owner", "b"), cx) {
                OpenResult::Opened(id) => id,
                other => panic!("expected a new session, got {other:?}"),
            };
            assert_ne!(a, b);

            let a_state = workspace.session_mut(a).expect("a is open");
            a_state
                .state_mut()
                .scroll
                .set_offset(gpui::point(gpui::px(0.0), gpui::px(-120.0)));
            a_state
                .state_mut()
                .repository
                .as_mut()
                .expect("repository")
                .branch = Some("main".to_string());
            a_state
                .state_mut()
                .repository
                .as_mut()
                .expect("repository")
                .file = Some("README.md".to_string());

            let b_state = workspace.session_mut(b).expect("b is open");
            b_state
                .state_mut()
                .repository
                .as_mut()
                .expect("repository")
                .branch = Some("release".to_string());

            let a_state = workspace.session(a).expect("a is open").state();
            let b_state = workspace.session(b).expect("b is open").state();
            assert_eq!(a_state.scroll.offset().y, gpui::px(-120.0));
            assert_eq!(b_state.scroll.offset().y, gpui::px(0.0));
            assert_ne!(a_state.focus, b_state.focus);
            assert_eq!(
                a_state
                    .repository
                    .as_ref()
                    .and_then(|r| r.branch.as_deref()),
                Some("main")
            );
            assert_eq!(
                b_state
                    .repository
                    .as_ref()
                    .and_then(|r| r.branch.as_deref()),
                Some("release")
            );
            assert_eq!(
                b_state.repository.as_ref().and_then(|r| r.file.as_deref()),
                None
            );
        });
    }
}
