use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{AppContext, Context, Entity, IntoElement, Render, Subscription, Window};
use tokio::sync::watch;

use crate::app::accounts::AccountServices;
use crate::app::avatar::AvatarKey;
use crate::data::page_cache::{
    CachedPageReadSession, PageCacheError, PageOrigin, PageReadMode, PageValue,
};
use crate::data::reader_images::RepositoryImageService;
use crate::data::{
    AccountCache, PageReadSession, ProfileReadme, ProfileReadmeIneligible, RepositoryReadContext,
};
use crate::domain::blob::BlobLimits;
use crate::domain::cache::CacheMiss;
use crate::domain::error::GitHubError;
use crate::domain::profile::{
    ContributionCalendar, ContributionRange, ProfileBasic, ProfileError, ProfilePinned,
};
use crate::domain::repository::{RepositoryError, RepositoryIdentity};
use crate::github::RepositoryLimits;
use crate::github::profile::ProfileLimits;
use crate::network::cancel::{CancellationToken, CancellationTokenSource};
use crate::network::reader_images::{ExternalImageBytes, ReqwestExternalImages};
use crate::storage::cache::{CacheError, CacheKey, CacheLease, CacheRead};
use crate::ui::profile::{
    PresentationError, ProfileAction, ProfileActionKind, ProfileBinding, ProfileProvenance,
    ProfileRequest, ProfileTarget, ProfileView, SectionState,
};
use crate::ui::reader::{ImageLimits, ImageProvider, LinkTarget, ReaderAction};
use crate::workspace::{Route, TabId};

use super::{NavigationSink, PageAvatars, PageNavigation};

/// The last rolling inclusive UTC year. GitHub accepts at most 366 days between endpoints; a
/// leap-year interval therefore moves the start forward one day instead of requesting an invalid
/// 367-day inclusive range.
pub fn rolling_year_at(now: SystemTime) -> ContributionRange {
    let seconds = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
    let days = seconds.div_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let to = format!("{year:04}-{month:02}-{day:02}");
    let start_days = days - 365;
    let (start_year, start_month, start_day) = civil_from_days(start_days);
    let mut from = format!("{start_year:04}-{start_month:02}-{start_day:02}");
    if ContributionRange::new(from.clone(), to.clone()).is_err() {
        let (year, month, day) = civil_from_days(start_days + 1);
        from = format!("{year:04}-{month:02}-{day:02}");
    }
    ContributionRange::new(from, to).expect("one rolling year is a valid contribution range")
}

pub fn rolling_year() -> ContributionRange {
    rolling_year_at(SystemTime::now())
}

// Howard Hinnant's civil_from_days, the inverse of the existing domain date conversion.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    if month <= 2 {
        year += 1;
    }
    (year, month, day)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    Basic,
    Repositories,
    Contributions,
    Readme,
}

impl Section {
    fn index(self) -> usize {
        match self {
            Self::Basic => 0,
            Self::Repositories => 1,
            Self::Contributions => 2,
            Self::Readme => 3,
        }
    }
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

/// Monotonic token proving the aggregate (`repository = None`) cache gate has not been
/// revoked, replaced, rejected or retired since the historical snapshot was read.
///
/// The retained [`CacheLease`] pins the stamp entry for as long as the page binding depends on it,
/// so ordinary capacity eviction cannot make a still-authorized page look revoked. Revocation,
/// PAT replacement, credential rejection and retirement delete the entry, which invalidates the
/// lease's snapshot and therefore the stamp.
#[derive(Clone)]
struct AggregateGateStamp {
    partition: String,
    stamp_id: String,
    lease: Arc<CacheLease>,
}

impl std::fmt::Debug for AggregateGateStamp {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AggregateGateStamp")
            .field("partition", &self.partition)
            .field("stamp_id", &self.stamp_id)
            .finish_non_exhaustive()
    }
}

impl PartialEq for AggregateGateStamp {
    fn eq(&self, other: &Self) -> bool {
        self.partition == other.partition && self.stamp_id == other.stamp_id
    }
}

impl Eq for AggregateGateStamp {}

struct SectionOutcome<T> {
    value: T,
    provenance: Option<ProfileProvenance>,
    gate_stamp: Option<AggregateGateStamp>,
}

enum SectionResult {
    Basic(Result<SectionOutcome<ProfileBasic>, PresentationError>),
    Repositories(Result<SectionOutcome<ProfilePinned>, PresentationError>),
    /// The calendar and the interval it was actually read at (an offline snapshot may have been
    /// stored under an earlier rolling year).
    Contributions(
        Result<(SectionOutcome<ContributionCalendar>, ContributionRange), PresentationError>,
    ),
    Readme(Result<SectionOutcome<ProfileReadme>, PresentationError>),
}

struct Reply {
    section: Section,
    generation: u64,
    request: u64,
    selection: u64,
    context: RepositoryReadContext,
    login: String,
    result: SectionResult,
}

pub struct ProfilePage {
    tab: TabId,
    login: String,
    services: Option<AccountServices>,
    session: Option<PageReadSession>,
    cached_session: Option<CachedPageReadSession>,
    external_images: Option<Arc<dyn ExternalImageBytes>>,
    image_limits: ImageLimits,
    selection: u64,
    generation: u64,
    requests: [u64; 4],
    work: [Option<Work>; 4],
    cache_monitor: Option<Work>,
    historical_stamps: Arc<Mutex<[Option<AggregateGateStamp>; 4]>>,
    inbox: Arc<Mutex<VecDeque<Reply>>>,
    cache_invalidated: Arc<std::sync::atomic::AtomicBool>,
    alive: Arc<watch::Sender<bool>>,
    receiver: Option<gpui::Task<()>>,
    view: Option<Entity<ProfileView>>,
    request: Option<ProfileRequest>,
    range: ContributionRange,
    avatars: PageAvatars,
    navigation: NavigationSink,
    active: bool,
    unavailable: Option<PresentationError>,
    _changed: Option<Subscription>,
    _avatars_changed: Option<Subscription>,
}

impl ProfilePage {
    pub fn new(
        tab: TabId,
        login: String,
        services: Option<AccountServices>,
        navigation: NavigationSink,
        avatars: PageAvatars,
        cx: &mut Context<Self>,
    ) -> Self {
        let (alive, _) = watch::channel(true);
        let external_images = ReqwestExternalImages::new()
            .ok()
            .map(|client| Arc::new(client) as Arc<dyn ExternalImageBytes>);
        let mut this = Self {
            tab,
            login,
            services: None,
            session: None,
            cached_session: None,
            external_images,
            image_limits: ImageLimits::default(),
            selection: 0,
            generation: 0,
            requests: [0; 4],
            work: [None, None, None, None],
            cache_monitor: None,
            historical_stamps: Arc::new(Mutex::new([None, None, None, None])),
            inbox: Arc::new(Mutex::new(VecDeque::new())),
            cache_invalidated: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            alive: Arc::new(alive),
            receiver: None,
            view: None,
            request: None,
            range: rolling_year(),
            avatars,
            navigation,
            active: true,
            unavailable: None,
            _changed: None,
            _avatars_changed: None,
        };
        this.start_receiver(cx);
        this.rebind(services, cx);
        this
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.active = active;
        if !active && let Some(view) = &self.view {
            view.update(cx, |view, cx| view.suspend(cx));
        }
    }

    pub fn rebind(&mut self, services: Option<AccountServices>, cx: &mut Context<Self>) {
        if let Some(view) = &self.view {
            view.update(cx, |view, cx| view.clear_reader(cx));
        }
        self.stop();
        self.services = services;
        self.selection = self.services.as_ref().map_or(0, AccountServices::selection);
        self.generation = self.generation.saturating_add(1);
        self.requests = [0; 4];
        self.request = None;
        self.unavailable = None;
        self.cache_invalidated
            .store(false, std::sync::atomic::Ordering::Release);
        *self
            .historical_stamps
            .lock()
            .expect("profile historical stamps") = [None, None, None, None];
        self._avatars_changed = None;
        self.view = None;
        self._changed = None;
        let Some(services) = self.services.clone() else {
            cx.notify();
            return;
        };
        match services.accounts().page_session() {
            Ok(session) if services.repositories().is_current(session.context()) => {
                self.cached_session =
                    services
                        .repositories()
                        .cache(session.context())
                        .and_then(|cache| {
                            let (store, scope) = cache.into_parts();
                            CachedPageReadSession::new(session.clone(), store, scope).ok()
                        });
                self.session = Some(session);
                self.ensure_cache_monitor(&services);
                self.start(Section::Basic, cx);
            }
            _ => {}
        }
        cx.notify();
    }

    fn placeholder(&self, message: &str, cx: &mut Context<Self>) -> gpui::AnyElement {
        use ely_core::theme::{ActiveTheme, TextSize};
        use gpui::{IntoElement, ParentElement, Styled, div, prelude::*};
        div()
            .debug_selector(|| "profile-page-unavailable".to_string())
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p_6()
            .text_align(gpui::TextAlign::Center)
            .text_size(cx.theme().text_size(TextSize::Sm))
            .text_color(cx.theme().colors.fg_subtle)
            .child(message.to_string())
            .into_any_element()
    }

    fn error_placeholder(
        &self,
        error: &PresentationError,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        use ely_core::theme::{ActiveTheme, TextSize};
        use gpui::{IntoElement, ParentElement, Styled, div, prelude::*};
        let message = error.message();
        let weak = cx.weak_entity();
        let generation = self.generation;
        let mark = crate::ui::compact_status::failed(
            "profile-page-basic-error",
            "Profile could not be loaded",
            message,
            Some(move |_: &mut Window, cx: &mut gpui::App| {
                let _ = weak.update(cx, |page, cx| {
                    if page.generation == generation {
                        page.retry_basic(cx);
                    }
                });
            }),
            Some("profile-page-basic-error"),
            cx,
        );
        div()
            .debug_selector(|| "profile-page-unavailable".to_string())
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .gap_2()
            .p_6()
            .text_align(gpui::TextAlign::Center)
            .text_size(cx.theme().text_size(TextSize::Sm))
            .text_color(cx.theme().colors.warning)
            .child(mark)
            .child(message.to_string())
            .into_any_element()
    }

    fn retry_basic(&mut self, cx: &mut Context<Self>) {
        if self.work[Section::Basic.index()].is_some() {
            return;
        }
        self.unavailable = None;
        if let (Some(request), Some(view)) = (self.request.clone(), self.view.clone()) {
            view.update(cx, |view, cx| {
                view.update_basic(&request, SectionState::Loading, cx);
            });
        }
        self.start(Section::Basic, cx);
        cx.notify();
    }

    fn retry_section(&mut self, section: Section, window: &mut Window, cx: &mut Context<Self>) {
        if self.work[section.index()].is_some() {
            return;
        }
        let (Some(request), Some(view)) = (self.request.clone(), self.view.clone()) else {
            if section == Section::Basic {
                self.retry_basic(cx);
            }
            return;
        };
        match section {
            Section::Basic => {
                self.retry_basic(cx);
                return;
            }
            Section::Repositories => {
                view.update(cx, |view, cx| {
                    view.update_repositories(&request, SectionState::Loading, cx);
                });
            }
            Section::Contributions => {
                let range = self.range.clone();
                view.update(cx, |view, cx| {
                    view.update_contributions(&request, &range, SectionState::Loading, cx);
                });
            }
            Section::Readme => {
                view.update(cx, |view, cx| {
                    view.update_readme(&request, SectionState::Loading, window, cx);
                });
            }
        }
        self.start(section, cx);
        cx.notify();
    }

