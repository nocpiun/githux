//! A reusable, real-data repository browsing page.
//!
//! The page owns one browsing generation at a time: a verified
//! [`RepositorySession`] captured from the host's [`AccountService`], the
//! repository summary, the branch list, one fixed-commit snapshot, the lazily
//! expanded directory tree and the reader showing the README or a selected file.
//!
//! ## Integration boundary
//!
//! ```ignore
//! let services = RepositoryPageServices::new(accounts.clone(), executor.clone());
//! let page = cx.new(|cx| {
//!     let mut page = RepositoryPage::new(services, locator, saved_navigation, window, cx);
//!     page.activate(cx);          // opens a fresh, verified repository session
//!     page
//! });
//! // Teardown: `deactivate` cancels reads and keeps navigation intent;
//! // `rebind(new_services)` restores that intent under new authorization.
//! ```
//!
//! The host supplies only capabilities that already exist: `Arc<AccountService>`,
//! `Arc<BackgroundExecutor>` and a `RepositoryLocator`. The page never opens a
//! database, reads a credential or fetches a PAT itself; every read goes through
//! `AccountService::repo_service()`.
//!
//! ## Publication
//!
//! Every background result is checked before it can change anything: the page
//! generation, the captured authorization context, the fixed commit, and the
//! resource identity the result belongs to. A stale success and a stale error
//! are discarded the same way, so switching branch, account (A→B→A), replacing
//! the PAT or hiding the page can never paste an old result over a new one.

mod navigation;
mod page_tabs;
#[cfg(test)]
mod tests;
mod view;

pub use page_tabs::RepositoryTab;

use std::collections::{BTreeSet, VecDeque};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Render, Styled, Subscription, Task, UniformListScrollHandle, WeakEntity, Window,
    div, prelude::*, px,
};
use tokio::sync::watch;

use crate::content::PreparedContent;
use crate::data::{AccountService, RepositoryReadContext, RepositorySession, Scoped};
use crate::domain::blob::{BlobIdentity, BlobLimits};
use crate::domain::repository::{
    BranchesOutcome, DirectoryTree, DynamicRef, ObjectSha, RepoSnapshot, RepositoryDetails,
    RepositoryError, RepositoryIdentity, RepositoryLocator, RevisionSpec, SnapshotDirectory,
    TreeEntryKind,
};
use crate::github::RepositoryLimits;
use crate::network::cancel::{CancellationToken, CancellationTokenSource};
use crate::network::executor::BackgroundExecutor;
use crate::ui::reader::{DocumentKind, LinkTarget, ReaderAction, ReaderView};

use navigation::{BranchChoice, RowState, TreeModel, branch_choice};
pub use navigation::{PageError, ReadmeState, RepositoryNavigation, TreeRow, TreeRowKind};

/// Width of the bounded file tree column.
pub const TREE_WIDTH: f32 = 268.0;

/// Height of one file-tree row.
pub const TREE_ROW_HEIGHT: f32 = 26.0;

/// The capabilities a host injects. They are the only way this page reaches
/// GitHub; nothing here opens storage or credentials on its own.
#[derive(Clone)]
pub struct RepositoryPageServices {
    accounts: Arc<AccountService>,
    executor: Arc<BackgroundExecutor>,
    limits: PageLimits,
}

/// Engineering limits, overridable by tests and previews.
#[derive(Clone, Debug, Default)]
pub struct PageLimits {
    pub repository: RepositoryLimits,
    pub blob: BlobLimits,
}

impl RepositoryPageServices {
    pub fn new(accounts: Arc<AccountService>, executor: Arc<BackgroundExecutor>) -> Self {
        Self {
            accounts,
            executor,
            limits: PageLimits::default(),
        }
    }

    pub fn with_limits(mut self, limits: PageLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn accounts(&self) -> &Arc<AccountService> {
        &self.accounts
    }
}

/// What the page asks the host to do on its behalf.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageAction {
    /// An absolute `http(s)` URL the user pressed. The page never opens it.
    OpenExternal(String),
    /// A repository link that this page generation did not follow.
    LinkNotFollowed { path: String, reason: String },
}

pub type OnPageAction = Rc<dyn Fn(PageAction, &mut Window, &mut App)>;

/// Which document the reader shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    /// The repository's README at the fixed commit, as identified by the
    /// descriptor's own path (root, `docs/`, `.github/`, …).
    Readme,
    /// A file at the fixed commit.
    File(String),
}

impl Selection {
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::Readme => None,
            Self::File(path) => Some(path),
        }
    }
}

/// The reader's current presentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReaderStatus {
    /// A content request is in flight.
    Loading,
    /// A [`ReaderView`] is on screen.
    Showing,
    /// A complete empty root tree proved there is no README.
    NoReadme,
    /// A 404 that cannot distinguish absence from lost visibility.
    ReadmeUncertain,
    /// README discovery failed.
    ReadmeUnavailable(PageError),
    /// The selected path is not a readable file at this commit.
    SelectionUnavailable { reason: String },
    /// Reading the selected document failed.
    ContentFailed(PageError),
}

impl ReaderStatus {
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Loading => "loading",
            Self::Showing => "showing",
            Self::NoReadme => "no-readme",
            Self::ReadmeUncertain => "readme-uncertain",
            Self::ReadmeUnavailable(_) => "readme-unavailable",
            Self::SelectionUnavailable { .. } => "selection-unavailable",
            Self::ContentFailed(_) => "content-failed",
        }
    }
}

/// One fixed-commit browsing snapshot plus its lazily loaded tree.
struct BranchState {
    name: String,
    snapshot: RepoSnapshot,
    tree: TreeModel,
    readme: ReadmeState,
}

impl BranchState {
    fn commit(&self) -> &ObjectSha {
        self.snapshot.commit()
    }
}

/// One browsing generation's in-flight work.
struct Work {
    token: CancellationToken,
    source: CancellationTokenSource,
}

impl Work {
    fn new() -> Self {
        let (token, source) = CancellationToken::new();
        Self { token, source }
    }

    fn token(&self) -> CancellationToken {
        self.token.clone()
    }
}

/// A background result. Every message carries the page generation and the
/// authorization context captured when the request started.
struct Message {
    generation: u64,
    context: RepositoryReadContext,
    body: Body,
}

enum Body {
    Repository(Result<RepositoryDetails, RepositoryError>),
    Branches(Result<BranchesOutcome, RepositoryError>),
    Revision {
        branch: String,
        request: u64,
        result: Result<RepoSnapshot, RepositoryError>,
    },
    Root {
        branch: String,
        commit: ObjectSha,
        result: Result<DirectoryTree, RepositoryError>,
    },
    Readme {
        branch: String,
        commit: ObjectSha,
        result: Result<ReadmeState, RepositoryError>,
    },
    Directory {
        branch: String,
        commit: ObjectSha,
        path: String,
        result: Result<DirectoryTree, RepositoryError>,
    },
    Content(ContentReply),
}

