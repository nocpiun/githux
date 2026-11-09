//! Native Shell repository browsing page.
//!
//! The page owns one browsing generation at a time: the shell's captured [`PageContext`] (account,
//! workspace identity, repository service and cache authorization), the repository summary, the
//! branch list, one fixed-commit snapshot, the lazily expanded directory tree and the reader showing
//! the README or a selected file.
//!
//! ## Wiring
//!
//! ```ignore
//! let page = RepositoryPage::new(context, owner, name, intent, navigation);
//! // `PageView::poll(&runtime)` on every shell drain; `refresh(&runtime)` on an explicit refresh;
//! // `retire()` when the tab closes or the account binding changes.
//! ```
//!
//! The shell is the only writer of routes. This page emits [`PageAction`]s (external URLs,
//! contributor/release navigation, links it refused to follow) and never opens a browser, a new
//! repository or a profile by itself.
//!
//! ## Publication
//!
//! Every background result is checked before it can change anything: the page generation, the
//! captured `PageContext` (account + workspace identity + repository service + cache
//! authorization), the fixed commit and the resource identity the result belongs to. A stale
//! success and a stale error are discarded the same way, so switching branch, account (A→B→A),
//! replacing the PAT, revoking the repository or retiring the page can never paste an old result
//! over a new one. Displayed values are re-checked on every drain, not only when a request starts.

mod extras;
mod locator_index;
mod navigation;
mod page_tabs;
mod view;

#[cfg(test)]
mod tests;

pub use navigation::{
    PageError, ReadmeState, RepositoryNavigation, RowState, TreeRow, TreeRowKind,
};
pub use page_tabs::RepositoryTab;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::rc::{Rc, Weak};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use nsh_core::element::SignalEquals;
use nsh_core::prelude::*;

use crate::app::pages::{NavigationSink, PageNavigation, PageView};
use crate::app::runtime::{PageContext, PageSlot, PageTicket, RuntimeHandle};
use crate::content::PreparedContent;
use crate::content::images::{ImageLimits, ImageProvider};
use crate::data::reader_images::RepositoryImageService;
use crate::data::repository_actions::RepositoryActionService;
use crate::data::{
    AccountCache, CachedRepositorySession, RepositoryCacheError, RepositoryCacheMode,
    RepositoryCacheOrigin, RepositoryCacheProvenance, RepositoryReadContext, RepositorySession,
    Scoped,
};
use crate::domain::blob::{BlobIdentity, BlobLimits};
use crate::domain::repo::RepoVisibility;
use crate::domain::repository::{
    BranchesOutcome, DirectoryTree, DynamicRef, ObjectSha, RepoSnapshot, RepositoryDetails,
    RepositoryError, RepositoryIdentity, RepositoryLocator, RevisionSpec, SnapshotDirectory,
};
use crate::domain::repository_actions::{RepositoryActionError, StarActionOutcome, StarredState};
use crate::github::RepositoryLimits;
use crate::network::cancel::{CancellationToken, CancellationTokenSource};
use crate::network::reader_images::{ExternalImageBytes, ReqwestExternalImages};
use crate::storage::cache::{CacheAuthorization, CacheScope, CacheStore};
use crate::ui::reader::{
    DocumentKind, LinkTarget, ReaderAction, ReaderDocument, ReaderHandle, ReaderLayout,
    images::ImageBridge,
};
use crate::workspace::{PageIdentity, RepositoryNavigation as WorkspaceNavigation, Sessions};

use navigation::{BranchChoice, TreeModel, branch_choice};

/// Width of the bounded file tree column.
pub const TREE_WIDTH: f32 = 268.0;

/// Height of one file-tree row.
pub const TREE_ROW_HEIGHT: f32 = 26.0;

/// Engineering limits, overridable by tests.
#[derive(Clone, Debug, Default)]
pub struct PageLimits {
    pub repository: RepositoryLimits,
    pub blob: BlobLimits,
}

/// The shared in-process cache token captured before one read and kept while what it authorized is
/// displayed. Checking it is a lock-free atomic load.
type CapturedAuthorization = Option<CacheAuthorization>;

/// What the page asks the shell to do on its behalf.
#[derive(Clone, Debug, PartialEq)]
pub enum PageAction {
    /// A validated repository-scoped release or contributor navigation.
    Overview(crate::ui::repository_overview::OverviewAction),
    /// An absolute `http(s)` URL the user pressed. The page never opens it.
    OpenExternal(String),
    /// A repository link that this page generation did not follow.
    LinkNotFollowed { path: String, reason: String },
    /// The Star write was submitted or observed; kept for host logging and tests.
    StarIntent { starred: bool },
}

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
    /// A prepared document is on screen.
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

/// The current account's observed starred state and its independent UI phase.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StarState {
    observed: Option<StarredState>,
    phase: StarPhase,
}

#[derive(Clone, Debug, Default, PartialEq)]
enum StarPhase {
    #[default]
    Idle,
    Loading,
    Ready,
    Writing {
        desired: StarredState,
    },
    Failed {
        message: String,
    },
}

/// What the view binds for Star.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StarPresentation {
    pub label: &'static str,
    pub observed: Option<StarredState>,
    pub busy: bool,
    pub failure: Option<String>,
    /// A Star press is enabled only while the full current binding exists.
    pub enabled: bool,
    pub count: Option<u64>,
    pub retry: bool,
}

/// One fixed-commit browsing snapshot plus its lazily loaded tree.
struct BranchState {
    name: String,
    snapshot: RepoSnapshot,
    snapshot_provenance: Option<RepositoryCacheProvenance>,
    tree: TreeModel,
    readme: ReadmeState,
}

impl BranchState {
    fn commit(&self) -> &ObjectSha {
        self.snapshot.commit()
    }
}

/// One browsing generation's in-flight work.
pub(super) struct Work {
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

/// The authorization stamp displayed next to a gated top-level [`PageSlot`] value.
#[derive(Clone)]
struct SectionStamp {
    authorization: Option<CacheAuthorization>,
}

/// One content generation's identity, so the page can retire the document it replaced.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ContentStamp {
    key: u64,
}

/// The document and picture provider the NSH page mounts for one content generation.
type PreparedReaderParts = (Arc<ReaderDocument>, Option<Arc<dyn ImageProvider>>);

/// A prepared reader document plus the picture provider it was built with. The page creates the
/// authorization bridge, so it can remount the same document after an in-page tab switch without
/// re-reading it or reusing a retired bridge.
struct PreparedReader {
    path: String,
    document: Arc<ReaderDocument>,
    provider: Option<Arc<dyn ImageProvider>>,
}

/// A background result. Every message carries the page generation, the
/// authorization context captured when the request started and the shared
/// [`CacheAuthorization`] captured before its read.
struct Message {
    generation: u64,
    branch_request: u64,
    /// In-process cache invalidation token captured before the read. Once a
    /// revocation/PAT replacement/rejection invalidates it, it never becomes
    /// valid again, so a reply that finished before that event cannot be applied
    /// afterwards even if a later read restored access.
    authorization: Option<CacheAuthorization>,
    context: RepositoryReadContext,
    /// The top-level page slot ticket the request began; absent for sub-resources
    /// of the revision (root tree, README discovery, directories).
    ticket: Option<u64>,
    body: Body,
}

enum Body {
    Repository {
        result: Result<(RepositoryDetails, Option<RepositoryCacheProvenance>), PageError>,
        star_write_generation: u64,
    },
    Branches(Result<(BranchesOutcome, Option<RepositoryCacheProvenance>), PageError>),
    Revision {
        branch: String,
        request: u64,
        result: Result<(RepoSnapshot, Option<RepositoryCacheProvenance>), PageError>,
    },
    Root {
        branch: String,
        commit: ObjectSha,
        result: Result<DirectoryTree, PageError>,
    },
    Readme {
        branch: String,
        commit: ObjectSha,
        result: Result<ReadmeState, PageError>,
    },
    Directory {
        branch: String,
        commit: ObjectSha,
        path: String,
        result: Result<DirectoryTree, PageError>,
    },
    Content(ContentReply),
}

/// One content reply, bundled so the publication check reads as one step.
struct ContentReply {
    branch: String,
    commit: ObjectSha,
    role: Role,
    selection_request: u64,
    path: String,
    result: Result<PreparedContent, PageError>,
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

struct StarWork {
    service: RepositoryActionService,
    work: Work,
    binding: crate::ui::repository_overview::OverviewBinding,
    request: u64,
}

#[derive(Clone, Copy)]
enum StarSubmission {
    Observe,
    Write(StarredState),
}

struct StarReply {
    binding: crate::ui::repository_overview::OverviewBinding,
    context: RepositoryReadContext,
    request: u64,
    body: StarReplyBody,
}

enum StarReplyBody {
    Changed(Box<Result<StarActionOutcome, RepositoryActionError>>),
}

/// The browsing controller. It owns no display state: the page mirrors what it publishes into NSH
/// signals after every drain and user action.
pub struct RepositoryController {
    context: PageContext,
    extras: extras::Extras,
    locator: Option<RepositoryLocator>,
    generation: u64,
    session: Option<RepositorySession>,
    cached_session: Option<CachedRepositorySession>,
    cache_binding: Option<AccountCache>,
    context_data: Option<RepositoryReadContext>,
    work: Option<Work>,
    /// Navigation intent: branch, selected path and expanded directories.
    intent: RepositoryNavigation,
    details: Option<RepositoryDetails>,
    details_provenance: Option<RepositoryCacheProvenance>,
    branches: Option<BranchesOutcome>,
    branches_provenance: Option<RepositoryCacheProvenance>,
    branches_error: Option<PageError>,
    branch: Option<BranchState>,
    /// Monotonic identity of the newest branch resolution. A → B → A shares the
    /// page generation, so a branch name alone cannot tell an older A reply from
    /// the current one; this does.
    branch_request: u64,
    branches_loading: bool,
    selection_request: u64,
    pending_content: Option<BlobIdentity>,
    selection: Selection,
    /// The README's own descriptor path, once discovery found one.
    readme_path: Option<String>,
    reader_status: ReaderStatus,
    /// A page-level failure (repository summary, branch list, revision).
    error: Option<PageError>,
    /// The in-page content tab.
    tab: RepositoryTab,
    /// True while a browsing generation is running.
    active: bool,
    /// Set when authorization changed under a running generation.
    authorization_lost: bool,
    summary_slot: PageSlot<SectionStamp>,
    branches_slot: PageSlot<SectionStamp>,
    revision_slot: PageSlot<SectionStamp>,
    content_slot: PageSlot<ContentStamp>,
    tickets: HashMap<u64, PageTicket>,
    next_request: u64,
    prepared: Option<PreparedReader>,
    /// The newest content generation the page should mount.
    content_key: u64,
    /// Captured tokens for the values currently on screen, one per resource.
    captured_summary: CapturedAuthorization,
    captured_branches: CapturedAuthorization,
    captured_revision: CapturedAuthorization,
    captured_content: CapturedAuthorization,
    /// Flattened visible rows, rebuilt only when the tree or expansion changes.
    rows: Rc<Vec<TreeRow>>,
    rows_dirty: bool,
    rows_capped: bool,
    /// The keyboard's row, as a stable path rather than an index.
    cursor: Option<String>,
    star: StarState,
    star_work: Option<StarWork>,
    star_request: u64,
    /// Monotonic user-write intent. A repository summary captured before this
    /// changes cannot replace the post-write authoritative summary.
    star_write_generation: u64,
    star_inbox: Arc<Mutex<VecDeque<StarReply>>>,
    /// Typed actions the shell has to perform (navigation, external URLs).
    actions: VecDeque<PageAction>,
    limits: PageLimits,
    external_images: Option<Arc<dyn ExternalImageBytes>>,
    /// The application's single background executor, used to build the README image provider. The
    /// shell passes it (see integration-notes); without it the reader keeps honest image
    /// placeholders instead of opening another executor or network path.
    image_executor: Option<Arc<crate::network::executor::BackgroundExecutor>>,
    image_limits: ImageLimits,
    clock: std::time::Instant,
    pinned_clock: Option<u64>,
    inbox: Arc<Mutex<VecDeque<Message>>>,
    alive: Arc<AtomicBool>,
}

impl RepositoryController {
    /// Builds the controller and starts the first read. Everything reached from here is an
    /// existing service captured by `context`; no database, credential or HTTP client is opened.
    pub fn new(context: PageContext, owner: &str, name: &str, intent: WorkspaceNavigation) -> Self {
        let locator = RepositoryLocator::new(owner, name).ok();
        let intent = RepositoryNavigation {
            branch: intent.branch,
            path: intent.file,
            expanded: BTreeSet::new(),
        };
        let mut controller = Self {
            extras: extras::Extras::default(),
            context,
            locator,
            generation: 0,
            session: None,
            cached_session: None,
            cache_binding: None,
            context_data: None,
            work: None,
            selection: match &intent.path {
                Some(path) => Selection::File(path.clone()),
                None => Selection::Readme,
            },
            intent,
            details: None,
            details_provenance: None,
            branches: None,
            branches_provenance: None,
            branches_error: None,
            branch: None,
            branch_request: 0,
            branches_loading: false,
            selection_request: 0,
            pending_content: None,
            readme_path: None,
            reader_status: ReaderStatus::Loading,
            error: None,
            tab: RepositoryTab::Code,
            active: false,
            authorization_lost: false,
            summary_slot: PageSlot::default(),
            branches_slot: PageSlot::default(),
            revision_slot: PageSlot::default(),
            content_slot: PageSlot::default(),
            tickets: HashMap::new(),
            next_request: 0,
            prepared: None,
            content_key: 0,
            captured_summary: None,
            captured_branches: None,
            captured_revision: None,
            captured_content: None,
            rows: Rc::new(Vec::new()),
            rows_dirty: false,
            rows_capped: false,
            cursor: None,
            star: StarState::default(),
            star_work: None,
            star_request: 0,
            star_write_generation: 0,
            star_inbox: Arc::new(Mutex::new(VecDeque::new())),
            actions: VecDeque::new(),
            limits: PageLimits::default(),
            external_images: None,
            image_executor: None,
            image_limits: ImageLimits::default(),
            clock: std::time::Instant::now(),
            pinned_clock: None,
            inbox: Arc::new(Mutex::new(VecDeque::new())),
            alive: Arc::new(AtomicBool::new(true)),
        };
        controller.activate();
        controller
    }