    fn create_view(
        &mut self,
        basic: &ProfileBasic,
        context: &RepositoryReadContext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = ProfileTarget::new(context.auth().account.host.clone(), basic.id);
        let binding = ProfileBinding::new(context.auth().account.clone(), context.auth().epoch);
        let view = cx.new(|cx| ProfileView::new(target, binding, self.range.clone(), window, cx));
        let weak = cx.weak_entity();
        view.update(cx, |view, _| {
            view.on_action(Rc::new(move |action, window, cx| {
                // ProfileView is still being updated while it emits this action. Navigation or a
                // reader link that re-enters the same host would panic, so dispatch after the
                // component update completes.
                let weak = weak.clone();
                window.defer(cx, move |window, cx| {
                    let _ = weak.update(cx, |page, cx| page.action(action, window, cx));
                });
            }));
        });
        let request = view.read(cx).request();
        self.request = Some(request);
        self._changed = Some(cx.observe(&view, |_, _, cx| cx.notify()));
        self.view = Some(view);
        // The shared avatar cache notifies its controller when a decode lands. Reapply the header
        // image from that event, not from `render`, so a late picture updates the page once.
        self._avatars_changed = self.avatars.accounts().and_then(|accounts| {
            let entity = accounts.upgrade()?;
            Some(cx.observe(&entity, |page, _, cx| {
                if let Some(basic) =
                    page.view
                        .as_ref()
                        .and_then(|view| match view.read(cx).basic() {
                            SectionState::Ready(basic) => Some(basic.clone()),
                            _ => None,
                        })
                {
                    page.sync_avatar(basic, cx);
                }
            }))
        });
    }

    pub(super) fn action(
        &mut self,
        action: ProfileAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.request.as_ref() != Some(&action.request) {
            return;
        }
        match action.kind {
            ProfileActionKind::Profile => {
                let active = self
                    .services
                    .as_ref()
                    .and_then(|services| services.accounts().active_summary());
                (self.navigation)(
                    PageNavigation::Open(Route::profile(self.login.clone(), active.as_ref())),
                    window,
                    cx,
                );
            }
            ProfileActionKind::Repository(repository) => {
                if let Some((owner, name)) = repository.name_with_owner.split_once('/') {
                    (self.navigation)(
                        PageNavigation::Open(Route::Repository {
                            namespace: owner.to_string(),
                            name: name.to_string(),
                        }),
                        window,
                        cx,
                    );
                }
            }
            ProfileActionKind::ExternalLink { url } => {
                (self.navigation)(PageNavigation::External(url), window, cx)
            }
            ProfileActionKind::Readme(ReaderAction::Link(LinkTarget::External { url })) => {
                (self.navigation)(PageNavigation::External(url), window, cx)
            }
            ProfileActionKind::Readme(ReaderAction::Link(LinkTarget::Repository(link))) => {
                // Profile README repository-relative links open the real repository. The current
                // file selection stays README; deep-linking a fixed commit is not a new route.
                (self.navigation)(
                    PageNavigation::Open(Route::Repository {
                        namespace: link.owner,
                        name: link.name,
                    }),
                    window,
                    cx,
                );
            }
            ProfileActionKind::Readme(_) => {}
            ProfileActionKind::RetryBasic => {
                self.retry_basic(cx);
            }
            ProfileActionKind::RetryRepositories => {
                self.retry_section(Section::Repositories, window, cx);
            }
            ProfileActionKind::RetryContributions => {
                self.retry_section(Section::Contributions, window, cx);
            }
            ProfileActionKind::RetryReadme => {
                self.retry_section(Section::Readme, window, cx);
            }
        }
    }

    fn build_readme_image_provider(
        &self,
        repository: RepositoryIdentity,
        context: &RepositoryReadContext,
    ) -> Option<Arc<dyn ImageProvider>> {
        let services = self.services.as_ref()?;
        let external = self.external_images.clone()?;
        let session = services.repositories().session().ok()?;
        if session.context() != context || !services.repositories().is_current(context) {
            return None;
        }
        // Profile README comes only from a verified public same-name repository (`is_private = false`).
        let cache = services
            .repositories()
            .cache(context)
            .map(|account_cache: AccountCache| {
                let (store, scope) = account_cache.into_parts();
                (store, scope, false)
            });
        Some(Arc::new(RepositoryImageService::with_cache(
            session,
            repository,
            services.executor().clone(),
            external,
            self.image_limits.clone(),
            cache,
        )))
    }

    fn start(&mut self, section: Section, cx: &mut Context<Self>) {
        let index = section.index();
        if self.work[index].is_some() {
            return;
        }
        let (Some(session), Some(services)) = (self.session.clone(), self.services.clone()) else {
            return;
        };
        let context = session.context().clone();
        if !services.repositories().is_current(&context) {
            return;
        }
        // Re-check cache availability from RepoService::cache(context) before starting.
        let cached = services.repositories().cache(&context).and_then(|cache| {
            let (store, scope) = cache.into_parts();
            CachedPageReadSession::new(session.clone(), store, scope).ok()
        });
        self.cached_session = cached.clone();
        self.ensure_cache_monitor(&services);
        self.requests[index] = self.requests[index].saturating_add(1);
        let request = self.requests[index];
        let work = Work::new();
        let cancel = work.token.clone();
        self.work[index] = Some(work);
        let generation = self.generation;
        let selection = self.selection;
        let login = self.login.clone();
        let range = self.range.clone();
        let basic = self
            .view
            .as_ref()
            .and_then(|view| match view.read(cx).basic() {
                SectionState::Ready(basic) => Some(basic.clone()),
                _ => None,
            });
        let alive = self.alive.clone();
        let inbox = self.inbox.clone();
        let repos_for_stamp = services.repositories();
        let stamp_context = context.clone();
        let submitted = services.executor().handle().spawn(async move {
            // Prepare the aggregate gate stamp before any read, so a revocation that lands during
            // this read deletes it and the historical value can never be published.
            let stamp = prepare_gate_stamp(&repos_for_stamp, &stamp_context).await;
            let stamp = stamp.as_ref();
            let result = match section {
                Section::Basic => SectionResult::Basic(
                    read_profile_basic(
                        &repos_for_stamp,
                        &session,
                        cached.as_ref(),
                        stamp,
                        &login,
                        cancel,
                    )
                    .await,
                ),
                Section::Repositories => SectionResult::Repositories(
                    read_profile_pinned(
                        &repos_for_stamp,
                        &session,
                        cached.as_ref(),
                        stamp,
                        &login,
                        cancel,
                    )
                    .await,
                ),
                Section::Contributions => SectionResult::Contributions(
                    read_profile_contributions(
                        &repos_for_stamp,
                        &session,
                        cached.as_ref(),
                        stamp,
                        &login,
                        &range,
                        cancel,
                    )
                    .await,
                ),
                Section::Readme => SectionResult::Readme(match basic {
                    Some(basic) => {
                        read_profile_readme(
                            &repos_for_stamp,
                            &session,
                            cached.as_ref(),
                            stamp,
                            &basic,
                            cancel,
                        )
                        .await
                    }
                    None => Err(PresentationError::InvalidData),
                }),
            };
            if *alive.borrow() {
                inbox.lock().expect("profile inbox").push_back(Reply {
                    section,
                    generation,
                    request,
                    selection,
                    context,
                    login,
                    result,
                });
            }
        });
        if submitted.is_err() {
            self.work[index] = None;
        }
    }

