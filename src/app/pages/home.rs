use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{AppContext, Context, Entity, IntoElement, Render, Subscription, Window};
use tokio::sync::watch;

use crate::app::accounts::AccountServices;
use crate::app::avatar::AvatarKey;
use crate::data::{PageReadSession, RepositoryReadContext};
use crate::domain::home::{FeedOutcome, HomeError, HomeSearchOutcome};
use crate::github::home::{FeedLimits, FollowingLimits, HomeSearchLimits, PullRequestDetailLimits};
use crate::network::cancel::{CancellationToken, CancellationTokenSource};
use crate::ui::avatar::AvatarSlot;
use crate::ui::home::{Column, HomeAction, HomeView, RefreshAvailability, RequestBinding};
use crate::workspace::TabId;

use super::{NavigationSink, PageAvatars, PageNavigation};
use crate::workspace::Route;

/// Upper bound on merged pull-request detail reads per Feed refresh. A 300-event
/// window with a handful of merges stays far below this; the cap exists so a
/// pathological page cannot turn into dozens of sequential requests. Targets over
/// the cap are reported as unresolved rather than silently dropped.
const FEED_AUTHOR_LOOKUP_LIMIT: usize = 30;

/// The deadline is presentation-only. Expiring it restores the Feed refresh control; it never
/// initiates a request. Extracted so the expiry rule can be exercised without a wall-clock sleep.
fn cooldown_expired(ready: Option<Instant>, now: Instant) -> bool {
    ready.is_some_and(|ready| now >= ready)
}

struct Work {
    token: CancellationToken,
    source: CancellationTokenSource,
}

impl Work {
    fn new() -> Self {
        let (token, source) = CancellationToken::new();
        Self { token, source }
    }
}

enum ColumnResult {
    Home(Result<HomeSearchOutcome, HomeError>),
    Feed(Result<FeedOutcome, HomeError>),
}

struct Reply {
    generation: u64,
    column: Column,
    request: u64,
    selection: u64,
    context: RepositoryReadContext,
    result: ColumnResult,
}

/// An end-of-scroll append result, routed separately from the column reply so it
/// can never be mistaken for a fresh first-page read.
struct AppendReply {
    generation: u64,
    /// Monotonic append epoch, bumped whenever the Feed starts a replacement
    /// (first-page) read. An append started before that read carries a lower
    /// epoch and is rejected, so a refresh can never be skipped or overwritten.
    append_epoch: u64,
    selection: u64,
    context: RepositoryReadContext,
    result: Result<(FeedOutcome, std::collections::HashSet<String>), HomeError>,
}

pub struct HomePage {
    tab: TabId,
    services: Option<AccountServices>,
    session: Option<PageReadSession>,
    selection: u64,
    binding: RequestBinding,
    generation: u64,
    requests: [u64; 2],
    work: [Option<Work>; 2],
    inbox: Arc<Mutex<VecDeque<Reply>>>,
    append_inbox: Arc<Mutex<VecDeque<AppendReply>>>,
    alive: Arc<watch::Sender<bool>>,
    receiver: Option<gpui::Task<()>>,
    feed_ready: Option<Instant>,
    feed_cooldown_task: Option<gpui::Task<()>>,
    /// Continuation state for Feed end-of-scroll pagination, scoped to the
    /// current binding/generation. Cleared on rebind/refresh.
    feed_next_page: Option<crate::domain::home::FeedSourceCursor>,
    feed_seen_ids: std::collections::HashSet<String>,
    feed_append: Option<Work>,
    /// Monotonic append epoch; bumped before every replacement Feed read so an
    /// in-flight append from before a refresh is rejected and cancelled.
    feed_append_epoch: u64,
    view: Entity<HomeView>,
    navigation: NavigationSink,
    _changed: Subscription,
}