    pub fn with_limits(mut self, limits: PageLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn with_external_images(mut self, external: Arc<dyn ExternalImageBytes>) -> Self {
        self.external_images = Some(external);
        self
    }

    pub fn with_image_limits(mut self, limits: ImageLimits) -> Self {
        self.image_limits = limits;
        self
    }

    /// The shell's single background executor, required only to fetch README pictures.
    pub fn with_image_executor(
        mut self,
        executor: Arc<crate::network::executor::BackgroundExecutor>,
    ) -> Self {
        self.image_executor = Some(executor);
        self
    }

    /// The navigation intent a host may save and restore.
    pub fn navigation(&self) -> RepositoryNavigation {
        self.intent.clone()
    }

    /// The shell's own saved navigation (branch and file only).
    pub fn workspace_intent(&self) -> WorkspaceNavigation {
        WorkspaceNavigation {
            branch: self.intent.branch.clone(),
            file: self.intent.path.clone(),
        }
    }

    pub fn identity(&self) -> &PageIdentity {
        self.context.identity()
    }

    pub fn context(&self) -> &PageContext {
        &self.context
    }

    pub fn locator(&self) -> Option<&RepositoryLocator> {
        self.locator.as_ref()
    }

    pub fn details(&self) -> Option<&RepositoryDetails> {
        self.details.as_ref()
    }

    pub fn details_provenance(&self) -> Option<&RepositoryCacheProvenance> {
        self.details_provenance.as_ref()
    }

    pub fn branches(&self) -> Option<&BranchesOutcome> {
        self.branches.as_ref()
    }

    pub fn branches_provenance(&self) -> Option<&RepositoryCacheProvenance> {
        self.branches_provenance.as_ref()
    }

    pub fn branches_error(&self) -> Option<&PageError> {
        self.branches_error.as_ref()
    }

    pub fn branches_loading(&self) -> bool {
        self.branches_loading
    }

    pub fn revision_provenance(&self) -> Option<&RepositoryCacheProvenance> {
        self.branch
            .as_ref()
            .and_then(|branch| branch.snapshot_provenance.as_ref())
    }

    pub fn is_showing_cached_snapshot(&self) -> bool {
        self.historical_verified_at_ms().is_some()
    }

    /// The oldest verification time among the currently displayed sources that
    /// actually came from the persistent cache, or `None` when every displayed
    /// source was verified online.
    pub fn historical_verified_at_ms(&self) -> Option<i64> {
        [
            self.details_provenance.as_ref(),
            self.branches_provenance.as_ref(),
            self.revision_provenance(),
        ]
        .into_iter()
        .flatten()
        .filter(|provenance| provenance.origin == RepositoryCacheOrigin::Snapshot)
        .map(|provenance| provenance.verified_at_ms)
        .min()
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

    pub fn tab(&self) -> RepositoryTab {
        self.tab
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn authorization_lost(&self) -> bool {
        self.authorization_lost
    }

    pub fn is_showing_readme(&self) -> bool {
        self.selection == Selection::Readme
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

    /// The loaded tree for `path`, for tests.
    pub fn tree(&self, path: &str) -> Option<&DirectoryTree> {
        self.branch.as_ref()?.tree.tree(path)
    }

    pub fn cursor(&self) -> Option<&str> {
        self.cursor.as_deref()
    }

    pub fn focus_row(&mut self, path: &str) {
        self.cursor = Some(path.to_string());
    }

    pub fn starred_state(&self) -> Option<StarredState> {
        self.star.observed
    }

    pub fn star_is_busy(&self) -> bool {
        matches!(
            self.star.phase,
            StarPhase::Loading | StarPhase::Writing { .. }
        )
    }

    pub fn star_phase_failure(&self) -> Option<&str> {
        match &self.star.phase {
            StarPhase::Failed { message } => Some(message.as_str()),
            _ => None,
        }
    }

    pub fn overview(&self) -> Option<&crate::ui::repository_overview::RepositoryOverview> {
        self.extras.overview()
    }

    /// Takes the next typed action for the shell.
    pub fn take_action(&mut self) -> Option<PageAction> {
        self.actions.pop_front()
    }

    /// Takes the newest prepared reader document (once per content generation).
    pub fn take_prepared_reader(&mut self) -> Option<PreparedReaderParts> {
        let prepared = self.prepared.take()?;
        Some((prepared.document, prepared.provider))
    }

    /// The reader's image limits (the provider was built with the same values).
    pub fn image_limits(&self) -> ImageLimits {
        self.image_limits.clone()
    }

    /// Moves the tree cursor to the first visible row.
    pub fn focus_first_row(&mut self) {
        if let Some(path) = self.refresh_rows().first().map(|row| row.path.clone()) {
            self.cursor = Some(path);
        }
    }

    /// Moves the tree cursor to the last visible row.
    pub fn focus_last_row(&mut self) {
        if let Some(path) = self.refresh_rows().last().map(|row| row.path.clone()) {
            self.cursor = Some(path);
        }
    }

    pub fn content_key(&self) -> u64 {
        self.content_key
    }

    /// The image-authorization guard for the prepared reader. It performs in-memory checks only.
    pub fn reader_guard(&self) -> Arc<dyn Fn() -> bool + Send + Sync> {
        let service = self.context.services().repositories();
        let context = self.context.context().clone();
        let alive = self.alive.clone();
        let authorization = self
            .captured_content
            .clone()
            .or_else(|| self.current_authorization());
        Arc::new(move || {
            alive.load(Ordering::Acquire)
                && service.is_current(&context)
                && authorization
                    .as_ref()
                    .is_none_or(CacheAuthorization::is_current)
        })
    }

    pub fn reader_layout(&self) -> ReaderLayout {
        ReaderLayout::Repository
    }

    fn now_ms(&self) -> u64 {
        if let Some(ms) = self.pinned_clock {
            return ms;
        }
        self.clock.elapsed().as_millis() as u64
    }

    #[cfg(test)]
    pub fn pin_clock(&mut self, ms: u64) {
        self.pinned_clock = Some(ms);
    }

    #[cfg(test)]
    pub fn unpin_clock(&mut self) {
        self.pinned_clock = None;
    }

    // ---- lifecycle ----

    /// Starts (or restarts) reading under the captured page context.
    pub fn activate(&mut self) {
        self.stop_star();
        extras::stop(self);
        self.branches_loading = false;
        self.stop_work();
        self.pending_content = None;
        self.selection_request = self.selection_request.wrapping_add(1);
        self.generation = self.generation.wrapping_add(1);
        self.reset_authorization();
        self.authorization_lost = false;
        // Clear every authorization-bound value before reading: a failing first
        // read must not leave the previous account's summary/tree/reader behind.
        self.active = false;
        self.session = None;
        self.cached_session = None;
        self.cache_binding = None;
        self.context_data = None;
        self.details = None;
        self.details_provenance = None;
        self.branches = None;
        self.branches_provenance = None;
        self.branches_error = None;
        self.branch = None;
        self.reader_status = ReaderStatus::Loading;
        self.prepared = None;
        self.content_key = self.content_key.wrapping_add(1);
        self.readme_path = None;
        self.cursor = None;
        self.invalidate_rows();
        let Some(locator) = self.locator.clone() else {
            self.error = Some(PageError::new(
                "This repository address is not valid.",
                Some(
                    "The owner or repository name was rejected before any request was sent.".into(),
                ),
                false,
            ));
            return;
        };
        let session = match self.context.repository_session() {
            Ok(session) => session,
            Err(error) => {
                self.error = Some(PageError::from_repository(&error));
                return;
            }
        };
        let cache_binding = self.context.cache().cloned();
        let cached_session = cache_binding.as_ref().and_then(|binding| {
            let (store, scope) = binding.clone().into_parts();
            CachedRepositorySession::new(session.clone(), store, scope).ok()
        });
        self.context_data = Some(session.context().clone());
        self.cache_binding = cache_binding;
        self.cached_session = cached_session;
        self.session = Some(session);
        self.active = true;
        self.error = None;
        self.work = Some(Work::new());
        self.spawn_repository_read(RepositoryCacheMode::OnlinePreferred, locator);
    }

    /// Cancels in-flight work and keeps only navigation intent.
    pub fn retire(&mut self) {
        self.stop_star();
        extras::stop(self);
        self.stop_work();
        self.generation = self.generation.wrapping_add(1);
        self.reset_authorization();
        self.alive.store(false, Ordering::Release);
        self.active = false;
        self.session = None;
        self.cached_session = None;
        self.cache_binding = None;
        self.context_data = None;
        self.prepared = None;
        self.content_key = self.content_key.wrapping_add(1);
    }

    /// Suspend background gestures for an ordinary workspace-tab switch.
    pub fn suspend(&mut self) {}

    /// Re-reads the saved navigation under the current context.
    pub fn rebind(&mut self, context: PageContext) {
        self.context = context;
        self.alive.store(true, Ordering::Release);
        self.activate();
    }

    pub fn set_active(&mut self, active: bool) {
        if active {
            self.suspend();
        }
    }

    /// One user-initiated retry of the failed step. There is no automatic retry
    /// loop; each press starts a new bounded attempt of that step only.
    pub fn retry(&mut self) {
        if !self.active || self.authorization_lost || self.details.is_none() {
            self.activate();
            return;
        }
        let Some(branch) = self.branch.as_ref().map(|branch| branch.name.clone()) else {
            if self.branches.is_some()
                && let Some(name) = self.intent.branch.clone()
            {
                self.begin_branch(name);
                return;
            }
            if let Some(details) = self.details.clone() {
                self.read_branches(details.identity, RepositoryCacheMode::OnlinePreferred);
            }
            return;
        };
        self.begin_branch(branch);
    }

    /// Refreshes branch enumeration without replacing the displayed snapshot.
    pub fn retry_branches(&mut self) {
        if self.branches_loading {
            return;
        }
        if let Some(details) = &self.details {
            self.branches_loading = true;
            self.read_branches(
                details.identity.clone(),
                RepositoryCacheMode::OnlinePreferred,
            );
        }
    }

    /// Revalidates repository summary, branches, revision and overview online.
    pub fn refresh(&mut self) {
        if !self.active || self.authorization_lost {
            self.activate();
            return;
        }
        let Some(locator) = self.locator.clone() else {
            return;
        };
        self.stop_star();
        extras::stop(self);
        self.stop_work();
        self.generation = self.generation.wrapping_add(1);
        self.branch_request = self.branch_request.wrapping_add(1);
        self.reset_authorization();
        self.inbox.lock().expect("repository inbox").clear();
        self.work = Some(Work::new());
        self.pending_content = None;
        self.selection_request = self.selection_request.wrapping_add(1);
        self.details = None;
        self.details_provenance = None;
        self.branch = None;
        self.branches = None;
        self.branches_provenance = None;
        self.branches_error = None;
        self.branches_loading = false;
        self.prepared = None;
        self.content_key = self.content_key.wrapping_add(1);
        self.readme_path = None;
        self.reader_status = ReaderStatus::Loading;
        self.error = None;
        self.cursor = None;
        self.invalidate_rows();
        self.spawn_repository_read(RepositoryCacheMode::OnlinePreferred, locator);
    }

    // ---- user actions ----

    /// Switches to `name`, re-resolving it to a new fixed commit.
    pub fn select_branch(&mut self, name: &str) {
        if !self.active || self.authorization_lost {
            return;
        }
        if let Ok(branch) = DynamicRef::new(name) {
            self.begin_branch(branch.as_str().to_string());
        }
    }

    /// Switches the in-page content tab.
    pub fn select_tab(&mut self, tab: RepositoryTab) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        if tab == RepositoryTab::Code {
            // Returning to Code: if authorization was lost while hidden, the page
            // must re-verify rather than show data from an old session.
            self.check_authorization(&Sessions::new(), true);
        }
    }

    /// Expands or collapses a directory, loading it the first time it opens.
    pub fn toggle_directory(&mut self, path: &str) {
        if self.intent.expanded.remove(path) {
            self.invalidate_rows();
        } else {
            self.intent.expanded.insert(path.to_string());
            self.invalidate_rows();
            self.load_directory(path);
        }
    }

    fn expand_directory(&mut self, path: &str) {
        if self.intent.expanded.insert(path.to_string()) {
            self.invalidate_rows();
            self.load_directory(path);
        }
    }

    fn collapse_directory(&mut self, path: &str) {
        if self.intent.expanded.remove(path) {
            self.invalidate_rows();
        }
    }

    /// Selects a file at the current fixed commit.
    pub fn select_file(&mut self, path: &str) {
        if self.branch.is_none() {
            return;
        }
        self.selection_request = self.selection_request.wrapping_add(1);
        self.pending_content = None;
        self.intent.path = Some(path.to_string());
        self.selection = Selection::File(path.to_string());
        self.reader_status = ReaderStatus::Loading;
        self.prepared = None;
        self.cursor = Some(path.to_string());
        self.resolve_selection();
    }

    /// Returns the reader to the repository README (its real path, wherever it is).
    pub fn show_readme(&mut self) {
        if self.branch.is_none() {
            return;
        }
        self.selection_request = self.selection_request.wrapping_add(1);
        self.pending_content = None;
        self.intent.path = None;
        self.selection = Selection::Readme;
        self.reader_status = ReaderStatus::Loading;
        self.prepared = None;
        if let Some(path) = self.readme_path.clone() {
            self.cursor = Some(path);
        }
        self.resolve_selection();
    }

    /// Re-reads README discovery after a failed or uncertain attempt.
    pub fn retry_readme(&mut self) {
        let Some(branch) = self.branch.as_ref() else {
            return;
        };
        let snapshot = branch.snapshot.clone();
        let name = branch.name.clone();
        self.selection_request = self.selection_request.wrapping_add(1);
        self.pending_content = None;
        if let Some(state) = self.branch.as_mut() {
            state.readme = ReadmeState::Unknown;
        }
        self.prepared = None;
        self.readme_path = None;
        self.reader_status = ReaderStatus::Loading;
        self.spawn_readme(name, snapshot, RepositoryCacheMode::OnlinePreferred);
    }

    /// Retries one failed directory only.
    pub fn retry_directory(&mut self, path: &str) {
        let Some(branch) = self.branch.as_mut() else {
            return;
        };
        let handle = if path.is_empty() {
            Ok(SnapshotDirectory::root(&branch.snapshot))
        } else {
            branch.tree.child_directory(path)
        };
        let branch_name = branch.name.clone();
        let commit = branch.commit().clone();
        let handle = match handle {
            Ok(handle) => handle,
            Err(_) => {
                self.invalidate_rows();
                return;
            }
        };
        branch.tree.begin_load(path);
        self.invalidate_rows();
        self.spawn_directory(
            branch_name,
            commit,
            path.to_string(),
            handle,
            RepositoryCacheMode::OnlinePreferred,
        );
    }

    /// Retries only the currently selected document (README or file).
    pub fn retry_reader(&mut self) {
        if self.selection == Selection::Readme
            && self.branch.as_ref().is_some_and(|branch| {
                matches!(
                    branch.readme,
                    ReadmeState::Failed(_) | ReadmeState::Unknown | ReadmeState::Uncertain
                )
            })
        {
            self.retry_readme();
            return;
        }
        self.selection_request = self.selection_request.wrapping_add(1);
        self.pending_content = None;
        self.prepared = None;
        self.reader_status = ReaderStatus::Loading;
        if let Some(path) = self.current_path().map(str::to_string) {
            for ancestor in RepositoryNavigation::ancestors(&path) {
                if self.tree_failure(&ancestor).is_some() {
                    self.retry_directory(&ancestor);
                    return;
                }
            }
        }
        self.resolve_selection();
    }

    /// Star press: only a real server observation toggles, never a local guess.
    pub fn press_star(&mut self) {
        if self.star_work.is_some() || self.is_showing_cached_snapshot() {
            return;
        }
        let Some(observed) = self.star.observed else {
            self.start_star_read();
            return;
        };
        let Some(binding) = self.star_binding() else {
            return;
        };
        let Some(service) = self.star_service() else {
            return;
        };
        let desired = observed.toggled();
        self.star_write_generation = self.star_write_generation.wrapping_add(1);
        self.star.phase = StarPhase::Writing { desired };
        self.submit_star(binding, service, StarSubmission::Write(desired));
        self.actions.push_back(PageAction::StarIntent {
            starred: desired.is_starred(),
        });
    }

    /// Reconcile again after an unknown transport result. Starts a new GET only.
    pub fn retry_star(&mut self) {
        if self.star_work.is_some() {
            return;
        }
        self.star_request = self.star_request.wrapping_add(1);
        self.start_star_read();
    }

    /// The view's Star entry: retry an unknown outcome, otherwise submit the toggle.
    pub fn star_entry(&mut self) {
        if self.star_is_busy() {
            return;
        }
        if self.star_phase_failure().is_some() {
            self.retry_star();
            return;
        }
        self.press_star();
    }

    /// A release or contributor typed action, re-validated against what is displayed.
    pub fn overview_action(&mut self, action: crate::ui::repository_overview::OverviewAction) {
        use crate::ui::repository_overview::{OverviewAction, OverviewSection};
        let binding = match &action {
            OverviewAction::Refresh { binding, .. }
            | OverviewAction::OpenRelease { binding, .. }
            | OverviewAction::OpenContributor { binding, .. } => binding,
        };
        if !extras::accepts(self, binding) {
            return;
        }
        let authorization = self.extras.displayed_authorization();
        match action {
            OverviewAction::Refresh { section, .. } => match section {
                OverviewSection::Releases => {
                    extras::read_section(
                        self,
                        extras::Endpoint::Releases,
                        false,
                        authorization.clone(),
                    );
                    extras::read_section(self, extras::Endpoint::Latest, false, authorization);
                }
                OverviewSection::Contributors => {
                    extras::read_section(
                        self,
                        extras::Endpoint::Contributors,
                        false,
                        authorization,
                    );
                }
                OverviewSection::Languages => {
                    extras::read_section(self, extras::Endpoint::Languages, false, authorization);
                }
            },
            action => {
                if extras::valid_action(self, &action) {
                    self.actions.push_back(PageAction::Overview(action));
                }
            }
        }
    }

    /// A reader link press. External URLs go to the shell; a same-repository,
    /// same-commit relative link is followed inside this page generation.
    pub fn reader_action(&mut self, action: ReaderAction) {
        let ReaderAction::Link(target) = action else {
            return;
        };
        match target {
            LinkTarget::External { url } => self.actions.push_back(PageAction::OpenExternal(url)),
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
                    self.actions.push_back(PageAction::LinkNotFollowed {
                        path: link.path,
                        reason:
                            "The link points at a different repository; this page keeps its own authorization and commit."
                                .to_string(),
                    });
                    return;
                }
                if link.commit != commit {
                    self.actions.push_back(PageAction::LinkNotFollowed {
                        path: link.path,
                        reason: "The link names a different commit than the one on screen."
                            .to_string(),
                    });
                    return;
                }
                self.select_file(&link.path);
            }
            LinkTarget::Anchor { .. } | LinkTarget::Refused { .. } => {}
        }
    }

    // ---- tree keyboard navigation ----

    /// Moves the tree cursor by `delta` visible rows.
    pub fn move_cursor(&mut self, delta: isize) {
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
        self.cursor = Some(rows[next].path.clone());
    }

    /// Left: collapse the current directory, or climb to its parent.
    pub fn cursor_left(&mut self) {
        let Some(path) = self.cursor.clone() else {
            return;
        };
        let rows = self.refresh_rows();
        let Some(row) = rows.iter().find(|row| row.path == path) else {
            return;
        };
        if row.kind == (TreeRowKind::Directory { expanded: true }) {
            self.collapse_directory(&path);
            return;
        }
        if !row.parent.is_empty() {
            self.cursor = Some(row.parent.clone());
        }
    }

    /// Right: expand the current directory, or step into its first child.
    pub fn cursor_right(&mut self) {
        let Some(path) = self.cursor.clone() else {
            return;
        };
        let rows = self.refresh_rows();
        let Some(row) = rows.iter().find(|row| row.path == path) else {
            return;
        };
        match row.kind {
            TreeRowKind::Directory { expanded: false } => self.expand_directory(&path),
            TreeRowKind::Directory { expanded: true } => {
                if let Some(index) = rows.iter().position(|row| row.path == path)
                    && let Some(child) = rows.get(index + 1)
                    && child.depth > row.depth
                {
                    self.cursor = Some(child.path.clone());
                }
            }
            _ => {}
        }
    }

    /// Enter: activate the current row (toggle a directory, open a file).
    pub fn cursor_activate(&mut self) {
        let Some(path) = self.cursor.clone() else {
            return;
        };
        let rows = self.refresh_rows();
        let Some(row) = rows.iter().find(|row| row.path == path) else {
            return;
        };
        if row.kind.is_directory() {
            self.toggle_directory(&path);
        } else {
            self.select_file(&path);
        }
    }

    // ---- drain ----

    /// Drains background results, then re-checks that everything on screen is still authorized.
    pub fn poll(&mut self, sessions: &Sessions) {
        self.check_authorization(sessions, false);
        let messages: Vec<Message> = {
            let mut inbox = self.inbox.lock().expect("repository inbox");
            inbox.drain(..).collect()
        };
        for message in messages {
            self.apply(message, sessions);
        }
        extras::drain(self, sessions);
        self.drain_star(sessions);
        self.sync_displayed(sessions);
    }

    /// Detects an account/authorization change (including cache gate revocation or
    /// credential rejection) even when no message is pending, so data read under a
    /// previous or revoked authorization cannot stay on screen.
    fn check_authorization(&mut self, sessions: &Sessions, explicit: bool) {
        if !self.active || self.authorization_lost {
            return;
        }
        let current = explicit || self.context.is_current(sessions);
        if current && self.cache_allowed() {
            return;
        }
        self.stop_star();
        extras::stop(self);
        self.authorization_lost = true;
        self.active = false;
        self.stop_work();
        self.reset_authorization();
        self.session = None;
        self.cached_session = None;
        self.cache_binding = None;
        self.context_data = None;
        self.details = None;
        self.details_provenance = None;
        self.branches = None;
        self.branches_provenance = None;
        self.branches_error = None;
        self.branch = None;
        self.prepared = None;
        self.content_key = self.content_key.wrapping_add(1);
        self.readme_path = None;
        self.reader_status = ReaderStatus::Loading;
        self.cursor = None;
        self.invalidate_rows();
        self.error = Some(PageError::new(
            "The account or authorization changed.",
            Some(
                "Data read under the previous authorization was cleared. Reopen the repository under the current account to read it."
                    .to_string(),
            ),
            true,
        ));
    }

    /// Drops displayed values whose ticket or authorization is no longer valid.
    fn sync_displayed(&mut self, sessions: &Sessions) {
        if self.summary_slot.value(sessions).is_none() && self.details.is_some() {
            self.details = None;
            self.details_provenance = None;
            self.captured_summary = None;
        }
        if self.branches_slot.value(sessions).is_none() && self.branches.is_some() {
            self.branches = None;
            self.branches_provenance = None;
            self.captured_branches = None;
        }
        if self.revision_slot.value(sessions).is_none() && self.branch.is_some() {
            self.branch = None;
            self.prepared = None;
            self.captured_revision = None;
            self.content_key = self.content_key.wrapping_add(1);
            self.invalidate_rows();
        }
        if self.content_slot.value(sessions).is_none()
            && matches!(self.reader_status, ReaderStatus::Showing)
        {
            self.reader_status = ReaderStatus::Loading;
            self.prepared = None;
            self.captured_content = None;
            self.content_key = self.content_key.wrapping_add(1);
        }
    }

    /// Cheap, non-blocking publication/display check: lock-free atomic loads only.
    fn cache_allowed(&self) -> bool {
        extras::authorization_current(self) && self.displayed_authorization_current()
    }

    fn displayed_authorization_current(&self) -> bool {
        self.captured_summary
            .iter()
            .chain(self.captured_branches.iter())
            .chain(self.captured_revision.iter())
            .chain(self.captured_content.iter())
            .all(CacheAuthorization::is_current)
    }

    fn reset_authorization(&mut self) {
        self.captured_summary = None;
        self.captured_branches = None;
        self.captured_revision = None;
        self.captured_content = None;
    }

    /// Captures the shared cache authorization token for the current session and
    /// (when known) repository. In-process only.
    fn capture_authorization(
        store: &CacheStore,
        scope: &CacheScope,
        repository_id: Option<u64>,
    ) -> CapturedAuthorization {
        store.authorization(scope, repository_id)
    }

    fn current_authorization(&self) -> CapturedAuthorization {
        let binding = self.cache_binding.as_ref()?;
        let repository = self.details.as_ref().map(|details| details.identity.id());
        Self::capture_authorization(binding.store(), binding.scope(), repository)
    }

    fn cache_hint(&self) -> Option<CacheHint> {
        let binding = self.cache_binding.as_ref()?;
        Some(CacheHint {
            store: binding.store().clone(),
            scope: binding.scope().clone(),
        })
    }

    fn stop_work(&mut self) {
        if let Some(work) = self.work.take() {
            work.source.cancel();
        }
        self.tickets.clear();
        self.summary_slot.cancel();
        self.branches_slot.cancel();
        self.revision_slot.cancel();
        self.content_slot.cancel();
    }

    fn stop_star(&mut self) {
        if let Some(star) = self.star_work.take() {
            star.work.source.cancel();
        }
        self.star_request = self.star_request.wrapping_add(1);
        self.star_inbox.lock().expect("star inbox").clear();
        self.star = StarState::default();
    }

    fn invalidate_rows(&mut self) {
        self.rows_dirty = true;
    }
}

// ---- read submission ----

impl RepositoryController {
    fn next_ticket(&mut self, slot: SlotKind) -> Option<u64> {
        let context = self.context.clone();
        let ticket = match slot {
            SlotKind::Summary => self.summary_slot.begin(context),
            SlotKind::Branches => self.branches_slot.begin(context),
            SlotKind::Revision => self.revision_slot.begin(context),
            SlotKind::Content => self.content_slot.begin(context),
        };
        self.next_request = self.next_request.wrapping_add(1);
        let id = self.next_request;
        self.tickets.insert(id, ticket);
        Some(id)
    }