    fn ensure_cache_monitor(&mut self, services: &AccountServices) {
        if self.cache_monitor.is_some() || self.cached_session.is_none() {
            return;
        }
        let Some(session) = self.session.clone() else {
            return;
        };
        let context = session.context().clone();
        let repos = services.repositories();
        let alive = self.alive.clone();
        let invalidated = self.cache_invalidated.clone();
        let stamps = self.historical_stamps.clone();
        let work = Work::new();
        let cancel = work.token.clone();
        self.cache_monitor = Some(work);
        let spawned = services.executor().handle().spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = cancel.clone().cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_millis(80)) => {}
                }
                if !*alive.borrow() || !repos.is_current(&context) || !session.verify() {
                    break;
                }
                let active_stamps: Vec<AggregateGateStamp> = stamps
                    .lock()
                    .expect("profile historical stamps")
                    .iter()
                    .flatten()
                    .cloned()
                    .collect();
                if active_stamps.is_empty() {
                    continue;
                }
                let Some(cache) = repos.cache(&context) else {
                    invalidated.store(true, std::sync::atomic::Ordering::Release);
                    break;
                };
                let (store, scope) = cache.into_parts();
                let valid = tokio::task::spawn_blocking(move || {
                    active_stamps
                        .iter()
                        .all(|stamp| verify_aggregate_gate_stamp(&store, &scope, stamp))
                })
                .await
                .unwrap_or(false);
                if !valid {
                    invalidated.store(true, std::sync::atomic::Ordering::Release);
                    break;
                }
            }
        });
        if spawned.is_err() {
            self.cache_monitor = None;
        }
    }

    fn start_receiver(&mut self, cx: &mut Context<Self>) {
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
                let _ = this.update_in(cx, |page, window, cx| page.drain(window, cx));
            }
        }));
    }

    fn drain(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.check_authorization(cx) {
            return;
        }
        let replies: Vec<_> = self
            .inbox
            .lock()
            .expect("profile inbox")
            .drain(..)
            .collect();
        for reply in replies {
            self.apply(reply, window, cx);
        }
    }

    fn check_authorization(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(session) = &self.session else {
            return false;
        };
        let Some(services) = &self.services else {
            return false;
        };
        if services.repositories().is_current(session.context()) && session.verify() {
            // If a cache binding was attached, ensure the cache gate has not been retired/suspended/rejected.
            let gate_lost = self
                .cache_invalidated
                .load(std::sync::atomic::Ordering::Acquire)
                || (self.cached_session.is_some()
                    && services.repositories().cache(session.context()).is_none());
            if gate_lost {
                self.cached_session = None;
                if let Some(view) = &self.view {
                    let any_historical = view
                        .read(cx)
                        .basic_provenance()
                        .is_some_and(ProfileProvenance::is_historical)
                        || view
                            .read(cx)
                            .repositories_provenance()
                            .is_some_and(ProfileProvenance::is_historical)
                        || view
                            .read(cx)
                            .contributions_provenance()
                            .is_some_and(ProfileProvenance::is_historical)
                        || view
                            .read(cx)
                            .readme_provenance()
                            .is_some_and(ProfileProvenance::is_historical);
                    if any_historical {
                        view.update(cx, |view, cx| {
                            view.clear_reader(cx);
                            view.suspend(cx);
                        });
                        *self
                            .historical_stamps
                            .lock()
                            .expect("profile historical stamps") = [None, None, None, None];
                        self.view = None;
                        self.request = None;
                        self.unavailable = Some(PresentationError::Forbidden);
                        cx.notify();
                        return false;
                    }
                }
            }
            return true;
        }
        if let Some(view) = &self.view {
            view.update(cx, |view, cx| {
                view.clear_reader(cx);
                view.suspend(cx);
            });
        }
        self.stop();
        self.view = None;
        self.request = None;
        self.unavailable = None;
        cx.notify();
        false
    }

    fn accepts(&self, reply: &Reply, has_historical_cache: bool) -> bool {
        if reply.generation != self.generation
            || reply.selection != self.selection
            || reply.request != self.requests[reply.section.index()]
            || reply.login != self.login
        {
            return false;
        }
        let Some(services) = &self.services else {
            return false;
        };
        if !services.repositories().is_current(&reply.context) {
            return false;
        }
        if !self
            .session
            .as_ref()
            .is_some_and(|session| session.context() == &reply.context && session.verify())
        {
            return false;
        }
        if has_historical_cache
            && (self
                .cache_invalidated
                .load(std::sync::atomic::Ordering::Acquire)
                || services.repositories().cache(&reply.context).is_none())
        {
            return false;
        }
        true
    }

    fn apply(&mut self, reply: Reply, window: &mut Window, cx: &mut Context<Self>) {
        let (is_historical, gate_stamp) = match &reply.result {
            SectionResult::Basic(Ok(outcome)) => (
                outcome
                    .provenance
                    .as_ref()
                    .is_some_and(ProfileProvenance::is_historical),
                outcome.gate_stamp.clone(),
            ),
            SectionResult::Repositories(Ok(outcome)) => (
                outcome
                    .provenance
                    .as_ref()
                    .is_some_and(ProfileProvenance::is_historical),
                outcome.gate_stamp.clone(),
            ),
            SectionResult::Contributions(Ok((outcome, _))) => (
                outcome
                    .provenance
                    .as_ref()
                    .is_some_and(ProfileProvenance::is_historical),
                outcome.gate_stamp.clone(),
            ),
            SectionResult::Readme(Ok(outcome)) => (
                outcome
                    .provenance
                    .as_ref()
                    .is_some_and(ProfileProvenance::is_historical),
                outcome.gate_stamp.clone(),
            ),
            _ => (false, None),
        };
        if !self.accepts(&reply, is_historical) {
            return;
        }
        self.work[reply.section.index()] = None;
        self.historical_stamps
            .lock()
            .expect("profile historical stamps")[reply.section.index()] =
            if is_historical { gate_stamp } else { None };
        match reply.section {
            Section::Basic => self.apply_basic(reply, window, cx),
            section => {
                let Some(request) = self.request.clone() else {
                    return;
                };
                let Some(view) = self.view.clone() else {
                    return;
                };
                match section {
                    Section::Repositories => {
                        let SectionResult::Repositories(result) = reply.result else {
                            return;
                        };
                        match result {
                            Ok(outcome) => {
                                view.update(cx, |view, cx| {
                                    view.update_repositories_with_provenance(
                                        &request,
                                        SectionState::Ready(outcome.value),
                                        outcome.provenance,
                                        cx,
                                    );
                                });
                            }
                            Err(error) => {
                                view.update(cx, |view, cx| {
                                    view.update_repositories(
                                        &request,
                                        SectionState::Error(error),
                                        cx,
                                    );
                                });
                            }
                        }
                    }
                    Section::Contributions => {
                        let SectionResult::Contributions(result) = reply.result else {
                            return;
                        };
                        match result {
                            Ok((outcome, range)) => {
                                // An offline snapshot can belong to an earlier rolling year; adopt
                                // the interval it was stored under so the section, the tooltip and
                                // the visible range stay consistent with the restored data.
                                if range != self.range {
                                    self.range = range.clone();
                                    view.update(cx, |view, cx| {
                                        view.set_range(range.clone(), cx);
                                    });
                                }
                                view.update(cx, |view, cx| {
                                    view.update_contributions_with_provenance(
                                        &request,
                                        &range,
                                        SectionState::Ready(outcome.value),
                                        outcome.provenance,
                                        cx,
                                    );
                                });
                            }
                            Err(error) => {
                                let range = self.range.clone();
                                view.update(cx, |view, cx| {
                                    view.update_contributions(
                                        &request,
                                        &range,
                                        SectionState::Error(error),
                                        cx,
                                    );
                                });
                            }
                        }
                    }
                    Section::Readme => {
                        let SectionResult::Readme(result) = reply.result else {
                            return;
                        };
                        match result {
                            Ok(SectionOutcome {
                                value:
                                    ProfileReadme::Prepared {
                                        repository,
                                        content,
                                        ..
                                    },
                                provenance,
                                ..
                            }) => {
                                let provider =
                                    self.build_readme_image_provider(repository, &reply.context);
                                view.update(cx, |view, cx| {
                                    view.update_readme_with_provider(
                                        &request,
                                        SectionState::Ready(content),
                                        provider,
                                        provenance,
                                        window,
                                        cx,
                                    );
                                });
                            }
                            Ok(SectionOutcome {
                                value: ProfileReadme::Missing { .. },
                                provenance,
                                ..
                            }) => {
                                view.update(cx, |view, cx| {
                                    view.set_readme_unavailable_with_provenance(
                                        &request,
                                        "No root README.md exists in this user's public same-name repository.",
                                        provenance,
                                        cx,
                                    );
                                });
                            }
                            Ok(SectionOutcome {
                                value: ProfileReadme::Unavailable { .. },
                                provenance,
                                ..
                            }) => {
                                view.update(cx, |view, cx| {
                                    view.set_readme_unavailable_with_provenance(
                                        &request,
                                        "The root README could not be confirmed from an incomplete repository tree.",
                                        provenance,
                                        cx,
                                    );
                                });
                            }
                            Ok(SectionOutcome {
                                value: ProfileReadme::NotEligible { reason, .. },
                                provenance,
                                ..
                            }) => {
                                let message = match reason {
                                    ProfileReadmeIneligible::NotPublicSameNameRepository => {
                                        "This user has no visible public same-name profile repository."
                                    }
                                    ProfileReadmeIneligible::NoDefaultBranch => {
                                        "The public same-name profile repository has no default branch."
                                    }
                                };
                                view.update(cx, |view, cx| {
                                    view.set_readme_unavailable_with_provenance(
                                        &request, message, provenance, cx,
                                    );
                                });
                            }
                            Err(error) => {
                                // A 404 from the same-name repository (or its fixed tree/blob)
                                // lookup is how GitHub reports an absent or not-visible profile
                                // repository: it hides missing and private resources the same
                                // way. That is a quiet optional absence, so it must not surface
                                // the generic "not found" error sentence. Real network, auth,
                                // rate-limit, cache-miss and parse failures keep their typed error
                                // and the compact warning mark.
                                if error == PresentationError::NotFound {
                                    view.update(cx, |view, cx| {
                                        view.set_readme_unavailable(
                                            &request,
                                            "No visible public same-name profile repository or root README.",
                                            cx,
                                        );
                                    });
                                } else {
                                    view.update(cx, |view, cx| {
                                        view.update_readme_with_context(
                                            &request,
                                            SectionState::Error(error),
                                            crate::ui::reader::ResourceContext::new(
                                                &reply.login,
                                                &reply.login,
                                                "0".repeat(40),
                                                "README.md",
                                            ),
                                            window,
                                            cx,
                                        );
                                    });
                                }
                            }
                        }
                    }
                    Section::Basic => unreachable!(),
                }
            }
        }
        cx.notify();
    }

    fn apply_basic(&mut self, reply: Reply, window: &mut Window, cx: &mut Context<Self>) {
        let SectionResult::Basic(result) = reply.result else {
            return;
        };
        #[cfg(test)]
        eprintln!("apply_basic {:?}", result.as_ref().err());
        match result {
            Ok(SectionOutcome {
                value: basic,
                provenance,
                gate_stamp,
            }) => {
                // Reset sibling section stamps when a new Basic profile creates a new view.
                let mut stamps = self
                    .historical_stamps
                    .lock()
                    .expect("profile historical stamps");
                *stamps = [
                    provenance
                        .as_ref()
                        .is_some_and(ProfileProvenance::is_historical)
                        .then_some(gate_stamp)
                        .flatten(),
                    None,
                    None,
                    None,
                ];
                drop(stamps);
                // Server id/login are authoritative. An earlier login-only action is never used as
                // a numeric identity; Shell canonicalizes the matching TabId after this callback.
                self.login = basic.login.clone();
                self.create_view(&basic, &reply.context, window, cx);
                let canonical = PageNavigation::CanonicalProfile {
                    tab: self.tab,
                    login: basic.login.clone(),
                };
                let request = self.request.clone().expect("view request");
                if let Some(view) = self.view.clone() {
                    let state = SectionState::Ready(basic.clone());
                    view.update(cx, |view, cx| {
                        view.update_basic_with_provenance(&request, state, provenance, cx);
                    });
                    self.sync_avatar(basic, cx);
                }
                for section in [
                    Section::Repositories,
                    Section::Contributions,
                    Section::Readme,
                ] {
                    self.start(section, cx);
                }
                // Shell is the outer mutator while it drives this page's update, so it cannot be
                // re-entered synchronously. The canonical rename is delivered after this frame.
                let navigation = self.navigation.clone();
                window.defer(cx, move |window, cx| {
                    navigation(canonical, window, cx);
                });
            }
            Err(error) => {
                self.view = None;
                self.request = None;
                self.unavailable = Some(error);
            }
        }
    }

    fn sync_avatar(&mut self, basic: ProfileBasic, cx: &mut Context<Self>) {
        let (Some(request), Some(view), Some(url)) =
            (self.request.clone(), self.view.clone(), basic.avatar_url)
        else {
            return;
        };
        let key = AvatarKey::new("github.com", basic.id, url);
        let Some(image) = self.avatars.image(key, cx) else {
            // Loading or terminal failure: the component keeps its stable initials. Do not publish
            // a transient None on every frame.
            return;
        };
        view.update(cx, |view, cx| {
            view.set_avatar(&request, Some(image), cx);
        });
    }

    #[cfg(test)]
    pub(crate) fn set_external_images_for_test(&mut self, external: Arc<dyn ExternalImageBytes>) {
        self.external_images = Some(external);
    }

    #[cfg(test)]
    pub(crate) fn range_for_test(&self) -> &ContributionRange {
        &self.range
    }

    /// Replaces the host's contribution interval, as a later calendar day would.
    #[cfg(test)]
    pub(crate) fn set_range_for_test(&mut self, range: ContributionRange) {
        self.range = range;
    }

    #[cfg(test)]
    pub(crate) fn view_for_test(&self) -> Option<&Entity<ProfileView>> {
        self.view.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn target_for_test(&self, cx: &gpui::App) -> Option<(crate::domain::HostId, u64)> {
        let view = self.view.as_ref()?;
        let target = view.read(cx).target().clone();
        Some((target.host, target.id))
    }

    #[cfg(test)]
    pub(crate) fn drain_for_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.drain(window, cx);
    }

    fn stop(&mut self) {
        for work in &mut self.work {
            if let Some(work) = work.take() {
                work.source.cancel();
            }
        }
        if let Some(monitor) = self.cache_monitor.take() {
            monitor.source.cancel();
        }
        *self
            .historical_stamps
            .lock()
            .expect("profile historical stamps") = [None, None, None, None];
        self.inbox.lock().expect("profile inbox").clear();
        self.session = None;
        self.cached_session = None;
    }
}

fn should_fallback_profile(error: &ProfileError) -> bool {
    match error {
        ProfileError::GitHub(
            GitHubError::Network { .. }
            | GitHubError::Timeout { .. }
            | GitHubError::RateLimited { .. },
        ) => true,
        ProfileError::GitHub(GitHubError::UnexpectedStatus { status }) => *status >= 500,
        ProfileError::Graphql { kind, .. } => matches!(
            kind,
            crate::domain::GraphqlErrorKind::RateLimitPrimary
                | crate::domain::GraphqlErrorKind::RateLimitSecondary
        ),
        _ => false,
    }
}

fn should_fallback_readme(error: &RepositoryError) -> bool {
    match error {
        RepositoryError::GitHub(
            GitHubError::Network { .. }
            | GitHubError::Timeout { .. }
            | GitHubError::RateLimited { .. },
        ) => true,
        RepositoryError::GitHub(GitHubError::UnexpectedStatus { status }) => *status >= 500,
        _ => false,
    }
}

fn map_cache_error(error: CacheError) -> PresentationError {
    match error {
        CacheError::AccessDenied => PresentationError::Forbidden,
        CacheError::Stale => PresentationError::Unauthorized,
        CacheError::Corrupt => PresentationError::CacheCorrupt,
        _ => PresentationError::Unavailable,
    }
}

fn aggregate_gate_stamp_key() -> Result<CacheKey, CacheError> {
    CacheKey::mutable(
        None,
        true,
        "profile-aggregate-gate-stamp",
        "v1",
        "profile-aggregate-gate-stamp-v1",
        1,
    )
}

fn read_aggregate_gate_stamp(
    store: &crate::storage::cache::CacheStore,
    scope: &crate::storage::cache::CacheScope,
) -> Option<AggregateGateStamp> {
    let key = aggregate_gate_stamp_key().ok()?;
    let CacheRead::Hit(lease) = store.read(scope, &key).ok()? else {
        return None;
    };
    let snapshot = lease.snapshot().ok()??;
    let stamp_id = std::str::from_utf8(&snapshot.bytes).ok()?;
    if stamp_id.is_empty() {
        return None;
    }
    Some(AggregateGateStamp {
        partition: scope.partition().to_string(),
        stamp_id: stamp_id.to_string(),
        lease: Arc::new(lease),
    })
}

