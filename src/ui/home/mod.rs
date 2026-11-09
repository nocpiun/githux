//! Native Shell Home / Feed presentation over the existing domain read models.
//!
//! The host owns account-scoped reads and must verify authorization before it
//! supplies an outcome. `RequestBinding` and generation protect this view from
//! stale UI responses; they are not an authorization check. No network,
//! credentials, cache, routing or URL opening lives in this module.
//!
//! State lives in NSH signals created once by [`HomeView::new`] inside the host
//! page's reactive root. [`HomeView::element`] only reads those signals and
//! never performs I/O: the host publishes through the setters below from its own
//! drain, and user intent leaves through the `OnAction` sink after the render
//! pass has finished.

mod panel;
mod presentation;
mod rows;
#[cfg(test)]
mod tests;

use crate::ui::foundation::{controls, theme as t};
use nsh_core::element::ImageSrc;
use nsh_core::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::SystemTime;

use crate::domain::home::{FeedOutcome, HomeError, HomeItem, HomeSearchOutcome};

use presentation::home_summary;
use rows::{feed_row, home_row};

/// An opaque, host-selected request scope. Include the stable account/page
/// identity; do not put a PAT or other credential in this value.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RequestBinding(String);

impl RequestBinding {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The two independently refreshed panels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Column {
    Home,
    Feed,
}

/// A manual refresh request. It carries the binding/generation observed by the
/// view so the host can reject an action belonging to an obsolete page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefreshRequest {
    pub column: Column,
    pub binding: RequestBinding,
    pub generation: u64,
}

/// Provenance of a successful read. Historical values retain their original
/// timestamps; they are not promoted to current network results on display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageProvenance {
    pub fetched_at_ms: i64,
    pub verified_at_ms: i64,
}

impl PageProvenance {
    pub fn new(fetched_at_ms: i64, verified_at_ms: i64) -> Self {
        Self {
            fetched_at_ms,
            verified_at_ms,
        }
    }

    pub fn from_metadata(metadata: &crate::domain::cache::CacheMetadata) -> Self {
        Self::new(metadata.fetched_at_ms, metadata.verified_at_ms)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotPresentation<T> {
    pub outcome: T,
    pub provenance: PageProvenance,
}

impl<T> SnapshotPresentation<T> {
    pub fn historical(outcome: T, metadata: &crate::domain::cache::CacheMetadata) -> Self {
        Self {
            outcome,
            provenance: PageProvenance::from_metadata(metadata),
        }
    }

    pub fn historical_at(outcome: T, fetched_at_ms: i64) -> Self {
        Self {
            outcome,
            provenance: PageProvenance::new(fetched_at_ms, fetched_at_ms),
        }
    }
}

/// Stable repository identity used for navigation callbacks.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RepositoryIdentity {
    pub id: Option<u64>,
    pub owner: String,
    pub name: String,
}

impl RepositoryIdentity {
    pub fn full_name(&self) -> String {
        if self.owner.is_empty() {
            self.name.clone()
        } else {
            format!("{}/{}", self.owner, self.name)
        }
    }
}

/// Stable Issue/PR identity; `id` is GitHub's global issue/PR id, not the
/// repository-local `number`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkItemIdentity {
    pub id: u64,
    pub kind: crate::domain::home::HomeItemKind,
    pub number: u64,
    pub repository: RepositoryIdentity,
}

/// Stable profile identity. Actor ids remain real; repository-owner targets
/// carry only their canonical login (`id: None`) when the model has no id.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProfileIdentity {
    pub id: Option<u64>,
    pub login: String,
}