    fn spawn_repository_read(&mut self, mode: RepositoryCacheMode, locator: RepositoryLocator) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let Some(context) = self.context_data.clone() else {
            return;
        };
        let Some(cancel) = self.work.as_ref().map(Work::token) else {
            return;
        };
        let hint = self.cache_hint();
        let authorization = self
            .cache_binding
            .as_ref()
            .and_then(|b| Self::capture_authorization(b.store(), b.scope(), None));
        let Some(ticket) = self.next_ticket(SlotKind::Summary) else {
            return;
        };
        let generation = self.generation;
        let branch_request = self.branch_request;
        let cached = self.cached_session.clone();
        let inbox = self.inbox.clone();
        let alive = self.alive.clone();
        let limits = self.limits.clone();
        let star_write_generation = self.star_write_generation;
        let job = Job::Repository {
            locator,
            star_write_generation,
            mode,
            hint,
        };
        let spawned = self.context.services().spawn(async move {
            let body = run_job(cached.as_ref(), &session, job, &limits, cancel).await;
            if alive.load(Ordering::Acquire) {
                inbox.lock().expect("repository inbox").push_back(Message {
                    generation,
                    branch_request,
                    authorization,
                    context,
                    ticket: Some(ticket),
                    body,
                });
            }
        });
        if spawned.is_err() {
            self.error = Some(background_closing());
        }
    }