/// Serializes stamp creation inside this process. The store already holds an exclusive file lease,
/// so a second process cannot write the same partition; this lock only keeps concurrent section
/// reads from publishing different stamp values for one page binding.
static GATE_STAMP_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Ensures the aggregate gate stamp exists, converging on one value when several section reads
/// prepare it concurrently: a writer that loses the slot race re-reads the winner's stamp instead
/// of failing the section.
fn ensure_aggregate_gate_stamp(
    store: &crate::storage::cache::CacheStore,
    scope: &crate::storage::cache::CacheScope,
) -> Option<AggregateGateStamp> {
    let _creation = GATE_STAMP_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(stamp) = read_aggregate_gate_stamp(store, scope) {
        return Some(stamp);
    }
    let key = aggregate_gate_stamp_key().ok()?;
    let ticket = store.begin_write(scope, &key).ok()?;
    let stamp_id = uuid::Uuid::new_v4().to_string();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64;
    let metadata = crate::domain::cache::CacheMetadata {
        representation_version: 1,
        fetched_at_ms: now,
        verified_at_ms: now,
        completeness: crate::domain::cache::CacheCompleteness::Complete,
        etag: None,
        last_modified: None,
        cache_control: None,
    };
    // A `Stale` ticket means another task already published its own stamp; either way the entry
    // that is present now is the one this page must validate against.
    let _ = store.put(ticket, stamp_id.as_bytes(), metadata);
    read_aggregate_gate_stamp(store, scope)
}

fn verify_aggregate_gate_stamp(
    store: &crate::storage::cache::CacheStore,
    scope: &crate::storage::cache::CacheScope,
    expected: &AggregateGateStamp,
) -> bool {
    if scope.partition() != expected.partition {
        return false;
    }
    // The lease pins this exact entry: a capacity eviction cannot remove it, while a revocation,
    // replacement, rejection or retirement deletes it and every later snapshot is empty.
    let Ok(Some(snapshot)) = expected.lease.snapshot() else {
        return false;
    };
    if snapshot.bytes != expected.stamp_id.as_bytes() {
        return false;
    }
    // Also require that the live key still resolves to this stamp, so a stamp that was deleted and
    // replaced under the same partition cannot keep an old historical snapshot alive.
    read_aggregate_gate_stamp(store, scope)
        .is_some_and(|current| current.stamp_id == expected.stamp_id)
}

async fn prepare_gate_stamp(
    repos: &crate::data::RepoService,
    context: &RepositoryReadContext,
) -> Option<AggregateGateStamp> {
    let cache = repos.cache(context)?;
    let (store, scope) = cache.into_parts();
    tokio::task::spawn_blocking(move || ensure_aggregate_gate_stamp(&store, &scope))
        .await
        .ok()
        .flatten()
}

/// Re-verify the aggregate gate stamp for a historical value. A stamp that disappeared (explicit
/// repository revocation deletes every `repository = None` aggregate entry), a retired/rejected
/// scope or a replaced PAT must not publish historical bytes.
async fn verified_historical_stamp(
    repos: &crate::data::RepoService,
    context: &RepositoryReadContext,
    stamp: Option<&AggregateGateStamp>,
) -> Result<AggregateGateStamp, PresentationError> {
    let Some(stamp) = stamp.cloned() else {
        return Err(PresentationError::Forbidden);
    };
    let Some(cache) = repos.cache(context) else {
        return Err(PresentationError::Forbidden);
    };
    let (store, scope) = cache.into_parts();
    let expected = stamp.clone();
    let valid =
        tokio::task::spawn_blocking(move || verify_aggregate_gate_stamp(&store, &scope, &expected))
            .await
            .unwrap_or(false);
    if valid {
        Ok(stamp)
    } else {
        Err(PresentationError::Forbidden)
    }
}

fn unpack_page_value<T>(
    value: PageValue<T>,
    gate_stamp: Option<AggregateGateStamp>,
) -> Result<SectionOutcome<T>, PresentationError> {
    match value {
        PageValue::Value {
            value,
            origin,
            metadata,
        } => {
            let historical = origin == PageOrigin::HistoricalSnapshot;
            Ok(SectionOutcome {
                value,
                provenance: Some(ProfileProvenance::from_cache(historical, &metadata)),
                gate_stamp: historical.then_some(gate_stamp).flatten(),
            })
        }
        PageValue::Miss(miss) => Err(PresentationError::from_cache_miss(miss)),
    }
}

async fn read_profile_basic(
    repos: &crate::data::RepoService,
    session: &PageReadSession,
    cached: Option<&CachedPageReadSession>,
    stamp: Option<&AggregateGateStamp>,
    login: &str,
    cancel: CancellationToken,
) -> Result<SectionOutcome<ProfileBasic>, PresentationError> {
    let Some(cached) = cached else {
        return session
            .profile(login, cancel)
            .await
            .map(|scoped| SectionOutcome {
                value: scoped.value,
                provenance: None,
                gate_stamp: None,
            })
            .map_err(PresentationError::from);
    };
    match cached
        .profile(PageReadMode::Online, login, cancel.clone())
        .await
    {
        Ok(scoped) => unpack_page_value(scoped.value, None),
        Err(PageCacheError::Read(error)) => {
            if should_fallback_profile(&error) {
                match cached.profile(PageReadMode::Offline, login, cancel).await {
                    Ok(scoped) => match scoped.value {
                        PageValue::Value {
                            value,
                            origin,
                            metadata,
                        } => {
                            let historical = origin == PageOrigin::HistoricalSnapshot;
                            let gate_stamp = if historical {
                                Some(
                                    verified_historical_stamp(repos, session.context(), stamp)
                                        .await?,
                                )
                            } else {
                                None
                            };
                            Ok(SectionOutcome {
                                value,
                                provenance: Some(ProfileProvenance::from_cache(
                                    historical, &metadata,
                                )),
                                gate_stamp,
                            })
                        }
                        PageValue::Miss(miss) => {
                            if matches!(error, ProfileError::GitHub(GitHubError::Network { .. })) {
                                Err(PresentationError::from_cache_miss(miss))
                            } else {
                                Err(error.into())
                            }
                        }
                    },
                    Err(PageCacheError::Cache(CacheError::AccessDenied)) => Err(error.into()),
                    Err(PageCacheError::Cache(cache_err)) => Err(map_cache_error(cache_err)),
                    Err(PageCacheError::Context(gh)) => Err(ProfileError::GitHub(gh).into()),
                    Err(PageCacheError::Read(read_err)) => Err(read_err.into()),
                }
            } else {
                Err(error.into())
            }
        }
        Err(PageCacheError::Cache(CacheError::AccessDenied)) => session
            .profile(login, cancel)
            .await
            .map(|scoped| SectionOutcome {
                value: scoped.value,
                provenance: None,
                gate_stamp: None,
            })
            .map_err(PresentationError::from),
        Err(PageCacheError::Cache(cache_err)) => Err(map_cache_error(cache_err)),
        Err(PageCacheError::Context(gh)) => Err(ProfileError::GitHub(gh).into()),
    }
}

async fn read_profile_pinned(
    repos: &crate::data::RepoService,
    session: &PageReadSession,
    cached: Option<&CachedPageReadSession>,
    stamp: Option<&AggregateGateStamp>,
    login: &str,
    cancel: CancellationToken,
) -> Result<SectionOutcome<ProfilePinned>, PresentationError> {
    let limits = ProfileLimits::default();
    let Some(cached) = cached else {
        return session
            .profile_pinned(login, &limits, cancel)
            .await
            .map(|scoped| SectionOutcome {
                value: scoped.value,
                provenance: None,
                gate_stamp: None,
            })
            .map_err(PresentationError::from);
    };
    match cached
        .profile_pinned(PageReadMode::Online, login, &limits, cancel.clone())
        .await
    {
        Ok(scoped) => unpack_page_value(scoped.value, None),
        Err(PageCacheError::Read(error)) => {
            if should_fallback_profile(&error) {
                match cached
                    .profile_pinned(PageReadMode::Offline, login, &limits, cancel)
                    .await
                {
                    Ok(scoped) => match scoped.value {
                        PageValue::Value {
                            value,
                            origin,
                            metadata,
                        } => {
                            let historical = origin == PageOrigin::HistoricalSnapshot;
                            let gate_stamp = if historical {
                                Some(
                                    verified_historical_stamp(repos, session.context(), stamp)
                                        .await?,
                                )
                            } else {
                                None
                            };
                            Ok(SectionOutcome {
                                value,
                                provenance: Some(ProfileProvenance::from_cache(
                                    historical, &metadata,
                                )),
                                gate_stamp,
                            })
                        }
                        PageValue::Miss(miss) => {
                            if matches!(error, ProfileError::GitHub(GitHubError::Network { .. })) {
                                Err(PresentationError::from_cache_miss(miss))
                            } else {
                                Err(error.into())
                            }
                        }
                    },
                    Err(PageCacheError::Cache(CacheError::AccessDenied)) => Err(error.into()),
                    Err(PageCacheError::Cache(cache_err)) => Err(map_cache_error(cache_err)),
                    Err(PageCacheError::Context(gh)) => Err(ProfileError::GitHub(gh).into()),
                    Err(PageCacheError::Read(read_err)) => Err(read_err.into()),
                }
            } else {
                Err(error.into())
            }
        }
        Err(PageCacheError::Cache(CacheError::AccessDenied)) => session
            .profile_pinned(login, &limits, cancel)
            .await
            .map(|scoped| SectionOutcome {
                value: scoped.value,
                provenance: None,
                gate_stamp: None,
            })
            .map_err(PresentationError::from),
        Err(PageCacheError::Cache(cache_err)) => Err(map_cache_error(cache_err)),
        Err(PageCacheError::Context(gh)) => Err(ProfileError::GitHub(gh).into()),
    }
}

/// Cache identity for the last interval a contribution calendar was read at.
fn calendar_range_key(login: &str) -> Result<CacheKey, CacheError> {
    CacheKey::mutable(
        None,
        true,
        "profile-calendar-range",
        login,
        "profile-calendar-range-v1",
        1,
    )
}

fn read_calendar_range(
    store: &crate::storage::cache::CacheStore,
    scope: &crate::storage::cache::CacheScope,
    login: &str,
) -> Option<ContributionRange> {
    let key = calendar_range_key(login).ok()?;
    let CacheRead::Hit(lease) = store.read(scope, &key).ok()? else {
        return None;
    };
    let snapshot = lease.snapshot().ok()??;
    let text = std::str::from_utf8(&snapshot.bytes).ok()?;
    let (from, to) = text.split_once('|')?;
    ContributionRange::new(from, to).ok()
}

fn write_calendar_range(
    store: &crate::storage::cache::CacheStore,
    scope: &crate::storage::cache::CacheScope,
    login: &str,
    range: &ContributionRange,
) -> Result<(), CacheError> {
    let key = calendar_range_key(login)?;
    let ticket = store.begin_write(scope, &key)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64;
    let metadata = crate::domain::cache::CacheMetadata {
        representation_version: 1,
        fetched_at_ms: now,
        verified_at_ms: now,
        completeness: crate::domain::cache::CacheCompleteness::Complete,
        etag: None,
        last_modified: None,
        cache_control: None,
    };
    let payload = format!("{}|{}", range.from(), range.to());
    store.put(ticket, payload.as_bytes(), metadata)?;
    Ok(())
}

async fn load_calendar_range(
    repos: &crate::data::RepoService,
    context: &RepositoryReadContext,
    login: &str,
) -> Option<ContributionRange> {
    let cache = repos.cache(context)?;
    let (store, scope) = cache.into_parts();
    let login = login.to_string();
    tokio::task::spawn_blocking(move || read_calendar_range(&store, &scope, &login))
        .await
        .ok()
        .flatten()
}

/// Records the interval of a successful calendar read. Best effort: a cache write failure only
/// means a later offline reopen cannot restore the calendar, never that a wrong range is shown.
async fn store_calendar_range(
    repos: &crate::data::RepoService,
    context: &RepositoryReadContext,
    login: &str,
    range: &ContributionRange,
) {
    let Some(cache) = repos.cache(context) else {
        return;
    };
    let (store, scope) = cache.into_parts();
    let login = login.to_string();
    let range = range.clone();
    let _ =
        tokio::task::spawn_blocking(move || write_calendar_range(&store, &scope, &login, &range))
            .await;
}