/// Stable Feed identity. No raw event payload or URL is exposed as an action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedIdentity {
    pub id: String,
    pub event_type: crate::domain::home::FeedEventType,
    pub repository: RepositoryIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HomeAction {
    Refresh(RefreshRequest),
    Repository(RepositoryIdentity),
    Profile(ProfileIdentity),
    ActivateWorkItem(WorkItemIdentity),
    ActivateFeed(FeedIdentity),
    /// The Feed column reached its end and asks the host for the next older
    /// source page. Carries the binding/generation the view observed, so the host
    /// can reject a stale request after an account switch or refresh.
    LoadMoreFeed(RefreshRequest),
    /// Open the addressed pull request in the current account's Githux workspace.
    OpenPullRequest {
        repository: RepositoryIdentity,
        number: u64,
    },
}

/// User intent leaves the view through this sink. NSH has no `App` borrow, so
/// the host keeps the emitted actions in its own UI-thread queue and runs them
/// from `PageView::poll`; nothing re-enters a page mid-render.
pub type OnAction = Rc<dyn Fn(HomeAction)>;
/// Host presentation hook. The host supplies the stable identity of the credited
/// activity subject (the pull-request author for a merge, the event actor
/// otherwise) and receives an already-resolved image. No image request, decode
/// or cache belongs to this module.
pub type AvatarImage = Rc<dyn Fn(&crate::domain::home::FeedActor) -> Option<ImageSrc>>;

/// Successful data, a hard failure, or a host-declared no-data state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelContent<T> {
    Ready(T),
    Error(HomeError),
    /// No current or previously supplied snapshot. This is deliberately not
    /// called "empty": offline/no-data must not become "no activity".
    Unavailable {
        message: String,
    },
}

/// State of one column. Refreshing retains previously supplied data while the
/// host works, but does not claim that data was freshly verified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelState<T> {
    Loading,
    Ready {
        content: PanelContent<T>,
        refreshing: bool,
    },
}

fn begin_refresh<T>(state: &mut PanelState<T>) {
    match state {
        PanelState::Loading => {}
        PanelState::Ready { refreshing, .. } => *refreshing = true,
    }
}

impl<T> PanelState<T> {
    fn is_busy(&self) -> bool {
        matches!(self, Self::Loading)
            || matches!(
                self,
                Self::Ready {
                    refreshing: true,
                    ..
                }
            )
    }

    fn data(&self) -> Option<&T> {
        match self {
            Self::Ready {
                content: PanelContent::Ready(data),
                ..
            } => Some(data),
            _ => None,
        }
    }

    pub fn is_loading(&self) -> bool {
        matches!(self, Self::Loading)
    }

    pub fn refreshing(&self) -> bool {
        matches!(
            self,
            Self::Ready {
                refreshing: true,
                ..
            }
        )
    }
}

/// Host-computed refresh availability. The view does not count down cooldowns
/// or schedule polling; the host supplies changes when its state changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefreshAvailability {
    pub available: bool,
    pub reason: Option<String>,
}

impl RefreshAvailability {
    pub fn available() -> Self {
        Self {
            available: true,
            reason: None,
        }
    }

    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            available: false,
            reason: Some(reason.into()),
        }
    }
}

impl Default for RefreshAvailability {
    fn default() -> Self {
        Self::available()
    }
}

/// One column's host-supplied state, refresh affordance and provenance. One
/// signal per column keeps the two panels from repainting each other.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PanelModel<T> {
    state: PanelState<T>,
    refresh: RefreshAvailability,
    /// `None` is a current/network observation; `Some` is an explicitly marked
    /// historical snapshot, including its original fetch/verification times.
    provenance: Option<PageProvenance>,
}

impl<T> PanelModel<T> {
    fn new() -> Self {
        Self {
            state: PanelState::Ready {
                content: PanelContent::Unavailable {
                    message: "The host has not supplied a result for this binding.".to_string(),
                },
                refreshing: false,
            },
            refresh: RefreshAvailability::default(),
            provenance: None,
        }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }
}

/// End-of-scroll append state for the Feed column.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum FeedAppendState {
    #[default]
    Idle,
    Loading,
    Failed {
        /// The cursor to retry. Preserved so a refresh cannot lose continuation.
        page: u32,
    },
    NotCached {
        page: u32,
    },
}

