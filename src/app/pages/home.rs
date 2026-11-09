//! Production Home/Feed page host on Native Shell.
//!
//! The page owns one [`PageContext`] (account, workspace identity and cache
//! authorization captured at mount), one [`PageSlot`] per asynchronous read, and
//! the presentation in [`HomeView`]. Background work only touches the real
//! services and cancellation tokens; every result is published on the UI thread
//! from [`PageView::poll`] after the slot re-validates the captured ticket.
//!
//! Authorization is bound to the data itself: [`PageSlot::publish`] and
//! [`PageSlot::value`] check the captured `CacheAuthorization` and workspace
//! identity, so a revocation or account switch clears both in-flight replies and
//! already-displayed content in the same drain (LESSONS L022). Nothing in
//! `element()` performs I/O.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nsh_core::element::create_root;
use nsh_core::prelude::{AnyElement, Owner};

use crate::app::runtime::{PageContext, PageSlot, PageTicket, RuntimeHandle, Sessions};
use crate::data::page_cache::{
    CachedPageReadSession, PageCacheError, PageOrigin, PageReadMode, PageValue,
};
use crate::data::{PageReadSession, Scoped};
use crate::domain::cache::CacheMetadata;
use crate::domain::error::GitHubError;
use crate::domain::home::{FeedOutcome, FeedSourceCursor, HomeError, HomeSearchOutcome};
use crate::github::home::{FeedLimits, FollowingLimits, HomeSearchLimits, PullRequestDetailLimits};
use crate::storage::cache::CacheAuthorization;
use crate::ui::home::{
    AvatarImage, Column, HomeAction, HomeView, RefreshAvailability, RequestBinding,
    SnapshotPresentation,
};

use super::{NavigationSink, PageNavigation, PageView};
use crate::workspace::Route;

/// Upper bound on merged pull-request detail reads per Feed refresh. A 300-event
/// window with a handful of merges stays far below this; the cap exists so a
/// pathological page cannot turn into dozens of sequential requests. Targets over
/// the cap are reported as unresolved rather than silently dropped.
const FEED_AUTHOR_LOOKUP_LIMIT: usize = 30;

/// How often the page advances relative timestamps while it is the active page.
/// This is presentation only; it starts no request.
const CLOCK_TICK: Duration = Duration::from_secs(30);

#[derive(Clone)]
struct CachedRead<T> {
    value: T,
    source: ReadSource,
    metadata: Option<CacheMetadata>,
}

enum CachedReadFailure {
    Network(HomeError),
    AuthOrContext(HomeError),
    /// Quarantine, stale leases, I/O and other cache failures say nothing about
    /// the validity of the current account session. Retry the online read with
    /// the ordinary session, which keeps its original 401/cancel/context checks.
    UncachedOnline,
}

fn cache_failure(error: PageCacheError<HomeError>) -> CachedReadFailure {
    match error {
        PageCacheError::Read(error) if can_fall_back(&error) => CachedReadFailure::Network(error),
        PageCacheError::Read(error) => CachedReadFailure::AuthOrContext(error),
        PageCacheError::Context(error) => {
            CachedReadFailure::AuthOrContext(HomeError::GitHub(error))
        }
        PageCacheError::Cache(_) => CachedReadFailure::UncachedOnline,
    }
}

fn cache_miss_failure(error: PageCacheError<HomeError>) -> HomeError {
    match error {
        PageCacheError::Read(error) => error,
        PageCacheError::Context(error) => HomeError::GitHub(error),
        PageCacheError::Cache(_) => HomeError::InvalidResponse {
            message: "The saved page could not be read.".into(),
        },
    }
}

fn can_fall_back(error: &HomeError) -> bool {
    matches!(
        error,
        HomeError::GitHub(GitHubError::Network { .. } | GitHubError::Timeout { .. })
    )
}

fn unpack_cached<T>(
    scoped: Scoped<PageValue<T>>,
    partition: &str,
) -> Result<Option<CachedRead<T>>, PageCacheError<HomeError>> {
    match scoped.value {
        PageValue::Value {
            value,
            origin: PageOrigin::Network,
            metadata,
        } => {
            let collection = crate::data::page_cache::metadata_feed_collection(&metadata);
            Ok(Some(CachedRead {
                value,
                source: ReadSource::Network { collection },
                metadata: None,
            }))
        }
        PageValue::Value {
            value,
            origin: PageOrigin::HistoricalSnapshot,
            metadata,
        } => {
            let collection = crate::data::page_cache::metadata_feed_collection(&metadata);
            let source = ReadSource::Historical {
                partition: partition.to_string(),
                collection,
            };
            Ok(Some(CachedRead {
                value,
                source,
                metadata: Some(metadata),
            }))
        }
        PageValue::Miss(_) => Ok(None),
    }
}

