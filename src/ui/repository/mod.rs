//! A reusable, real-data repository browsing page.
//!
//! The page owns one browsing generation at a time: a verified
//! [`RepositorySession`] captured from the host's [`AccountService`], the
//! repository summary, the branch list, one fixed-commit snapshot, the lazily
//! expanded directory tree and the reader showing either the default README or
//! a selected file.
//!
//! ## Integration boundary
//!
//! ```ignore
//! let services = RepositoryPageServices::new(accounts.clone(), executor.clone());
//! let page = cx.new(|cx| {
//!     let mut page = RepositoryPage::new(services, locator, saved_navigation(), window, cx);
//!     page.activate(cx);
//!     page
//! });
//! ```
//!
//! The host supplies only capabilities that already exist: `Arc<AccountService>`,
//! `Arc<BackgroundExecutor>` and a `RepositoryLocator`. The page never opens a
//! database, reads a credential or fetches a PAT itself; every read goes through
//! `AccountService::repo_service()`.
//!
//! Lifecycle (§ "public API" below):
//!
//! * [`RepositoryPage::activate`] starts a fresh generation with a new session.
//! * [`RepositoryPage::deactivate`] cancels in-flight work and keeps only
//!   navigation intent; late successes and errors cannot publish.
//! * [`RepositoryPage::rebind`] swaps the capabilities (for example after the
//!   host restored a different account), clears data read under the old
//!   authorization, and re-reads the saved navigation under a new session. An
//!   old `RepositorySession` is never resurrected.
//! * [`RepositoryPage::navigation`] returns the intent a host may save.
//!
//! Everything slow runs on the existing background executor; results come back
//! through a queue drained on the GPUI thread, and every result is checked
//! against the page generation, the captured authorization context, the fixed
//! commit and the resource identity before it can replace what is on screen.

// The page is delivered before the Shell mounts it. This allowance is scoped to
// this module tree; the product binary's other modules stay warning-clean.
#![allow(dead_code)]

pub mod navigation;
#[cfg(test)]
mod tests;
mod view;

use std::collections::{BTreeSet, VecDeque};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    App, Context, Entity, IntoElement, ParentElement, Render, Styled, Subscription, Task,
    UniformListScrollHandle, WeakEntity, Window, div, prelude::*, px,
};
use tokio::sync::watch;

use crate::content::PreparedContent;
use crate::data::{AccountService, RepositoryReadContext, RepositorySession, Scoped};
use crate::domain::blob::{BlobIdentity, BlobLimits};
use crate::domain::repository::{
    BranchesOutcome, DirectoryTree, DynamicRef, ObjectSha, RepoSnapshot, RepositoryDetails,
    RepositoryError, RepositoryIdentity, RepositoryLocator, RevisionSpec, SnapshotDirectory,
};
use crate::github::RepositoryLimits;
use crate::network::cancel::{CancellationToken, CancellationTokenSource};
use crate::network::executor::BackgroundExecutor;
use crate::ui::reader::{DocumentKind, LinkTarget, ReaderAction, ReaderView};

use navigation::{
    BranchChoice, PageError, ReadmeState, RepositoryNavigation, TreeModel, TreeRowKind,
    branch_choice,
};

/// Width of the bounded file tree column.
pub const TREE_WIDTH: f32 = 268.0;

/// Height of one file-tree row.
pub const TREE_ROW_HEIGHT: f32 = 24.0;

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

pub type OnPageAction = std::rc::Rc<dyn Fn(PageAction, &mut Window, &mut App)>;

/// Which document the reader shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    /// The repository's default README at the fixed commit.
    Readme,
    /// A file at the fixed commit.
    File(String),
}