    fn read_branches(&mut self, identity: RepositoryIdentity, mode: RepositoryCacheMode) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let Some(context) = self.context_data.clone() else {
            return;
        };
        let Some(cancel) = self.work.as_ref().map(Work::token) else {
            return;
        };
        let authorization =
            Self::capture_authorization_for(self.cache_binding.as_ref(), identity.id());
        let Some(ticket) = self.next_ticket(SlotKind::Branches) else {
            return;
        };
        let generation = self.generation;
        let branch_request = self.branch_request;
        let cached = self.cached_session.clone();
        let inbox = self.inbox.clone();
        let alive = self.alive.clone();
        let limits = self.limits.clone();
        let job = Job::Branches { identity, mode };
        let spawned = self.context.services().spawn(async move {
            let body = run_job(cached.as_ref(), &session, job, &limits, cancel).await;
            if alive.load(Ordering::Acquire) {
                inbox.lock().expect("repository inbox").push_back(Message {
                    generation,
                    branch_request,
                    authorization,
                    context,
                    ticket: Some(ticket),
                    body,
                });
            }
        });
        if spawned.is_err() {
            self.error = Some(background_closing());
        }
    }

    fn begin_branch(&mut self, name: String) {
        if !self.active || self.authorization_lost {
            return;
        }
        let Some(details) = self.details.clone() else {
            return;
        };
        self.stop_work();
        self.work = Some(Work::new());
        self.pending_content = None;
        self.selection_request = self.selection_request.wrapping_add(1);
        self.intent.branch = Some(name.clone());
        self.branch = None;
        self.prepared = None;
        self.readme_path = None;
        self.reader_status = ReaderStatus::Loading;
        self.error = None;
        self.cursor = None;
        self.invalidate_rows();
        // Every resolution gets its own identity, so a slower reply for an
        // earlier request can never be mistaken for the current one even when
        // both name the same branch (A → B → A).
        self.branch_request = self.branch_request.wrapping_add(1);
        self.read_revision(
            name,
            self.branch_request,
            details.identity,
            RepositoryCacheMode::OnlinePreferred,
        );
    }