type Descriptions = HashMap<String, String>;

/// A reusable two-column Home/Feed view. Construct one per page while the
/// host's reactive root owner is current, and call [`HomeView::bind`] whenever
/// the account or page binding changes.
#[derive(Clone)]
pub struct HomeView {
    binding: Signal<RequestBinding>,
    generation: Signal<u64>,
    home: Signal<PanelModel<HomeSearchOutcome>>,
    feed: Signal<PanelModel<FeedOutcome>>,
    feed_append: Signal<FeedAppendState>,
    /// Enriched repository descriptions, keyed by lowercase "owner/name".
    descriptions: Signal<Descriptions>,
    /// Wall clock used by the shared relative-time formatter. The host updates
    /// it on its light tick; it is not part of any read.
    now: Signal<SystemTime>,
    /// Pointer-selected row per column. Selection is presentation state only;
    /// activation still goes through the host's navigation.
    selected_home: Signal<Option<usize>>,
    selected_feed: Signal<Option<usize>>,
    /// The Feed list, watched for near-end proximity. Actual list bounds are used
    /// rather than a scrollbar extent, which cannot see unmeasured rows (L015).
    feed_scroll: ElementRef,
    /// The action sink of the current mount, so the scroll watch (registered once)
    /// can ask the host for the next page.
    sink: Rc<RefCell<Option<OnAction>>>,
}

impl Default for HomeView {
    fn default() -> Self {
        Self::new()
    }
}

impl HomeView {
    /// Starts an unrequested binding (no current data). The host must call
    /// [`HomeView::set_loading`] and then supply each result with the same
    /// binding and generation.
    pub fn new() -> Self {
        let feed_scroll = element_ref();
        let sink: Rc<RefCell<Option<OnAction>>> = Rc::new(RefCell::new(None));
        let view = Self {
            binding: signal(RequestBinding::new("")),
            generation: signal(0),
            home: signal(PanelModel::new()),
            feed: signal(PanelModel::new()),
            feed_append: signal(FeedAppendState::Idle),
            descriptions: signal(Descriptions::new()),
            now: signal(SystemTime::now()),
            selected_home: signal(None),
            selected_feed: signal(None),
            feed_scroll,
            sink: sink.clone(),
        };
        // Registered once in the page's owner; the handler runs at the end of each
        // commit that sees a new offset, content size or viewport.
        {
            let this = view.clone();
            watch_scroll(
                move || feed_scroll.node(),
                move |event| {
                    this.on_feed_scrolled(event);
                },
            );
        }
        view
    }

    /// Near-end detection from the list's real bounds: the last viewport of
    /// content (or a first page that does not fill one) asks the host for the next
    /// older page. Requests stay idempotent while one is in flight.
    fn on_feed_scrolled(&self, event: &ScrollEvent) {
        if !self.feed_can_load_more() {
            return;
        }
        let any_action = self.sink.borrow().is_some();
        if !any_action {
            return;
        }
        let offset = event.offset.y.0;
        let content = event.content_size.height.0;
        let viewport = event.size.height.0;
        if viewport <= 0.0 {
            return;
        }
        let near_end = content <= viewport || offset + viewport >= content - viewport;
        if !near_end {
            return;
        }
        self.feed_append.set(FeedAppendState::Loading);
        if let Some(sink) = self.sink.borrow().as_ref() {
            sink(HomeAction::LoadMoreFeed(self.request(Column::Feed)));
        }
    }

    pub fn binding(&self) -> RequestBinding {
        self.binding.get_untracked()
    }

    pub fn generation(&self) -> u64 {
        self.generation.get_untracked()
    }

    /// Updates the wall clock behind relative timestamps. No read is started.
    pub fn set_now(&mut self, now: SystemTime) {
        self.now.set(now);
    }