async fn read_home(
    session: PageReadSession,
    cached: Option<CachedPageReadSession>,
    cache_partition: String,
    cancel: crate::network::cancel::CancellationToken,
) -> Result<Option<CachedRead<HomeSearchOutcome>>, HomeError> {
    if let Some(cached) = cached {
        match cached
            .home(
                PageReadMode::Online,
                None,
                &HomeSearchLimits::default(),
                cancel.clone(),
            )
            .await
        {
            Ok(value) => {
                return unpack_cached(value, &cache_partition).map_err(cache_miss_failure);
            }
            Err(error) => match cache_failure(error) {
                CachedReadFailure::Network(_) => {
                    let offline = cached
                        .home(
                            PageReadMode::Offline,
                            None,
                            &HomeSearchLimits::default(),
                            cancel.clone(),
                        )
                        .await
                        .map_err(cache_miss_failure)?;
                    return unpack_cached(offline, &cache_partition).map_err(cache_miss_failure);
                }
                CachedReadFailure::UncachedOnline => {}
                CachedReadFailure::AuthOrContext(error) => return Err(error),
            },
        }
    }
    session
        .home(None, &HomeSearchLimits::default(), cancel)
        .await
        .map(|value| {
            Some(CachedRead {
                value: value.value,
                source: ReadSource::Network { collection: None },
                metadata: None,
            })
        })
}

async fn read_feed(
    session: PageReadSession,
    cached: Option<CachedPageReadSession>,
    cache_partition: String,
    cancel: crate::network::cancel::CancellationToken,
) -> Result<Option<CachedRead<FeedOutcome>>, HomeError> {
    if let Some(cached) = cached {
        match cached
            .feed(
                PageReadMode::Online,
                &FeedLimits::default(),
                &FollowingLimits::default(),
                &PullRequestDetailLimits::default(),
                FEED_AUTHOR_LOOKUP_LIMIT,
                cancel.clone(),
            )
            .await
        {
            Ok(value) => {
                return unpack_cached(value, &cache_partition).map_err(cache_miss_failure);
            }
            Err(error) => match cache_failure(error) {
                CachedReadFailure::Network(_) => {
                    let offline = cached
                        .feed(
                            PageReadMode::Offline,
                            &FeedLimits::default(),
                            &FollowingLimits::default(),
                            &PullRequestDetailLimits::default(),
                            FEED_AUTHOR_LOOKUP_LIMIT,
                            cancel.clone(),
                        )
                        .await
                        .map_err(cache_miss_failure)?;
                    return unpack_cached(offline, &cache_partition).map_err(cache_miss_failure);
                }
                CachedReadFailure::UncachedOnline => {}
                CachedReadFailure::AuthOrContext(error) => return Err(error),
            },
        }
    }
    session
        .feed(
            &FeedLimits::default(),
            &FollowingLimits::default(),
            &PullRequestDetailLimits::default(),
            FEED_AUTHOR_LOOKUP_LIMIT,
            cancel,
        )
        .await
        .map(|value| {
            Some(CachedRead {
                value: value.value,
                source: ReadSource::Network { collection: None },
                metadata: None,
            })
        })
}