/// One content reply, bundled so the publication check reads as one step.
struct ContentReply {
    branch: String,
    commit: ObjectSha,
    role: Role,
    path: String,
    result: Result<PreparedContent, RepositoryError>,
}

/// Which document a content request answers.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Role {
    Readme,
    File(String),
}

impl Role {
    fn matches(&self, selection: &Selection) -> bool {
        match (self, selection) {
            (Self::Readme, Selection::Readme) => true,
            (Self::File(role), Selection::File(path)) => role == path,
            _ => false,
        }
    }
}

/// The page view.
pub struct RepositoryPage {
    services: RepositoryPageServices,
    locator: RepositoryLocator,
    generation: u64,
    session: Option<RepositorySession>,
    context: Option<RepositoryReadContext>,
    work: Option<Work>,
    /// Navigation intent: branch, selected path and expanded directories. This
    /// is the only thing a host is expected to save, and the only thing a
    /// rebind keeps.
    intent: RepositoryNavigation,
    details: Option<RepositoryDetails>,
    branches: Option<BranchesOutcome>,
    branches_error: Option<PageError>,
    branch: Option<BranchState>,
    /// Monotonic identity of the newest branch resolution. A → B → A shares the
    /// page generation, so a branch name alone cannot tell an older A reply from
    /// the current one; this does.
    branch_request: u64,
    selection: Selection,
    /// The README's own descriptor path, once discovery found one.
    readme_path: Option<String>,
    reader: Option<Entity<ReaderView>>,
    reader_status: ReaderStatus,
    /// A page-level failure (repository summary, branch list, revision).
    error: Option<PageError>,
    /// The in-page content tab. Only `Code` has content this round.
    tab: RepositoryTab,
    /// True while a browsing generation is running.
    active: bool,
    /// Set when authorization changed under a running generation.
    authorization_lost: bool,
    tree_scroll: UniformListScrollHandle,
    tree_motion: crate::ui::scroll::Motion<gpui::ScrollHandle>,
    tree_bars: crate::ui::scroll::Bars,
    /// Flattened visible rows, rebuilt only when the tree or expansion changes.
    rows: Rc<Vec<TreeRow>>,
    rows_dirty: bool,
    rows_capped: bool,
    /// The keyboard's row, as a stable path rather than an index.
    cursor: Option<String>,
    tree_focus: FocusHandle,
    clock: std::time::Instant,
    #[cfg(test)]
    pinned_clock: Option<u64>,
    _window_activation: Subscription,
    inbox: Arc<Mutex<VecDeque<Message>>>,
    alive: Arc<watch::Sender<bool>>,
    receiver: Option<Task<()>>,
    on_action: Option<OnPageAction>,
}

impl RepositoryPage {
    /// Builds an inactive page. Call [`RepositoryPage::activate`] once the host
    /// is ready for the page to read.
    pub fn new(
        services: RepositoryPageServices,
        locator: RepositoryLocator,
        intent: RepositoryNavigation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (alive, _) = watch::channel(true);
        let inbox = Arc::new(Mutex::new(VecDeque::new()));
        let tree_scroll = UniformListScrollHandle::new();
        let base = tree_scroll.0.borrow().base_handle.clone();
        let mut page = Self {
            services,
            locator,
            generation: 0,
            session: None,
            context: None,
            work: None,
            selection: match &intent.path {
                Some(path) => Selection::File(path.clone()),
                None => Selection::Readme,
            },
            intent,
            details: None,
            branches: None,
            branches_error: None,
            branch: None,
            branch_request: 0,
            readme_path: None,
            reader: None,
            reader_status: ReaderStatus::Loading,
            error: None,
            tab: RepositoryTab::Code,
            active: false,
            authorization_lost: false,
            tree_scroll,
            tree_motion: crate::ui::scroll::Motion::new(base),
            tree_bars: crate::ui::scroll::Bars::new(),
            rows: Rc::new(Vec::new()),
            rows_dirty: false,
            rows_capped: false,
            cursor: None,
            tree_focus: cx.focus_handle().tab_stop(true),
            clock: std::time::Instant::now(),
            #[cfg(test)]
            pinned_clock: None,
            _window_activation: cx.observe_window_activation(window, |page, window, cx| {
                if !window.is_window_active() {
                    page.cancel_scroll(cx);
                }
            }),
            inbox: inbox.clone(),
            alive: Arc::new(alive),
            receiver: None,
            on_action: None,
        };
        page.start_receiver(cx);
        page
    }

    /// The navigation intent a host may save and restore.
    pub fn navigation(&self) -> RepositoryNavigation {
        self.intent.clone()
    }

    /// Takes page actions (external links and unfollowed repository links).
    pub fn on_action(&mut self, handler: OnPageAction) {
        self.on_action = Some(handler);
    }

    /// Whether a browsing generation is running.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Starts (or restarts) reading under a freshly captured session.
    ///
    /// A new session is always created; a saved branch or file is restored
    /// against that new authorization and re-resolved to a fixed commit.
    pub fn activate(&mut self, cx: &mut Context<Self>) {
        self.stop_work();
        self.generation = self.generation.wrapping_add(1);
        self.authorization_lost = false;
        // Clear every authorization-bound value *before* capturing a session:
        // acquiring one can fail (for example the active account was removed),
        // and the previous account's summary/tree/reader must not survive that
        // early return.
        self.active = false;
        self.session = None;
        self.context = None;
        self.details = None;
        self.branches = None;
        self.branches_error = None;
        self.branch = None;
        self.reader = None;
        self.readme_path = None;
        self.reader_status = ReaderStatus::Loading;
        self.cursor = None;
        self.invalidate_rows();
        let session = match self.services.accounts.repo_service().session() {
            Ok(session) => session,
            Err(error) => {
                self.error = Some(PageError::from_repository(&error));
                cx.notify();
                return;
            }
        };
        self.context = Some(session.context().clone());
        self.session = Some(session);
        self.active = true;
        self.error = None;
        self.work = Some(Work::new());
        self.spawn_job(Job::Repository(self.locator.clone()));
        cx.notify();
    }

    /// Cancels in-flight work and keeps only navigation intent.
    ///
    /// Nothing that finishes later can replace the page's content, because the
    /// generation has already moved on.
    pub fn deactivate(&mut self, cx: &mut Context<Self>) {
        self.cancel_reader_interactions(cx);
        self.stop_work();
        self.generation = self.generation.wrapping_add(1);
        self.active = false;
        self.session = None;
        self.context = None;
        if self.details.is_some() || self.error.is_some() || self.branch.is_some() {
            self.error = Some(PageError::deactivated());
        }
        cx.notify();
    }