impl HomePage {
    pub fn new(
        tab: TabId,
        services: Option<AccountServices>,
        incarnation: u64,
        navigation: NavigationSink,
        avatars: PageAvatars,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let binding = RequestBinding::new(format!("home:{}:{incarnation}", tab.get()));
        let view = cx.new(|cx| HomeView::new(binding.clone(), 0, window, cx));
        // Feed images resolve through the same controller-owned loader as the sidebar. The view
        // receives decoded images only; it never sees a URL or performs a request. The renderer
        // is installed exactly once and receives the credited activity subject, which for a merged
        // pull request is the payload author, not the maintainer actor.
        let page_weak = cx.weak_entity();
        let avatar_loader = avatars.clone();
        view.update(cx, |view, _| {
            view.set_avatar_renderer(Rc::new(move |subject, index, cx| {
                let key = AvatarKey::new("github.com", subject.id, subject.avatar_url.clone()?);
                Some(avatar_loader.element(
                    &format!("home-feed-avatar-{}-{index}", subject.id),
                    &format!("home-feed-real-avatar-{index}"),
                    &subject.login,
                    Some(key),
                    AvatarSlot::Size(ely_core::theme::AvatarSize::Lg),
                    cx,
                ))
            }));
            view.on_action(Rc::new(move |action, window, cx| {
                // The emitting HomeView is still on the update stack here. Re-entering its host
                // synchronously (refresh calls HomeView::update again; navigation suspends the
                // same host) would panic. Deliver the action after this component update ends.
                let page_weak = page_weak.clone();
                window.defer(cx, move |window, cx| {
                    let _ = page_weak.update(cx, |page, cx| page.action(action, window, cx));
                });
            }));
        });
        let _changed = cx.observe(&view, |_, _, cx| cx.notify());
        let (alive, _) = watch::channel(true);
        let mut this = Self {
            tab,
            services: None,
            session: None,
            selection: 0,
            binding,
            generation: 0,
            requests: [0, 0],
            work: [None, None],
            inbox: Arc::new(Mutex::new(VecDeque::new())),
            append_inbox: Arc::new(Mutex::new(VecDeque::new())),
            alive: Arc::new(alive),
            receiver: None,
            feed_ready: None,
            feed_cooldown_task: None,
            feed_next_page: None,
            feed_seen_ids: std::collections::HashSet::new(),
            feed_append: None,
            feed_append_epoch: 0,
            view,
            navigation,
            _changed,
        };
        this.start_receiver(cx);
        this.rebind(services, incarnation, window, cx);
        this
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.view.update(cx, |view, cx| view.set_active(active, cx));
    }

    pub fn rebind(
        &mut self,
        services: Option<AccountServices>,
        incarnation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stop();
        self.services = services;
        self.selection = self.services.as_ref().map_or(0, AccountServices::selection);
        self.generation = self.generation.saturating_add(1);
        self.binding = RequestBinding::new(format!(
            "home:{}:{incarnation}:{}",
            self.tab.get(),
            self.selection
        ));
        self.feed_ready = None;
        self.feed_next_page = None;
        self.feed_seen_ids.clear();
        let binding = self.binding.clone();
        let generation = self.generation;
        self.view.update(cx, |view, cx| {
            view.bind(binding, generation, window, cx);
        });
        let Some(services) = self.services.clone() else {
            self.unavailable(
                "Sign in to read Home and Feed. There is no offline page cache.",
                cx,
            );
            return;
        };
        match services.accounts().page_session() {
            Ok(session) if services.repositories().is_current(session.context()) => {
                self.session = Some(session);
                self.start(Column::Home, true, cx);
                self.start(Column::Feed, true, cx);
            }
            _ => self.unavailable(
                "The current account is not verified or its credential is unavailable. No online results are shown.",
                cx,
            ),
        }
    }

    fn unavailable(&mut self, message: &str, cx: &mut Context<Self>) {
        let binding = self.binding.clone();
        let generation = self.generation;
        self.view.update(cx, |view, cx| {
            for column in [Column::Home, Column::Feed] {
                view.set_unavailable(column, &binding, generation, message, cx);
                view.set_refresh_availability(
                    column,
                    &binding,
                    generation,
                    RefreshAvailability::unavailable(message),
                    cx,
                );
            }
        });
    }

    #[cfg(test)]
    pub(crate) fn debug_reads(&self) -> ([u64; 2], bool) {
        (self.requests, self.session.is_some())
    }