    fn read_revision(
        &mut self,
        name: String,
        request: u64,
        identity: RepositoryIdentity,
        mode: RepositoryCacheMode,
    ) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let Some(context) = self.context_data.clone() else {
            return;
        };
        let Some(cancel) = self.work.as_ref().map(Work::token) else {
            return;
        };
        let authorization =
            Self::capture_authorization_for(self.cache_binding.as_ref(), identity.id());
        let Some(ticket) = self.next_ticket(SlotKind::Revision) else {
            return;
        };
        let generation = self.generation;
        let branch_request = self.branch_request;
        let cached = self.cached_session.clone();
        let inbox = self.inbox.clone();
        let alive = self.alive.clone();
        let limits = self.limits.clone();
        let job = Job::Revision {
            branch: name,
            request,
            identity,
            mode,
        };
        let spawned = self.context.services().spawn(async move {
            let body = run_job(cached.as_ref(), &session, job, &limits, cancel).await;
            if alive.load(Ordering::Acquire) {
                inbox.lock().expect("repository inbox").push_back(Message {
                    generation,
                    branch_request,
                    authorization,
                    context,
                    ticket: Some(ticket),
                    body,
                });
            }
        });
        if spawned.is_err() {
            self.error = Some(background_closing());
        }
    }

    fn spawn_root(&mut self, branch: String, snapshot: RepoSnapshot, mode: RepositoryCacheMode) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let Some(context) = self.context_data.clone() else {
            return;
        };
        let Some(cancel) = self.work.as_ref().map(Work::token) else {
            return;
        };
        let authorization =
            Self::capture_authorization_for(self.cache_binding.as_ref(), snapshot.repository.id());
        let generation = self.generation;
        let branch_request = self.branch_request;
        let cached = self.cached_session.clone();
        let inbox = self.inbox.clone();
        let alive = self.alive.clone();
        let limits = self.limits.clone();
        let job = Job::Root {
            branch,
            snapshot,
            mode,
        };
        let spawned = self.context.services().spawn(async move {
            let body = run_job(cached.as_ref(), &session, job, &limits, cancel).await;
            if alive.load(Ordering::Acquire) {
                inbox.lock().expect("repository inbox").push_back(Message {
                    generation,
                    branch_request,
                    authorization,
                    context,
                    ticket: None,
                    body,
                });
            }
        });
        if spawned.is_err() {
            self.error = Some(background_closing());
        }
    }

    fn spawn_readme(&mut self, branch: String, snapshot: RepoSnapshot, mode: RepositoryCacheMode) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let Some(context) = self.context_data.clone() else {
            return;
        };
        let Some(cancel) = self.work.as_ref().map(Work::token) else {
            return;
        };
        let authorization =
            Self::capture_authorization_for(self.cache_binding.as_ref(), snapshot.repository.id());
        let generation = self.generation;
        let branch_request = self.branch_request;
        let cached = self.cached_session.clone();
        let inbox = self.inbox.clone();
        let alive = self.alive.clone();
        let limits = self.limits.clone();
        let job = Job::Readme {
            branch,
            snapshot,
            mode,
        };
        let spawned = self.context.services().spawn(async move {
            let body = run_job(cached.as_ref(), &session, job, &limits, cancel).await;
            if alive.load(Ordering::Acquire) {
                inbox.lock().expect("repository inbox").push_back(Message {
                    generation,
                    branch_request,
                    authorization,
                    context,
                    ticket: None,
                    body,
                });
            }
        });
        if spawned.is_err() {
            self.error = Some(background_closing());
        }
    }

    fn spawn_directory(
        &mut self,
        branch: String,
        commit: ObjectSha,
        path: String,
        handle: SnapshotDirectory,
        mode: RepositoryCacheMode,
    ) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let Some(context) = self.context_data.clone() else {
            return;
        };
        let Some(cancel) = self.work.as_ref().map(Work::token) else {
            return;
        };
        let authorization =
            Self::capture_authorization_for(self.cache_binding.as_ref(), handle.repository().id());
        let generation = self.generation;
        let branch_request = self.branch_request;
        let cached = self.cached_session.clone();
        let inbox = self.inbox.clone();
        let alive = self.alive.clone();
        let limits = self.limits.clone();
        let job = Job::Directory {
            branch,
            commit,
            path,
            handle,
            mode,
        };
        let spawned = self.context.services().spawn(async move {
            let body = run_job(cached.as_ref(), &session, job, &limits, cancel).await;
            if alive.load(Ordering::Acquire) {
                inbox.lock().expect("repository inbox").push_back(Message {
                    generation,
                    branch_request,
                    authorization,
                    context,
                    ticket: None,
                    body,
                });
            }
        });
        if spawned.is_err() {
            self.error = Some(background_closing());
        }
    }

    fn spawn_content(
        &mut self,
        branch: String,
        commit: ObjectSha,
        role: Role,
        identity: BlobIdentity,
        mode: RepositoryCacheMode,
    ) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let Some(context) = self.context_data.clone() else {
            return;
        };
        let Some(cancel) = self.work.as_ref().map(Work::token) else {
            return;
        };
        let authorization = Self::capture_authorization_for(
            self.cache_binding.as_ref(),
            identity.repository().id(),
        );
        let Some(ticket) = self.next_ticket(SlotKind::Content) else {
            return;
        };
        let generation = self.generation;
        let branch_request = self.branch_request;
        let selection_request = self.selection_request;
        let cached = self.cached_session.clone();
        let inbox = self.inbox.clone();
        let alive = self.alive.clone();
        let limits = self.limits.clone();
        let job = Job::Content {
            branch,
            commit,
            role,
            selection_request,
            identity,
            mode,
        };
        let spawned = self.context.services().spawn(async move {
            let body = run_job(cached.as_ref(), &session, job, &limits, cancel).await;
            if alive.load(Ordering::Acquire) {
                inbox.lock().expect("repository inbox").push_back(Message {
                    generation,
                    branch_request,
                    authorization,
                    context,
                    ticket: Some(ticket),
                    body,
                });
            }
        });
        if spawned.is_err() {
            self.error = Some(background_closing());
        }
    }

    fn capture_authorization_for(
        binding: Option<&AccountCache>,
        repository_id: u64,
    ) -> CapturedAuthorization {
        let binding = binding?;
        Self::capture_authorization(binding.store(), binding.scope(), Some(repository_id))
    }

    // ---- applying results ----

    fn apply(&mut self, message: Message, sessions: &Sessions) {
        if message.generation != self.generation || !self.active {
            return;
        }
        if self.context_data.as_ref() != Some(&message.context)
            || !self
                .context
                .services()
                .repositories()
                .is_current(&message.context)
            || !message
                .authorization
                .as_ref()
                .is_none_or(CacheAuthorization::is_current)
            || (message.branch_request != self.branch_request
                && !matches!(&message.body, Body::Branches(_)))
        {
            return;
        }
        let ticket = message.ticket.and_then(|id| self.tickets.remove(&id));
        let authorization = message.authorization.clone();
        match message.body {
            Body::Repository {
                result,
                star_write_generation,
            } => {
                let Some(ticket) = ticket else {
                    return;
                };
                if !self.summary_slot.publish(
                    ticket,
                    SectionStamp {
                        authorization: authorization.clone(),
                    },
                    sessions,
                ) {
                    return;
                }
                if self.apply_repository(result, star_write_generation) {
                    self.captured_summary = authorization;
                }
            }
            Body::Branches(result) => {
                let Some(ticket) = ticket else {
                    return;
                };
                if !self.branches_slot.publish(
                    ticket,
                    SectionStamp {
                        authorization: authorization.clone(),
                    },
                    sessions,
                ) {
                    return;
                }
                if self.apply_branches(result) {
                    self.captured_branches = authorization;
                }
            }
            Body::Revision {
                branch,
                request,
                result,
            } => {
                let Some(ticket) = ticket else {
                    return;
                };
                if !self.revision_slot.publish(
                    ticket,
                    SectionStamp {
                        authorization: authorization.clone(),
                    },
                    sessions,
                ) {
                    return;
                }
                if self.apply_revision(branch, request, result) {
                    self.captured_revision = authorization;
                }
            }
            Body::Root {
                branch,
                commit,
                result,
            } => {
                if !self.cache_allowed() {
                    return;
                }
                self.apply_root(branch, commit, result);
            }
            Body::Readme {
                branch,
                commit,
                result,
            } => {
                if !self.cache_allowed() {
                    return;
                }
                self.apply_readme(branch, commit, result);
            }
            Body::Directory {
                branch,
                commit,
                path,
                result,
            } => {
                if !self.cache_allowed() {
                    return;
                }
                self.apply_directory(branch, commit, path, result);
            }
            Body::Content(content) => {
                let Some(ticket) = ticket else {
                    return;
                };
                // The content slot publication still happens even when the reply
                // is for a document the user already left; it is rejected below
                // by the selection identity, and the slot stays available.
                self.apply_content(content, ticket, authorization, sessions);
            }
        }
    }

    fn apply_repository(
        &mut self,
        result: Result<(RepositoryDetails, Option<RepositoryCacheProvenance>), PageError>,
        star_write_generation: u64,
    ) -> bool {
        if star_write_generation != self.star_write_generation && self.details.is_some() {
            return false;
        }
        match result {
            Ok((details, provenance)) => {
                let star_request = self.star_request;
                let identity = details.identity.clone();
                self.details = Some(details);
                // Provenance is applied *before* choosing the next read mode.
                self.details_provenance = provenance;
                let mode = self.current_read_mode();
                self.read_branches(identity.clone(), mode);
                extras::start(self, identity);
                if star_request == self.star_request {
                    self.start_star_read();
                }
                true
            }
            Err(error) => {
                self.error = Some(error);
                false
            }
        }
    }

    fn apply_branches(
        &mut self,
        result: Result<(BranchesOutcome, Option<RepositoryCacheProvenance>), PageError>,
    ) -> bool {
        self.branches_loading = false;
        let Some(details) = self.details.clone() else {
            return false;
        };
        let (outcome, provenance, error) = match result {
            Ok((outcome, provenance)) => (Some(outcome), provenance, None),
            Err(error) => (None, None, Some(error)),
        };
        self.branches_error = error;
        self.branches_provenance = provenance;
        self.branches = outcome.clone();
        if self.branch.is_some() {
            // Retrying enumeration updates only the picker, never the loaded snapshot.
            return true;
        }
        let choice = match &outcome {
            Some(outcome) => branch_choice(
                outcome,
                details.default_branch.as_ref(),
                self.intent.branch.as_deref(),
            ),
            // Without a branch list the repository's observed default branch is
            // still a valid ref; a failed list is not an empty repository.
            None => match self
                .intent
                .branch
                .as_deref()
                .and_then(|name| DynamicRef::new(name).ok())
                .or_else(|| details.default_branch.clone())
            {
                Some(reference) => BranchChoice::Selected(reference),
                None => BranchChoice::None { empty: false },
            },
        };
        match choice {
            BranchChoice::Selected(branch) => {
                self.begin_branch(branch.as_str().to_string());
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
        true
    }

    fn apply_revision(
        &mut self,
        branch: String,
        request: u64,
        result: Result<(RepoSnapshot, Option<RepositoryCacheProvenance>), PageError>,
    ) -> bool {
        // A superseded resolution is discarded whether it succeeded or failed.
        if request != self.branch_request || self.intent.branch.as_deref() != Some(branch.as_str())
        {
            return false;
        }
        match result {
            Ok((snapshot, provenance)) => {
                self.branch = Some(BranchState {
                    name: branch.clone(),
                    snapshot: snapshot.clone(),
                    snapshot_provenance: provenance,
                    tree: TreeModel::default(),
                    readme: ReadmeState::Unknown,
                });
                self.error = None;
                self.prepared = None;
                self.readme_path = None;
                self.reader_status = ReaderStatus::Loading;
                self.invalidate_rows();
                let mode = self.current_read_mode();
                self.spawn_root(branch.clone(), snapshot.clone(), mode);
                self.spawn_readme(branch, snapshot, mode);
                true
            }
            Err(error) => {
                self.branch = None;
                self.prepared = None;
                self.reader_status = ReaderStatus::Loading;
                self.error = Some(error);
                false
            }
        }
    }

    fn apply_root(
        &mut self,
        branch: String,
        commit: ObjectSha,
        result: Result<DirectoryTree, PageError>,
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
                self.load_expanded();
                self.resolve_selection();
            }
            Err(error) => {
                state.tree.fail_load("", error.clone());
                self.invalidate_rows();
                self.error = Some(error);
            }
        }
    }

    fn apply_readme(
        &mut self,
        branch: String,
        commit: ObjectSha,
        result: Result<ReadmeState, PageError>,
    ) {
        let Some(state) = self.branch.as_mut() else {
            return;
        };
        if state.name != branch || state.commit() != &commit {
            return;
        }
        state.readme = match result {
            Ok(readme) => readme,
            Err(error) => ReadmeState::Failed(error),
        };
        if self.selection == Selection::Readme {
            self.request_readme_content();
        }
    }

    fn apply_directory(
        &mut self,
        branch: String,
        commit: ObjectSha,
        path: String,
        result: Result<DirectoryTree, PageError>,
    ) {
        let Some(state) = self.branch.as_mut() else {
            return;
        };
        if state.name != branch || state.commit() != &commit {
            return;
        }
        match result {
            Ok(tree) if path.is_empty() => {
                state.tree.set_root(tree);
                self.error = None;
            }
            Ok(tree) => state.tree.set_children(tree),
            Err(error) => state.tree.fail_load(&path, error),
        }
        let failure = state.tree.failure(&path).cloned();
        if failure.is_none()
            && matches!(self.reader_status, ReaderStatus::ContentFailed(_))
            && self
                .selection
                .path()
                .is_some_and(|selected| RepositoryNavigation::ancestors(selected).contains(&path))
        {
            self.reader_status = ReaderStatus::Loading;
        }
        self.invalidate_rows();
        if let Some(error) = failure
            && self
                .selection
                .path()
                .is_some_and(|selected| RepositoryNavigation::ancestors(selected).contains(&path))
        {
            self.prepared = None;
            self.reader_status = ReaderStatus::ContentFailed(PageError::new(
                "A parent directory could not be read.",
                Some(format!("`{path}`: {}", error.title)),
                error.retryable,
            ));
        }
        self.load_expanded();
        self.resolve_selection();
    }

    fn apply_content(
        &mut self,
        content: ContentReply,
        ticket: PageTicket,
        authorization: Option<CacheAuthorization>,
        sessions: &Sessions,
    ) {
        let ContentReply {
            branch,
            commit,
            role,
            selection_request,
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
        if selection_request != self.selection_request || !role.matches(&self.selection) {
            return;
        }
        self.pending_content = None;
        match result {
            Ok(content) => {
                if content.identity.path().as_str() != path {
                    return;
                }
                let kind = DocumentKind::for_path(&path);
                let provider = self.image_provider_for(&content);
                let document = ReaderDocument::from_prepared(content, kind);
                self.content_key = self.content_key.wrapping_add(1);
                let key = self.content_key;
                if !self
                    .content_slot
                    .publish(ticket, ContentStamp { key }, sessions)
                {
                    return;
                }
                self.captured_content = authorization;
                self.prepared = Some(PreparedReader {
                    path,
                    document,
                    provider,
                });
                self.reader_status = ReaderStatus::Showing;
            }
            Err(error) => {
                self.prepared = None;
                self.reader_status = ReaderStatus::ContentFailed(error);
                // A failed document still occupies the content section: publish a
                // stamp so `sync_displayed` does not treat it as replaced content.
                self.content_key = self.content_key.wrapping_add(1);
                let key = self.content_key;
                if self
                    .content_slot
                    .publish(ticket, ContentStamp { key }, sessions)
                {
                    self.captured_content = authorization;
                }
            }
        }
    }

    /// Loads the directories the user expanded, parents first, one request each.
    fn load_expanded(&mut self) {
        let pending: Vec<String> = {
            let Some(branch) = self.branch.as_ref() else {
                return;
            };
            let mut paths: Vec<&String> = self.intent.expanded.iter().collect();
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
            self.load_directory(&path);
        }
    }

    fn load_directory(&mut self, path: &str) {
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
        let mode = self.current_read_mode();
        self.invalidate_rows();
        self.spawn_directory(branch_name, commit, path.to_string(), handle, mode);
    }

    fn resolve_selection(&mut self) {
        if matches!(self.reader_status, ReaderStatus::ContentFailed(_)) {
            return;
        }
        if self.reader_status == ReaderStatus::Showing
            && self.prepared_path() == self.current_path().map(str::to_string)
        {
            return;
        }
        match self.selection.clone() {
            Selection::Readme => self.request_readme_content(),
            Selection::File(path) => self.resolve_path(&path),
        }
    }

    fn prepared_path(&self) -> Option<String> {
        self.prepared.as_ref().map(|prepared| prepared.path.clone())
    }

    fn resolve_path(&mut self, path: &str) {
        if self
            .branch
            .as_ref()
            .is_none_or(|branch| branch.tree.root().is_none())
        {
            // The root request will resume this selection, including saved paths.
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
                return;
            }
            if let Some(error) = branch.tree.failure(ancestor) {
                self.prepared = None;
                self.reader_status = ReaderStatus::ContentFailed(PageError::new(
                    "A parent directory could not be read.",
                    Some(format!("`{ancestor}`: {}", error.title)),
                    error.retryable,
                ));
                return;
            }
            if branch.tree.child_directory(ancestor).is_err() {
                // The parent *is* loaded but has no such directory entry: no
                // request can ever arrive, so publish a terminal status.
                let error = PageError::new(
                    "A parent directory is not available at this commit.",
                    Some(format!("`{ancestor}` is not a directory at this commit.")),
                    false,
                );
                self.prepared = None;
                self.reader_status = ReaderStatus::ContentFailed(error);
                return;
            }
            self.load_directory(ancestor);
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
            self.load_directory(path);
            self.request_readme_content();
            return;
        }
        match branch.tree.locate_file(path) {
            Some((tree, index)) => match BlobIdentity::from_file(tree, index) {
                Ok(identity) => {
                    if self.pending_content.as_ref() == Some(&identity) {
                        return;
                    }
                    self.reader_status = ReaderStatus::Loading;
                    let commit = branch.commit().clone();
                    let branch_name = branch.name.clone();
                    let mode = self.current_read_mode();
                    self.pending_content = Some(identity.clone());
                    self.spawn_content(
                        branch_name,
                        commit,
                        Role::File(path.to_string()),
                        identity,
                        mode,
                    );
                }
                Err(error) => {
                    self.prepared = None;
                    self.reader_status = ReaderStatus::SelectionUnavailable {
                        reason: PageError::from_repository(&error).title,
                    };
                }
            },
            None => {
                self.prepared = None;
                self.reader_status = ReaderStatus::SelectionUnavailable {
                    reason: format!(
                        "`{path}` is not an ordinary file at this commit. Symbolic links, submodules and unknown Git types are not opened as text."
                    ),
                };
            }
        }
    }

    fn request_readme_content(&mut self) {
        let Some(branch) = self.branch.as_mut() else {
            return;
        };
        match branch.readme.clone() {
            ReadmeState::Found(descriptor) => {
                let path = descriptor.path.as_str().to_string();
                if self.reader_status == ReaderStatus::Showing
                    && self.selection == Selection::Readme
                    && self.readme_path.as_deref() == Some(path.as_str())
                {
                    return;
                }
                let identity = BlobIdentity::from_readme(&descriptor);
                let branch_name = branch.name.clone();
                let commit = branch.commit().clone();
                self.readme_path = Some(path.clone());
                for ancestor in RepositoryNavigation::ancestors(&path) {
                    self.intent.expanded.insert(ancestor);
                }
                self.invalidate_rows();
                self.load_expanded();
                if self.pending_content.as_ref() == Some(&identity) {
                    return;
                }
                let mode = self.current_read_mode();
                self.pending_content = Some(identity.clone());
                self.reader_status = ReaderStatus::Loading;
                self.spawn_content(branch_name, commit, Role::Readme, identity, mode);
            }
            ReadmeState::Absent => {
                self.prepared = None;
                self.readme_path = None;
                self.reader_status = ReaderStatus::NoReadme;
            }
            ReadmeState::Uncertain => {
                self.prepared = None;
                self.readme_path = None;
                self.reader_status = ReaderStatus::ReadmeUncertain;
            }
            ReadmeState::Failed(error) => {
                self.prepared = None;
                self.readme_path = None;
                self.reader_status = ReaderStatus::ReadmeUnavailable(error);
            }
            ReadmeState::Unknown => {
                self.prepared = None;
                self.readme_path = None;
                self.reader_status = ReaderStatus::Loading;
            }
        }
    }

    /// Builds the README picture provider for one document generation.
    fn image_provider_for(&self, content: &PreparedContent) -> Option<Arc<dyn ImageProvider>> {
        let details = self.details.as_ref()?;
        let identity = details.identity.clone();
        if !content
            .identity
            .repository()
            .locator()
            .owner()
            .eq_ignore_ascii_case(identity.locator().owner())
            || !content
                .identity
                .repository()
                .locator()
                .name()
                .eq_ignore_ascii_case(identity.locator().name())
        {
            return None;
        }
        let session = self.session.clone()?;
        let external = match &self.external_images {
            Some(external) => external.clone(),
            None => match ReqwestExternalImages::new() {
                Ok(client) => Arc::new(client) as Arc<dyn ExternalImageBytes>,
                Err(_) => return None,
            },
        };
        let cache = self.cache_binding.as_ref().map(|binding| {
            (
                binding.store().clone(),
                binding.scope().clone(),
                details.visibility == RepoVisibility::Private,
            )
        });
        let executor = self.image_executor.clone()?;
        let service = RepositoryImageService::with_cache(
            session,
            identity,
            executor,
            external,
            self.image_limits.clone(),
            cache,
        );
        Some(Arc::new(service) as Arc<dyn ImageProvider>)
    }

    fn current_read_mode(&self) -> RepositoryCacheMode {
        if self
            .branch
            .as_ref()
            .and_then(|branch| branch.snapshot_provenance.as_ref())
            .is_some_and(|prov| prov.origin == RepositoryCacheOrigin::Snapshot)
            || self
                .details_provenance
                .as_ref()
                .is_some_and(|prov| prov.origin == RepositoryCacheOrigin::Snapshot)
        {
            RepositoryCacheMode::OfflineOnly
        } else {
            RepositoryCacheMode::OnlinePreferred
        }
    }

    /// Whether a fully valid repository binding exists for Star and the overview.
    fn repository_binding(&self) -> Option<crate::ui::repository_overview::OverviewBinding> {
        let binding = extras::current_binding(self)?;
        extras::accepts(self, &binding).then_some(binding)
    }

    /// The presentation snapshot the NSH bindings read.
    pub fn view_model(&mut self) -> ViewModel {
        let rows = self.refresh_rows();
        let details = self.details.clone();
        let cached_label = self.historical_verified_at_ms().map(format_sync_age);
        let star = StarPresentation {
            label: details
                .as_ref()
                .map(|_| {
                    self.star
                        .observed
                        .map(StarredState::label)
                        .unwrap_or("Star")
                })
                .unwrap_or("Star"),
            observed: self.star.observed,
            busy: self.star_is_busy(),
            failure: self.star_phase_failure().map(str::to_string),
            enabled: self.repository_binding().is_some()
                && self.star.observed.is_some()
                && !self.star_is_busy()
                && !self.is_showing_cached_snapshot(),
            count: details.as_ref().map(|details| details.stars),
            retry: self.star_phase_failure().is_some(),
        };
        let overview = self.extras.overview().cloned();
        ViewModel {
            owner: self
                .locator
                .as_ref()
                .map(|locator| locator.owner().to_string())
                .unwrap_or_default(),
            name: self
                .locator
                .as_ref()
                .map(|locator| locator.name().to_string())
                .unwrap_or_default(),
            details,
            details_provenance: self.details_provenance.clone(),
            branches: self.branches.clone(),
            branches_error: self.branches_error.clone(),
            branches_loading: self.branches_loading,
            branch: self.selected_branch().map(str::to_string),
            commit: self
                .fixed_commit()
                .map(|commit| commit.as_str().to_string()),
            rows,
            rows_capped: self.rows_capped,
            tree_incomplete: self.tree("").is_some_and(navigation::tree_incomplete),
            root_failure: self.tree_failure("").cloned(),
            empty_root: self
                .tree("")
                .is_some_and(|tree| tree.entries.is_empty() && !navigation::tree_incomplete(tree)),
            selection: self.selection.clone(),
            readme_path: self.readme_path.clone(),
            can_return_to_readme: self.can_return_to_readme(),
            reader_status: self.reader_status.clone(),
            page_error: self.error.clone(),
            tab: self.tab,
            cursor: self.cursor.clone(),
            cached_label,
            star,
            overview,
            counts: self.extras.counts.clone(),
            counts_historical: self.extras.counts_historical,
            active: self.active,
        }
    }

    /// Whether a README is available and a different file is currently shown.
    pub fn can_return_to_readme(&self) -> bool {
        let Selection::File(path) = &self.selection else {
            return false;
        };
        self.branch
            .as_ref()
            .and_then(|branch| branch.readme.path())
            .is_some_and(|readme| readme != path.as_str())
    }

    // ---- star ----

    fn star_binding(&self) -> Option<crate::ui::repository_overview::OverviewBinding> {
        let repository = self.details.as_ref()?.identity.clone();
        let account = self.context_data.as_ref()?.auth().account.clone();
        let binding = crate::ui::repository_overview::OverviewBinding::new(
            account,
            repository,
            self.generation,
        );
        (self.active && extras::accepts(self, &binding)).then_some(binding)
    }

    fn star_service(&mut self) -> Option<RepositoryActionService> {
        let session = self.session.clone()?;
        let service =
            RepositoryActionService::new(&session, self.context.services().accounts()).ok()?;
        if Some(service.context()) == self.context_data.as_ref() && service.verify() {
            Some(service)
        } else {
            None
        }
    }

    fn start_star_read(&mut self) {
        if self.star_work.is_some() {
            return;
        }
        // A repository summary loaded from the offline cache means the network
        // was unreachable; do not send a follow-up Star status request until the
        // page is refreshed online.
        if self
            .details_provenance
            .as_ref()
            .is_some_and(|prov| prov.origin == RepositoryCacheOrigin::Snapshot)
        {
            self.star.observed = None;
            self.star.phase = StarPhase::Idle;
            return;
        }
        let Some(binding) = self.star_binding() else {
            return;
        };
        let Some(service) = self.star_service() else {
            return;
        };
        self.star.phase = StarPhase::Loading;
        self.submit_star(binding, service, StarSubmission::Observe);
    }

    fn submit_star(
        &mut self,
        binding: crate::ui::repository_overview::OverviewBinding,
        service: RepositoryActionService,
        submission: StarSubmission,
    ) {
        self.star_request = self.star_request.wrapping_add(1);
        let request = self.star_request;
        let work = Work::new();
        let cancel = work.token();
        let context = service.context().clone();
        let inbox = self.star_inbox.clone();
        let alive = self.alive.clone();
        let repository = binding.repository.clone();
        let limits = self.limits.repository.clone();
        self.star_work = Some(StarWork {
            service: service.clone(),
            work,
            binding: binding.clone(),
            request,
        });
        let spawned = self.context.services().spawn(async move {
            let result = match submission {
                StarSubmission::Observe => {
                    service.refresh_starred(&repository, &limits, cancel).await
                }
                StarSubmission::Write(desired) => {
                    service
                        .set_starred(&repository, desired, &limits, cancel)
                        .await
                }
            };
            let body = StarReplyBody::Changed(Box::new(result.map(|scoped| scoped.value)));
            if alive.load(Ordering::Acquire) {
                inbox.lock().expect("star inbox").push_back(StarReply {
                    binding,
                    context,
                    request,
                    body,
                });
            }
        });
        if spawned.is_err() {
            self.star_work = None;
            self.star.phase = StarPhase::Failed {
                message: "Background services are closing; the Star action was not submitted."
                    .into(),
            };
        }
    }

    fn star_accepts(
        &self,
        binding: &crate::ui::repository_overview::OverviewBinding,
        request: u64,
    ) -> bool {
        self.active
            && self.star_request == request
            && self.star_work.as_ref().is_some_and(|work| {
                work.request == request && work.binding == *binding && work.service.verify()
            })
            && self.star_binding().as_ref() == Some(binding)
    }

    fn drain_star(&mut self, sessions: &Sessions) {
        let replies: Vec<StarReply> = self
            .star_inbox
            .lock()
            .expect("star inbox")
            .drain(..)
            .collect();
        for reply in replies {
            if !self.star_accepts(&reply.binding, reply.request)
                || self.context_data.as_ref() != Some(&reply.context)
                || !self
                    .context
                    .services()
                    .repositories()
                    .is_current(&reply.context)
                || !self.context.is_current(sessions)
            {
                continue;
            }
            self.star_work = None;
            match reply.body {
                StarReplyBody::Changed(result) if result.is_ok() => {
                    let outcome = result.expect("checked Ok");
                    self.star.observed = Some(outcome.observation.state);
                    match outcome.summary {
                        Ok(details) => {
                            self.details = Some(details);
                            self.star.phase = StarPhase::Ready;
                        }
                        Err(error) => {
                            self.star.phase = StarPhase::Failed {
                                message: format!(
                                    "The starred state was observed, but the repository summary could not be refreshed: {error}"
                                ),
                            };
                        }
                    }
                }
                StarReplyBody::Changed(result) => {
                    self.apply_star_error(result.expect_err("Ok handled above"));
                }
            }
        }
    }

    fn apply_star_error(&mut self, error: RepositoryActionError) {
        if matches!(
            error,
            RepositoryActionError::GitHub(crate::domain::GitHubError::StaleContext)
        ) {
            return;
        }
        // Keep the last actual server observation; never flip it optimistically.
        self.star.phase = StarPhase::Failed {
            message: error.to_string(),
        };
    }
}