    pub fn now(&self) -> SystemTime {
        self.now.get_untracked()
    }

    pub fn home_state(&self) -> PanelState<HomeSearchOutcome> {
        self.home.get_untracked().state
    }

    pub fn feed_state(&self) -> PanelState<FeedOutcome> {
        self.feed.get_untracked().state
    }

    pub fn feed_outcome(&self) -> Option<FeedOutcome> {
        self.feed.get_untracked().state.data().cloned()
    }

    pub fn feed_append_state(&self) -> FeedAppendState {
        self.feed_append.get_untracked()
    }

    pub fn provenance(&self, column: Column) -> Option<PageProvenance> {
        match column {
            Column::Home => self.home.get_untracked().provenance,
            Column::Feed => self.feed.get_untracked().provenance,
        }
    }

    pub fn refresh_availability(&self, column: Column) -> RefreshAvailability {
        match column {
            Column::Home => self.home.get_untracked().refresh,
            Column::Feed => self.feed.get_untracked().refresh,
        }
    }

    pub fn description(&self, repository: &str) -> Option<String> {
        self.descriptions
            .get_untracked()
            .get(&repository.trim().to_ascii_lowercase())
            .cloned()
    }

    /// Clears both columns and all host state before a new binding can receive
    /// input. Even A→B→A never revives an old response.
    pub fn bind(&mut self, binding: RequestBinding, generation: u64) {
        self.binding.set(binding);
        self.generation.set(generation);
        self.home.update(PanelModel::reset);
        self.feed.update(PanelModel::reset);
        self.feed_append.set(FeedAppendState::Idle);
        self.descriptions.set(Descriptions::new());
        self.selected_home.set(None);
        self.selected_feed.set(None);
    }

    fn accepts(&self, binding: &RequestBinding, generation: u64) -> bool {
        &self.binding.get_untracked() == binding && self.generation.get_untracked() == generation
    }

    /// A historical page whose next source page was never cached remains
    /// readable, but must not pretend that the timeline ended or offer a retry
    /// which can only repeat the same cache miss.
    pub fn feed_page_not_cached(
        &mut self,
        binding: &RequestBinding,
        generation: u64,
        page: u32,
    ) -> bool {
        if !self.accepts(binding, generation) {
            return false;
        }
        self.feed_append.set(FeedAppendState::NotCached { page });
        true
    }

    /// Enriches multiple repository descriptions at once, guarded by the active
    /// request binding and generation.
    pub fn set_repository_descriptions(
        &mut self,
        binding: &RequestBinding,
        generation: u64,
        descriptions: Descriptions,
    ) -> bool {
        if !self.accepts(binding, generation) {
            return false;
        }
        let mut merged = self.descriptions.get_untracked();
        for (key, description) in descriptions {
            let key = key.trim().to_ascii_lowercase();
            if !description.trim().is_empty() {
                merged.insert(key, description);
            }
        }
        self.descriptions.set(merged);
        true
    }

    /// A replacement Feed read starts a new description generation too.
    pub fn clear_repository_descriptions(&mut self) {
        self.descriptions.set(Descriptions::new());
    }

    /// Starts loading one column without disturbing the other. A host normally
    /// calls this before its first read; refreshing an existing snapshot can be
    /// left to the refresh action itself.
    pub fn set_loading(
        &mut self,
        column: Column,
        binding: &RequestBinding,
        generation: u64,
    ) -> bool {
        if !self.accepts(binding, generation) {
            return false;
        }
        match column {
            Column::Home => self.edit_home(|model| model.state = PanelState::Loading),
            Column::Feed => self.edit_feed(|model| model.state = PanelState::Loading),
        }
        true
    }

    /// Applies a typed result only if it belongs to the current binding and
    /// generation. Non-open entries are dropped here as well as in the adapter:
    /// a closed, merged or unknown state must never become a visible row.
    pub fn apply_home(
        &mut self,
        binding: &RequestBinding,
        generation: u64,
        outcome: HomeSearchOutcome,
    ) -> bool {
        self.apply_home_internal(binding, generation, outcome, None)
    }