    /// Replaces the injected capabilities and re-reads the saved navigation
    /// under a new session.
    pub fn rebind(&mut self, services: RepositoryPageServices, cx: &mut Context<Self>) {
        self.cancel_reader_interactions(cx);
        self.stop_work();
        self.generation = self.generation.wrapping_add(1);
        self.active = false;
        self.session = None;
        self.context = None;
        // Data read under the previous authorization must not stay visible.
        self.details = None;
        self.branches = None;
        self.branches_error = None;
        self.branch = None;
        self.reader = None;
        self.readme_path = None;
        self.reader_status = ReaderStatus::Loading;
        self.error = None;
        self.authorization_lost = false;
        self.cursor = None;
        self.invalidate_rows();
        self.services = services;
        self.activate(cx);
    }

    /// One user-initiated retry of the failed step. There is no automatic retry
    /// loop; each press starts a new bounded attempt of that step only.
    pub fn retry(&mut self, cx: &mut Context<Self>) {
        if !self.active || self.authorization_lost || self.details.is_none() {
            self.activate(cx);
            return;
        }
        let Some(branch) = self.branch.as_ref().map(|branch| branch.name.clone()) else {
            if let Some(details) = self.details.clone() {
                self.spawn_job(Job::Branches(details.identity));
            }
            cx.notify();
            return;
        };
        self.begin_branch(branch, cx);
    }

    /// Switches to `name`, re-resolving it to a new fixed commit. Old tree and
    /// reader content are dropped immediately.
    pub fn select_branch(&mut self, name: &str, cx: &mut Context<Self>) {
        if !self.active || self.authorization_lost {
            return;
        }
        if let Ok(branch) = DynamicRef::new(name) {
            self.begin_branch(branch.as_str().to_string(), cx);
        }
    }

    /// Switches the in-page content tab.
    ///
    /// Leaving Code stops drawing the tree and reader for that tab but keeps the
    /// navigation intent, so returning restores it under the still-valid session.
    pub fn select_tab(&mut self, tab: RepositoryTab, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        if tab != RepositoryTab::Code {
            // Hidden content must not keep a gesture alive.
            self.cancel_reader_interactions(cx);
        } else {
            // Returning to Code: if authorization was lost while hidden, the page
            // must re-verify rather than show data from an old session.
            self.check_authorization(cx);
        }
        cx.notify();
    }

    pub fn tab(&self) -> RepositoryTab {
        self.tab
    }

    /// Expands or collapses a directory, loading it the first time it opens.
    pub fn toggle_directory(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.intent.expanded.remove(path) {
            self.invalidate_rows();
        } else {
            self.intent.expanded.insert(path.to_string());
            self.invalidate_rows();
            self.load_directory(path, cx);
        }
        cx.notify();
    }

    /// Expands a directory (no-op when already open), loading it if needed.
    fn expand_directory(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.intent.expanded.insert(path.to_string()) {
            self.invalidate_rows();
            self.load_directory(path, cx);
            cx.notify();
        }
    }

    fn collapse_directory(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.intent.expanded.remove(path) {
            self.invalidate_rows();
            cx.notify();
        }
    }

    /// Selects a file at the current fixed commit.
    pub fn select_file(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.branch.is_none() {
            return;
        }
        self.intent.path = Some(path.to_string());
        self.selection = Selection::File(path.to_string());
        self.reader = None;
        self.readme_path = None;
        self.reader_status = ReaderStatus::Loading;
        self.cursor = Some(path.to_string());
        self.resolve_selection(cx);
        cx.notify();
    }

    /// Returns the reader to the repository README (its real path, wherever it is).
    pub fn show_readme(&mut self, cx: &mut Context<Self>) {
        if self.branch.is_none() {
            return;
        }
        self.intent.path = None;
        self.selection = Selection::Readme;
        self.reader = None;
        self.reader_status = ReaderStatus::Loading;
        if let Some(path) = self.readme_path.clone() {
            self.cursor = Some(path);
        }
        self.resolve_selection(cx);
        cx.notify();
    }

    /// Re-reads README discovery after a failed or uncertain attempt.
    pub fn retry_readme(&mut self, cx: &mut Context<Self>) {
        let Some(branch) = self.branch.as_ref() else {
            return;
        };
        let snapshot = branch.snapshot.clone();
        let name = branch.name.clone();
        if let Some(state) = self.branch.as_mut() {
            state.readme = ReadmeState::Unknown;
        }
        self.reader = None;
        self.readme_path = None;
        self.reader_status = ReaderStatus::Loading;
        self.spawn_job(Job::Readme {
            branch: name,
            snapshot,
        });
        cx.notify();
    }

    /// Retries one failed directory only.
    pub fn retry_directory(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(branch) = self.branch.as_mut() else {
            return;
        };
        branch.tree.clear_failure(path);
        branch.tree.begin_load(path);
        let branch_name = branch.name.clone();
        let commit = branch.commit().clone();
        let handle = match branch.tree.child_directory(path) {
            Ok(handle) => handle,
            Err(_) => {
                // The parent is not loaded; the expansion replays when it is.
                self.invalidate_rows();
                cx.notify();
                return;
            }
        };
        self.invalidate_rows();
        self.spawn_job(Job::Directory {
            branch: branch_name,
            commit,
            path: path.to_string(),
            handle,
        });
        cx.notify();
    }

    /// Retries only the currently selected document (README or file).
    pub fn retry_reader(&mut self, cx: &mut Context<Self>) {
        if self.selection == Selection::Readme
            && self.branch.as_ref().is_some_and(|branch| {
                matches!(branch.readme, ReadmeState::Failed(_) | ReadmeState::Unknown)
            })
        {
            self.retry_readme(cx);
            return;
        }
        self.reader = None;
        self.reader_status = ReaderStatus::Loading;
        self.resolve_selection(cx);
        cx.notify();
    }

    // ---- accessors used by the view, the preview and the tests ----

    pub fn locator(&self) -> &RepositoryLocator {
        &self.locator
    }

    pub fn details(&self) -> Option<&RepositoryDetails> {
        self.details.as_ref()
    }

    pub fn branches(&self) -> Option<&BranchesOutcome> {
        self.branches.as_ref()
    }

    pub fn branches_error(&self) -> Option<&PageError> {
        self.branches_error.as_ref()
    }

    pub fn selected_branch(&self) -> Option<&str> {
        self.branch.as_ref().map(|branch| branch.name.as_str())
    }

    pub fn fixed_commit(&self) -> Option<&ObjectSha> {
        self.branch.as_ref().map(BranchState::commit)
    }

    pub fn expanded(&self) -> &BTreeSet<String> {
        &self.intent.expanded
    }

    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    pub fn reader(&self) -> Option<&Entity<ReaderView>> {
        self.reader.as_ref()
    }

    pub fn reader_status(&self) -> &ReaderStatus {
        &self.reader_status
    }