/// The presentation snapshot the NSH bindings read. Built from the controller after every drain
/// and user action; it owns no services and performs no I/O.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewModel {
    pub owner: String,
    pub name: String,
    pub details: Option<RepositoryDetails>,
    pub details_provenance: Option<RepositoryCacheProvenance>,
    pub branches: Option<BranchesOutcome>,
    pub branches_error: Option<PageError>,
    pub branches_loading: bool,
    pub branch: Option<String>,
    pub commit: Option<String>,
    pub rows: Rc<Vec<TreeRow>>,
    pub rows_capped: bool,
    pub tree_incomplete: bool,
    pub root_failure: Option<PageError>,
    pub empty_root: bool,
    pub selection: Selection,
    pub readme_path: Option<String>,
    pub can_return_to_readme: bool,
    pub reader_status: ReaderStatus,
    pub page_error: Option<PageError>,
    pub tab: RepositoryTab,
    pub cursor: Option<String>,
    pub cached_label: Option<String>,
    pub star: StarPresentation,
    pub overview: Option<crate::ui::repository_overview::RepositoryOverview>,
    pub counts: Option<crate::domain::repository_counts::RepositoryCounts>,
    pub counts_historical: bool,
    pub active: bool,
}

/// One account-scoped cache handle handed to a background job so the locator
/// index can be read and refreshed off the UI thread.
#[derive(Clone)]
struct CacheHint {
    store: CacheStore,
    scope: CacheScope,
}