    pub fn apply_home_snapshot(
        &mut self,
        binding: &RequestBinding,
        generation: u64,
        snapshot: SnapshotPresentation<HomeSearchOutcome>,
    ) -> bool {
        self.apply_home_internal(
            binding,
            generation,
            snapshot.outcome,
            Some(snapshot.provenance),
        )
    }

    fn apply_home_internal(
        &mut self,
        binding: &RequestBinding,
        generation: u64,
        mut outcome: HomeSearchOutcome,
        provenance: Option<PageProvenance>,
    ) -> bool {
        if !self.accepts(binding, generation) {
            return false;
        }
        outcome.items.retain(HomeItem::is_open);
        self.selected_home.set(None);
        let model = PanelModel {
            state: PanelState::Ready {
                content: PanelContent::Ready(outcome),
                refreshing: false,
            },
            refresh: self.home.get_untracked().refresh,
            provenance,
        };
        self.home.set(model);
        true
    }

    pub fn apply_feed(
        &mut self,
        binding: &RequestBinding,
        generation: u64,
        outcome: FeedOutcome,
    ) -> bool {
        self.apply_feed_internal(binding, generation, outcome, None)
    }

    pub fn apply_feed_snapshot(
        &mut self,
        binding: &RequestBinding,
        generation: u64,
        snapshot: SnapshotPresentation<FeedOutcome>,
    ) -> bool {
        self.apply_feed_internal(
            binding,
            generation,
            snapshot.outcome,
            Some(snapshot.provenance),
        )
    }

    fn apply_feed_internal(
        &mut self,
        binding: &RequestBinding,
        generation: u64,
        outcome: FeedOutcome,
        provenance: Option<PageProvenance>,
    ) -> bool {
        if !self.accepts(binding, generation) {
            return false;
        }
        // The host applies the product policy (retained activity kinds, resolved
        // contribution author, followed-account membership) before publishing, so
        // this view never re-derives membership from a display string.
        let model = PanelModel {
            state: PanelState::Ready {
                content: PanelContent::Ready(outcome),
                refreshing: false,
            },
            refresh: self.feed.get_untracked().refresh,
            provenance,
        };
        self.feed.set(model);
        self.feed_append.set(FeedAppendState::Idle);
        self.selected_feed.set(None);
        true
    }

    /// Appends one policy-applied page to the retained Feed without resetting
    /// scroll or selection. The caller passes a merged outcome (retained + new
    /// activities) so the list grows in place; the host reports append failure
    /// separately via [`Self::feed_append_failed`].
    pub fn apply_feed_page(
        &mut self,
        binding: &RequestBinding,
        generation: u64,
        outcome: FeedOutcome,
    ) -> bool {
        if !self.accepts(binding, generation) {
            return false;
        }
        let provenance = self.feed.get_untracked().provenance;
        self.feed.set(PanelModel {
            state: PanelState::Ready {
                content: PanelContent::Ready(outcome),
                refreshing: false,
            },
            refresh: self.feed.get_untracked().refresh,
            provenance,
        });
        self.feed_append.set(FeedAppendState::Idle);
        true
    }

    /// Marks an append request in flight so repeated end-of-scroll notifications
    /// cannot duplicate it. The host calls this when it begins a load-more.
    pub fn feed_append_loading(&mut self, binding: &RequestBinding, generation: u64) -> bool {
        if !self.accepts(binding, generation) {
            return false;
        }
        self.feed_append.set(FeedAppendState::Loading);
        true
    }

    /// Records a retryable append failure while keeping the visible list intact.
    pub fn feed_append_failed(
        &mut self,
        binding: &RequestBinding,
        generation: u64,
        page: u32,
    ) -> bool {
        if !self.accepts(binding, generation) {
            return false;
        }
        self.feed_append.set(FeedAppendState::Failed { page });
        true
    }