    pub(super) fn action(
        &mut self,
        action: HomeAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            HomeAction::Refresh(request)
                if request.binding == self.binding && request.generation == self.generation =>
            {
                self.start(request.column, false, cx);
            }
            HomeAction::LoadMoreFeed(request)
                if request.binding == self.binding
                    && request.generation == self.generation
                    && request.column == Column::Feed =>
            {
                self.load_more_feed(request, cx);
            }
            HomeAction::Repository(repository) => (self.navigation)(
                PageNavigation::Open(Route::Repository {
                    namespace: repository.owner,
                    name: repository.name,
                }),
                window,
                cx,
            ),
            HomeAction::Profile(profile) => {
                let active = self
                    .services
                    .as_ref()
                    .and_then(|services| services.accounts().active_summary());
                (self.navigation)(
                    PageNavigation::Open(Route::profile(profile.login, active.as_ref())),
                    window,
                    cx,
                );
            }
            // Issue/PR detail is deliberately not routed in this release.
            HomeAction::ActivateWorkItem(_)
            | HomeAction::ActivateFeed(_)
            | HomeAction::Refresh(_)
            | HomeAction::LoadMoreFeed(_) => {}
        }
    }

    fn index(column: Column) -> usize {
        match column {
            Column::Home => 0,
            Column::Feed => 1,
        }
    }

    fn start(&mut self, column: Column, initial: bool, cx: &mut Context<Self>) {
        let index = Self::index(column);
        if self.work[index].is_some() {
            return;
        }
        if column == Column::Feed && self.feed_ready.is_some_and(|ready| Instant::now() < ready) {
            return;
        }
        if column == Column::Feed {
            // A replacement Feed read invalidates any outstanding append: cancel it
            // and bump the append epoch so its late reply is rejected.
            if let Some(append) = self.feed_append.take() {
                append.source.cancel();
            }
            self.feed_append_epoch = self.feed_append_epoch.saturating_add(1);
        }
        let Some(session) = self.session.clone() else {
            return;
        };
        let Some(services) = self.services.clone() else {
            return;
        };
        let context = session.context().clone();
        if !services.repositories().is_current(&context) {
            self.session = None;
            self.unavailable(
                "The account or authorization changed. Home will read again in the current workspace.",
                cx,
            );
            return;
        }
        self.requests[index] = self.requests[index].saturating_add(1);
        let request = self.requests[index];
        let work = Work::new();
        let cancel = work.token.clone();
        self.work[index] = Some(work);
        let binding = self.binding.clone();
        let generation = self.generation;
        self.view.update(cx, |view, cx| {
            if initial {
                view.set_loading(column, &binding, generation, cx);
            }
            view.set_refresh_availability(
                column,
                &binding,
                generation,
                RefreshAvailability::unavailable("A read is already in flight."),
                cx,
            );
        });
        let inbox = self.inbox.clone();
        let alive = self.alive.clone();
        let selection = self.selection;
        let reply_context = context.clone();
        let submitted = services.executor().handle().spawn(async move {
            let result = match column {
                Column::Home => ColumnResult::Home(
                    session
                        .home(None, &HomeSearchLimits::default(), cancel)
                        .await
                        .map(|value| value.value),
                ),
                // The data layer composes the raw events read, the followed-account
                // read and the bounded merged-author lookups, then applies the
                // product policy. Bounds keep one snapshot from fanning out: a
                // followed list of at most 1,000 accounts and at most 30 distinct
                // author lookups per refresh.
                Column::Feed => ColumnResult::Feed(
                    session
                        .feed(
                            &FeedLimits::default(),
                            &FollowingLimits::default(),
                            &PullRequestDetailLimits::default(),
                            FEED_AUTHOR_LOOKUP_LIMIT,
                            cancel,
                        )
                        .await
                        .map(|value| value.value),
                ),
            };
            if *alive.borrow() {
                inbox.lock().expect("home inbox").push_back(Reply {
                    generation,
                    column,
                    request,
                    selection,
                    context: reply_context,
                    result,
                });
            }
        });
        if submitted.is_err() {
            self.work[index] = None;
            self.apply(
                Reply {
                    generation,
                    column,
                    request,
                    selection,
                    context,
                    result: match column {
                        Column::Home => ColumnResult::Home(Err(HomeError::InvalidResponse {
                            message: "Background services are closing.".into(),
                        })),
                        Column::Feed => ColumnResult::Feed(Err(HomeError::InvalidResponse {
                            message: "Background services are closing.".into(),
                        })),
                    },
                },
                cx,
            );
        }
    }

    fn start_receiver(&mut self, cx: &mut Context<Self>) {
        if cx.background_executor().is_test() {
            // Test scheduling is stepped explicitly by the caller's pump; a permanent 30ms
            // foreground timer would race the test's own clock advance.
            return;
        }
        self.receiver = Some(cx.spawn(async |this, cx| {
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
                let _ = this.update(cx, |page, cx| page.drain(cx));
            }
        }));
    }

    fn accepts(&self, reply: &Reply) -> bool {
        reply.generation == self.generation
            && reply.selection == self.selection
            && reply.request == self.requests[Self::index(reply.column)]
            && self
                .services
                .as_ref()
                .is_some_and(|services| services.repositories().is_current(&reply.context))
            && self
                .session
                .as_ref()
                .is_some_and(|session| session.context() == &reply.context)
    }

    fn drain(&mut self, cx: &mut Context<Self>) {
        let replies: Vec<_> = self.inbox.lock().expect("home inbox").drain(..).collect();
        for reply in replies {
            self.apply(reply, cx);
        }
        let appends: Vec<_> = self
            .append_inbox
            .lock()
            .expect("home append inbox")
            .drain(..)
            .collect();
        for append in appends {
            self.apply_append(append, cx);
        }
    }

    fn apply_append(&mut self, append: AppendReply, cx: &mut Context<Self>) {
        if append.generation != self.generation
            || append.append_epoch != self.feed_append_epoch
            || append.selection != self.selection
            || !self
                .services
                .as_ref()
                .is_some_and(|services| services.repositories().is_current(&append.context))
            || !self
                .session
                .as_ref()
                .is_some_and(|session| session.context() == &append.context)
        {
            return;
        }
        self.feed_append = None;
        let binding = self.binding.clone();
        let generation = self.generation;
        match append.result {
            Ok((page, seen)) => {
                self.feed_seen_ids = seen;
                self.feed_next_page = page.next_page;
                // Merge into the retained outcome in place, preserving the
                // earlier snapshot's partial reasons so an append can never make
                // a previously-withheld set look complete.
                self.view.update(cx, |view, cx| {
                    if let crate::ui::home::PanelState::Ready {
                        content: crate::ui::home::PanelContent::Ready(existing),
                        ..
                    } = view.feed_state()
                    {
                        let mut merged = existing.clone();
                        let mut reasons = merged.readiness.reasons().to_vec();
                        reasons.extend(page.readiness.reasons().iter().cloned());
                        merged.activities.extend(page.activities.clone());
                        merged.next_page = page.next_page;
                        merged.readiness =
                            crate::domain::home::FeedReadiness::from_reasons(reasons);
                        merged.etag = if merged.readiness.is_complete() {
                            page.etag
                        } else {
                            None
                        };
                        if let Some(poll) = page.poll_interval_seconds {
                            merged.poll_interval_seconds = Some(poll);
                        }
                        view.apply_feed_page(&binding, generation, merged, cx);
                    }
                });
            }
            Err(_error) => {
                let page = self.feed_next_page.map(|cursor| cursor.page).unwrap_or(1);
                self.view.update(cx, |view, cx| {
                    view.feed_append_failed(&binding, generation, page, cx);
                });
            }
        }
        cx.notify();
    }

    fn apply(&mut self, reply: Reply, cx: &mut Context<Self>) {
        if !self.accepts(&reply) {
            return;
        }
        self.work[Self::index(reply.column)] = None;
        let binding = self.binding.clone();
        let generation = self.generation;
        let column = reply.column;
        self.view.update(cx, |view, cx| match reply.result {
            ColumnResult::Home(Ok(outcome)) => {
                view.apply_home(&binding, generation, outcome, cx);
            }
            ColumnResult::Home(Err(error)) => {
                view.apply_error(column, &binding, generation, error, cx);
            }
            ColumnResult::Feed(Ok(outcome)) => {
                if let Some(seconds) = outcome.poll_interval_seconds {
                    self.feed_ready = Some(Instant::now() + Duration::from_secs(seconds.max(1)));
                } else {
                    self.feed_ready = None;
                }
                // Seed continuation from the first page: the validated next
                // source cursor and the seen event ids become the append
                // baseline, scoped to this generation. Replace (not extend) the
                // seen set so a refresh can re-reveal previously viewed older
                // activities that are no longer in the new first-page snapshot.
                self.feed_next_page = outcome.next_page;
                self.feed_seen_ids.clear();
                for activity in &outcome.activities {
                    self.feed_seen_ids.insert(activity.event.id.clone());
                }
                view.apply_feed(&binding, generation, outcome, cx);
            }
            ColumnResult::Feed(Err(error)) => {
                self.feed_ready = None;
                view.apply_error(column, &binding, generation, error, cx);
            }
        });
        let reason = match column {
            Column::Feed => self.feed_ready.and_then(|ready| {
                ready.checked_duration_since(Instant::now()).map(|remaining| {
                    format!(
                        "GitHub asked Feed clients to wait about {} more second(s) before polling.",
                        remaining.as_secs().max(1)
                    )
                })
            }),
            Column::Home => None,
        };
        self.view.update(cx, |view, cx| {
            view.set_refresh_availability(
                column,
                &binding,
                generation,
                match reason {
                    Some(reason) => RefreshAvailability::unavailable(reason),
                    None => RefreshAvailability::available(),
                },
                cx,
            );
        });
        if column == Column::Feed
            && let Some(ready) = self.feed_ready
        {
            self.schedule_feed_cooldown(ready, generation, cx);
        }
        cx.notify();
    }

    /// Re-enable only the Feed refresh affordance when its server-requested poll interval ends.
    /// This schedules no read and does not touch either snapshot.
    fn schedule_feed_cooldown(&mut self, ready: Instant, generation: u64, cx: &mut Context<Self>) {
        self.feed_cooldown_task = None;
        self.feed_cooldown_task = Some(cx.spawn(async move |this, cx| {
            let wait = ready.saturating_duration_since(Instant::now());
            if !wait.is_zero() {
                cx.background_executor().timer(wait).await;
            }
            let _ = this.update(cx, |page, cx| {
                if page.generation != generation
                    || !cooldown_expired(page.feed_ready, Instant::now())
                {
                    return;
                }
                page.feed_ready = None;
                page.feed_cooldown_task = None;
                let binding = page.binding.clone();
                let generation = page.generation;
                page.view.update(cx, |view, cx| {
                    view.set_refresh_availability(
                        Column::Feed,
                        &binding,
                        generation,
                        RefreshAvailability::available(),
                        cx,
                    );
                });
                cx.notify();
            });
        }));
    }

    #[cfg(test)]
    pub(crate) fn refresh_for_test(
        &mut self,
        column: Column,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.view
            .update(cx, |view, cx| view.refresh(column, window, cx));
    }

    #[cfg(test)]
    pub(crate) fn load_more_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.view
            .update(cx, |view, cx| view.request_more_feed(window, cx));
    }

    #[cfg(test)]
    pub(crate) fn expire_feed_cooldown_for_test(&mut self, cx: &mut Context<Self>) {
        let Some(ready) = self.feed_ready else {
            return;
        };
        if !cooldown_expired(Some(ready), Instant::now()) {
            // Drive the same expiry branch without waiting for a real clock tick.
            self.feed_ready = Some(Instant::now() - Duration::from_millis(1));
        }
        let generation = self.generation;
        let binding = self.binding.clone();
        if self.generation != generation || !cooldown_expired(self.feed_ready, Instant::now()) {
            return;
        }
        self.feed_ready = None;
        self.feed_cooldown_task = None;
        self.view.update(cx, |view, cx| {
            view.set_refresh_availability(
                Column::Feed,
                &binding,
                generation,
                RefreshAvailability::available(),
                cx,
            );
        });
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn feed_refresh_available_for_test(&self, cx: &gpui::App) -> bool {
        let view = self.view.read(cx);
        matches!(view.feed_state(), crate::ui::home::PanelState::Ready { .. })
            && view.feed_refresh_available()
    }

    #[cfg(test)]
    pub(crate) fn is_ready_for_test(&self, cx: &gpui::App) -> bool {
        let view = self.view.read(cx);
        matches!(view.home_state(), crate::ui::home::PanelState::Ready { .. })
            && matches!(view.feed_state(), crate::ui::home::PanelState::Ready { .. })
    }

    #[cfg(test)]
    pub(crate) fn drain_for_test(&mut self, cx: &mut Context<Self>) {
        self.drain(cx);
    }

    fn stop(&mut self) {
        for work in &mut self.work {
            if let Some(work) = work.take() {
                work.source.cancel();
            }
        }
        if let Some(append) = self.feed_append.take() {
            append.source.cancel();
        }
        self.feed_append_epoch = self.feed_append_epoch.saturating_add(1);
        self.inbox.lock().expect("home inbox").clear();
        self.append_inbox.lock().expect("home append inbox").clear();
        self.feed_cooldown_task = None;
        self.feed_ready = None;
        self.feed_next_page = None;
        self.feed_seen_ids.clear();
        self.session = None;
    }

    /// Starts one bounded load-more of the Feed's next older source page.
    ///
    /// At most one append runs per account/generation: the in-flight guard plus
    /// the view's loading state prevent duplicates from repeated scroll/resize
    /// callbacks. The request uses the same captured session and account context
    /// as the first read, so account switches and refresh reject stale replies.
    fn load_more_feed(&mut self, request: crate::ui::home::RefreshRequest, cx: &mut Context<Self>) {
        if self.feed_append.is_some() {
            return;
        }
        let Some(next) = self.feed_next_page else {
            return;
        };
        let Some(session) = self.session.clone() else {
            return;
        };
        let Some(services) = self.services.clone() else {
            return;
        };
        let context = session.context().clone();
        if !services.repositories().is_current(&context) {
            self.feed_append_failed(request.generation, next.page, cx);
            return;
        }
        let work = Work::new();
        let cancel = work.token.clone();
        self.feed_append = Some(work);
        let binding = self.binding.clone();
        let generation = self.generation;
        self.view.update(cx, |view, cx| {
            view.feed_append_loading(&binding, generation, cx);
        });
        let append_inbox = self.append_inbox.clone();
        let alive = self.alive.clone();
        let selection = self.selection;
        let append_epoch = self.feed_append_epoch;
        let reply_context = context.clone();
        let mut seen = self.feed_seen_ids.clone();
        let submitted = services.executor().handle().spawn(async move {
            let result = session
                .feed_page(
                    next,
                    &FeedLimits::default(),
                    &FollowingLimits::default(),
                    &PullRequestDetailLimits::default(),
                    FEED_AUTHOR_LOOKUP_LIMIT,
                    &mut seen,
                    cancel,
                )
                .await;
            if *alive.borrow() {
                append_inbox
                    .lock()
                    .expect("home append inbox")
                    .push_back(AppendReply {
                        generation,
                        append_epoch,
                        selection,
                        context: reply_context,
                        result: result.map(|page| {
                            (
                                FeedOutcome {
                                    activities: page.value.activities,
                                    etag: page.value.etag,
                                    poll_interval_seconds: page.value.poll_interval_seconds,
                                    next_page: page.value.next_page,
                                    readiness: crate::domain::home::FeedReadiness::from_reasons(
                                        page.value.reasons,
                                    ),
                                },
                                seen,
                            )
                        }),
                    });
            }
        });
        if submitted.is_err() {
            self.feed_append = None;
            self.feed_append_failed(generation, next.page, cx);
        }
    }

    fn feed_append_failed(&mut self, generation: u64, page: u32, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }
        self.feed_append = None;
        let binding = self.binding.clone();
        self.view.update(cx, |view, cx| {
            view.feed_append_failed(&binding, generation, page, cx);
        });
        cx.notify();
    }
}

impl Render for HomePage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.view.clone()
    }
}

impl Drop for HomePage {
    fn drop(&mut self) {
        self.alive.send_replace(false);
        self.stop();
    }
}