impl Selection {
    fn path(&self) -> Option<&str> {
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
    /// A short, testable tag for the state that is on screen.
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
    Content {
        branch: String,
        commit: ObjectSha,
        role: Role,
        path: String,
        result: Result<PreparedContent, RepositoryError>,
    },
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
    /// Flattened visible tree rows, rebuilt only when the tree or the expanded
    /// set changes. Rendering clones an `Rc`, never the rows.
    rows: Rc<Vec<navigation::TreeRow>>,
    rows_dirty: bool,
    rows_capped: bool,
    selection: Selection,
    /// Path of the README body currently shown (the descriptor's path).
    readme_path: Option<String>,
    reader: Option<Entity<ReaderView>>,
    reader_status: ReaderStatus,
    /// A page-level failure while opening or switching branches.
    error: Option<PageError>,
    /// True while a browsing generation is running.
    active: bool,
    /// Set when authorization changed under a running generation.
    authorization_lost: bool,
    tree_scroll: UniformListScrollHandle,
    tree_motion: crate::ui::scroll::Motion<gpui::ScrollHandle>,
    tree_bars: crate::ui::scroll::Bars,
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
            rows: Rc::new(Vec::new()),
            rows_dirty: false,
            rows_capped: false,
            readme_path: None,
            reader: None,
            reader_status: ReaderStatus::Loading,
            error: None,
            active: false,
            authorization_lost: false,
            tree_scroll,
            tree_motion: crate::ui::scroll::Motion::new(base),
            tree_bars: crate::ui::scroll::Bars::new(),
            clock: std::time::Instant::now(),
            #[cfg(test)]
            pinned_clock: None,
            _window_activation: cx.observe_window_activation(window, |page, window, cx| {
                if !window.is_window_active() {
                    page.tree_bars.cancel(&page.tree_motion);
                    cx.notify();
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
        let session = match self.services.accounts.repo_service().session() {
            Ok(session) => session,
            Err(error) => {
                self.active = false;
                self.session = None;
                self.context = None;
                self.error = Some(PageError::from_repository(&error));
                cx.notify();
                return;
            }
        };
        self.context = Some(session.context().clone());
        self.session = Some(session);
        self.active = true;
        self.error = None;
        self.details = None;
        self.branches = None;
        self.branches_error = None;
        self.branch = None;
        self.reader = None;
        self.readme_path = None;
        self.reader_status = ReaderStatus::Loading;
        self.invalidate_rows();
        self.work = Some(Work::new());
        self.spawn_job(Job::Repository(self.locator.clone()));
        cx.notify();
    }

    /// Cancels in-flight work and keeps only navigation intent.
    ///
    /// Nothing that finishes later can replace the page's content, because the
    /// generation has already moved on.
    pub fn deactivate(&mut self, cx: &mut Context<Self>) {
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
        self.invalidate_rows();
        self.services = services;
        self.activate(cx);
    }

    /// One user-initiated retry of the failed step. There is no automatic retry
    /// loop; each press starts a new bounded attempt.
    pub fn retry(&mut self, cx: &mut Context<Self>) {
        if !self.active || self.authorization_lost || self.details.is_none() {
            self.activate(cx);
            return;
        }
        let Some(branch) = self.branch.as_ref().map(|branch| branch.name.clone()) else {
            // The branch list or the revision failed; restart from the identity
            // the page already holds rather than opening a new session.
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

    /// Expands or collapses a directory, loading it the first time it opens.
    pub fn toggle_directory(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.intent.expanded.remove(path) {
            self.intent.expanded.insert(path.to_string());
            self.load_directory(path, cx);
        }
        self.invalidate_rows();
        cx.notify();
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
        self.resolve_selection(cx);
        cx.notify();
    }

    /// Returns the reader to the repository README.
    pub fn show_readme(&mut self, cx: &mut Context<Self>) {
        if self.branch.is_none() {
            return;
        }
        self.intent.path = None;
        self.selection = Selection::Readme;
        self.reader = None;
        self.reader_status = ReaderStatus::Loading;
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
        self.reader_status = ReaderStatus::Loading;
        self.spawn_job(Job::Readme {
            branch: name,
            snapshot,
        });
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

    pub fn branches_error(&self) -> Option<&PageError> {
        self.branches_error.as_ref()
    }

    /// The visible tree rows as of the last flatten. They are rebuilt only when
    /// the tree or the expanded set changed, so reading them is free.
    pub fn rows(&self) -> &[navigation::TreeRow] {
        &self.rows
    }

    /// Rebuilds the flattened rows if the tree or the expanded set changed.
    pub fn refresh_rows(&mut self) -> Rc<Vec<navigation::TreeRow>> {
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

    /// Marks the flattened rows stale after a tree or expansion change.
    fn invalidate_rows(&mut self) {
        self.rows_dirty = true;
    }

    /// The loaded tree for `path`, for tests and the preview.
    pub fn tree(&self, path: &str) -> Option<&DirectoryTree> {
        self.branch.as_ref()?.tree.tree(path)
    }

    pub fn readme_state(&self) -> Option<&ReadmeState> {
        self.branch.as_ref().map(|branch| &branch.readme)
    }

    /// The path of the README currently described or shown.
    pub fn readme_path(&self) -> Option<&str> {
        self.readme_path.as_deref()
    }

    /// A directory load failure for `path` (`""` is the root).
    pub fn tree_failure(&self, path: &str) -> Option<&PageError> {
        self.branch.as_ref()?.tree.failure(path)
    }

    /// Whether the flattened tree stopped at the row cap.
    pub fn tree_capped(&self) -> bool {
        self.rows_capped
    }

    // ---- internals ----

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
    fn resolve_selection(&mut self, cx: &mut Context<Self>) {
        match self.selection.clone() {
            Selection::Readme => self.request_readme_content(cx),
            Selection::File(path) => self.resolve_path(&path, cx),
        }
    }

    /// Walks the ancestors of `path`, loading only the directories on the way.
    /// Nothing recursive happens: each opened directory is one request.
    fn resolve_path(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.branch.is_none() {
            return;
        }
        // A link can name a directory: open it rather than report a bad file.
        if self.branch.as_ref().is_some_and(|branch| {
            branch
                .tree
                .tree(RepositoryNavigation::parent(path))
                .is_some_and(|tree| {
                    tree.entries.iter().any(|entry| {
                        entry.path.as_str() == RepositoryNavigation::name(path)
                            && entry.kind == crate::domain::repository::TreeEntryKind::Directory
                    })
                })
        }) {
            self.intent.expanded.insert(path.to_string());
            self.invalidate_rows();
            self.intent.path = None;
            self.selection = Selection::Readme;
            self.load_directory(path, cx);
            self.request_readme_content(cx);
            return;
        }
        for ancestor in RepositoryNavigation::ancestors(path) {
            self.intent.expanded.insert(ancestor.clone());
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
            self.load_directory(ancestor, cx);
            return;
        }
        let Some(branch) = self.branch.as_mut() else {
            return;
        };
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
                        "`{path}` is not an ordinary file at this commit. Directories, symbolic links and submodules are not opened as text."
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
        self.intent.branch = Some(name.clone());
        self.branch = None;
        self.reader = None;
        self.readme_path = None;
        self.reader_status = ReaderStatus::Loading;
        self.error = None;
        self.invalidate_rows();
        self.spawn_job(Job::Revision {
            branch: name,
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
            Body::Revision { branch, result } => self.apply_revision(branch, result, cx),
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
            Body::Content {
                branch,
                commit,
                role,
                path,
                result,
            } => self.apply_content(
                ContentReply {
                    branch,
                    commit,
                    role,
                    path,
                    result,
                },
                window,
                cx,
            ),
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
        result: Result<RepoSnapshot, RepositoryError>,
        cx: &mut Context<Self>,
    ) {
        if self.intent.branch.as_deref() != Some(branch.as_str()) {
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
        self.invalidate_rows();
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
                    reader.on_action(std::rc::Rc::new(move |action, window, cx| {
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

enum Job {
    Repository(RepositoryLocator),
    Branches(RepositoryIdentity),
    Revision {
        branch: String,
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
        Job::Revision { branch, identity } => {
            // `branch` was validated as a ref before the job was created.
            let spec = RevisionSpec::Ref(DynamicRef::new(branch.clone()).expect("validated ref"));
            let result = session
                .revision(&identity, spec, &limits.repository, cancel)
                .await;
            Body::Revision {
                branch,
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
            Body::Content {
                branch,
                commit,
                role,
                path,
                result: result.map(|Scoped { value, .. }| value),
            }
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
            .debug_selector(|| "repository-page".to_string())
            .size_full()
            .min_h_0()
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