async fn read_identity(
    hint: &CacheHint,
    locator: &RepositoryLocator,
) -> Option<RepositoryIdentity> {
    let store = hint.store.clone();
    let scope = hint.scope.clone();
    let locator = locator.clone();
    tokio::task::spawn_blocking(move || locator_index::read(&store, &scope, &locator))
        .await
        .ok()
        .flatten()
}

async fn write_identity(hint: CacheHint, identity: RepositoryIdentity) {
    let CacheHint { store, scope } = hint;
    let _ =
        tokio::task::spawn_blocking(move || locator_index::write(&store, &scope, &identity)).await;
}

/// A short, human-readable "synced N ago" label for an explicit historical
/// snapshot. This never presents cached data as freshly verified.
pub(crate) fn format_sync_age(verified_at_ms: i64) -> String {
    let now = crate::data::reader_image_cache::now_ms();
    let seconds = ((now - verified_at_ms).max(0) / 1000) as u64;
    match seconds {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{} min ago", seconds / 60),
        3600..=86_399 => format!("{} h ago", seconds / 3600),
        _ => format!("{} d ago", seconds / 86_400),
    }
}

enum Job {
    Repository {
        locator: RepositoryLocator,
        star_write_generation: u64,
        mode: RepositoryCacheMode,
        hint: Option<CacheHint>,
    },
    Branches {
        identity: RepositoryIdentity,
        mode: RepositoryCacheMode,
    },
    Revision {
        branch: String,
        request: u64,
        identity: RepositoryIdentity,
        mode: RepositoryCacheMode,
    },
    Root {
        branch: String,
        snapshot: RepoSnapshot,
        mode: RepositoryCacheMode,
    },
    Readme {
        branch: String,
        snapshot: RepoSnapshot,
        mode: RepositoryCacheMode,
    },
    Directory {
        branch: String,
        commit: ObjectSha,
        path: String,
        handle: SnapshotDirectory,
        mode: RepositoryCacheMode,
    },
    Content {
        branch: String,
        commit: ObjectSha,
        role: Role,
        selection_request: u64,
        identity: BlobIdentity,
        mode: RepositoryCacheMode,
    },
}