async fn read_profile_contributions(
    repos: &crate::data::RepoService,
    session: &PageReadSession,
    cached: Option<&CachedPageReadSession>,
    stamp: Option<&AggregateGateStamp>,
    login: &str,
    range: &ContributionRange,
    cancel: CancellationToken,
) -> Result<(SectionOutcome<ContributionCalendar>, ContributionRange), PresentationError> {
    let limits = ProfileLimits::default();
    let Some(cached) = cached else {
        let outcome = session
            .contribution_calendar(login, range, &limits, cancel)
            .await
            .map(|scoped| SectionOutcome {
                value: scoped.value,
                provenance: None,
                gate_stamp: None,
            })
            .map_err(PresentationError::from)?;
        store_calendar_range(repos, session.context(), login, range).await;
        return Ok((outcome, range.clone()));
    };
    match cached
        .contribution_calendar(PageReadMode::Online, login, range, &limits, cancel.clone())
        .await
    {
        Ok(scoped) => {
            let outcome = unpack_page_value(scoped.value, None)?;
            store_calendar_range(repos, session.context(), login, range).await;
            Ok((outcome, range.clone()))
        }
        Err(PageCacheError::Read(error)) => {
            if !should_fallback_profile(&error) {
                return Err(error.into());
            }
            // The requested interval is today's rolling year. A snapshot stored on an earlier date
            // lives under that earlier interval, so also try the recorded interval instead of
            // reporting a cache miss that would make offline reading date-dependent.
            let mut offline_ranges = vec![range.clone()];
            if let Some(previous) = load_calendar_range(repos, session.context(), login).await
                && &previous != range
            {
                offline_ranges.push(previous);
            }
            let mut last_miss = None;
            for offline_range in offline_ranges {
                match cached
                    .contribution_calendar(
                        PageReadMode::Offline,
                        login,
                        &offline_range,
                        &limits,
                        cancel.clone(),
                    )
                    .await
                {
                    Ok(scoped) => match scoped.value {
                        PageValue::Value {
                            value,
                            origin,
                            metadata,
                        } => {
                            let historical = origin == PageOrigin::HistoricalSnapshot;
                            let gate_stamp = if historical {
                                Some(
                                    verified_historical_stamp(repos, session.context(), stamp)
                                        .await?,
                                )
                            } else {
                                None
                            };
                            let outcome = SectionOutcome {
                                value,
                                provenance: Some(ProfileProvenance::from_cache(
                                    historical, &metadata,
                                )),
                                gate_stamp,
                            };
                            store_calendar_range(repos, session.context(), login, &offline_range)
                                .await;
                            return Ok((outcome, offline_range));
                        }
                        PageValue::Miss(miss) => last_miss = Some(miss),
                    },
                    Err(PageCacheError::Cache(CacheError::AccessDenied)) => {
                        return Err(error.into());
                    }
                    Err(PageCacheError::Cache(cache_err)) => {
                        return Err(map_cache_error(cache_err));
                    }
                    Err(PageCacheError::Context(gh)) => {
                        return Err(ProfileError::GitHub(gh).into());
                    }
                    Err(PageCacheError::Read(read_err)) => return Err(read_err.into()),
                }
            }
            if matches!(error, ProfileError::GitHub(GitHubError::Network { .. })) {
                Err(PresentationError::from_cache_miss(
                    last_miss.unwrap_or(CacheMiss::NotCached),
                ))
            } else {
                Err(error.into())
            }
        }
        Err(PageCacheError::Cache(CacheError::AccessDenied)) => {
            let outcome = session
                .contribution_calendar(login, range, &limits, cancel)
                .await
                .map(|scoped| SectionOutcome {
                    value: scoped.value,
                    provenance: None,
                    gate_stamp: None,
                })
                .map_err(PresentationError::from)?;
            store_calendar_range(repos, session.context(), login, range).await;
            Ok((outcome, range.clone()))
        }
        Err(PageCacheError::Cache(cache_err)) => Err(map_cache_error(cache_err)),
        Err(PageCacheError::Context(gh)) => Err(ProfileError::GitHub(gh).into()),
    }
}

async fn read_profile_readme(
    repos: &crate::data::RepoService,
    session: &PageReadSession,
    cached: Option<&CachedPageReadSession>,
    stamp: Option<&AggregateGateStamp>,
    basic: &ProfileBasic,
    cancel: CancellationToken,
) -> Result<SectionOutcome<ProfileReadme>, PresentationError> {
    let repo_limits = RepositoryLimits::default();
    let blob_limits = BlobLimits::default();
    let Some(cached) = cached else {
        return session
            .profile_readme(basic, &repo_limits, &blob_limits, cancel)
            .await
            .map(|scoped| SectionOutcome {
                value: scoped.value,
                provenance: None,
                gate_stamp: None,
            })
            .map_err(repository_error);
    };
    match cached
        .profile_readme(
            PageReadMode::Online,
            basic,
            &repo_limits,
            &blob_limits,
            cancel.clone(),
        )
        .await
    {
        Ok(scoped) => unpack_page_value(scoped.value, None),
        Err(PageCacheError::Read(error)) => {
            if should_fallback_readme(&error) {
                match cached
                    .profile_readme(
                        PageReadMode::Offline,
                        basic,
                        &repo_limits,
                        &blob_limits,
                        cancel,
                    )
                    .await
                {
                    Ok(scoped) => match scoped.value {
                        PageValue::Value {
                            value,
                            origin,
                            metadata,
                        } => {
                            let historical = origin == PageOrigin::HistoricalSnapshot;
                            let gate_stamp = if historical {
                                Some(
                                    verified_historical_stamp(repos, session.context(), stamp)
                                        .await?,
                                )
                            } else {
                                None
                            };
                            Ok(SectionOutcome {
                                value,
                                provenance: Some(ProfileProvenance::from_cache(
                                    historical, &metadata,
                                )),
                                gate_stamp,
                            })
                        }
                        PageValue::Miss(miss) => {
                            if matches!(error, RepositoryError::GitHub(GitHubError::Network { .. }))
                            {
                                Err(PresentationError::from_cache_miss(miss))
                            } else {
                                Err(repository_error(error))
                            }
                        }
                    },
                    Err(PageCacheError::Cache(CacheError::AccessDenied)) => {
                        Err(repository_error(error))
                    }
                    Err(PageCacheError::Cache(cache_err)) => Err(map_cache_error(cache_err)),
                    Err(PageCacheError::Context(gh)) => {
                        Err(PresentationError::from(ProfileError::GitHub(gh)))
                    }
                    Err(PageCacheError::Read(read_err)) => Err(repository_error(read_err)),
                }
            } else {
                Err(repository_error(error))
            }
        }
        Err(PageCacheError::Cache(CacheError::AccessDenied)) => session
            .profile_readme(basic, &repo_limits, &blob_limits, cancel)
            .await
            .map(|scoped| SectionOutcome {
                value: scoped.value,
                provenance: None,
                gate_stamp: None,
            })
            .map_err(repository_error),
        Err(PageCacheError::Cache(cache_err)) => Err(map_cache_error(cache_err)),
        Err(PageCacheError::Context(gh)) => Err(PresentationError::from(ProfileError::GitHub(gh))),
    }
}

fn repository_error(error: RepositoryError) -> PresentationError {
    match error {
        RepositoryError::GitHub(GitHubError::NotFound) => PresentationError::NotFound,
        RepositoryError::GitHub(error) => PresentationError::from(ProfileError::GitHub(error)),
        RepositoryError::SsoPartial { .. } => PresentationError::SsoRequired,
        RepositoryError::InvalidInput { .. }
        | RepositoryError::InvalidResponse { .. }
        | RepositoryError::EmptyOrUnavailable => PresentationError::InvalidData,
        RepositoryError::LimitExceeded { .. } => PresentationError::Unavailable,
    }
}

impl Render for ProfilePage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(view) = self.view.clone() {
            return view.into_any_element();
        }
        if let Some(error) = &self.unavailable {
            return self.error_placeholder(error, cx);
        }
        if self.services.is_none() {
            return self
                .placeholder(
                    "Sign in to read public profile information. There is no offline page cache.",
                    cx,
                )
                .into_any_element();
        }
        if self.session.is_none() {
            return self
                .placeholder(
                    "The current account cannot read this profile until its authorization is restored.",
                    cx,
                )
                .into_any_element();
        }
        self.placeholder("Loading profile…", cx).into_any_element()
    }
}