    pub fn error(&self) -> Option<&PageError> {
        self.error.as_ref()
    }

    pub fn readme_state(&self) -> Option<&ReadmeState> {
        self.branch.as_ref().map(|branch| &branch.readme)
    }

    pub fn readme_path(&self) -> Option<&str> {
        self.readme_path.as_deref()
    }

    /// The repository-relative path of the document on screen, once known.
    pub fn current_path(&self) -> Option<&str> {
        match &self.selection {
            Selection::Readme => self.readme_path.as_deref(),
            Selection::File(path) => Some(path.as_str()),
        }
    }

    /// A directory load failure for `path` (`""` is the root).
    pub fn tree_failure(&self, path: &str) -> Option<&PageError> {
        self.branch.as_ref()?.tree.failure(path)
    }

    /// The visible tree rows as of the last flatten.
    pub fn rows(&self) -> &[TreeRow] {
        &self.rows
    }

    /// Rebuilds the flattened rows if the tree or the expanded set changed.
    pub fn refresh_rows(&mut self) -> Rc<Vec<TreeRow>> {
        if self.rows_dirty {
            self.rows_dirty = false;
            match self.branch.as_mut() {
                Some(branch) => {
                    let rows = branch.tree.rows(&self.intent.expanded);
                    self.rows_capped = branch.tree.capped();
                    self.rows = Rc::new(rows);
                }
                None => {
                    self.rows_capped = false;
                    self.rows = Rc::new(Vec::new());
                }
            }
        }
        self.rows.clone()
    }

    /// Whether the flattened tree stopped at the row cap.
    pub fn tree_capped(&self) -> bool {
        self.rows_capped
    }

    /// The loaded tree for `path`, for tests and the preview.
    pub fn tree(&self, path: &str) -> Option<&DirectoryTree> {
        self.branch.as_ref()?.tree.tree(path)
    }

    /// The keyboard's current row path.
    pub fn cursor(&self) -> Option<&str> {
        self.cursor.as_deref()
    }

    /// Moves the keyboard cursor to `path` (a tree row).
    pub fn focus_row(&mut self, path: &str, cx: &mut Context<Self>) {
        self.cursor = Some(path.to_string());
        cx.notify();
    }

    // ---- internals ----

    fn invalidate_rows(&mut self) {
        self.rows_dirty = true;
    }

    fn cancel_scroll(&mut self, cx: &mut Context<Self>) {
        self.tree_bars.cancel(&self.tree_motion);
        cx.notify();
    }

    /// Ends the reader's own selection/scroll gesture before the page hides or
    /// invalidates the content it belongs to. Uses the reader's public API.
    fn cancel_reader_interactions(&mut self, cx: &mut Context<Self>) {
        if let Some(reader) = self.reader.clone() {
            reader.update(cx, |reader, cx| reader.cancel_interactions(cx));
        }
    }

    fn load_directory(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(branch) = self.branch.as_mut() else {
            return;
        };
        if branch.tree.tree(path).is_some() || branch.tree.is_loading(path) {
            return;
        }
        let handle = match branch.tree.child_directory(path) {
            Ok(handle) => handle,
            Err(_) => {
                // The parent tree is not loaded (yet), so there is nothing to
                // fail: the expansion is replayed when the parent arrives.
                return;
            }
        };
        branch.tree.begin_load(path);
        let branch_name = branch.name.clone();
        let commit = branch.commit().clone();
        self.invalidate_rows();
        self.spawn_job(Job::Directory {
            branch: branch_name,
            commit,
            path: path.to_string(),
            handle,
        });
        cx.notify();
    }

    /// Replays the user's expanded directories that are still waiting for a
    /// parent tree. Each path is fetched at most once; a directory that was
    /// never expanded is never requested.
    fn load_expanded(&mut self, cx: &mut Context<Self>) {
        let pending: Vec<String> = {
            let Some(branch) = self.branch.as_ref() else {
                return;
            };
            let mut paths: Vec<&String> = self.intent.expanded.iter().collect();
            // Parents first, so a nested expansion can resolve immediately after.
            paths.sort();
            paths
                .into_iter()
                .filter(|path| {
                    branch.tree.tree(path).is_none()
                        && !branch.tree.is_loading(path)
                        && branch.tree.failure(path).is_none()
                })
                .cloned()
                .collect()
        };
        for path in pending {
            self.load_directory(&path, cx);
        }
    }

    /// Applies the current selection: README content, or the selected file once
    /// its ancestor directories are loaded.
    ///
    /// An already displayed document for this exact selection is kept as is: an
    /// unrelated directory that finishes loading must not turn the reader back
    /// into Loading, re-download the same blob and throw away the reader's
    /// scroll position and selection.
    fn resolve_selection(&mut self, cx: &mut Context<Self>) {
        if self.reader_status == ReaderStatus::Showing
            && self.reader.as_ref().is_some_and(|reader| {
                reader.read(cx).context().path() == self.current_path().unwrap_or_default()
            })
        {
            return;
        }
        match self.selection.clone() {
            Selection::Readme => self.request_readme_content(cx),
            Selection::File(path) => self.resolve_path(&path, cx),
        }
    }

    /// Walks the ancestors of `path`, loading only the directories on the way.
    fn resolve_path(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.branch.is_none() {
            return;
        }
        for ancestor in RepositoryNavigation::ancestors(path) {
            self.intent.expanded.insert(ancestor);
        }
        self.invalidate_rows();
        let ancestors = RepositoryNavigation::ancestors(path);
        for ancestor in &ancestors {
            let Some(branch) = self.branch.as_ref() else {
                return;
            };
            if branch.tree.tree(ancestor).is_some() {
                continue;
            }
            if branch.tree.is_loading(ancestor) {
                // The pending directory result resumes this walk.
                return;
            }
            if let Some(error) = branch.tree.failure(ancestor) {
                self.reader = None;
                self.reader_status = ReaderStatus::ContentFailed(PageError::new(
                    "A parent directory could not be read.",
                    Some(format!("`{ancestor}`: {}", error.title)),
                    error.retryable,
                ));
                return;
            }
            if let Err(error) = branch.tree.child_directory(ancestor) {
                // The parent *is* loaded but has no such directory entry (or the
                // entry is a file). No request can ever arrive for it, so publish
                // a terminal status instead of waiting forever.
                let error = PageError::from_repository(&error);
                self.reader = None;
                self.reader_status = ReaderStatus::ContentFailed(PageError::new(
                    "A parent directory is not available at this commit.",
                    Some(format!("`{ancestor}`: {}", error.title)),
                    false,
                ));
                return;
            }
            self.load_directory(ancestor, cx);
            return;
        }
        let Some(branch) = self.branch.as_mut() else {
            return;
        };
        // Following a link to a directory opens it instead of failing.
        if branch.tree.entry_kind(path) == Some(TreeRowKind::Directory { expanded: false }) {
            self.intent.expanded.insert(path.to_string());
            self.intent.path = None;
            self.selection = Selection::Readme;
            self.invalidate_rows();
            self.load_directory(path, cx);
            self.request_readme_content(cx);
            return;
        }
        match branch.tree.locate_file(path) {
            Some((tree, index)) => match BlobIdentity::from_file(tree, index) {
                Ok(identity) => {
                    self.reader_status = ReaderStatus::Loading;
                    let commit = branch.commit().clone();
                    let branch_name = branch.name.clone();
                    self.spawn_job(Job::Content {
                        branch: branch_name,
                        commit,
                        role: Role::File(path.to_string()),
                        identity,
                    });
                }
                Err(error) => {
                    self.reader = None;
                    self.reader_status = ReaderStatus::SelectionUnavailable {
                        reason: PageError::from_repository(&error).title,
                    };
                }
            },
            None => {
                self.reader = None;
                self.reader_status = ReaderStatus::SelectionUnavailable {
                    reason: format!(
                        "`{path}` is not an ordinary file at this commit. Symbolic links, submodules and unknown Git types are not opened as text."
                    ),
                };
            }
        }
    }