    /// Whether the host has more older pages to load and no append is in flight.
    pub fn feed_can_load_more(&self) -> bool {
        matches!(self.feed_append.get_untracked(), FeedAppendState::Idle)
            && self
                .feed
                .get_untracked()
                .state
                .data()
                .and_then(|outcome| outcome.next_page)
                .is_some()
    }

    pub fn apply_error(
        &mut self,
        column: Column,
        binding: &RequestBinding,
        generation: u64,
        error: HomeError,
    ) -> bool {
        if !self.accepts(binding, generation) {
            return false;
        }
        match column {
            Column::Home => self.edit_home(|model| {
                model.state = PanelState::Ready {
                    content: PanelContent::Error(error),
                    refreshing: false,
                }
            }),
            Column::Feed => self.edit_feed(|model| {
                model.state = PanelState::Ready {
                    content: PanelContent::Error(error),
                    refreshing: false,
                }
            }),
        }
        true
    }

    pub fn set_unavailable(
        &mut self,
        column: Column,
        binding: &RequestBinding,
        generation: u64,
        message: impl Into<String>,
    ) -> bool {
        if !self.accepts(binding, generation) {
            return false;
        }
        let message = message.into();
        match column {
            Column::Home => self.edit_home(|model| {
                model.state = PanelState::Ready {
                    content: PanelContent::Unavailable {
                        message: message.clone(),
                    },
                    refreshing: false,
                }
            }),
            Column::Feed => self.edit_feed(|model| {
                model.state = PanelState::Ready {
                    content: PanelContent::Unavailable {
                        message: message.clone(),
                    },
                    refreshing: false,
                }
            }),
        }
        true
    }

    /// Host-provided cooldown, in-flight, rate-limit or polling availability.
    pub fn set_refresh_availability(
        &mut self,
        column: Column,
        binding: &RequestBinding,
        generation: u64,
        availability: RefreshAvailability,
    ) -> bool {
        if !self.accepts(binding, generation) {
            return false;
        }
        match column {
            Column::Home => self.edit_home(|model| model.refresh = availability),
            Column::Feed => self.edit_feed(|model| model.refresh = availability),
        }
        true
    }

    /// Begins a manual refresh in the retained snapshot: the visible content
    /// stays, marked as refreshing, until the host publishes or fails it.
    pub fn begin_refresh(&mut self, column: Column) {
        match column {
            Column::Home => self.edit_home(|model| begin_refresh(&mut model.state)),
            Column::Feed => self.edit_feed(|model| begin_refresh(&mut model.state)),
        }
    }

    fn edit_home(&mut self, edit: impl FnOnce(&mut PanelModel<HomeSearchOutcome>)) {
        let mut model = self.home.get_untracked();
        edit(&mut model);
        self.home.set(model);
    }

    fn edit_feed(&mut self, edit: impl FnOnce(&mut PanelModel<FeedOutcome>)) {
        let mut model = self.feed.get_untracked();
        edit(&mut model);
        self.feed.set(model);
    }

    pub fn item_count(&self, column: Column) -> usize {
        match column {
            Column::Home => self
                .home
                .get_untracked()
                .state
                .data()
                .map_or(0, |outcome| outcome.items.len()),
            Column::Feed => self
                .feed
                .get_untracked()
                .state
                .data()
                .map_or(0, |outcome| outcome.activities.len()),
        }
    }

    /// Whether the Feed column can be refreshed right now (host availability and
    /// no in-flight request). Refresh itself stays a host decision.
    pub fn can_refresh(&self, column: Column) -> bool {
        let model_is_busy = match column {
            Column::Home => self.home.get_untracked().state.is_busy(),
            Column::Feed => self.feed.get_untracked().state.is_busy(),
        };
        !model_is_busy && self.refresh_availability(column).available
    }
}