impl Drop for ProfilePage {
    fn drop(&mut self) {
        self.alive.send_replace(false);
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, Entity, TestAppContext, VisualContext, VisualTestContext, px};
    use std::collections::HashMap;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};

    use crate::app::AccountsHandle;
    use crate::app::accounts::Accounts;
    use crate::app::accounts::runtime::AccountRuntime;
    use crate::auth::InMemoryCredentialStore;
    use crate::auth::token::Pat;
    use crate::content::images::ImageKey;
    use crate::data::AccountService;
    use crate::domain::{AccountKey, HostId};
    use crate::github::{GitHubClient, GitHubConfig};
    use crate::network::executor::BackgroundExecutor;
    use crate::network::reader_images::{
        ExternalImageBytes, ExternalImageError, ExternalImageLimits, ExternalImageMetadata,
        ExternalImageResponse, ValidationHeaders,
    };
    use crate::network::transport::{HttpRequest, HttpResponse, Transport, TransportError};
    use crate::storage::accounts::AccountDatabaseConfig;
    use crate::storage::cache::{CacheConfig, CacheStore};

    const COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const ROOT_TREE: &str = "1111111111111111111111111111111111111111";
    const DOCS_TREE: &str = "2222222222222222222222222222222222222222";
    const README_BLOB: &str = "3333333333333333333333333333333333333333";
    const DIAGRAM_BLOB: &str = "4444444444444444444444444444444444444444";
    const SLOW_BLOB: &str = "5555555555555555555555555555555555555555";
    const LOGO_URL: &str = "https://images.invalid/public/logo.png";

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba([(x % 200) as u8, (y % 200) as u8, 120, 255])
        });
        let mut cursor = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut cursor, image::ImageFormat::Png)
            .expect("encode png");
        cursor.into_inner()
    }

    fn b64(bytes: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn json(status: u16, body: serde_json::Value) -> HttpResponse {
        HttpResponse {
            status,
            headers: vec![],
            body: body.to_string(),
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum ReadmeMode {
        WithPictures,
        Missing,
        RepoNotFound,
        ServerError,
        SlowPicture,
    }

    struct ProfileFixtureTransport {
        offline: AtomicBool,
        readme_mode: Mutex<ReadmeMode>,
        pinned_partial: AtomicBool,
        calendar_partial: AtomicBool,
        calendar_fail: AtomicBool,
        pinned_fail: AtomicBool,
        user_id: Mutex<u64>,
        user_login: Mutex<String>,
        reads: Mutex<Vec<String>>,
        diagram: Vec<u8>,
        slow_gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }

    impl ProfileFixtureTransport {
        fn new() -> Self {
            Self {
                offline: AtomicBool::new(false),
                readme_mode: Mutex::new(ReadmeMode::WithPictures),
                pinned_partial: AtomicBool::new(false),
                calendar_partial: AtomicBool::new(false),
                calendar_fail: AtomicBool::new(false),
                pinned_fail: AtomicBool::new(false),
                user_id: Mutex::new(1),
                user_login: Mutex::new("alice".into()),
                reads: Mutex::new(Vec::new()),
                diagram: png_bytes(160, 90),
                slow_gate: Mutex::new(None),
            }
        }

        fn set_offline(&self, offline: bool) {
            self.offline.store(offline, Ordering::SeqCst);
        }

        fn set_readme_mode(&self, mode: ReadmeMode) {
            *self.readme_mode.lock().unwrap() = mode;
        }

        fn set_user(&self, id: u64, login: &str) {
            *self.user_id.lock().unwrap() = id;
            *self.user_login.lock().unwrap() = login.to_string();
        }

        fn readme_markdown(mode: ReadmeMode) -> String {
            match mode {
                ReadmeMode::SlowPicture => {
                    "# Alice Profile\n\n![slow](docs/slow.png)\n\nEnd of profile.\n".into()
                }
                _ => format!(
                    "# Alice Profile\n\nWelcome to Alice's profile.\n\n![diagram](docs/diagram.png)\n\n![logo]({LOGO_URL})\n\nEnd of profile.\n"
                ),
            }
        }
    }

    impl Transport for ProfileFixtureTransport {
        fn execute_bounded<'a>(
            &'a self,
            request: HttpRequest,
            cancel: CancellationToken,
            max_body_bytes: usize,
        ) -> Pin<
            Box<dyn std::future::Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>,
        > {
            Box::pin(async move {
                let resp = self.execute(request, cancel).await?;
                if resp.body.len() > max_body_bytes {
                    return Err(TransportError::ResponseTooLarge {
                        limit: max_body_bytes,
                    });
                }
                Ok(resp)
            })
        }

        fn execute<'a>(
            &'a self,
            request: HttpRequest,
            cancel: CancellationToken,
        ) -> Pin<
            Box<dyn std::future::Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>,
        > {
            Box::pin(async move {
                let url = request.url.clone();
                self.reads.lock().unwrap().push(url.clone());
                if url.ends_with("/user") {
                    let id = *self.user_id.lock().unwrap();
                    let login = self.user_login.lock().unwrap().clone();
                    return Ok(json(200, serde_json::json!({"id": id, "login": login})));
                }
                if url.contains("/user/repos") {
                    return Ok(json(200, serde_json::json!([])));
                }
                if self.offline.load(Ordering::SeqCst) {
                    return Err(TransportError::Network {
                        message: "simulated offline".into(),
                    });
                }
                let lower = url.to_ascii_lowercase();
                if lower.ends_with("/users/alice") {
                    return Ok(json(
                        200,
                        serde_json::json!({
                            "id": 1,
                            "login": "alice",
                            "name": "Alice Example",
                            "bio": "Systems programmer",
                            "avatar_url": null,
                            "followers": 12,
                            "following": 5
                        }),
                    ));
                }
                if lower.ends_with("/users/bob") {
                    return Ok(json(
                        200,
                        serde_json::json!({
                            "id": 2,
                            "login": "bob",
                            "name": "Bob Example",
                            "bio": "Other user bio",
                            "avatar_url": null
                        }),
                    ));
                }
                if url.ends_with("/graphql") {
                    let body = request.body.unwrap_or_default();
                    if body.contains("itemShowcase") {
                        if self.pinned_fail.load(Ordering::SeqCst) {
                            return Err(TransportError::Network {
                                message: "pinned network fail".into(),
                            });
                        }
                        let partial = self.pinned_partial.load(Ordering::SeqCst);
                        let mut payload = serde_json::json!({
                            "data": {
                                "user": {
                                    "pinnedItems": {
                                        "totalCount": if partial { 5 } else { 1 },
                                        "pageInfo": {"hasNextPage": partial},
                                        "nodes": [{
                                            "__typename": "Repository",
                                            "databaseId": 101,
                                            "name": "core",
                                            "nameWithOwner": "alice/core",
                                            "description": "Core library",
                                            "url": "https://github.com/alice/core",
                                            "isPrivate": false,
                                            "isFork": false,
                                            "stargazerCount": 42,
                                            "forkCount": 3,
                                            "primaryLanguage": {"name": "Rust"},
                                            "owner": {"login": "alice"}
                                        }]
                                    },
                                    "itemShowcase": {
                                        "hasPinnedItems": true,
                                        "items": {
                                            "totalCount": 1,
                                            "pageInfo": {"hasNextPage": partial},
                                            "nodes": [{
                                                "__typename": "Repository",
                                                "databaseId": 101,
                                                "name": "core",
                                                "nameWithOwner": "alice/core",
                                                "description": "Core library",
                                                "url": "https://github.com/alice/core",
                                                "isPrivate": false,
                                                "isFork": false,
                                                "stargazerCount": 42,
                                                "forkCount": 3,
                                                "primaryLanguage": {"name": "Rust"},
                                                "owner": {"login": "alice"}
                                            }]
                                        }
                                    }
                                }
                            }
                        });
                        if partial {
                            payload["errors"] = serde_json::json!([{
                                "type": "FORBIDDEN",
                                "path": ["user", "pinnedItems"]
                            }]);
                        }
                        return Ok(json(200, payload));
                    }
                    if body.contains("contributionsCollection") {
                        if self.calendar_fail.load(Ordering::SeqCst) {
                            return Err(TransportError::Network {
                                message: "calendar network fail".into(),
                            });
                        }
                        let partial = self.calendar_partial.load(Ordering::SeqCst);
                        let mut payload = serde_json::json!({
                            "data": {
                                "user": {
                                    "contributionsCollection": {
                                        "contributionCalendar": {
                                            "totalContributions": 5,
                                            "weeks": [{
                                                "contributionDays": [{
                                                    "date": "2026-01-01",
                                                    "contributionCount": 5,
                                                    "contributionLevel": "SECOND_QUARTILE",
                                                    "color": "#112233"
                                                }]
                                            }],
                                            "months": []
                                        }
                                    }
                                }
                            }
                        });
                        if partial {
                            payload["errors"] = serde_json::json!([{
                                "type": "FORBIDDEN",
                                "path": ["user", "contributionsCollection"]
                            }]);
                        }
                        return Ok(json(200, payload));
                    }
                }
                if lower.ends_with("/repos/alice/alice") {
                    let mode = *self.readme_mode.lock().unwrap();
                    return match mode {
                        ReadmeMode::RepoNotFound => Ok(json(404, serde_json::json!({}))),
                        ReadmeMode::ServerError => Ok(json(503, serde_json::json!({}))),
                        _ => Ok(json(
                            200,
                            serde_json::json!({
                                "id": 77,
                                "name": "alice",
                                "full_name": "alice/alice",
                                "owner": {"id": 1, "login": "alice"},
                                "visibility": "public",
                                "private": false,
                                "default_branch": "main",
                                "fork": false,
                                "archived": false,
                                "stargazers_count": 0,
                                "forks_count": 0,
                                "open_issues_count": 0,
                                "size": 1,
                                "html_url": "https://github.com/alice/alice"
                            }),
                        )),
                    };
                }
                if lower.contains("/repos/alice/alice/commits/") {
                    return Ok(json(
                        200,
                        serde_json::json!({
                            "sha": COMMIT,
                            "commit": {
                                "message": "profile readme",
                                "tree": {"sha": ROOT_TREE},
                                "author": null,
                                "committer": null
                            }
                        }),
                    ));
                }
                if lower.ends_with(&format!("/repos/alice/alice/git/trees/{ROOT_TREE}")) {
                    let mode = *self.readme_mode.lock().unwrap();
                    if mode == ReadmeMode::Missing {
                        return Ok(json(
                            200,
                            serde_json::json!({
                                "sha": ROOT_TREE,
                                "truncated": false,
                                "tree": []
                            }),
                        ));
                    }
                    let text = Self::readme_markdown(mode);
                    return Ok(json(
                        200,
                        serde_json::json!({
                            "sha": ROOT_TREE,
                            "truncated": false,
                            "tree": [
                                {"path": "README.md", "type": "blob", "mode": "100644", "sha": README_BLOB, "size": text.len()},
                                {"path": "docs", "type": "tree", "mode": "040000", "sha": DOCS_TREE}
                            ]
                        }),
                    ));
                }
                if lower.ends_with(&format!("/repos/alice/alice/git/trees/{DOCS_TREE}")) {
                    return Ok(json(
                        200,
                        serde_json::json!({
                            "sha": DOCS_TREE,
                            "truncated": false,
                            "tree": [
                                {"path": "diagram.png", "type": "blob", "mode": "100644", "sha": DIAGRAM_BLOB, "size": self.diagram.len()},
                                {"path": "slow.png", "type": "blob", "mode": "100644", "sha": SLOW_BLOB, "size": self.diagram.len()}
                            ]
                        }),
                    ));
                }
                if lower.ends_with(&format!("/repos/alice/alice/git/blobs/{README_BLOB}")) {
                    let mode = *self.readme_mode.lock().unwrap();
                    let text = Self::readme_markdown(mode);
                    return Ok(json(
                        200,
                        serde_json::json!({
                            "sha": README_BLOB,
                            "encoding": "base64",
                            "size": text.len(),
                            "content": b64(text.as_bytes())
                        }),
                    ));
                }
                if lower.ends_with(&format!("/repos/alice/alice/git/blobs/{DIAGRAM_BLOB}")) {
                    return Ok(json(
                        200,
                        serde_json::json!({
                            "sha": DIAGRAM_BLOB,
                            "encoding": "base64",
                            "size": self.diagram.len(),
                            "content": b64(&self.diagram)
                        }),
                    ));
                }
                if lower.ends_with(&format!("/repos/alice/alice/git/blobs/{SLOW_BLOB}")) {
                    let rx = self.slow_gate.lock().unwrap().take();
                    if let Some(rx) = rx {
                        let _ = rx.await;
                    } else {
                        tokio::time::sleep(Duration::from_millis(400)).await;
                    }
                    if cancel.is_cancelled() {
                        return Err(TransportError::Cancelled);
                    }
                    return Ok(json(
                        200,
                        serde_json::json!({
                            "sha": SLOW_BLOB,
                            "encoding": "base64",
                            "size": self.diagram.len(),
                            "content": b64(&self.diagram)
                        }),
                    ));
                }
                Ok(json(404, serde_json::json!({})))
            })
        }
    }

    struct TestExternalImages {
        offline: AtomicBool,
        bodies: Mutex<HashMap<String, Vec<u8>>>,
    }

    impl TestExternalImages {
        fn new() -> Self {
            let mut bodies = HashMap::new();
            bodies.insert(LOGO_URL.to_string(), png_bytes(64, 64));
            Self {
                offline: AtomicBool::new(false),
                bodies: Mutex::new(bodies),
            }
        }

        fn set_offline(&self, offline: bool) {
            self.offline.store(offline, Ordering::SeqCst);
        }
    }

    impl ExternalImageBytes for TestExternalImages {
        fn load_with_metadata<'a>(
            &'a self,
            url: &'a str,
            _limits: &'a ExternalImageLimits,
            _validation: ValidationHeaders<'a>,
            cancel: CancellationToken,
        ) -> Pin<
            Box<
                dyn std::future::Future<Output = Result<ExternalImageResponse, ExternalImageError>>
                    + Send
                    + 'a,
            >,
        > {
            Box::pin(async move {
                if cancel.is_cancelled() {
                    return Err(ExternalImageError::Cancelled);
                }
                if self.offline.load(Ordering::SeqCst) {
                    return Err(ExternalImageError::Network);
                }
                let bytes = self
                    .bodies
                    .lock()
                    .unwrap()
                    .get(url)
                    .cloned()
                    .ok_or(ExternalImageError::Status(404))?;
                Ok(ExternalImageResponse::Fetched {
                    bytes,
                    metadata: ExternalImageMetadata {
                        etag: Some("\"logo-etag\"".into()),
                        last_modified: None,
                        cache_control: Some("public, max-age=3600".into()),
                        age_secs: None,
                    },
                })
            })
        }
    }

    fn setup_test_app(cx: &mut TestAppContext) {
        cx.update(|cx| {
            ely_core::init_for_tests(cx);
            crate::platform::fonts::register(cx).expect("test fonts");
        });
    }

    struct Harness {
        _temp: tempfile::TempDir,
        transport: Arc<ProfileFixtureTransport>,
        external: Arc<TestExternalImages>,
        accounts: Arc<AccountService>,
        controller: Entity<Accounts>,
        page: Entity<ProfilePage>,
    }

    fn build_harness(
        cx: &mut TestAppContext,
        with_cache: bool,
    ) -> (Harness, &mut VisualTestContext) {
        setup_test_app(cx);
        let temp = tempfile::tempdir().unwrap();
        let account_db = temp.path().join("accounts.sqlite3");
        let cache_dir = temp.path().join("cache");
        let cred_store = Arc::new(InMemoryCredentialStore::new());
        let transport = Arc::new(ProfileFixtureTransport::new());
        let external = Arc::new(TestExternalImages::new());
        let executor = Arc::new(BackgroundExecutor::new(4));
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let svc = rt.block_on(async {
            let svc = if with_cache {
                let store = CacheStore::open(CacheConfig::at(cache_dir.clone())).unwrap();
                AccountService::open_with_cache(
                    AccountDatabaseConfig::at(account_db),
                    cred_store.clone(),
                    Arc::new(GitHubClient::new(
                        GitHubConfig::default(),
                        transport.clone(),
                    )),
                    executor.clone(),
                    store,
                )
                .await
                .unwrap()
            } else {
                AccountService::open(
                    AccountDatabaseConfig::at(account_db),
                    cred_store.clone(),
                    Arc::new(GitHubClient::new(
                        GitHubConfig::default(),
                        transport.clone(),
                    )),
                    executor.clone(),
                )
                .await
                .unwrap()
            };
            svc.login(Pat::from_str("ghp_alice_token")).await.unwrap();
            svc
        });
        let runtime = Arc::new(AccountRuntime::new(svc, executor));
        let accounts = runtime.accounts.clone();
        let external_for_page = external.clone();
        let mut controller_slot = None;
        let (page, cx) = cx.add_window_view(|window, cx| {
            let controller = cx.new(|cx| Accounts::new(Some(runtime), window, cx));
            cx.set_global(AccountsHandle(controller.clone()));
            let services = controller.read(cx).services();
            let avatars = PageAvatars::new(Some(controller.downgrade()));
            let nav: NavigationSink = Rc::new(|_, _, _| {});
            controller_slot = Some(controller);
            let sessions = crate::workspace::Sessions::new(cx);
            let tab_id = sessions.active_id();
            let mut page =
                ProfilePage::new(tab_id, "alice".to_string(), services, nav, avatars, cx);
            page.set_external_images_for_test(external_for_page);
            page
        });
        let controller = controller_slot.unwrap();
        (
            Harness {
                _temp: temp,
                transport,
                external,
                accounts,
                controller,
                page,
            },
            cx,
        )
    }

    fn pump_page(harness: &Harness, cx: &mut VisualTestContext) {
        harness
            .controller
            .update(cx, |accounts, cx| accounts.drain_for_test(cx));
        cx.update_window_entity(&harness.page, |page, window, cx| {
            page.drain_for_test(window, cx);
            if let Some(view) = page.view_for_test().cloned()
                && let Some(reader) = view.read(cx).reader().cloned()
            {
                reader.update(cx, |reader, cx| reader.beat_images(cx));
            }
        });
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }

    fn wait_for_profile(
        harness: &Harness,
        cx: &mut VisualTestContext,
        mut check: impl FnMut(&mut VisualTestContext) -> bool,
    ) {
        // A wall-clock deadline rather than a fixed iteration count: the shared test binary runs
        // many suites in parallel, so background reads and image decoding can be starved for a while.
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            pump_page(harness, cx);
            if check(cx) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "profile page did not reach expected state"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[gpui::test]
    fn online_warmup_and_reopen_offline_restores_profile_readme_and_images_at_natural_height(
        cx: &mut TestAppContext,
    ) {
        let (harness, cx) = build_harness(cx, true);
        cx.simulate_resize(gpui::size(px(1000.0), px(800.0)));
        let diagram_key = ImageKey::new(format!("repo:alice/alice@{COMMIT}:docs/diagram.png"));
        let logo_key = ImageKey::new(format!("url:{LOGO_URL}"));

        // 1. Warm up online: wait until basic, repos, calendar, README and both images are ready.
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                let Some(view) = page.view_for_test() else {
                    return false;
                };
                let v = view.read(cx);
                matches!(v.basic(), SectionState::Ready(_))
                    && matches!(v.repositories(), SectionState::Ready(_))
                    && matches!(v.contributions(), SectionState::Ready(_))
                    && matches!(v.readme_state(), SectionState::Ready(()))
                    && v.reader().is_some_and(|r| {
                        r.read(cx).image_state(&diagram_key).as_deref() == Some("ready")
                            && r.read(cx).image_state(&logo_key).as_deref() == Some("ready")
                    })
            })
        });
        // Let background cache writes settle.
        std::thread::sleep(Duration::from_millis(50));
        pump_page(&harness, cx);

        // Check natural height and no nested scrollbars in online state.
        let doc_bounds = cx.debug_bounds("reader-markdown").expect("README markdown");
        assert!(doc_bounds.size.height > px(100.0));
        assert!(cx.debug_bounds("reader-markdown-scrollbar-thumb").is_none());
        assert!(
            cx.debug_bounds("profile-basic-provenance-historical")
                .is_none()
        );

        // 2. Cut network (both GitHub transport and external image transport) and reopen ProfilePage.
        harness.transport.set_offline(true);
        harness.external.set_offline(true);
        let reads_before = harness.transport.reads.lock().unwrap().len();

        cx.update_window_entity(&harness.page, |page, _, cx| {
            let services = harness.controller.read(cx).services();
            page.rebind(services, cx);
        });

        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                let Some(view) = page.view_for_test() else {
                    return false;
                };
                let v = view.read(cx);
                matches!(v.basic(), SectionState::Ready(_))
                    && matches!(v.repositories(), SectionState::Ready(_))
                    && matches!(v.contributions(), SectionState::Ready(_))
                    && matches!(v.readme_state(), SectionState::Ready(()))
                    && v.reader().is_some_and(|r| {
                        r.read(cx).image_state(&diagram_key).as_deref() == Some("ready")
                            && r.read(cx).image_state(&logo_key).as_deref() == Some("ready")
                    })
            })
        });
        let _ = reads_before;
        // Historical badges are shown on all four sections.
        assert!(
            cx.debug_bounds("profile-basic-provenance-historical")
                .is_some()
        );
        assert!(
            cx.debug_bounds("profile-repositories-provenance-historical")
                .is_some()
        );
        assert!(
            cx.debug_bounds("profile-contributions-provenance-historical")
                .is_some()
        );
        assert!(
            cx.debug_bounds("profile-readme-provenance-historical")
                .is_some()
        );
        assert!(cx.debug_bounds("reader-markdown").is_some());
        assert!(cx.debug_bounds("reader-markdown-scrollbar-thumb").is_none());
    }

    #[gpui::test]
    fn uncached_offline_and_online_recovery_work_without_masking_errors_as_absent_readme(
        cx: &mut TestAppContext,
    ) {
        let (harness, cx) = build_harness(cx, true);
        // Start offline with an empty cache: basic fails with NotCached and offers retry.
        harness.transport.set_offline(true);
        cx.update_window_entity(&harness.page, |page, _, cx| {
            let services = harness.controller.read(cx).services();
            page.rebind(services, cx);
        });
        wait_for_profile(&harness, cx, |cx| {
            cx.debug_bounds("profile-page-basic-error").is_some()
        });

        // Restore network, but make README return 503 ServerError first: README must show compact warning mark, NOT disappear!
        harness.transport.set_offline(false);
        harness.transport.set_readme_mode(ReadmeMode::ServerError);
        let basic_retry = cx.debug_bounds("profile-page-basic-error").unwrap();
        cx.simulate_click(basic_retry.center(), gpui::Modifiers::none());

        wait_for_profile(&harness, cx, |cx| {
            cx.debug_bounds("profile-readme-error").is_some()
                && cx.debug_bounds("profile-basic").is_some()
        });
        assert!(
            cx.debug_bounds("profile-readme-section").is_some(),
            "503 error must not be disguised as an absent README"
        );

        // Now switch README to 404 (RepoNotFound) and click the README retry mark.
        harness.transport.set_readme_mode(ReadmeMode::RepoNotFound);
        let readme_retry = cx.debug_bounds("profile-readme-error").unwrap();
        cx.simulate_click(readme_retry.center(), gpui::Modifiers::none());

        wait_for_profile(&harness, cx, |cx| {
            cx.debug_bounds("profile-readme-section").is_none()
        });

        // Switch to Missing (empty root tree) online so it caches a Missing README outcome.
        harness.transport.set_readme_mode(ReadmeMode::Missing);
        cx.update_window_entity(&harness.page, |page, _, cx| {
            let services = harness.controller.read(cx).services();
            page.rebind(services, cx);
        });
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                page.view_for_test().is_some_and(|v| {
                    matches!(v.read(cx).readme_state(), SectionState::Unavailable(_))
                })
            })
        });

        // Go offline and reopen: confirmed Missing README is restored from cache and still omitted!
        harness.transport.set_offline(true);
        cx.update_window_entity(&harness.page, |page, _, cx| {
            let services = harness.controller.read(cx).services();
            page.rebind(services, cx);
        });
        wait_for_profile(&harness, cx, |cx| {
            cx.debug_bounds("profile-basic-provenance-historical")
                .is_some()
                && harness.page.read_with(cx, |page, cx| {
                    page.view_for_test().is_some_and(|v| {
                        matches!(v.read(cx).readme_state(), SectionState::Unavailable(_))
                    })
                })
        });
        assert!(
            cx.debug_bounds("profile-readme-section").is_none(),
            "cached Missing README remains quietly omitted offline"
        );
    }

    #[gpui::test]
    fn partial_pins_and_calendar_and_section_miss_preserve_distinct_states(
        cx: &mut TestAppContext,
    ) {
        let (harness, cx) = build_harness(cx, true);
        harness
            .transport
            .pinned_partial
            .store(true, Ordering::SeqCst);
        harness
            .transport
            .calendar_fail
            .store(true, Ordering::SeqCst);
        cx.update_window_entity(&harness.page, |page, _, cx| {
            let services = harness.controller.read(cx).services();
            page.rebind(services, cx);
        });

        wait_for_profile(&harness, cx, |cx| {
            cx.debug_bounds("profile-repositories-provenance-partial")
                .is_some()
                && cx.debug_bounds("profile-section-error-mark").is_some()
        });

        // Now go offline and reopen: basic & partial pins & README come from cache, while calendar is NotCached!
        harness.transport.set_offline(true);
        cx.update_window_entity(&harness.page, |page, _, cx| {
            let services = harness.controller.read(cx).services();
            page.rebind(services, cx);
        });

        wait_for_profile(&harness, cx, |cx| {
            cx.debug_bounds("profile-basic-provenance-historical")
                .is_some()
                && cx
                    .debug_bounds("profile-repositories-provenance-historical")
                    .is_some()
                && cx
                    .debug_bounds("profile-repositories-provenance-partial")
                    .is_some()
                && harness.page.read_with(cx, |page, cx| {
                    page.view_for_test().is_some_and(|v| {
                        matches!(
                            v.read(cx).contributions(),
                            SectionState::Error(PresentationError::NotCached)
                        )
                    })
                })
        });

        // Bring network back with complete calendar, and click retry on contributions section!
        harness.transport.set_offline(false);
        harness
            .transport
            .calendar_fail
            .store(false, Ordering::SeqCst);
        let retry_mark = cx.debug_bounds("profile-section-error-mark").unwrap();
        cx.simulate_click(retry_mark.center(), gpui::Modifiers::none());

        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                page.view_for_test().is_some_and(|v| {
                    matches!(v.read(cx).contributions(), SectionState::Ready(_))
                        && v.read(cx)
                            .contributions_provenance()
                            .is_some_and(|p| !p.is_historical())
                })
            })
        });
    }

    #[gpui::test]
    fn account_switch_pat_replace_removal_and_revocation_isolate_cache_and_stop_slow_pictures(
        cx: &mut TestAppContext,
    ) {
        let (harness, cx) = build_harness(cx, true);
        let (tx_slow, rx_slow) = tokio::sync::oneshot::channel();
        *harness.transport.slow_gate.lock().unwrap() = Some(rx_slow);
        harness.transport.set_readme_mode(ReadmeMode::SlowPicture);

        cx.update_window_entity(&harness.page, |page, _, cx| {
            let services = harness.controller.read(cx).services();
            page.rebind(services, cx);
        });

        // Wait until README is mounted and slow picture is in loading state.
        let slow_key = ImageKey::new(format!("repo:alice/alice@{COMMIT}:docs/slow.png"));
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                page.view_for_test().is_some_and(|v| {
                    v.read(cx).reader().is_some_and(|r| {
                        r.read(cx).image_state(&slow_key).as_deref() == Some("loading")
                    })
                })
            })
        });
        let old_reader = harness.page.read_with(cx, |page, cx| {
            page.view_for_test()
                .unwrap()
                .read(cx)
                .reader()
                .unwrap()
                .clone()
        });

        // Replace PAT while slow picture is in flight -> quarantines aggregate cache and advances epoch.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            harness
                .accounts
                .replace(
                    AccountKey::new(1, HostId::github()),
                    Pat::from_str("ghp_alice_replaced"),
                )
                .await
                .unwrap();
        });
        // Release slow picture after PAT replacement.
        let _ = tx_slow.send(());
        pump_page(&harness, cx);

        // Old view is cleared on next drain because session is no longer current.
        wait_for_profile(&harness, cx, |cx| {
            harness
                .page
                .read_with(cx, |page, _| page.view_for_test().is_none())
        });
        // Old reader never transitioned to ready.
        assert_ne!(
            old_reader
                .read_with(cx, |r, _| r.image_state(&slow_key))
                .as_deref(),
            Some("ready")
        );

        // Offline under replaced PAT: aggregate cache is quarantined (C3), so offline read cannot use old snapshot.
        harness.transport.set_offline(true);
        cx.update_window_entity(&harness.page, |page, _, cx| {
            let services = harness.controller.read(cx).services();
            page.rebind(services, cx);
        });
        wait_for_profile(&harness, cx, |cx| {
            cx.debug_bounds("profile-page-basic-error").is_some()
                && harness
                    .page
                    .read_with(cx, |page, _| page.view_for_test().is_none())
        });

        // Remove account (C2): immediately clears/blocks.
        rt.block_on(async {
            harness
                .accounts
                .remove(AccountKey::new(1, HostId::github()))
                .await
                .unwrap();
        });
        pump_page(&harness, cx);
        assert!(
            harness
                .page
                .read_with(cx, |page, _| page.view_for_test().is_none())
        );

        // Sign in a second account (bob) and verify account switch isolation.
        harness.transport.set_offline(false);
        harness.transport.set_user(2, "bob");
        rt.block_on(async {
            harness
                .accounts
                .login(Pat::from_str("ghp_bob_token"))
                .await
                .unwrap();
        });
        pump_page(&harness, cx);
    }

    #[gpui::test]
    fn explicit_repository_revocation_clears_historical_profile_and_blocks_offline_revival(
        cx: &mut TestAppContext,
    ) {
        let (harness, cx) = build_harness(cx, true);
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                page.view_for_test()
                    .is_some_and(|v| matches!(v.read(cx).basic(), SectionState::Ready(_)))
            })
        });
        // Go offline and reopen so ProfilePage displays a historical snapshot.
        harness.transport.set_offline(true);
        cx.update_window_entity(&harness.page, |page, _, cx| {
            let services = harness.controller.read(cx).services();
            page.rebind(services, cx);
        });
        wait_for_profile(&harness, cx, |cx| {
            cx.debug_bounds("profile-basic-provenance-historical")
                .is_some()
        });

        // Explicit repository revocation invalidates the account aggregate snapshot and advances revision.
        // Perform revocation IMMEDIATELY after the historical view mounts (before the 80ms monitor tick),
        // and even re-populate a fresh basic snapshot to prove that the revision-bound stamp still
        // invalidates the displayed historical snapshot.
        let session = harness.accounts.page_session().unwrap();
        let context = session.context().clone();
        let cache = harness.accounts.repo_service().cache(&context).unwrap();
        cache
            .store()
            .revoke_repository(cache.scope(), Some(101))
            .unwrap();
        // Re-create a gate stamp right after revocation, exactly as a later read would. A fresh
        // stamp must NOT re-validate the historical snapshot that was read under the old revision.
        let (store, scope) = cache.into_parts();
        let recreated = ensure_aggregate_gate_stamp(&store, &scope);
        assert!(
            recreated.is_some(),
            "a new read can still prepare its own gate stamp"
        );

        // Wait for cache monitor + drain to evict the historical view!
        wait_for_profile(&harness, cx, |cx| {
            harness
                .page
                .read_with(cx, |page, _| page.view_for_test().is_none())
        });
    }

    #[gpui::test]
    fn tab_suspension_preserves_readme_image_provider_across_deactivate_and_reactivate(
        cx: &mut TestAppContext,
    ) {
        let (harness, cx) = build_harness(cx, true);
        let (tx_slow, rx_slow) = tokio::sync::oneshot::channel();
        *harness.transport.slow_gate.lock().unwrap() = Some(rx_slow);
        harness.transport.set_readme_mode(ReadmeMode::SlowPicture);
        cx.update_window_entity(&harness.page, |page, _, cx| {
            let services = harness.controller.read(cx).services();
            page.rebind(services, cx);
        });

        let slow_key = ImageKey::new(format!("repo:alice/alice@{COMMIT}:docs/slow.png"));
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                page.view_for_test().is_some_and(|v| {
                    v.read(cx).reader().is_some_and(|r| {
                        r.read(cx).image_state(&slow_key).as_deref() == Some("loading")
                    })
                })
            })
        });

        // Switch away from the Profile tab (`set_active(false)`) and back (`set_active(true)`).
        cx.update_window_entity(&harness.page, |page, _, cx| {
            page.set_active(false, cx);
            page.set_active(true, cx);
        });
        // Release the in-flight picture and verify the attached provider still completes it.
        let _ = tx_slow.send(());
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                page.view_for_test().is_some_and(|v| {
                    v.read(cx).reader().is_some_and(|r| {
                        r.read(cx).image_state(&slow_key).as_deref() == Some("ready")
                    })
                })
            })
        });
    }

    #[test]
    fn aggregate_gate_stamp_survives_capacity_eviction_but_not_revocation() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = CacheConfig::at(temp.path().join("cache"));
        config.capacity_bytes = 128 * 1024;
        config.max_entry_bytes = 64 * 1024;
        config.inline_bytes = 1024;
        config.max_entries = 6;
        let store = CacheStore::open(config).unwrap();
        let scope = store
            .saved_account(AccountKey::new(1, HostId::github()))
            .unwrap();
        let stamp = ensure_aggregate_gate_stamp(&store, &scope).expect("gate stamp");

        // Unrelated account-level writes fill the budget and force LRU eviction of ordinary
        // entries. The stamp is held by a lease, so eviction must not drop it.
        let payload = vec![7u8; 40 * 1024];
        for index in 0..12u32 {
            let key = CacheKey::mutable(
                None,
                true,
                "profile-eviction-filler",
                &index.to_string(),
                "profile-eviction-filler-v1",
                1,
            )
            .unwrap();
            let ticket = store.begin_write(&scope, &key).unwrap();
            let now = 1_760_000_000_000i64;
            let metadata = crate::domain::cache::CacheMetadata {
                representation_version: 1,
                fetched_at_ms: now,
                verified_at_ms: now,
                completeness: crate::domain::cache::CacheCompleteness::Complete,
                etag: None,
                last_modified: None,
                cache_control: None,
            };
            let _ = store.put(ticket, &payload, metadata);
        }
        // Prove the budget was really exceeded, otherwise the assertion below would be vacuous.
        let first_filler = CacheKey::mutable(
            None,
            true,
            "profile-eviction-filler",
            "0",
            "profile-eviction-filler-v1",
            1,
        )
        .unwrap();
        assert!(
            matches!(store.read(&scope, &first_filler), Ok(CacheRead::Miss(_))),
            "the earliest ordinary entry must have been evicted"
        );
        assert!(
            verify_aggregate_gate_stamp(&store, &scope, &stamp),
            "ordinary capacity eviction must not make an authorized page look revoked"
        );

        // Explicit revocation is still authoritative.
        store.revoke_repository(&scope, Some(101)).unwrap();
        assert!(!verify_aggregate_gate_stamp(&store, &scope, &stamp));
    }

    #[test]
    fn concurrently_prepared_gate_stamps_converge_on_one_value() {
        let temp = tempfile::tempdir().unwrap();
        let store = CacheStore::open(CacheConfig::at(temp.path().join("cache"))).unwrap();
        let scope = store
            .saved_account(AccountKey::new(1, HostId::github()))
            .unwrap();
        // Four section reads prepare the stamp at the same time. A losing writer must re-read the
        // winner instead of failing its section.
        let stamps = std::thread::scope(|threads| {
            let handles = (0..4)
                .map(|_| {
                    let store = store.clone();
                    let scope = scope.clone();
                    threads.spawn(move || ensure_aggregate_gate_stamp(&store, &scope))
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert!(
            stamps.iter().all(Option::is_some),
            "every concurrent preparation resolves a stamp: {stamps:?}"
        );
        let first = stamps[0].clone().unwrap();
        assert!(
            stamps
                .iter()
                .flatten()
                .all(|stamp| stamp.stamp_id == first.stamp_id),
            "all sections validate against the same stamp"
        );
    }

    #[gpui::test]
    fn offline_reopen_on_a_later_day_restores_the_cached_contribution_interval(
        cx: &mut TestAppContext,
    ) {
        let (harness, cx) = build_harness(cx, true);
        // Online visit: the calendar is stored under the interval the host requested that day.
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                page.view_for_test()
                    .is_some_and(|v| matches!(v.read(cx).contributions(), SectionState::Ready(_)))
            })
        });
        let cached_range = harness
            .page
            .read_with(cx, |page, _| page.range_for_test().clone());
        // A later UTC date means a different rolling year; without the recorded interval the
        // snapshot could never be found again.
        let later = ContributionRange::new("2024-01-01", "2024-12-31").unwrap();
        assert_ne!(later, cached_range);
        cx.update_window_entity(&harness.page, |page, _, cx| {
            page.set_range_for_test(later.clone());
            let services = harness.controller.read(cx).services();
            page.rebind(services, cx);
        });
        harness.transport.set_offline(true);
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                page.view_for_test().is_some_and(|v| {
                    let v = v.read(cx);
                    matches!(v.contributions(), SectionState::Ready(_))
                        && v.contributions_provenance()
                            .is_some_and(|p| p.is_historical())
                })
            })
        });
        assert_eq!(
            harness
                .page
                .read_with(cx, |page, _| page.range_for_test().clone()),
            cached_range,
            "the restored interval is the one the cached calendar was stored under"
        );
        let view_range = harness.page.read_with(cx, |page, cx| {
            page.view_for_test()
                .map(|v| v.read(cx).range().clone())
                .unwrap()
        });
        assert_eq!(view_range, cached_range);
        assert!(
            cx.debug_bounds("profile-contributions-provenance-historical")
                .is_some()
        );
    }

    #[gpui::test]
    fn uncached_service_preserves_online_behavior_without_regression(cx: &mut TestAppContext) {
        let (harness, cx) = build_harness(cx, false);
        let diagram_key = ImageKey::new(format!("repo:alice/alice@{COMMIT}:docs/diagram.png"));
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                let Some(view) = page.view_for_test() else {
                    return false;
                };
                let v = view.read(cx);
                matches!(v.basic(), SectionState::Ready(_))
                    && matches!(v.repositories(), SectionState::Ready(_))
                    && matches!(v.contributions(), SectionState::Ready(_))
                    && matches!(v.readme_state(), SectionState::Ready(()))
                    && v.reader().is_some_and(|r| {
                        r.read(cx).image_state(&diagram_key).as_deref() == Some("ready")
                    })
            })
        });
        assert!(
            cx.debug_bounds("profile-basic-provenance-historical")
                .is_none()
        );
    }

    #[test]
    fn rolling_year_handles_ordinary_leap_and_epoch_boundaries() {
        let at = |days: u64| rolling_year_at(UNIX_EPOCH + Duration::from_secs(days * 86_400));
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        let ordinary = at(19_723); // 2024-01-01: 2023 is not a leap year.
        assert_eq!(ordinary.from(), "2023-01-01");
        assert_eq!(ordinary.to(), "2024-01-01");
        let leap = at(20_455); // 2026-01-02: the preceding year includes 2024-02-29.
        assert_eq!(leap.from(), "2025-01-02");
        assert_eq!(leap.to(), "2026-01-02");
        // The domain's cap is 366 days between endpoints. A leap-adjacent year that would need
        // 367 inclusive days is moved forward; otherwise the same calendar day is kept.
        let capped = at(20_454); // 2026-01-01, a 366-day endpoint difference.
        assert_eq!(capped.from(), "2025-01-01");
        assert_eq!(capped.to(), "2026-01-01");
        for range in [leap, ordinary, capped, at(0)] {
            assert!(ContributionRange::new(range.from(), range.to()).is_ok());
        }
    }
}