    fn request_readme_content(&mut self, _cx: &mut Context<Self>) {
        let Some(branch) = self.branch.as_mut() else {
            return;
        };
        match branch.readme.clone() {
            ReadmeState::Found(descriptor) => {
                let path = descriptor.path.as_str().to_string();
                // Expanding an unrelated directory re-applies the selection;
                // the README already on screen is not fetched again.
                if self.reader_status == ReaderStatus::Showing
                    && self.selection == Selection::Readme
                    && self.readme_path.as_deref() == Some(path.as_str())
                {
                    return;
                }
                let identity = BlobIdentity::from_readme(&descriptor);
                let branch_name = branch.name.clone();
                let commit = branch.commit().clone();
                self.readme_path = Some(path);
                self.reader_status = ReaderStatus::Loading;
                self.spawn_job(Job::Content {
                    branch: branch_name,
                    commit,
                    role: Role::Readme,
                    identity,
                });
            }
            ReadmeState::Absent => {
                self.reader = None;
                self.readme_path = None;
                self.reader_status = ReaderStatus::NoReadme;
            }
            ReadmeState::Uncertain => {
                self.reader = None;
                self.readme_path = None;
                self.reader_status = ReaderStatus::ReadmeUncertain;
            }
            ReadmeState::Failed(error) => {
                self.reader = None;
                self.readme_path = None;
                self.reader_status = ReaderStatus::ReadmeUnavailable(error);
            }
            ReadmeState::Unknown => {
                self.reader = None;
                self.readme_path = None;
                self.reader_status = ReaderStatus::Loading;
            }
        }
    }

    fn begin_branch(&mut self, name: String, cx: &mut Context<Self>) {
        if !self.active || self.authorization_lost {
            return;
        }
        let Some(details) = self.details.clone() else {
            return;
        };
        self.cancel_reader_interactions(cx);
        self.intent.branch = Some(name.clone());
        self.branch = None;
        self.reader = None;
        self.readme_path = None;
        self.reader_status = ReaderStatus::Loading;
        self.error = None;
        self.cursor = None;
        self.invalidate_rows();
        // Every resolution gets its own identity, so a slower reply for an
        // earlier request can never be mistaken for the current one even when
        // both name the same branch (A → B → A).
        self.branch_request = self.branch_request.wrapping_add(1);
        let request = self.branch_request;
        self.spawn_job(Job::Revision {
            branch: name,
            request,
            identity: details.identity,
        });
        cx.notify();
    }

    fn stop_work(&mut self) {
        if let Some(work) = self.work.take() {
            work.source.cancel();
        }
        self.tree_bars.cancel(&self.tree_motion);
    }

    fn now_ms(&self) -> u64 {
        #[cfg(test)]
        if let Some(ms) = self.pinned_clock {
            return ms;
        }
        self.clock.elapsed().as_millis() as u64
    }