/// Runs exactly one service call. Sequencing stays in the foreground so every
/// result is checked before the next step is requested.
async fn run_job(
    session: Option<&CachedRepositorySession>,
    plain: &RepositorySession,
    job: Job,
    limits: &PageLimits,
    cancel: CancellationToken,
) -> Body {
    match job {
        Job::Repository {
            locator,
            star_write_generation,
            mode,
            hint,
        } => {
            let known = match &hint {
                Some(hint) => read_identity(hint, &locator).await,
                None => None,
            };
            let result = match (session, known) {
                (Some(cached), Some(identity)) => {
                    match cached
                        .repository(&identity, &limits.repository, mode, cancel.clone())
                        .await
                    {
                        Ok(scoped) => Ok((scoped.value.value, Some(scoped.value.provenance))),
                        Err(RepositoryCacheError::Online(RepositoryError::InvalidResponse {
                            ..
                        })) if mode != RepositoryCacheMode::OfflineOnly => plain
                            .repository(locator.clone(), &limits.repository, cancel)
                            .await
                            .map(|Scoped { value, .. }| (value, None))
                            .map_err(|error| PageError::from_repository(&error)),
                        Err(error) => Err(PageError::from(error)),
                    }
                }
                (Some(cached), None) if mode != RepositoryCacheMode::OfflineOnly => match plain
                    .repository(locator.clone(), &limits.repository, cancel.clone())
                    .await
                {
                    Ok(Scoped { value, .. }) => {
                        let identity = value.identity.clone();
                        match cached
                            .repository(
                                &identity,
                                &limits.repository,
                                RepositoryCacheMode::OnlinePreferred,
                                cancel,
                            )
                            .await
                        {
                            Ok(scoped) => Ok((scoped.value.value, Some(scoped.value.provenance))),
                            Err(_) => Ok((value, None)),
                        }
                    }
                    Err(error) => Err(PageError::from_repository(&error)),
                },
                _ => plain
                    .repository(locator.clone(), &limits.repository, cancel)
                    .await
                    .map(|Scoped { value, .. }| (value, None))
                    .map_err(|error| PageError::from_repository(&error)),
            };
            if let (Some(hint), Ok((details, _))) = (hint, &result) {
                write_identity(hint.clone(), details.identity.clone()).await;
            }
            Body::Repository {
                result,
                star_write_generation,
            }
        }
        Job::Branches { identity, mode } => Body::Branches(match session {
            Some(cached) => cached
                .branches(&identity, &limits.repository, mode, cancel)
                .await
                .map(|Scoped { value, .. }| (value.value, Some(value.provenance)))
                .map_err(PageError::from),
            None => plain
                .branches(&identity, &limits.repository, cancel)
                .await
                .map(|Scoped { value, .. }| (value, None))
                .map_err(|error| PageError::from_repository(&error)),
        }),
        Job::Revision {
            branch,
            request,
            identity,
            mode,
        } => {
            let spec = RevisionSpec::Ref(DynamicRef::new(branch.clone()).expect("validated ref"));
            let result = match session {
                Some(cached) => cached
                    .revision(&identity, spec, &limits.repository, mode, cancel)
                    .await
                    .map(|Scoped { value, .. }| (value.value, Some(value.provenance)))
                    .map_err(PageError::from),
                None => plain
                    .revision(&identity, spec, &limits.repository, cancel)
                    .await
                    .map(|Scoped { value, .. }| (value, None))
                    .map_err(|error| PageError::from_repository(&error)),
            };
            Body::Revision {
                branch,
                request,
                result,
            }
        }
        Job::Root {
            branch,
            snapshot,
            mode,
        } => {
            let commit = snapshot.commit().clone();
            let root = SnapshotDirectory::root(&snapshot);
            let result = match session {
                Some(cached) => cached
                    .directory(&root, &limits.repository, mode, cancel)
                    .await
                    .map(|Scoped { value, .. }| value.value)
                    .map_err(PageError::from),
                None => plain
                    .directory(&root, &limits.repository, cancel)
                    .await
                    .map(|Scoped { value, .. }| value)
                    .map_err(|error| PageError::from_repository(&error)),
            };
            Body::Root {
                branch,
                commit,
                result,
            }
        }
        Job::Readme {
            branch,
            snapshot,
            mode,
        } => {
            let commit = snapshot.commit().clone();
            let result = match session {
                Some(cached) => cached
                    .readme(&snapshot, &limits.repository, mode, cancel)
                    .await
                    .map(|Scoped { value, .. }| ReadmeState::from_discovery(Ok(value.value)))
                    .map_err(PageError::from),
                None => plain
                    .readme(&snapshot, &limits.repository, cancel)
                    .await
                    .map(|Scoped { value, .. }| ReadmeState::from_discovery(Ok(value)))
                    .map_err(|error| PageError::from_repository(&error)),
            };
            Body::Readme {
                branch,
                commit,
                result,
            }
        }
        Job::Directory {
            branch,
            commit,
            path,
            handle,
            mode,
        } => {
            let result = match session {
                Some(cached) => cached
                    .directory(&handle, &limits.repository, mode, cancel)
                    .await
                    .map(|Scoped { value, .. }| value.value)
                    .map_err(PageError::from),
                None => plain
                    .directory(&handle, &limits.repository, cancel)
                    .await
                    .map(|Scoped { value, .. }| value)
                    .map_err(|error| PageError::from_repository(&error)),
            };
            Body::Directory {
                branch,
                commit,
                path,
                result,
            }
        }
        Job::Content {
            branch,
            commit,
            role,
            selection_request,
            identity,
            mode,
        } => {
            let path = identity.path().as_str().to_string();
            let result = match session {
                Some(cached) => cached
                    .content(&identity, &limits.blob, mode, cancel)
                    .await
                    .map(|Scoped { value, .. }| value.value)
                    .map_err(PageError::from),
                None => plain
                    .content(&identity, &limits.blob, cancel)
                    .await
                    .map(|Scoped { value, .. }| value)
                    .map_err(|error| PageError::from_repository(&error)),
            };
            Body::Content(ContentReply {
                branch,
                commit,
                role,
                selection_request,
                path,
                result,
            })
        }
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

#[derive(Clone, Copy)]
enum SlotKind {
    Summary,
    Branches,
    Revision,
    Content,
}

fn background_closing() -> PageError {
    PageError::new(
        "Background services are closing.",
        Some("The read was not submitted because the network executor is shutting down.".into()),
        true,
    )
}

// ---- the NSH page ----

/// The Native Shell page wrapper: owns the controller on the UI thread, the reactive presentation
/// snapshot and the reader handle, and forwards typed actions to the shell.
pub struct RepositoryPage {
    inner: Rc<PageRuntime>,
}

impl nsh_core::element::JsValue for PageHandle {
    // The page handle is a Rust-only component property: no JS view creates or receives it.
    fn ts_type(_: &mut nsh_core::element::TsTypes) -> String {
        "unknown".to_string()
    }

    fn from_js(value: nsh_core::element::JsIn<'_>) -> Result<Self, nsh_core::element::PropError> {
        Err(value.expected("a repository page handle"))
    }

    fn to_js<'a>(
        &self,
        host: &'a dyn nsh_core::element::JsHost,
    ) -> Result<nsh_core::element::JsOwned<'a>, nsh_core::element::PropError> {
        Ok(nsh_core::element::JsOwned::new(host, host.null()))
    }
}

/// A cheap, comparable handle the view component receives. It compares by identity so a shell
/// re-render reuses the mounted component instead of replacing it.
#[derive(Clone, Default)]
pub struct PageHandle(Option<Rc<PageRuntime>>);

impl PartialEq for PageHandle {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Some(a), Some(b)) => Rc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }
}

struct PageRuntime {
    controller: RefCell<RepositoryController>,
    state: Signal<ViewModel>,
    reader: Signal<Option<ReaderHandle>>,
    /// The document and picture provider of the current content generation, kept so an in-page tab
    /// switch can remount the same authorized document without another read.
    document: RefCell<Option<PageDocument>>,
    navigation: NavigationSink,
    retired: Cell<bool>,
    /// A weak self handle, so the reader's action callback never keeps the page alive.
    weak: RefCell<Weak<PageRuntime>>,
}

#[derive(Clone)]
struct PageDocument {
    document: Arc<ReaderDocument>,
    provider: Option<Arc<dyn ImageProvider>>,
}

impl PageRuntime {
    /// Mirrors the controller into the presentation signal and mounts a replaced reader.
    fn sync(&self) {
        if self.retired.get() {
            return;
        }
        let mut actions = Vec::new();
        let (model, prepared) = {
            let mut controller = self.controller.borrow_mut();
            while let Some(action) = controller.take_action() {
                actions.push(action);
            }
            let prepared = controller
                .take_prepared_reader()
                .map(|(document, provider)| PageDocument { document, provider });
            (controller.view_model(), prepared)
        };
        for action in actions {
            self.emit(action);
        }
        if let Some(prepared) = prepared {
            *self.document.borrow_mut() = Some(prepared.clone());
            if model.tab == RepositoryTab::Code && model.reader_status == ReaderStatus::Showing {
                self.mount_reader(prepared);
            }
        }
        let code = model.tab == RepositoryTab::Code;
        if (!code || model.reader_status != ReaderStatus::Showing)
            && self.reader.get_untracked().is_some()
        {
            self.reader.set(None);
        }
        if model.reader_status != ReaderStatus::Showing {
            *self.document.borrow_mut() = None;
        }
        if code
            && model.reader_status == ReaderStatus::Showing
            && self.reader.get_untracked().is_none()
            && let Some(document) = self.document.borrow().clone()
        {
            self.mount_reader(document);
        }
        self.state.set(model);
    }

    /// Creates a fresh authorization bridge for `document` and mounts it. A previous handle is
    /// replaced; its element's cleanup retires its own bridge.
    fn mount_reader(&self, document: PageDocument) {
        let (guard, limits) = {
            let controller = self.controller.borrow();
            (controller.reader_guard(), controller.image_limits())
        };
        let bridge = ImageBridge::new(document.provider, guard, limits);
        let weak = self.weak.borrow().clone();
        let handle = ReaderHandle::new(
            document.document,
            bridge,
            ReaderLayout::Repository,
            move |action| {
                if let Some(runtime) = weak.upgrade() {
                    runtime.reader_action(action);
                }
            },
        );
        self.reader.set(Some(handle));
    }

    fn reader_action(&self, action: ReaderAction) {
        {
            let mut controller = self.controller.borrow_mut();
            controller.reader_action(action);
        }
        self.sync();
    }

    fn emit(&self, action: PageAction) {
        match action {
            PageAction::OpenExternal(url) => (self.navigation)(PageNavigation::External(url)),
            PageAction::Overview(
                crate::ui::repository_overview::OverviewAction::OpenContributor {
                    login: Some(login),
                    ..
                },
            ) => (self.navigation)(PageNavigation::CanonicalProfile {
                tab: self.controller.borrow().identity().tab,
                login,
            }),
            PageAction::Overview(crate::ui::repository_overview::OverviewAction::OpenRelease {
                html_url: Some(url),
                ..
            }) => (self.navigation)(PageNavigation::External(url)),
            PageAction::Overview(_)
            | PageAction::LinkNotFollowed { .. }
            | PageAction::StarIntent { .. } => {}
        }
    }

    fn user(self: &Rc<Self>, f: impl FnOnce(&mut RepositoryController)) {
        if self.retired.get() {
            return;
        }
        {
            let mut controller = self.controller.borrow_mut();
            f(&mut controller);
        }
        self.sync();
    }
}

impl RepositoryPage {
    /// Creates the page and starts its first authorized read.
    pub fn new(
        context: PageContext,
        owner: String,
        name: String,
        intent: WorkspaceNavigation,
        navigation: NavigationSink,
    ) -> Self {
        let mut controller = RepositoryController::new(context, &owner, &name, intent);
        let inner = Rc::new_cyclic(|weak| PageRuntime {
            state: signal(controller.view_model()),
            reader: signal_with(None, SignalEquals::Never),
            controller: RefCell::new(controller),
            document: RefCell::new(None),
            navigation,
            retired: Cell::new(false),
            weak: RefCell::new(weak.clone()),
        });
        PageRuntime::sync(&inner);
        Self { inner }
    }

    /// Passes the shell's single background executor to the page's README image provider.
    pub fn set_image_executor(
        &mut self,
        executor: Arc<crate::network::executor::BackgroundExecutor>,
    ) {
        self.inner.controller.borrow_mut().image_executor = Some(executor);
    }

    /// The controller, for in-process tests.
    pub fn controller(&self) -> std::cell::Ref<'_, RepositoryController> {
        self.inner.controller.borrow()
    }

    /// The controller, mutable, for in-process tests.
    pub fn controller_mut(&self) -> std::cell::RefMut<'_, RepositoryController> {
        self.inner.controller.borrow_mut()
    }

    /// The current presentation snapshot.
    pub fn model(&self) -> ViewModel {
        self.inner.state.get_untracked()
    }

    /// Re-syncs the presentation after an out-of-band controller mutation (tests, previews).
    pub fn sync(&self) {
        self.inner.sync();
    }

    /// The reader handle currently mounted, if any.
    pub fn reader_handle(&self) -> Option<ReaderHandle> {
        self.inner.reader.get_untracked()
    }
}

impl PageView for RepositoryPage {
    fn element(&self) -> AnyElement {
        view::repository_view()
            .page(PageHandle(Some(self.inner.clone())))
            .into_element()
    }

    fn poll(&mut self, runtime: &RuntimeHandle) {
        {
            let mut controller = self.inner.controller.borrow_mut();
            controller.poll(runtime.sessions());
        }
        self.inner.sync();
        if let Some(handle) = self.inner.reader.get_untracked() {
            handle.poll();
        }
    }

    fn refresh(&mut self, _runtime: &RuntimeHandle) {
        self.inner.user(|controller| controller.refresh());
    }

    fn set_active(&mut self, active: bool) {
        self.inner.user(|controller| controller.set_active(active));
    }

    fn retire(&mut self) {
        if self.inner.retired.replace(true) {
            return;
        }
        if let Some(handle) = self.inner.reader.get_untracked() {
            handle.retire();
            self.inner.reader.set(None);
        }
        *self.inner.document.borrow_mut() = None;
        let mut controller = self.inner.controller.borrow_mut();
        controller.retire();
    }
}