/// The deadline is presentation-only. Expiring it restores the Feed refresh
/// control; it never initiates a request.
fn cooldown_expired(ready: Option<Instant>, now: Instant) -> bool {
    ready.is_some_and(|ready| now >= ready)
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ReadSource {
    Network {
        collection: Option<crate::data::page_cache::CollectionGeneration>,
    },
    /// The saved-account partition and collection generation which produced this snapshot.
    Historical {
        partition: String,
        collection: Option<crate::data::page_cache::CollectionGeneration>,
    },
}

#[derive(Clone, Debug)]
enum AppendError {
    Read(HomeError),
    NotCached,
}

#[derive(Clone)]
struct EnrichmentResult {
    descriptions: HashMap<String, Option<String>>,
    cache_partition: Option<String>,
}

enum Reply {
    Home {
        request: u64,
        result: HomeRead,
    },
    Feed {
        request: u64,
        result: FeedRead,
    },
    Append {
        request: u64,
        source: ReadSource,
        result: AppendOutcome,
    },
    Enrich {
        request: u64,
        result: EnrichOutcome,
    },
}

type HomeRead = Result<Option<CachedRead<HomeSearchOutcome>>, HomeError>;
type FeedRead = Result<Option<CachedRead<FeedOutcome>>, HomeError>;
type AppendOutcome = Result<(FeedOutcome, HashSet<String>, ReadSource), AppendError>;
type EnrichOutcome = Result<EnrichmentResult, HomeError>;

pub struct HomePage {
    /// Disposes the page's signals when the page is retired.
    owner: Owner,
    view: HomeView,
    context: PageContext,
    navigation: NavigationSink,
    actions: Rc<RefCell<VecDeque<HomeAction>>>,
    avatar: Option<AvatarImage>,
    binding: RequestBinding,
    generation: u64,
    session: Option<PageReadSession>,
    cached: Option<CachedPageReadSession>,
    cache_partition: Option<String>,
    cache_authorization: Option<CacheAuthorization>,
    home_slot: PageSlot<HomeRead>,
    feed_slot: PageSlot<FeedRead>,
    append_slot: PageSlot<AppendOutcome>,
    enrich_slot: PageSlot<EnrichOutcome>,
    home_ticket: Option<PageTicket>,
    feed_ticket: Option<PageTicket>,
    append_ticket: Option<PageTicket>,
    enrich_ticket: Option<PageTicket>,
    inbox: Arc<Mutex<VecDeque<Reply>>>,
    requests: [u64; 2],
    working: [bool; 2],
    home_shown: bool,
    feed_shown: bool,
    feed_ready: Option<Instant>,
    append_working: bool,
    append_request: u64,
    /// Continuation state for Feed end-of-scroll pagination, scoped to the
    /// current binding/generation. Cleared on rebind/refresh.
    feed_next_page: Option<FeedSourceCursor>,
    feed_seen_ids: HashSet<String>,
    feed_source: Option<ReadSource>,
    enrich_working: bool,
    enrich_request: u64,
    /// Known repository descriptions, keyed by lowercase "owner/name".
    descriptions: HashMap<String, Option<String>>,
    enrich_queue: Vec<(String, String)>,
    enrich_in_flight: HashSet<String>,
    last_clock: Instant,
    active: bool,
    retired: bool,
}

impl HomePage {
    /// The documented factory: real services, real navigation sink. Reads start
    /// asynchronously; no UI work happens here.
    pub fn new(context: PageContext, navigation: NavigationSink) -> Self {
        let tab = context.identity().tab;
        let incarnation = context.identity().incarnation;
        let selection = context.context().auth().epoch.0;
        let binding =
            RequestBinding::new(format!("home:{}:{incarnation}:{}", tab.get(), selection));
        let actions: Rc<RefCell<VecDeque<HomeAction>>> = Rc::new(RefCell::new(VecDeque::new()));
        let (owner, view) = create_root(HomeView::new);
        let mut page = Self {
            owner,
            view,
            context,
            navigation,
            actions,
            avatar: None,
            binding,
            generation: incarnation,
            session: None,
            cached: None,
            cache_partition: None,
            cache_authorization: None,
            home_slot: PageSlot::default(),
            feed_slot: PageSlot::default(),
            append_slot: PageSlot::default(),
            enrich_slot: PageSlot::default(),
            home_ticket: None,
            feed_ticket: None,
            append_ticket: None,
            enrich_ticket: None,
            inbox: Arc::new(Mutex::new(VecDeque::new())),
            requests: [0, 0],
            working: [false, false],
            home_shown: false,
            feed_shown: false,
            feed_ready: None,
            append_working: false,
            append_request: 0,
            feed_next_page: None,
            feed_seen_ids: HashSet::new(),
            feed_source: None,
            enrich_working: false,
            enrich_request: 0,
            descriptions: HashMap::new(),
            enrich_queue: Vec::new(),
            enrich_in_flight: HashSet::new(),
            last_clock: Instant::now(),
            active: true,
            retired: false,
        };
        page.view.bind(page.binding.clone(), page.generation);
        page.capture_session();
        page.start(Column::Home, true);
        page.start(Column::Feed, true);
        page
    }

    /// Host presentation hook for decoded Feed avatars. The account/avatar
    /// controller owns loading, decoding and the disk cache; this page only draws
    /// what it is handed, so no second image stack exists here.
    pub fn set_avatar_resolver(&mut self, avatar: Option<AvatarImage>) {
        self.avatar = avatar;
    }

    fn capture_session(&mut self) {
        let Some(session) = self.page_session() else {
            self.unavailable(
                "The current account is not verified or its credential is unavailable. No online results are shown.",
            );
            return;
        };
        // cache-runtime owns the app-level store. This consumer only clones the
        // binding issued for the session's current account, epoch and selection;
        // no second database is opened here.
        if let Some(cache) = self.context.cache() {
            let (store, scope) = cache.clone().into_parts();
            self.cache_partition = Some(scope.partition().to_string());
            self.cached =
                CachedPageReadSession::new(session.clone(), store.clone(), scope.clone()).ok();
            self.cache_authorization = self
                .cached
                .as_ref()
                .and_then(|_| store.authorization(&scope, None));
        }
        self.session = Some(session);
    }

    fn page_session(&self) -> Option<PageReadSession> {
        let session = self.context.page_session().ok()?;
        if self
            .context
            .services()
            .repositories()
            .is_current(session.context())
        {
            Some(session)
        } else {
            None
        }
    }

    fn cache_binding_lost(&self) -> bool {
        self.session.as_ref().is_some_and(|session| {
            self.cached.is_some()
                && self
                    .context
                    .services()
                    .repositories()
                    .cache(session.context())
                    .is_none()
        })
    }

    /// Cache-only invalidation removes copied bytes but leaves the verified
    /// account session usable, so the user can recover with an online refresh.
    fn cache_invalidated(&mut self, message: &str) {
        self.cancel_work();
        for column in [Column::Home, Column::Feed] {
            self.view
                .set_unavailable(column, &self.binding, self.generation, message);
            self.view.set_refresh_availability(
                column,
                &self.binding,
                self.generation,
                RefreshAvailability::available(),
            );
        }
        self.home_shown = false;
        self.feed_shown = false;
    }

    fn unavailable(&mut self, message: &str) {
        self.cancel_work();
        for column in [Column::Home, Column::Feed] {
            self.view
                .set_unavailable(column, &self.binding, self.generation, message);
            self.view.set_refresh_availability(
                column,
                &self.binding,
                self.generation,
                RefreshAvailability::unavailable(message),
            );
        }
        self.home_shown = false;
        self.feed_shown = false;
    }

    fn index(column: Column) -> usize {
        match column {
            Column::Home => 0,
            Column::Feed => 1,
        }
    }

    fn start(&mut self, column: Column, initial: bool) {
        if self.retired {
            return;
        }
        let index = Self::index(column);
        if self.working[index] {
            return;
        }
        if column == Column::Feed && self.feed_ready.is_some_and(|ready| Instant::now() < ready) {
            return;
        }
        if column == Column::Feed {
            // A replacement Feed read invalidates any outstanding append or
            // description read: their late replies must not overwrite it.
            self.append_slot.cancel();
            self.append_ticket = None;
            self.append_working = false;
            self.enrich_slot.cancel();
            self.enrich_ticket = None;
            self.enrich_working = false;
            self.enrich_queue.clear();
            self.enrich_in_flight.clear();
            self.descriptions.clear();
            self.view.clear_repository_descriptions();
        }
        let Some(session) = self.session.clone() else {
            return;
        };
        if self.cached.is_none() {
            self.capture_session();
        }
        if !self
            .context
            .services()
            .repositories()
            .is_current(session.context())
        {
            self.session = None;
            self.unavailable(
                "The account or authorization changed. Home will read again in the current workspace.",
            );
            return;
        }
        self.requests[index] = self.requests[index].saturating_add(1);
        let request = self.requests[index];
        self.working[index] = true;
        if initial {
            self.view
                .set_loading(column, &self.binding, self.generation);
        } else {
            self.view.begin_refresh(column);
        }
        self.view.set_refresh_availability(
            column,
            &self.binding,
            self.generation,
            RefreshAvailability::unavailable("A read is already in flight."),
        );
        let ticket = match column {
            Column::Home => {
                let ticket = self.home_slot.begin(self.context.clone());
                self.home_ticket = Some(ticket.clone());
                ticket
            }
            Column::Feed => {
                let ticket = self.feed_slot.begin(self.context.clone());
                self.feed_ticket = Some(ticket.clone());
                ticket
            }
        };
        let cancel = ticket.cancellation();
        let inbox = self.inbox.clone();
        let cached = self.cached.clone();
        let cache_partition = self.cache_partition.clone().unwrap_or_default();
        let services = self.context.services().clone();
        let spawned = services.spawn(async move {
            let reply = match column {
                Column::Home => Reply::Home {
                    request,
                    result: read_home(session, cached, cache_partition, cancel).await,
                },
                // The data layer composes the raw events read, the followed-account
                // read and the bounded merged-author lookups, then applies the
                // product policy. Bounds keep one snapshot from fanning out: a
                // followed list of at most 1,000 accounts and at most 30 distinct
                // author lookups per refresh.
                Column::Feed => Reply::Feed {
                    request,
                    result: read_feed(session, cached, cache_partition, cancel).await,
                },
            };
            inbox.lock().expect("home inbox").push_back(reply);
        });
        if spawned.is_err() {
            // The worker could not even be scheduled: report it through the same
            // presentation path as any other failed read, without publishing a
            // slot value for a read that never ran.
            self.working[index] = false;
            self.view.apply_error(
                column,
                &self.binding,
                self.generation,
                HomeError::InvalidResponse {
                    message: "Background services are closing.".into(),
                },
            );
            self.view.set_refresh_availability(
                column,
                &self.binding,
                self.generation,
                RefreshAvailability::available(),
            );
        }
    }

    /// Starts one bounded load-more of the Feed's next older source page.
    ///
    /// At most one append runs per account/generation. The request uses the same
    /// captured session and account context as the first read, so account switches
    /// and refreshes reject stale replies.
    fn load_more_feed(&mut self) {
        if self.append_working || self.working[1] {
            return;
        }
        let Some(next) = self.feed_next_page else {
            return;
        };
        let (Some(session), Some(source)) = (self.session.clone(), self.feed_source.clone()) else {
            return;
        };
        let context = session.context().clone();
        if !self.context.services().repositories().is_current(&context) {
            self.feed_append_failed(next.page);
            return;
        }
        self.append_request = self.append_request.saturating_add(1);
        let request = self.append_request;
        self.append_working = true;
        let (ticket, cancel) = {
            let ticket = self.append_slot.begin(self.context.clone());
            let cancel = ticket.cancellation();
            (ticket, cancel)
        };
        self.append_ticket = Some(ticket);
        self.view
            .feed_append_loading(&self.binding, self.generation);
        let inbox = self.inbox.clone();
        let mut seen = self.feed_seen_ids.clone();
        let cached = self.cached.clone();
        let cache_partition = self.cache_partition.clone().unwrap_or_default();
        let reply_context = context.clone();
        let services = self.context.services().clone();
        let spawned = services.spawn(async move {
            let result = if let Some(cached) = cached {
                // A collection keeps its origin for its whole lifetime. Never
                // append a historical page to a network first page (or vice versa).
                let (mode, collection) = match &source {
                    ReadSource::Network { collection } => (PageReadMode::Online, *collection),
                    ReadSource::Historical { collection, .. } => {
                        (PageReadMode::Offline, *collection)
                    }
                };
                let cached_result = cached
                    .feed_page(
                        mode,
                        next,
                        collection,
                        &FeedLimits::default(),
                        &FollowingLimits::default(),
                        &PullRequestDetailLimits::default(),
                        FEED_AUTHOR_LOOKUP_LIMIT,
                        &mut seen,
                        cancel.clone(),
                    )
                    .await;
                let result = match cached_result {
                    Err(error) if matches!(source, ReadSource::Network { .. }) => {
                        match cache_failure(error) {
                            CachedReadFailure::UncachedOnline => session
                                .feed_page(
                                    next,
                                    &FeedLimits::default(),
                                    &FollowingLimits::default(),
                                    &PullRequestDetailLimits::default(),
                                    FEED_AUTHOR_LOOKUP_LIMIT,
                                    &mut seen,
                                    cancel.clone(),
                                )
                                .await
                                .map_err(AppendError::Read),
                            CachedReadFailure::Network(error) => Err(AppendError::Read(error)),
                            CachedReadFailure::AuthOrContext(error) => {
                                Err(AppendError::Read(error))
                            }
                        }
                    }
                    Err(error) => Err(AppendError::Read(cache_miss_failure(error))),
                    Ok(page) => unpack_cached(page, &cache_partition)
                        .map_err(|error| AppendError::Read(cache_miss_failure(error)))
                        .and_then(|page| {
                            page.ok_or(AppendError::NotCached).map(|page| Scoped {
                                context: reply_context.clone(),
                                value: page.value,
                            })
                        }),
                };
                let reply_source = source.clone();
                result.map(|page| (page.value.into_outcome(), seen.clone(), reply_source))
            } else {
                session
                    .feed_page(
                        next,
                        &FeedLimits::default(),
                        &FollowingLimits::default(),
                        &PullRequestDetailLimits::default(),
                        FEED_AUTHOR_LOOKUP_LIMIT,
                        &mut seen,
                        cancel,
                    )
                    .await
                    .map_err(AppendError::Read)
                    .map(|page| {
                        (
                            page.value.into_outcome(),
                            seen.clone(),
                            ReadSource::Network { collection: None },
                        )
                    })
            };
            inbox.lock().expect("home inbox").push_back(Reply::Append {
                request,
                source,
                result,
            });
        });
        if spawned.is_err() {
            self.append_working = false;
            self.feed_append_failed(next.page);
        }
    }

    fn feed_append_failed(&mut self, page: u32) {
        self.append_working = false;
        self.view
            .feed_append_failed(&self.binding, self.generation, page);
    }

    fn apply(&mut self, reply: Reply, sessions: &Sessions) {
        match reply {
            Reply::Home { request, result } => self.apply_home(request, result, sessions),
            Reply::Feed { request, result } => self.apply_feed(request, result, sessions),
            Reply::Append {
                request,
                source,
                result,
            } => self.apply_append(request, source, result, sessions),
            Reply::Enrich { request, result } => self.apply_enrichment(request, result, sessions),
        }
    }

    fn apply_home(&mut self, request: u64, result: HomeRead, sessions: &Sessions) {
        if request != self.requests[0] {
            return;
        }
        self.working[0] = false;
        let Some(ticket) = self.home_ticket.take() else {
            return;
        };
        if !self.home_slot.publish(ticket, result.clone(), sessions) {
            return;
        }
        self.home_shown = true;
        match result {
            Ok(Some(read)) => match read.source {
                ReadSource::Network { .. } => {
                    self.view
                        .apply_home(&self.binding, self.generation, read.value);
                }
                ReadSource::Historical { .. } => {
                    if let Some(metadata) = read.metadata {
                        self.view.apply_home_snapshot(
                            &self.binding,
                            self.generation,
                            SnapshotPresentation::historical(read.value, &metadata),
                        );
                    }
                }
            },
            Ok(None) => {
                self.view.set_unavailable(
                    Column::Home,
                    &self.binding,
                    self.generation,
                    "This page is not available offline.",
                );
            }
            Err(error) => self.apply_error(Column::Home, error),
        }
        self.view.set_refresh_availability(
            Column::Home,
            &self.binding,
            self.generation,
            RefreshAvailability::available(),
        );
    }

    fn apply_feed(&mut self, request: u64, result: FeedRead, sessions: &Sessions) {
        if request != self.requests[1] {
            return;
        }
        self.working[1] = false;
        let Some(ticket) = self.feed_ticket.take() else {
            return;
        };
        if !self.feed_slot.publish(ticket, result.clone(), sessions) {
            return;
        }
        self.feed_shown = true;
        let feed_activities = match &result {
            Ok(Some(read)) => Some(read.value.activities.clone()),
            _ => None,
        };
        match result {
            Ok(Some(read)) => {
                let outcome = read.value;
                if matches!(read.source, ReadSource::Network { .. })
                    && let Some(seconds) = outcome.poll_interval_seconds
                {
                    self.feed_ready = Some(Instant::now() + Duration::from_secs(seconds.max(1)));
                } else if matches!(read.source, ReadSource::Network { .. }) {
                    self.feed_ready = None;
                }
                // Seed continuation from the first page: the validated next
                // source cursor and the seen event ids become the append
                // baseline, scoped to this generation.
                self.feed_next_page = outcome.next_page;
                self.feed_source = Some(read.source.clone());
                self.feed_seen_ids.clear();
                for activity in &outcome.activities {
                    self.feed_seen_ids.insert(activity.event.id.clone());
                }
                match read.source {
                    ReadSource::Network { .. } => {
                        self.view
                            .apply_feed(&self.binding, self.generation, outcome);
                    }
                    ReadSource::Historical { .. } => {
                        let applied = match read.metadata {
                            Some(metadata) => self.view.apply_feed_snapshot(
                                &self.binding,
                                self.generation,
                                SnapshotPresentation::historical(outcome, &metadata),
                            ),
                            None => false,
                        };
                        if !applied {
                            self.feed_next_page = None;
                            self.feed_source = None;
                        }
                    }
                }
            }
            Ok(None) => {
                self.feed_next_page = None;
                self.feed_source = None;
                self.view.set_unavailable(
                    Column::Feed,
                    &self.binding,
                    self.generation,
                    "This page is not available offline.",
                );
            }
            Err(error) => {
                self.feed_ready = None;
                self.feed_next_page = None;
                self.feed_source = None;
                self.apply_error(Column::Feed, error);
            }
        }
        if let Some(activities) = feed_activities {
            self.enrich_starred_repositories(&activities);
        }
        let reason = self.feed_ready.and_then(|ready| {
            ready
                .checked_duration_since(Instant::now())
                .map(|remaining| {
                    format!(
                        "GitHub asked Feed clients to wait about {} more second(s) before polling.",
                        remaining.as_secs().max(1)
                    )
                })
        });
        self.view.set_refresh_availability(
            Column::Feed,
            &self.binding,
            self.generation,
            match reason {
                Some(reason) => RefreshAvailability::unavailable(reason),
                None => RefreshAvailability::available(),
            },
        );
    }

    fn apply_append(
        &mut self,
        request: u64,
        source: ReadSource,
        result: AppendOutcome,
        sessions: &Sessions,
    ) {
        if request != self.append_request {
            return;
        }
        self.append_working = false;
        let Some(ticket) = self.append_ticket.take() else {
            return;
        };
        if self.feed_source.as_ref() != Some(&source) {
            return;
        }
        if !self.append_slot.publish(ticket, result.clone(), sessions) {
            return;
        }
        if self.cache_binding_lost() {
            self.unavailable(
                "Saved content is no longer available for this account or authorization.",
            );
            return;
        }
        let page = self.feed_next_page.map(|cursor| cursor.page).unwrap_or(1);
        match result {
            Ok((page_outcome, seen, source)) => {
                if self.feed_source.as_ref() != Some(&source) {
                    return;
                }
                self.feed_seen_ids = seen;
                self.feed_next_page = page_outcome.next_page;
                // Merge into the retained outcome in place, preserving the
                // earlier snapshot's partial reasons so an append can never make
                // a previously-withheld set look complete.
                let Some(existing) = self.view.feed_outcome() else {
                    return;
                };
                let mut merged = existing;
                let mut reasons = merged.readiness.reasons().to_vec();
                reasons.extend(page_outcome.readiness.reasons().iter().cloned());
                merged.activities.extend(page_outcome.activities.clone());
                merged.next_page = page_outcome.next_page;
                merged.readiness = crate::domain::home::FeedReadiness::from_reasons(reasons);
                merged.etag = if merged.readiness.is_complete() {
                    page_outcome.etag
                } else {
                    None
                };
                if let Some(poll) = page_outcome.poll_interval_seconds {
                    merged.poll_interval_seconds = Some(poll);
                }
                let activities = merged.activities.clone();
                self.view
                    .apply_feed_page(&self.binding, self.generation, merged);
                self.enrich_starred_repositories(&activities);
            }
            Err(AppendError::NotCached) => {
                // Keep the original historical collection intact, but stop its
                // automatic continuation at the exact missing page.
                self.feed_next_page = None;
                self.view
                    .feed_page_not_cached(&self.binding, self.generation, page);
            }
            Err(AppendError::Read(_error)) => {
                self.view
                    .feed_append_failed(&self.binding, self.generation, page);
            }
        }
    }

    /// Asynchronously enriches repository descriptions for starred repository
    /// cards. Requests are deduplicated and serialized; descriptions stay scoped
    /// to the current Feed lifecycle.
    fn enrich_starred_repositories(&mut self, activities: &[crate::domain::home::FeedActivity]) {
        for activity in activities {
            if activity.kind != crate::domain::home::FeedActivityKind::RepositoryStarred {
                continue;
            }
            let (owner, repo) = activity
                .event
                .repository
                .name
                .split_once('/')
                .map_or(("", activity.event.repository.name.as_str()), |(o, r)| {
                    (o, r)
                });
            if owner.is_empty() || repo.is_empty() {
                continue;
            }
            let key = format!("{owner}/{repo}").to_ascii_lowercase();
            if self.descriptions.contains_key(&key)
                || self.enrich_in_flight.contains(&key)
                || self
                    .enrich_queue
                    .iter()
                    .any(|(o, r)| format!("{o}/{r}").eq_ignore_ascii_case(&key))
            {
                continue;
            }
            self.enrich_queue
                .push((owner.to_string(), repo.to_string()));
        }
        let known: HashMap<String, String> = self
            .descriptions
            .iter()
            .filter_map(|(k, v)| v.as_ref().map(|desc| (k.clone(), desc.clone())))
            .collect();
        if !known.is_empty() {
            self.view
                .set_repository_descriptions(&self.binding, self.generation, known);
        }
        self.start_enrichment();
    }

    fn start_enrichment(&mut self) {
        if self.enrich_working || self.enrich_queue.is_empty() {
            return;
        }
        let Some(session) = self.session.clone() else {
            return;
        };
        let context = session.context().clone();
        if !self.context.services().repositories().is_current(&context) {
            return;
        }
        let needed: Vec<(String, String)> = std::mem::take(&mut self.enrich_queue);
        self.enrich_in_flight = needed
            .iter()
            .map(|(owner, name)| format!("{owner}/{name}").to_ascii_lowercase())
            .collect();
        self.enrich_request = self.enrich_request.saturating_add(1);
        let request = self.enrich_request;
        self.enrich_working = true;
        let (ticket, cancel) = {
            let ticket = self.enrich_slot.begin(self.context.clone());
            let cancel = ticket.cancellation();
            (ticket, cancel)
        };
        self.enrich_ticket = Some(ticket);
        let mode = match self.feed_source {
            Some(ReadSource::Historical { .. }) => PageReadMode::Offline,
            _ => PageReadMode::Online,
        };
        let cached = self.cached.clone();
        let cache_partition = self.cache_partition.clone();
        let inbox = self.inbox.clone();
        let services = self.context.services().clone();
        let spawned = services.spawn(async move {
            let result =
                if let Some(cached) = cached {
                    match cached
                        .repository_descriptions(mode, needed.clone(), 20, cancel.clone())
                        .await
                    {
                        Err(error) if matches!(mode, PageReadMode::Online) => {
                            match cache_failure(error) {
                                CachedReadFailure::UncachedOnline => session
                                    .repository_descriptions(needed, 20, cancel.clone())
                                    .await
                                    .map(|value| EnrichmentResult {
                                        descriptions: value.value,
                                        cache_partition: None,
                                    }),
                                CachedReadFailure::Network(error) => Err(error),
                                CachedReadFailure::AuthOrContext(error) => Err(error),
                            }
                        }
                        result => result
                            .map_err(cache_miss_failure)
                            .and_then(|value| match value.value {
                                PageValue::Value { value, origin, .. } => Ok(EnrichmentResult {
                                    descriptions: value,
                                    cache_partition: matches!(
                                        origin,
                                        PageOrigin::HistoricalSnapshot
                                    )
                                    .then_some(cache_partition)
                                    .flatten(),
                                }),
                                PageValue::Miss(_) => Err(HomeError::InvalidResponse {
                                    message: "Repository descriptions are not cached.".into(),
                                }),
                            }),
                    }
                } else {
                    session
                        .repository_descriptions(needed, 20, cancel)
                        .await
                        .map(|value| EnrichmentResult {
                            descriptions: value.value,
                            cache_partition: None,
                        })
                };
            inbox
                .lock()
                .expect("home inbox")
                .push_back(Reply::Enrich { request, result });
        });
        if spawned.is_err() {
            self.enrich_working = false;
            self.enrich_in_flight.clear();
        }
    }

    fn apply_enrichment(&mut self, request: u64, result: EnrichOutcome, sessions: &Sessions) {
        if request != self.enrich_request {
            return;
        }
        self.enrich_working = false;
        self.enrich_in_flight.clear();
        let Some(ticket) = self.enrich_ticket.take() else {
            return;
        };
        if !self.enrich_slot.publish(ticket, result.clone(), sessions) {
            return;
        }
        if self.cache_binding_lost() {
            self.unavailable(
                "Saved content is no longer available for this account or authorization.",
            );
            return;
        }
        if let Ok(result) = result {
            if let Some(partition) = result.cache_partition
                && self.cache_partition.as_deref() != Some(partition.as_str())
            {
                return;
            }
            for (key, description) in result.descriptions {
                self.descriptions.insert(key, description);
            }
            let known: HashMap<String, String> = self
                .descriptions
                .iter()
                .filter_map(|(k, v)| v.as_ref().map(|desc| (k.clone(), desc.clone())))
                .collect();
            self.view
                .set_repository_descriptions(&self.binding, self.generation, known);
        }
        // A queued batch (for example after an append) continues without the
        // caller having to notice; each batch is bounded and serialized.
        self.start_enrichment();
    }

    fn apply_error(&mut self, column: Column, error: HomeError) {
        if matches!(error, HomeError::GitHub(GitHubError::Unauthorized)) {
            // Credential rejection invalidates copied page content but does not
            // make the captured page session itself stale. Keep the session
            // available for explicit online recovery; never replay old bytes.
            self.cancel_work();
            self.unavailable(
                "GitHub rejected the credential. Revalidate or replace the PAT before reading cached content.",
            );
            return;
        }
        if matches!(error, HomeError::GitHub(GitHubError::StaleContext)) && self.cached.is_some() {
            self.session = None;
            self.cached = None;
            self.unavailable(
                "Saved content is no longer available for this account or authorization.",
            );
            return;
        }
        // A result that passed the cache lease checks may still arrive after a
        // lifecycle event. Do not keep even the already-rendered historical copy
        // when this account's cache permission is no longer available.
        if self.cache_binding_lost() {
            self.session = None;
            self.cached = None;
            self.unavailable(
                "Saved content is no longer available for this account or authorization.",
            );
            return;
        }
        self.view
            .apply_error(column, &self.binding, self.generation, error);
    }

    /// Drops every in-flight read and the continuation state. Displayed content
    /// is cleared by the caller (a revocation, an account change or a teardown).
    fn cancel_work(&mut self) {
        self.home_slot.cancel();
        self.feed_slot.cancel();
        self.append_slot.cancel();
        self.enrich_slot.cancel();
        self.home_ticket = None;
        self.feed_ticket = None;
        self.append_ticket = None;
        self.enrich_ticket = None;
        self.working = [false, false];
        self.append_working = false;
        self.enrich_working = false;
        self.enrich_queue.clear();
        self.enrich_in_flight.clear();
        self.feed_ready = None;
        self.feed_next_page = None;
        self.feed_seen_ids.clear();
        self.feed_source = None;
        self.descriptions.clear();
        self.inbox.lock().expect("home inbox").clear();
    }

    fn stop(&mut self) {
        self.cancel_work();
        self.session = None;
        self.cached = None;
        self.cache_partition = None;
        self.cache_authorization = None;
    }

    /// Clears already-displayed content whose captured authorization or
    /// workspace identity is no longer current. This is the foreground drain, so
    /// it uses only atomic/memory checks from the slot ticket.
    fn clear_stale_display(&mut self, sessions: &Sessions) {
        let home_valid = !self.home_shown || self.home_slot.is_valid(sessions);
        let feed_valid = !self.feed_shown || self.feed_slot.is_valid(sessions);
        if home_valid && feed_valid && !self.cache_binding_lost() {
            return;
        }
        let authorization_stale = self
            .cache_authorization
            .as_ref()
            .is_some_and(|authorization| !authorization.is_current());
        let session_usable = self.session.as_ref().is_some_and(|session| {
            self.context
                .services()
                .repositories()
                .is_current(session.context())
        }) && !self.cache_binding_lost();
        if authorization_stale && session_usable {
            self.home_slot.clear();
            self.feed_slot.clear();
            self.cancel_work();
            self.cached = None;
            self.cache_authorization = None;
            self.cache_partition = None;
            self.cache_invalidated(
                "Saved content is no longer available; refresh to read online for the current account.",
            );
        } else if !home_valid || !feed_valid || self.cache_binding_lost() {
            self.stop();
            self.unavailable(
                "The account or authorization changed. Refresh in the current account.",
            );
        }
    }

    fn expire_cooldown(&mut self) {
        if !cooldown_expired(self.feed_ready, Instant::now()) {
            return;
        }
        let had_cooldown = self.feed_ready.is_some();
        self.feed_ready = None;
        if had_cooldown {
            self.view.set_refresh_availability(
                Column::Feed,
                &self.binding,
                self.generation,
                RefreshAvailability::available(),
            );
        }
    }

    fn tick_clock(&mut self) {
        if !self.active {
            return;
        }
        if self.last_clock.elapsed() < CLOCK_TICK {
            return;
        }
        self.last_clock = Instant::now();
        self.view.set_now(std::time::SystemTime::now());
    }

    /// Continues a Feed whose visible batch is empty. The virtual list reports
    /// near-bottom for short content, but a zero-row page lays out no list at all,
    /// so that case is driven from the foreground tick instead (LESSONS L015).
    fn continue_feed_if_needed(&mut self) {
        if !self.feed_shown
            || self.append_working
            || self.working[1]
            || self.feed_next_page.is_none()
            || self.view.item_count(Column::Feed) != 0
        {
            return;
        }
        if self.view.feed_append_state() != crate::ui::home::FeedAppendState::Idle {
            return;
        }
        self.load_more_feed();
    }

    fn action(&mut self, action: HomeAction) {
        match action {
            HomeAction::Refresh(request)
                if request.binding == self.binding && request.generation == self.generation =>
            {
                match request.column {
                    Column::Home => self.start(Column::Home, false),
                    Column::Feed => self.start(Column::Feed, false),
                }
            }
            HomeAction::LoadMoreFeed(request)
                if request.binding == self.binding
                    && request.generation == self.generation
                    && request.column == Column::Feed =>
            {
                self.load_more_feed();
            }
            HomeAction::Repository(repository) => {
                (self.navigation)(PageNavigation::Open(Route::Repository {
                    namespace: repository.owner,
                    name: repository.name,
                }))
            }
            HomeAction::Profile(profile) => {
                let active = self.context.services().accounts().active_summary();
                (self.navigation)(PageNavigation::Open(Route::profile(
                    profile.login,
                    active.as_ref(),
                )))
            }
            HomeAction::OpenPullRequest { repository, number } => {
                (self.navigation)(PageNavigation::Open(Route::PullRequest {
                    namespace: repository.owner,
                    name: repository.name,
                    number,
                }))
            }
            // Other Issue/PR entry points are outside this navigation correction.
            HomeAction::ActivateWorkItem(_)
            | HomeAction::ActivateFeed(_)
            | HomeAction::Refresh(_)
            | HomeAction::LoadMoreFeed(_) => {}
        }
    }
}

impl PageView for HomePage {
    fn element(&self) -> AnyElement {
        self.view.element(
            &{
                let actions = self.actions.clone();
                Rc::new(move |action: HomeAction| {
                    actions.borrow_mut().push_back(action);
                })
            },
            self.avatar.as_ref(),
        )
    }

    fn poll(&mut self, runtime: &RuntimeHandle) {
        if self.retired {
            return;
        }
        let sessions = runtime.sessions();
        self.clear_stale_display(sessions);
        loop {
            let action = self.actions.borrow_mut().pop_front();
            let Some(action) = action else { break };
            self.action(action);
        }
        let replies: Vec<Reply> = self.inbox.lock().expect("home inbox").drain(..).collect();
        for reply in replies {
            self.apply(reply, sessions);
        }
        self.expire_cooldown();
        self.tick_clock();
        self.continue_feed_if_needed();
    }

    fn refresh(&mut self, _runtime: &RuntimeHandle) {
        self.start(Column::Home, false);
        self.start(Column::Feed, false);
    }

    fn set_active(&mut self, active: bool) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active {
            self.last_clock = Instant::now();
        }
    }

    fn retire(&mut self) {
        if self.retired {
            return;
        }
        self.retired = true;
        self.stop();
        self.owner.dispose();
    }
}

#[cfg(test)]
#[path = "home/tests.rs"]
mod tests;