    /// The receiver drains background results on a light GPUI-side cadence,
    /// exactly like the account bridge: no `block_on`, no I/O in render.
    fn start_receiver(&mut self, cx: &mut Context<Self>) {
        self.receiver = Some(cx.spawn(async |this: WeakEntity<Self>, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(30))
                    .await;
                let alive = this
                    .update(cx, |page, _| *page.alive.borrow())
                    .unwrap_or(false);
                if !alive {
                    break;
                }
                let _ = this.update_in(cx, |page, window, cx| page.drain(window, cx));
            }
        }));
    }

    /// Drains the result queue, then re-checks that the session is still the
    /// active, verified one.
    fn drain(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let messages: Vec<Message> = {
            let mut inbox = self.inbox.lock().expect("repository page inbox");
            inbox.drain(..).collect()
        };
        for message in messages {
            self.apply(message, window, cx);
        }
        self.check_authorization(cx);
    }

    /// Detects an account/authorization change even when no message is pending,
    /// so data read under a previous authorization cannot stay on screen.
    fn check_authorization(&mut self, cx: &mut Context<Self>) {
        if !self.active || self.authorization_lost {
            return;
        }
        if self.session.as_ref().is_some_and(RepositorySession::verify) {
            return;
        }
        self.cancel_reader_interactions(cx);
        self.authorization_lost = true;
        self.active = false;
        self.stop_work();
        self.session = None;
        self.context = None;
        self.details = None;
        self.branches = None;
        self.branches_error = None;
        self.branch = None;
        self.reader = None;
        self.readme_path = None;
        self.reader_status = ReaderStatus::Loading;
        self.cursor = None;
        self.invalidate_rows();
        self.error = Some(PageError::new(
            "The account or authorization changed.",
            Some(
                "Data read under the previous authorization was cleared. Activate the page to read this repository under the current account."
                    .to_string(),
            ),
            true,
        ));
        cx.notify();
    }

    fn apply(&mut self, message: Message, window: &mut Window, cx: &mut Context<Self>) {
        if message.generation != self.generation || !self.active {
            return;
        }
        if self.context.as_ref() != Some(&message.context) {
            return;
        }
        match message.body {
            Body::Repository(result) => self.apply_repository(result, cx),
            Body::Branches(result) => self.apply_branches(result, cx),
            Body::Revision {
                branch,
                request,
                result,
            } => self.apply_revision(branch, request, result, cx),
            Body::Root {
                branch,
                commit,
                result,
            } => self.apply_root(branch, commit, result, cx),
            Body::Readme {
                branch,
                commit,
                result,
            } => self.apply_readme(branch, commit, result, cx),
            Body::Directory {
                branch,
                commit,
                path,
                result,
            } => self.apply_directory(branch, commit, path, result, cx),
            Body::Content(content) => self.apply_content(content, window, cx),
        }
    }

    fn apply_repository(
        &mut self,
        result: Result<RepositoryDetails, RepositoryError>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(details) => {
                self.spawn_job(Job::Branches(details.identity.clone()));
                self.details = Some(details);
            }
            Err(error) => {
                self.error = Some(PageError::from_repository(&error));
            }
        }
        cx.notify();
    }

    fn apply_branches(
        &mut self,
        result: Result<BranchesOutcome, RepositoryError>,
        cx: &mut Context<Self>,
    ) {
        let Some(details) = self.details.clone() else {
            return;
        };
        let (outcome, error) = match result {
            Ok(outcome) => (Some(outcome), None),
            Err(error) => (None, Some(PageError::from_repository(&error))),
        };
        self.branches_error = error;
        self.branches = outcome.clone();
        let choice = match &outcome {
            Some(outcome) => branch_choice(
                outcome,
                details.default_branch.as_ref(),
                self.intent.branch.as_deref(),
            ),
            // Without a branch list the repository's observed default branch is
            // still a valid ref; a failed list is not an empty repository.
            None => match details.default_branch.clone() {
                Some(default) => BranchChoice::Selected(default),
                None => BranchChoice::None { empty: false },
            },
        };
        match choice {
            BranchChoice::Selected(branch) => {
                self.begin_branch(branch.as_str().to_string(), cx);
            }
            BranchChoice::None { empty } => {
                self.error = Some(PageError::new(
                    "No branch could be selected.",
                    Some(if empty {
                        "GitHub returned a complete, empty branch list, so there is no commit to browse."
                            .to_string()
                    } else {
                        "The branch list is incomplete and no default branch was observed. This is not a confirmed empty repository."
                            .to_string()
                    }),
                    true,
                ));
            }
        }
        cx.notify();
    }

    fn apply_revision(
        &mut self,
        branch: String,
        request: u64,
        result: Result<RepoSnapshot, RepositoryError>,
        cx: &mut Context<Self>,
    ) {
        // A superseded resolution is discarded whether it succeeded or failed.
        if request != self.branch_request || self.intent.branch.as_deref() != Some(branch.as_str())
        {
            return;
        }
        match result {
            Ok(snapshot) => {
                self.branch = Some(BranchState {
                    name: branch.clone(),
                    snapshot: snapshot.clone(),
                    tree: TreeModel::default(),
                    readme: ReadmeState::Unknown,
                });
                self.error = None;
                self.reader = None;
                self.readme_path = None;
                self.reader_status = ReaderStatus::Loading;
                self.invalidate_rows();
                self.spawn_job(Job::Root {
                    branch: branch.clone(),
                    snapshot: snapshot.clone(),
                });
                self.spawn_job(Job::Readme { branch, snapshot });
            }
            Err(error) => {
                self.branch = None;
                self.reader = None;
                self.reader_status = ReaderStatus::Loading;
                self.error = Some(PageError::from_repository(&error));
            }
        }
        cx.notify();
    }

    fn apply_root(
        &mut self,
        branch: String,
        commit: ObjectSha,
        result: Result<DirectoryTree, RepositoryError>,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.branch.as_mut() else {
            return;
        };
        if state.name != branch || state.commit() != &commit {
            return;
        }
        match result {
            Ok(tree) => {
                state.tree.set_root(tree);
                self.invalidate_rows();
                self.load_expanded(cx);
                self.resolve_selection(cx);
            }
            Err(error) => {
                let error = PageError::from_repository(&error);
                state.tree.fail_load("", error.clone());
                self.invalidate_rows();
                self.error = Some(error);
            }
        }
        cx.notify();
    }

    fn apply_readme(
        &mut self,
        branch: String,
        commit: ObjectSha,
        result: Result<ReadmeState, RepositoryError>,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.branch.as_mut() else {
            return;
        };
        if state.name != branch || state.commit() != &commit {
            return;
        }
        state.readme = match result {
            Ok(readme) => readme,
            Err(error) => ReadmeState::Failed(PageError::from_repository(&error)),
        };
        if self.selection == Selection::Readme {
            self.request_readme_content(cx);
        }
        cx.notify();
    }

    fn apply_directory(
        &mut self,
        branch: String,
        commit: ObjectSha,
        path: String,
        result: Result<DirectoryTree, RepositoryError>,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.branch.as_mut() else {
            return;
        };
        if state.name != branch || state.commit() != &commit {
            return;
        }
        match result {
            Ok(tree) => state.tree.set_children(tree),
            Err(error) => state
                .tree
                .fail_load(&path, PageError::from_repository(&error)),
        }
        let failure = state.tree.failure(&path).cloned();
        self.invalidate_rows();
        // A selection waiting on *this* directory has to end in a terminal state
        // rather than stay Loading; others keep what they were showing.
        if let Some(error) = failure
            && self
                .selection
                .path()
                .is_some_and(|selected| RepositoryNavigation::ancestors(selected).contains(&path))
        {
            self.reader = None;
            self.reader_status = ReaderStatus::ContentFailed(PageError::new(
                "A parent directory could not be read.",
                Some(format!("`{path}`: {}", error.title)),
                error.retryable,
            ));
        }
        // A pending selection or a deeper expanded directory may have been
        // waiting for this tree.
        self.load_expanded(cx);
        self.resolve_selection(cx);
        cx.notify();
    }

    fn apply_content(
        &mut self,
        content: ContentReply,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ContentReply {
            branch,
            commit,
            role,
            path,
            result,
        } = content;
        let Some(state) = self.branch.as_ref() else {
            return;
        };
        if state.name != branch || state.commit() != &commit {
            return;
        }
        // A result for a document the user has already moved away from must not
        // replace the current one.
        if !role.matches(&self.selection) {
            return;
        }
        match result {
            Ok(content) => {
                if content.identity.path().as_str() != path {
                    return;
                }
                let kind = DocumentKind::for_path(&path);
                let page = cx.entity().downgrade();
                let reader = cx.new(|cx| {
                    let mut reader = ReaderView::from_prepared(content, kind, window, cx);
                    reader.on_action(Rc::new(move |action, window, cx| {
                        let _ =
                            page.update(cx, |page, cx| page.on_reader_action(action, window, cx));
                    }));
                    reader
                });
                self.reader = Some(reader);
                self.reader_status = ReaderStatus::Showing;
            }
            Err(error) => {
                self.reader = None;
                self.reader_status =
                    ReaderStatus::ContentFailed(PageError::from_repository(&error));
            }
        }
        cx.notify();
    }

    /// Routes a reader link press. External URLs are handed to the host; a
    /// same-repository, same-commit relative link is followed inside this page
    /// generation, so the fixed commit and authorization stay identical.
    fn on_reader_action(
        &mut self,
        action: ReaderAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ReaderAction::Link(target) = action;
        match target {
            LinkTarget::External { url } => self.emit(PageAction::OpenExternal(url), window, cx),
            LinkTarget::Repository(link) => {
                let Some(branch) = self.branch.as_ref() else {
                    return;
                };
                let commit = branch.commit().as_str().to_string();
                let same_repository = self
                    .details
                    .as_ref()
                    .map(|details| {
                        details
                            .identity
                            .locator()
                            .owner()
                            .eq_ignore_ascii_case(&link.owner)
                            && details
                                .identity
                                .locator()
                                .name()
                                .eq_ignore_ascii_case(&link.name)
                    })
                    .unwrap_or(false);
                if !same_repository {
                    self.emit(
                        PageAction::LinkNotFollowed {
                            path: link.path,
                            reason: "The link points at a different repository; this page keeps its own authorization and commit."
                                .to_string(),
                        },
                        window,
                        cx,
                    );
                    return;
                }
                if link.commit != commit {
                    self.emit(
                        PageAction::LinkNotFollowed {
                            path: link.path,
                            reason: "The link names a different commit than the one on screen."
                                .to_string(),
                        },
                        window,
                        cx,
                    );
                    return;
                }
                self.select_file(&link.path, cx);
            }
            LinkTarget::Anchor { .. } | LinkTarget::Refused { .. } => {}
        }
    }

    fn emit(&mut self, action: PageAction, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(handler) = self.on_action.clone() {
            handler(action, window, cx);
        }
    }

    // ---- tree keyboard navigation ----

    /// Moves the tree cursor by `delta` visible rows.
    fn move_cursor(&mut self, delta: isize, cx: &mut Context<Self>) {
        let rows = self.refresh_rows();
        if rows.is_empty() {
            return;
        }
        let current = self
            .cursor
            .as_deref()
            .and_then(|path| rows.iter().position(|row| row.path == path));
        let next = match current {
            Some(index) => {
                let target = index as isize + delta;
                target.clamp(0, rows.len() as isize - 1) as usize
            }
            None => 0,
        };
        let path = rows[next].path.clone();
        self.cursor = Some(path.clone());
        self.reveal_row(&path, cx);
        cx.notify();
    }

    /// Enters the tree, starting at the current selection or the first row.
    fn focus_cursor(&mut self, cx: &mut Context<Self>) {
        if self.cursor.is_none() {
            self.cursor = self
                .current_path()
                .map(str::to_string)
                .or_else(|| self.rows.first().map(|row| row.path.clone()));
        }
        cx.notify();
    }

    /// Left: collapse the current directory, or climb to its parent.
    fn cursor_left(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.cursor.clone() else {
            return;
        };
        let rows = self.refresh_rows();
        let Some(row) = rows.iter().find(|row| row.path == path) else {
            return;
        };
        if row.kind == (TreeRowKind::Directory { expanded: true }) {
            self.collapse_directory(&path, cx);
            return;
        }
        let parent = row.parent.clone();
        if !parent.is_empty() {
            self.cursor = Some(parent);
            self.reveal_row(&self.cursor.clone().unwrap_or_default(), cx);
            cx.notify();
        }
    }

    /// Right: expand the current directory, or step into its first child.
    fn cursor_right(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.cursor.clone() else {
            return;
        };
        let rows = self.refresh_rows();
        let Some(row) = rows.iter().find(|row| row.path == path) else {
            return;
        };
        match row.kind {
            TreeRowKind::Directory { expanded: false } => {
                self.expand_directory(&path, cx);
            }
            TreeRowKind::Directory { expanded: true } => {
                if let Some(index) = rows.iter().position(|row| row.path == path)
                    && let Some(child) = rows.get(index + 1)
                    && child.depth > row.depth
                {
                    let child = child.path.clone();
                    self.cursor = Some(child.clone());
                    self.reveal_row(&child, cx);
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    /// Enter: activate the current row (toggle a directory, open a file).
    fn cursor_activate(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.cursor.clone() else {
            return;
        };
        let rows = self.refresh_rows();
        let Some(row) = rows.iter().find(|row| row.path == path) else {
            return;
        };
        match row.kind {
            TreeRowKind::Directory { .. } => self.toggle_directory(&path, cx),
            TreeRowKind::File => self.select_file(&path, cx),
            TreeRowKind::Symlink | TreeRowKind::Submodule | TreeRowKind::Unknown => {
                self.select_file(&path, cx)
            }
        }
    }

    /// Scrolls a row into view through the list's own scroll target.
    fn reveal_row(&mut self, path: &str, cx: &mut Context<Self>) {
        let rows = self.refresh_rows();
        let Some(index) = rows.iter().position(|row| row.path == path) else {
            return;
        };
        self.tree_scroll
            .scroll_to_item(index, gpui::ScrollStrategy::Nearest);
        cx.notify();
    }

    /// The tree's key handler, exposed to the view. Only plain navigation keys
    /// are consumed so the platform, IME and the reader's text selection keep
    /// their own events.
    pub(crate) fn press_tree_key_event(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.on_tree_key(event, window, cx)
    }

    fn on_tree_key(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        // Only plain navigation keys; modified keys belong to the platform, the
        // reader's selection and IME.
        if event.keystroke.modifiers.modified() {
            return;
        }
        match key {
            "down" => self.move_cursor(1, cx),
            "up" => self.move_cursor(-1, cx),
            "left" => self.cursor_left(cx),
            "right" => self.cursor_right(cx),
            "home" => {
                if let Some(path) = self.refresh_rows().first().map(|row| row.path.clone()) {
                    self.cursor = Some(path.clone());
                    self.reveal_row(&path, cx);
                    cx.notify();
                }
            }
            "end" => {
                if let Some(path) = self.refresh_rows().last().map(|row| row.path.clone()) {
                    self.cursor = Some(path.clone());
                    self.reveal_row(&path, cx);
                    cx.notify();
                }
            }
            "enter" | "space" => self.cursor_activate(cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    // ---- background work ----

    fn spawn_job(&mut self, job: Job) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let Some(context) = self.context.clone() else {
            return;
        };
        let Some(cancel) = self.work.as_ref().map(Work::token) else {
            return;
        };
        let inbox = self.inbox.clone();
        let alive = self.alive.clone();
        let generation = self.generation;
        let limits = self.services.limits.clone();
        let spawned = self.services.executor.handle().spawn(async move {
            let body = run_job(&session, job, &limits, cancel).await;
            if *alive.borrow() {
                inbox
                    .lock()
                    .expect("repository page inbox")
                    .push_back(Message {
                        generation,
                        context,
                        body,
                    });
            }
        });
        if spawned.is_err() {
            self.error = Some(PageError::new(
                "Background services are closing.",
                Some(
                    "The read was not submitted because the network executor is shutting down."
                        .to_string(),
                ),
                true,
            ));
        }
    }

    pub(crate) fn tree_base_handle(&self) -> gpui::ScrollHandle {
        self.tree_scroll.0.borrow().base_handle.clone()
    }
}

impl Focusable for RepositoryPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.tree_focus.clone()
    }
}

enum Job {
    Repository(RepositoryLocator),
    Branches(RepositoryIdentity),
    Revision {
        branch: String,
        request: u64,
        identity: RepositoryIdentity,
    },
    Root {
        branch: String,
        snapshot: RepoSnapshot,
    },
    Readme {
        branch: String,
        snapshot: RepoSnapshot,
    },
    Directory {
        branch: String,
        commit: ObjectSha,
        path: String,
        handle: SnapshotDirectory,
    },
    Content {
        branch: String,
        commit: ObjectSha,
        role: Role,
        identity: BlobIdentity,
    },
}

/// Runs exactly one service call. Sequencing stays in the foreground so every
/// result is checked before the next step is requested.
async fn run_job(
    session: &RepositorySession,
    job: Job,
    limits: &PageLimits,
    cancel: CancellationToken,
) -> Body {
    match job {
        Job::Repository(locator) => Body::Repository(
            session
                .repository(locator, &limits.repository, cancel)
                .await
                .map(|Scoped { value, .. }| value),
        ),
        Job::Branches(identity) => Body::Branches(
            session
                .branches(&identity, &limits.repository, cancel)
                .await
                .map(|Scoped { value, .. }| value),
        ),
        Job::Revision {
            branch,
            request,
            identity,
        } => {
            // `branch` was validated as a ref before the job was created.
            let spec = RevisionSpec::Ref(DynamicRef::new(branch.clone()).expect("validated ref"));
            let result = session
                .revision(&identity, spec, &limits.repository, cancel)
                .await;
            Body::Revision {
                branch,
                request,
                result: result.map(|Scoped { value, .. }| value),
            }
        }
        Job::Root { branch, snapshot } => {
            let commit = snapshot.commit().clone();
            let root = SnapshotDirectory::root(&snapshot);
            let result = session.directory(&root, &limits.repository, cancel).await;
            Body::Root {
                branch,
                commit,
                result: result.map(|Scoped { value, .. }| value),
            }
        }
        Job::Readme { branch, snapshot } => {
            let commit = snapshot.commit().clone();
            let result = session.readme(&snapshot, &limits.repository, cancel).await;
            Body::Readme {
                branch,
                commit,
                result: result.map(|Scoped { value, .. }| ReadmeState::from_discovery(Ok(value))),
            }
        }
        Job::Directory {
            branch,
            commit,
            path,
            handle,
        } => {
            let result = session.directory(&handle, &limits.repository, cancel).await;
            Body::Directory {
                branch,
                commit,
                path,
                result: result.map(|Scoped { value, .. }| value),
            }
        }
        Job::Content {
            branch,
            commit,
            role,
            identity,
        } => {
            let path = identity.path().as_str().to_string();
            let result = session.content(&identity, &limits.blob, cancel).await;
            Body::Content(ContentReply {
                branch,
                commit,
                role,
                path,
                result: result.map(|Scoped { value, .. }| value),
            })
        }
    }
}

impl Render for RepositoryPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.refresh_rows();
        let view = view::render(self, rows, window, cx);
        // Easing wheel motion and the bar's fade each ask for their own frames;
        // a settled region requests nothing.
        if self.tree_bars.is_animating() || self.tree_motion.is_animating() {
            window.request_animation_frame();
        }
        div()
            .id("repository-page")
            .debug_selector(|| "repository-page".to_string())
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .child(view)
    }
}

impl Drop for RepositoryPage {
    fn drop(&mut self) {
        // Stop late workers from enqueuing, then cancel in-flight work. The
        // executor keeps its own existing shutdown contract.
        self.alive.send_replace(false);
        if let Some(work) = self.work.take() {
            work.source.cancel();
        }
        self.tree_bars.cancel(&self.tree_motion);
    }
}

/// Each visible row's kind, used by tests to assert row state without pixels.
pub fn row_kind(rows: &[TreeRow], path: &str) -> Option<TreeRowKind> {
    rows.iter().find(|row| row.path == path).map(|row| row.kind)
}

/// Each visible row's state, used by tests to assert loading/failed/empty rows.
pub fn row_state(rows: &[TreeRow], path: &str) -> Option<RowState> {
    rows.iter()
        .find(|row| row.path == path)
        .map(|row| row.state.clone())
}

/// A tree row's path list, for tests that assert ordering and indentation.
pub fn row_paths(rows: &[TreeRow]) -> Vec<&str> {
    rows.iter().map(|row| row.path.as_str()).collect()
}

impl RepositoryPage {
    /// Whether a README is available and a different file is currently shown,
    /// which is exactly when the "back to README" entry belongs.
    ///
    /// The README's own path comes from the branch's discovery state, so
    /// selecting a file (which clears the *shown* README path) does not make a
    /// real README disappear.
    pub fn can_return_to_readme(&self) -> bool {
        let Selection::File(path) = &self.selection else {
            return false;
        };
        self.branch
            .as_ref()
            .and_then(|branch| branch.readme.path())
            .is_some_and(|readme| readme != path.as_str())
    }

    /// The `TreeEntryKind`-independent display name for a row kind, used by the
    /// accessibility label and by tests.
    pub fn row_label(rows: &[TreeRow], path: &str) -> Option<String> {
        rows.iter()
            .find(|row| row.path == path)
            .map(|row| row.name.clone())
    }
}

/// Ensures `TreeEntryKind` stays referenced here for the file/directory split.
fn _entry_kind_is_file(kind: &TreeEntryKind) -> bool {
    matches!(kind, TreeEntryKind::File { .. })
}

#[cfg(test)]
pub(crate) use test_support::*;

#[cfg(test)]
mod test_support {
    use super::*;

    impl RepositoryPage {
        /// The in-flight result count, for tests that assert no extra reads.
        pub(crate) fn queued_results(&self) -> usize {
            self.inbox.lock().expect("repository page inbox").len()
        }

        /// The authorization context of the current session, for rebind checks.
        pub(crate) fn context(&self) -> Option<&RepositoryReadContext> {
            self.context.as_ref()
        }

        /// Sends one plain navigation key to the page's tree handler, exactly as
        /// the real key handler receives it.
        pub(crate) fn press_tree_key(
            &mut self,
            key: &str,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            let event = KeyDownEvent {
                keystroke: gpui::Keystroke {
                    modifiers: Default::default(),
                    key: key.to_string(),
                    key_char: None,
                },
                is_held: false,
                prefer_character_input: false,
            };
            self.press_tree_key_event(&event, window, cx);
        }
    }

    /// The page's tree-focus handle, for tests that drive keyboard navigation.
    pub(crate) fn tree_focus_handle(page: &RepositoryPage, cx: &App) -> FocusHandle {
        page.focus_handle(cx)
    }
}
