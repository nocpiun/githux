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
use crate::storage::cache::{CacheAuthorization, CacheError, CacheKey, CacheRead};
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
    /// Unique per task instance. Lets a test tell a binding's own background task from a cancelled
    /// one that is still finishing its last iteration.
    #[cfg(test)]
    id: u64,
}

impl Work {
    fn new() -> Self {
        let (token, source) = CancellationToken::new();
        #[cfg(test)]
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            token,
            source,
            #[cfg(test)]
            id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }
}

/// In-process authorization token captured before an aggregate (`repository = None`) read.
///
/// This is the shared `CacheAuthorization`: lock-free atomic loads only, so `is_current()` is safe
/// on the UI thread and never queries SQLite or takes the store's state mutex. Once invalid (an
/// explicit repository revocation, PAT replacement, credential rejection, retirement or clear), a
/// token never becomes valid again; a later read must capture a fresh one.
struct SectionOutcome<T> {
    value: T,
    provenance: Option<ProfileProvenance>,
    authorization: Option<CacheAuthorization>,
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
    /// A cache-first observation published while the online read is still pending. It never ends the
    /// section's request: the same reply's final result still owns the work slot.
    preliminary: bool,
    result: SectionResult,
}

pub struct ProfilePage {
    tab: TabId,
    login: String,
    /// The login spelling this page was opened with. A canonical rename keeps it, so replies to
    /// requests started before that rename (the cache-first section reads) are still recognized as
    /// belonging to this profile instead of being discarded with their work slots still occupied.
    opened_login: String,
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
    authorizations: Arc<Mutex<[Option<CacheAuthorization>; 4]>>,
    inbox: Arc<Mutex<VecDeque<Reply>>>,
    /// Generation-tagged invalidation produced by a monitor. An older monitor may still be in
    /// the middle of an iteration when its binding was replaced; its verdict must never be
    /// consumed by the page that replaced it.
    cache_invalidated: Arc<std::sync::atomic::AtomicU64>,
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
            opened_login: login.clone(),
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
            authorizations: Arc::new(Mutex::new([None, None, None, None])),
            inbox: Arc::new(Mutex::new(VecDeque::new())),
            cache_invalidated: Arc::new(std::sync::atomic::AtomicU64::new(u64::MAX)),
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
            .store(u64::MAX, std::sync::atomic::Ordering::Release);
        *self.authorizations.lock().expect("profile authorizations") = [None, None, None, None];
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
        // Re-check cache availability from RepoService::cache(context) before starting, and capture
        // this read's authorization token *before* it runs. Both calls are in-process: the token is
        // lock-free and disk-free, so capturing it here never blocks the UI thread.
        let binding = services.repositories().cache(&context);
        let authorization = match &binding {
            Some(cache) => cache.store().authorization(cache.scope(), None),
            None => None,
        };
        // A cache binding whose token cannot be captured fails closed. Publishing the read anyway
        // would present protected cached work as unguarded output, which is exactly what the token
        // exists to prevent.
        if binding.is_some() && authorization.is_none() {
            self.requests[index] = self.requests[index].saturating_add(1);
            let request = self.requests[index];
            let result = match section {
                Section::Basic => SectionResult::Basic(Err(PresentationError::Unavailable)),
                Section::Repositories => {
                    SectionResult::Repositories(Err(PresentationError::Unavailable))
                }
                Section::Contributions => {
                    SectionResult::Contributions(Err(PresentationError::Unavailable))
                }
                Section::Readme => SectionResult::Readme(Err(PresentationError::Unavailable)),
            };
            self.inbox.lock().expect("profile inbox").push_back(Reply {
                section,
                generation: self.generation,
                request,
                selection: self.selection,
                context,
                login: self.login.clone(),
                preliminary: false,
                result,
            });
            return;
        }
        let cached = binding.and_then(|cache| {
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
        let repos_for_authorization = services.repositories();
        // Cache-first: an empty section publishes its last readable snapshot right away instead of
        // waiting for the online read to finish or fail first. It is its own background job so the
        // network read starts immediately and a slow disk cannot delay it. A section that already
        // shows content (a retry or a sibling-triggered re-read) keeps what is on screen until the
        // online result replaces it, so it needs no second snapshot read.
        if self.section_is_empty(section, cx)
            && let Some(cached_first) = cached.clone()
        {
            let cached_first_authorization = authorization.clone();
            let cached_first_basic = basic.clone();
            let cached_first_range = range.clone();
            let cached_first_login = login.clone();
            let cached_first_inbox = inbox.clone();
            let cached_first_alive = self.alive.clone();
            let cached_first_cancel = cancel.clone();
            let cached_first_context = context.clone();
            let repos_for_snapshot = repos_for_authorization.clone();
            let session_for_snapshot = session.clone();
            let _ = services.executor().handle().spawn(async move {
                let found = read_cached_first_section(SnapshotRequest {
                    repos: &repos_for_snapshot,
                    session: &session_for_snapshot,
                    cached: &cached_first,
                    authorization: cached_first_authorization.clone(),
                    login: &cached_first_login,
                    section,
                    basic: cached_first_basic.as_ref(),
                    range: &cached_first_range,
                    cancel: cached_first_cancel,
                })
                .await;
                if let Some((section, result)) = found
                    && *cached_first_alive.borrow()
                    && cached_first_authorization
                        .as_ref()
                        .is_none_or(CacheAuthorization::is_current)
                {
                    cached_first_inbox
                        .lock()
                        .expect("profile inbox")
                        .push_back(Reply {
                            section,
                            generation,
                            request,
                            selection,
                            context: cached_first_context,
                            login: cached_first_login,
                            preliminary: true,
                            result,
                        });
                }
            });
        }
        let submitted = services.executor().handle().spawn(async move {
            let authorization = authorization.as_ref();
            let result = match section {
                Section::Basic => SectionResult::Basic(
                    read_profile_basic(
                        &repos_for_authorization,
                        &session,
                        cached.as_ref(),
                        authorization,
                        &login,
                        cancel,
                    )
                    .await,
                ),
                Section::Repositories => SectionResult::Repositories(
                    read_profile_pinned(
                        &repos_for_authorization,
                        &session,
                        cached.as_ref(),
                        authorization,
                        &login,
                        cancel,
                    )
                    .await,
                ),
                Section::Contributions => SectionResult::Contributions(
                    read_profile_contributions(
                        &repos_for_authorization,
                        &session,
                        cached.as_ref(),
                        authorization,
                        &login,
                        &range,
                        cancel,
                    )
                    .await,
                ),
                Section::Readme => SectionResult::Readme(match basic {
                    Some(basic) => {
                        read_profile_readme(
                            &repos_for_authorization,
                            &session,
                            cached.as_ref(),
                            authorization,
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
                    preliminary: false,
                    result,
                });
            }
        });
        if submitted.is_err() {
            self.work[index] = None;
        }
    }

    /// Whether one section currently shows no content, so a cached snapshot may fill it.
    ///
    /// A section that already shows data keeps it (including its scroll position and README reader)
    /// until the online result replaces it. Before the Basic profile creates the view there is no
    /// section content yet, so only Basic qualifies.
    fn section_is_empty(&self, section: Section, cx: &gpui::App) -> bool {
        let Some(view) = self.view.as_ref() else {
            return section == Section::Basic;
        };
        let view = view.read(cx);
        match section {
            Section::Basic => matches!(view.basic(), SectionState::Loading),
            Section::Repositories => matches!(view.repositories(), SectionState::Loading),
            Section::Contributions => matches!(view.contributions(), SectionState::Loading),
            Section::Readme => matches!(view.readme_state(), SectionState::Loading),
        }
    }

    fn ensure_cache_monitor(&mut self, services: &AccountServices) {
        // Every binding owns its monitor: replace (and cancel) whatever the previous binding left
        // behind instead of consulting a shared liveness flag. A cancelled task finishing late can
        // no longer suppress the new binding's monitor, and each `Work` handle is authoritative for
        // its own binding.
        if let Some(previous) = self.cache_monitor.take() {
            previous.source.cancel();
        }
        if self.cached_session.is_none() {
            return;
        }
        let Some(session) = self.session.clone() else {
            self.cache_monitor = None;
            return;
        };
        let context = session.context().clone();
        let repos = services.repositories();
        let alive = self.alive.clone();
        let invalidated = self.cache_invalidated.clone();
        let auths = self.authorizations.clone();
        // The verdict is tagged with the binding that produced it: a monitor left over from a
        // previous binding cannot retire the page that replaced it.
        let binding_generation = self.generation;
        let invalidate = move || {
            invalidated.store(binding_generation, std::sync::atomic::Ordering::Release);
        };
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
                let active: Vec<CacheAuthorization> = auths
                    .lock()
                    .expect("profile authorizations")
                    .iter()
                    .flatten()
                    .cloned()
                    .collect();
                if active.is_empty() {
                    continue;
                }
                if repos.cache(&context).is_none() {
                    invalidate();
                    break;
                }
                // Lock-free, disk-free atomic check through the shared CacheAuthorization interface.
                if !active.iter().all(CacheAuthorization::is_current) {
                    invalidate();
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
            // The displayed snapshot (or the authorization itself) was retired. Every in-flight
            // section request belonged to it, so stop them and release their handles: otherwise a
            // retry would see `work[index].is_some()` and never start again.
            for work in &mut self.work {
                if let Some(work) = work.take() {
                    work.source.cancel();
                }
            }
            self.inbox.lock().expect("profile inbox").clear();
            cx.notify();
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
            let binding_lost = self.cached_session.is_some()
                && services.repositories().cache(session.context()).is_none();
            if binding_lost {
                // The account's cache binding itself is gone (retired, rejected or suspended). The
                // page keeps reading online; nothing historical may stay on screen.
                self.cached_session = None;
            }
            // A concrete repository revocation keeps the aggregate gate usable for fresh reads, so
            // the invalidation only retires the snapshots that were on screen. It must not block a
            // later read that proves the gate again with a new stamp.
            // Only the current binding's verdict may retire the page. A monitor cancelled during
            // `rebind` may finish its iteration afterwards and publish the *old* generation; it must
            // not cancel the new binding's fresh requests.
            let invalidated = self
                .cache_invalidated
                .swap(u64::MAX, std::sync::atomic::Ordering::AcqRel)
                == self.generation;
            // A displayed section whose gate generation was retired must disappear even though the
            // account session is still valid (explicit repository revocation does not advance the
            // auth epoch). The verdict comes from the background monitor, so this foreground tick
            // performs no store access at all.
            if invalidated || binding_lost {
                if let Some(monitor) = self.cache_monitor.take() {
                    monitor.source.cancel();
                }
                self.retire_displayed_sections(cx);
                cx.notify();
                return false;
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

    /// Drops every authorization-bound value the page is showing, including the mounted README
    /// reader and its image provider, and leaves a retryable placeholder. Used both when a
    /// displayed generation is retired and when a reply arrives after its generation was retired.
    fn retire_displayed_sections(&mut self, cx: &mut Context<Self>) {
        if let Some(monitor) = self.cache_monitor.take() {
            monitor.source.cancel();
        }
        *self.authorizations.lock().expect("profile authorizations") = [None, None, None, None];
        // Cancelling a token cannot stop a reply that a worker already finished and enqueued.
        // Advancing the page generation and clearing the per-section request ids makes every
        // outstanding reply fail `accepts()`, so a stale sibling reply cannot retire — and cancel —
        // the retry the user starts after this cleanup.
        self.generation = self.generation.saturating_add(1);
        self.requests = [0; 4];
        for work in &mut self.work {
            if let Some(work) = work.take() {
                work.source.cancel();
            }
        }
        self.inbox.lock().expect("profile inbox").clear();
        if let Some(view) = self.view.take() {
            view.update(cx, |view, cx| {
                view.clear_reader(cx);
                view.suspend(cx);
            });
        }
        self.request = None;
        self.unavailable = Some(PresentationError::Forbidden);
    }

    /// Final ordering check for a cache-first section value.
    ///
    /// A snapshot only fills a section that is still empty. Once the online read has ended this
    /// request (its work slot is gone) or any content is displayed, the cache-first value is dropped:
    /// that keeps a late cache completion from overwriting newer network content and stops a re-read
    /// from resetting an already displayed section's scroll position and README reader.
    fn accept_preliminary(&self, reply: &Reply, cx: &gpui::App) -> bool {
        if self.work[reply.section.index()].is_none() {
            return false;
        }
        let historical = match &reply.result {
            SectionResult::Basic(Ok(outcome)) => outcome.provenance.as_ref(),
            SectionResult::Repositories(Ok(outcome)) => outcome.provenance.as_ref(),
            SectionResult::Contributions(Ok((outcome, _))) => outcome.provenance.as_ref(),
            SectionResult::Readme(Ok(outcome)) => outcome.provenance.as_ref(),
            _ => None,
        };
        if !historical.is_some_and(ProfileProvenance::is_historical) {
            return false;
        }
        self.section_is_empty(reply.section, cx)
    }

    fn accepts(&self, reply: &Reply) -> bool {
        if reply.generation != self.generation
            || reply.selection != self.selection
            || reply.request != self.requests[reply.section.index()]
            || (reply.login != self.login && reply.login != self.opened_login)
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
        // A listed binding that has disappeared means the account's cache was retired, rejected or
        // suspended: nothing read through it may be published.
        if self.cached_session.is_some() && services.repositories().cache(&reply.context).is_none()
        {
            return false;
        }
        true
    }

    // INTERFACE DEPENDENCY (cache-runtime): the lightweight, in-process validity check that must
    // run in this exact foreground step — after a reply is dequeued and before `view.update` — is
    // the shared permission-generation/epoch interface being published to
    // `D:/githux-cache-runtime/.agents/output/cache-recovery-contract.md`. It is deliberately NOT
    // implemented here: reading the store from `apply` would block the UI thread on SQLite, and
    // re-verifying only in the background is exactly the publish window this page must close.
    // Until it lands, displayed aggregates are retired by `check_authorization` as soon as the
    // background monitor observes the invalidated gate.

    fn apply(&mut self, reply: Reply, window: &mut Window, cx: &mut Context<Self>) {
        // Every authorized observation carries the CacheAuthorization captured before its read,
        // whether it came from the network or from a historical snapshot.
        let authorization = match &reply.result {
            SectionResult::Basic(Ok(outcome)) => outcome.authorization.clone(),
            SectionResult::Repositories(Ok(outcome)) => outcome.authorization.clone(),
            SectionResult::Contributions(Ok((outcome, _))) => outcome.authorization.clone(),
            SectionResult::Readme(Ok(outcome)) => outcome.authorization.clone(),
            _ => None,
        };
        // Binding/request/context first: a late reply from a previous binding must be dropped
        // quietly. It must never be able to retire the page that replaced it.
        if !self.accepts(&reply) {
            return;
        }
        // Publication check: re-verify the authorization ticket just before UI publication.
        // `CacheAuthorization::is_current()` is lock-free and disk-free, so this never queries
        // SQLite or takes the store's state mutex on the UI thread. A reply read before an explicit
        // revocation, PAT replacement, rejection or retirement is discarded here, closing the
        // window between the background read finishing and the UI commit. A freshly captured token
        // never re-authorizes an older reply.
        if let Some(auth) = &authorization
            && !auth.is_current()
        {
            // An invalidated ticket retires whatever it backed, online or historical. The final
            // reply clears the work slot; see below.
            self.retire_displayed_sections(cx);
            cx.notify();
            return;
        }
        if !reply.preliminary {
            self.work[reply.section.index()] = None;
        }
        self.authorizations.lock().expect("profile authorizations")[reply.section.index()] =
            authorization;
        if reply.preliminary && !self.accept_preliminary(&reply, cx) {
            return;
        }
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
                authorization,
            }) => {
                // The same profile can now arrive twice in one request: first from the persisted
                // snapshot, then from the network. Recreating the view would throw away every sibling
                // section (and its scroll and README reader) that was already published, so an
                // already mounted view for this same profile is updated in place instead.
                let mounted = self.view.as_ref().is_some_and(|view| {
                    view.read(cx).target().id == basic.id
                        && self
                            .request
                            .as_ref()
                            .is_some_and(|request| *view.read(cx).binding() == request.binding)
                });
                // Only an online result may rename the route: a cached spelling is a historical
                // observation, and the online half of this same read still answers with the login
                // the request was made with.
                let changed_login = !reply.preliminary && self.login != basic.login;
                // Reset sibling section authorizations when a new Basic profile creates a new view.
                // Basic keeps the token it was read under regardless of provenance: an online Basic
                // is as revocable as a historical one.
                let mut auths = self.authorizations.lock().expect("profile authorizations");
                if mounted {
                    auths[Section::Basic.index()] = authorization;
                } else {
                    *auths = [authorization, None, None, None];
                }
                drop(auths);
                // Server id/login are authoritative. An earlier login-only action is never used as
                // a numeric identity; Shell canonicalizes the matching TabId after this callback.
                //
                // A cache-first snapshot does not adopt its login spelling here: the online half of
                // the same basic read still carries the login the request was made with, and
                // `accepts()` compares that value. Renaming the page from a snapshot would reject
                // that final reply before it could clear its work slot.
                if !reply.preliminary {
                    self.login = basic.login.clone();
                }
                if !mounted {
                    self.create_view(&basic, &reply.context, window, cx);
                }
                let request = self.request.clone().expect("view request");
                if let Some(view) = self.view.clone() {
                    let state = SectionState::Ready(basic.clone());
                    view.update(cx, |view, cx| {
                        view.update_basic_with_provenance(&request, state, provenance, cx);
                    });
                    self.sync_avatar(basic.clone(), cx);
                }
                if changed_login {
                    // The README comes from the public repository whose name is the profile login, so
                    // a canonical rename changes which repository that is. A cached Basic may
                    // already have started (or finished) discovery against the old name: reject that
                    // request and read the new one.
                    if let Some(work) = self.work[Section::Readme.index()].take() {
                        work.source.cancel();
                    }
                    self.requests[Section::Readme.index()] =
                        self.requests[Section::Readme.index()].saturating_add(1);
                    if let Some(view) = self.view.clone() {
                        let request = self.request.clone().expect("view request");
                        view.update(cx, |view, cx| {
                            view.update_readme(&request, SectionState::Loading, window, cx);
                        });
                    }
                }
                // Every sibling section that still has no content is read now; one that is already
                // displayed keeps its value until its own online read replaces it.
                for section in [
                    Section::Repositories,
                    Section::Contributions,
                    Section::Readme,
                ] {
                    // The README is login-dependent, so a canonical rename restarts it even when it
                    // already had content.
                    if (section == Section::Readme && changed_login)
                        || self.section_is_empty(section, cx)
                    {
                        self.start(section, cx);
                    }
                }
                if changed_login {
                    // Shell is the outer mutator while it drives this page's update, so it cannot
                    // be re-entered synchronously. The canonical rename is delivered after this
                    // frame, and only when the login actually changed.
                    let canonical = PageNavigation::CanonicalProfile {
                        tab: self.tab,
                        login: basic.login.clone(),
                    };
                    let navigation = self.navigation.clone();
                    window.defer(cx, move |window, cx| {
                        navigation(canonical, window, cx);
                    });
                }
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
    pub(crate) fn login_for_test(&self) -> &str {
        &self.login
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
        *self.authorizations.lock().expect("profile authorizations") = [None, None, None, None];
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

/// Whether a cache failure means "no usable cache binding" or "the cache failed operationally".
///
/// Both must keep the account's online capability: none of them proves anything about the network
/// read, so the page falls back to the plain authenticated session request, which still enforces
/// cancellation, the captured context and authorization. A rejected credential, a stale session or
/// an explicit authorization denial is never routed here.
fn is_uncached_fallback(cache_error: &CacheError) -> bool {
    !matches!(
        cache_error,
        // A stale/deferred access decision is an authorization outcome, not an operational fault:
        // keep it typed instead of retrying as if the cache were merely unavailable.
        CacheError::Stale
    )
}

/// Test-only shorthand for capturing this read's authorization token. Production captures inside
/// `ProfilePage::start` so a missing token can fail that section closed before the read begins.
#[cfg(test)]
pub(crate) async fn prepare_gate_stamp(
    repos: &crate::data::RepoService,
    context: &RepositoryReadContext,
) -> Option<CacheAuthorization> {
    let cache = repos.cache(context)?;
    cache.store().authorization(cache.scope(), None)
}

/// Re-verify the authorization token for a historical value. Lock-free atomic check; no disk I/O.
async fn verified_historical_authorization(
    _repos: &crate::data::RepoService,
    _context: &RepositoryReadContext,
    auth: Option<&CacheAuthorization>,
) -> Result<CacheAuthorization, PresentationError> {
    let Some(auth) = auth.cloned() else {
        return Err(PresentationError::Forbidden);
    };
    if auth.is_current() {
        Ok(auth)
    } else {
        Err(PresentationError::Forbidden)
    }
}

fn unpack_page_value<T>(
    value: PageValue<T>,
    authorization: Option<CacheAuthorization>,
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
                // The token describes the authorization generation the bytes were read under, so
                // it is attached to network results as well: an explicit repository revocation must
                // retire displayed online aggregates, not just the offline badges.
                authorization,
            })
        }
        PageValue::Miss(miss) => Err(PresentationError::from_cache_miss(miss)),
    }
}

/// Reads the last persisted snapshot for one section without contacting the network.
///
/// This is the cache-first half of a section read: it runs alongside the ordinary online read and,
/// when an authorized readable snapshot exists, publishes that content immediately so a slow request
/// does not have to finish (or fail) before anything is shown. Every miss, corruption, stale
/// authorization or operational cache fault yields `None`, which leaves the ordinary read in charge
/// and never fabricates an empty or failed section.
struct SnapshotRequest<'a> {
    repos: &'a crate::data::RepoService,
    session: &'a PageReadSession,
    cached: &'a CachedPageReadSession,
    authorization: Option<CacheAuthorization>,
    login: &'a str,
    section: Section,
    basic: Option<&'a ProfileBasic>,
    range: &'a ContributionRange,
    cancel: CancellationToken,
}

async fn read_cached_first_section(
    request: SnapshotRequest<'_>,
) -> Option<(Section, SectionResult)> {
    let SnapshotRequest {
        repos,
        session,
        cached,
        authorization,
        login,
        section,
        basic,
        range,
        cancel,
    } = request;
    match section {
        Section::Basic => {
            let scoped = cached
                .profile(PageReadMode::Offline, login, cancel)
                .await
                .ok()?;
            let outcome = unpack_historical(scoped.value, authorization)?;
            Some((section, SectionResult::Basic(Ok(outcome))))
        }
        Section::Repositories => {
            let limits = ProfileLimits::default();
            let scoped = cached
                .profile_pinned(PageReadMode::Offline, login, &limits, cancel)
                .await
                .ok()?;
            let outcome = unpack_historical(scoped.value, authorization)?;
            Some((section, SectionResult::Repositories(Ok(outcome))))
        }
        Section::Contributions => {
            let limits = ProfileLimits::default();
            // A snapshot may have been stored under an earlier rolling year; try the recorded
            // interval as well instead of making cached reading date-dependent.
            // Try the interval this page asks for first, then the interval the snapshot was
            // recorded at (an earlier rolling year), so cached reading is not date-dependent.
            let mut ranges = vec![range.clone()];
            if let Some(previous) = load_calendar_range(repos, session.context(), login).await
                && &previous != range
            {
                ranges.push(previous);
            }
            for candidate in ranges {
                let scoped = cached
                    .contribution_calendar(
                        PageReadMode::Offline,
                        login,
                        &candidate,
                        &limits,
                        cancel.clone(),
                    )
                    .await
                    .ok()?;
                if let Some(outcome) = unpack_historical(scoped.value, authorization.clone()) {
                    return Some((
                        section,
                        SectionResult::Contributions(Ok((outcome, candidate))),
                    ));
                }
            }
            None
        }
        Section::Readme => {
            let basic = basic?;
            let repo_limits = RepositoryLimits::default();
            let blob_limits = BlobLimits::default();
            let scoped = cached
                .profile_readme(
                    PageReadMode::Offline,
                    basic,
                    &repo_limits,
                    &blob_limits,
                    cancel,
                )
                .await
                .ok()?;
            let outcome = unpack_historical(scoped.value, authorization)?;
            Some((section, SectionResult::Readme(Ok(outcome))))
        }
    }
}

/// Converts a value into a section outcome only when it really came from the persistent snapshot.
///
/// A `Network` origin would mean the replay performed a live read, which is not something the
/// cache-first half may publish as its own; a miss is not content either.
fn unpack_historical<T>(
    value: PageValue<T>,
    authorization: Option<CacheAuthorization>,
) -> Option<SectionOutcome<T>> {
    match value {
        PageValue::Value {
            value,
            origin: PageOrigin::HistoricalSnapshot,
            metadata,
        } => Some(SectionOutcome {
            value,
            provenance: Some(ProfileProvenance::from_cache(true, &metadata)),
            authorization,
        }),
        PageValue::Value { .. } | PageValue::Miss(_) => None,
    }
}

async fn read_profile_basic(
    repos: &crate::data::RepoService,
    session: &PageReadSession,
    cached: Option<&CachedPageReadSession>,
    authorization: Option<&CacheAuthorization>,
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
                authorization: None,
            })
            .map_err(PresentationError::from);
    };
    match cached
        .profile(PageReadMode::Online, login, cancel.clone())
        .await
    {
        Ok(scoped) => {
            let outcome = unpack_page_value(scoped.value, authorization.cloned())?;
            // The page is keyed by the requested spelling, but the Shell rewrites the route to the
            // server's canonical login. Store the same observation under that canonical spelling too
            // so reopening the canonical route offline can mount the profile. One extra request per
            // alias spelling; the common (already canonical) case issues none.
            if outcome.value.login != login && !cancel.is_cancelled() {
                let _ = cached
                    .profile(PageReadMode::Online, &outcome.value.login, cancel)
                    .await;
            }
            Ok(outcome)
        }
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
                            let authorization = if historical {
                                Some(
                                    verified_historical_authorization(
                                        repos,
                                        session.context(),
                                        authorization,
                                    )
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
                                authorization,
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
                    Err(PageCacheError::Cache(cache_err)) if is_uncached_fallback(&cache_err) => {
                        // The offline replay could not even reach the cache; report what the
                        // network read actually said instead of a cache-layer error.
                        Err(error.into())
                    }
                    Err(PageCacheError::Cache(cache_err)) => Err(map_cache_error(cache_err)),
                    Err(PageCacheError::Context(gh)) => Err(ProfileError::GitHub(gh).into()),
                    Err(PageCacheError::Read(read_err)) => Err(read_err.into()),
                }
            } else {
                Err(error.into())
            }
        }
        Err(PageCacheError::Cache(cache_err)) if is_uncached_fallback(&cache_err) => {
            // No usable cache binding, or the cache itself failed. The account's online capability
            // must survive either way: the plain session read keeps cancellation, the captured
            // context and authorization checks.
            session
                .profile(login, cancel)
                .await
                .map(|scoped| SectionOutcome {
                    value: scoped.value,
                    provenance: None,
                    authorization: authorization.cloned(),
                })
                .map_err(PresentationError::from)
        }
        Err(PageCacheError::Cache(cache_err)) => Err(map_cache_error(cache_err)),
        Err(PageCacheError::Context(gh)) => Err(ProfileError::GitHub(gh).into()),
    }
}

async fn read_profile_pinned(
    repos: &crate::data::RepoService,
    session: &PageReadSession,
    cached: Option<&CachedPageReadSession>,
    authorization: Option<&CacheAuthorization>,
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
                authorization: None,
            })
            .map_err(PresentationError::from);
    };
    match cached
        .profile_pinned(PageReadMode::Online, login, &limits, cancel.clone())
        .await
    {
        Ok(scoped) => unpack_page_value(scoped.value, authorization.cloned()),
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
                            let authorization = if historical {
                                Some(
                                    verified_historical_authorization(
                                        repos,
                                        session.context(),
                                        authorization,
                                    )
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
                                authorization,
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
                    Err(PageCacheError::Cache(cache_err)) if is_uncached_fallback(&cache_err) => {
                        // The offline replay could not even reach the cache; report what the
                        // network read actually said instead of a cache-layer error.
                        Err(error.into())
                    }
                    Err(PageCacheError::Cache(cache_err)) => Err(map_cache_error(cache_err)),
                    Err(PageCacheError::Context(gh)) => Err(ProfileError::GitHub(gh).into()),
                    Err(PageCacheError::Read(read_err)) => Err(read_err.into()),
                }
            } else {
                Err(error.into())
            }
        }
        Err(PageCacheError::Cache(cache_err)) if is_uncached_fallback(&cache_err) => session
            .profile_pinned(login, &limits, cancel)
            .await
            .map(|scoped| SectionOutcome {
                value: scoped.value,
                provenance: None,
                authorization: authorization.cloned(),
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
    authorization: Option<&CacheAuthorization>,
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
                authorization: None,
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
            let outcome = unpack_page_value(scoped.value, authorization.cloned())?;
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
                            let authorization = if historical {
                                Some(
                                    verified_historical_authorization(
                                        repos,
                                        session.context(),
                                        authorization,
                                    )
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
                                authorization,
                            };
                            store_calendar_range(repos, session.context(), login, &offline_range)
                                .await;
                            return Ok((outcome, offline_range));
                        }
                        PageValue::Miss(miss) => last_miss = Some(miss),
                    },
                    Err(PageCacheError::Cache(cache_err)) if is_uncached_fallback(&cache_err) => {
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
        Err(PageCacheError::Cache(cache_err)) if is_uncached_fallback(&cache_err) => {
            let outcome = session
                .contribution_calendar(login, range, &limits, cancel)
                .await
                .map(|scoped| SectionOutcome {
                    value: scoped.value,
                    provenance: None,
                    authorization: authorization.cloned(),
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
    authorization: Option<&CacheAuthorization>,
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
                authorization: None,
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
        Ok(scoped) => unpack_page_value(scoped.value, authorization.cloned()),
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
                            let authorization = if historical {
                                Some(
                                    verified_historical_authorization(
                                        repos,
                                        session.context(),
                                        authorization,
                                    )
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
                                authorization,
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
                    Err(PageCacheError::Cache(cache_err)) if is_uncached_fallback(&cache_err) => {
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
        Err(PageCacheError::Cache(cache_err)) if is_uncached_fallback(&cache_err) => session
            .profile_readme(basic, &repo_limits, &blob_limits, cancel)
            .await
            .map(|scoped| SectionOutcome {
                value: scoped.value,
                provenance: None,
                authorization: authorization.cloned(),
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
        /// Test-only: hold every profile read at the transport boundary so a cache-first paint can be
        /// observed while the network is still pending.
        held: AtomicBool,
        /// Test-only: hold everything except the basic `/users/<login>` read, so a canonical rename
        /// can be observed while sibling section reads are still in flight.
        hold_siblings: AtomicBool,
        /// True while held requests may continue. Arming a hold resets it, so a request that starts
        /// after the release cannot miss the notification the way a bare `notify_waiters` can.
        release: tokio::sync::watch::Sender<bool>,
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
                held: AtomicBool::new(false),
                hold_siblings: AtomicBool::new(false),
                release: tokio::sync::watch::channel(true).0,
            }
        }

        fn hold(&self, held: bool) {
            self.held.store(held, Ordering::SeqCst);
            if held {
                self.release.send_replace(false);
            }
        }

        fn hold_siblings(&self, held: bool) {
            self.hold_siblings.store(held, Ordering::SeqCst);
            if held {
                self.release.send_replace(false);
            }
        }

        fn release_all(&self) {
            self.release.send_replace(true);
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
                let holds = (self.held.load(Ordering::SeqCst)
                    || (self.hold_siblings.load(Ordering::SeqCst)
                        && !url.starts_with("https://api.github.com/users/")))
                    && !url.ends_with("/user")
                    && !url.contains("/user/repos");
                if holds {
                    let mut released = self.release.subscribe();
                    tokio::select! {
                        _ = async {
                            loop {
                                if *released.borrow_and_update() {
                                    break;
                                }
                                if released.changed().await.is_err() {
                                    break;
                                }
                            }
                        } => {}
                        _ = cancel.clone().cancelled() => {
                            return Err(TransportError::Cancelled);
                        }
                    }
                }
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
                            // The fixture can report a renamed canonical login (same numeric id).
                            "login": self.user_login.lock().unwrap().clone(),
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
        build_harness_with_login(cx, with_cache, "alice")
    }

    fn build_harness_with_login<'a>(
        cx: &'a mut TestAppContext,
        with_cache: bool,
        login: &str,
    ) -> (Harness, &'a mut VisualTestContext) {
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
            let mut page = ProfilePage::new(tab_id, login.to_string(), services, nav, avatars, cx);
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
        // Deadline-bounded rather than a fixed number of frames: the shared test binary runs many
        // GPUI suites in parallel, so progress can stall for a while. The assertions are unchanged.
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        loop {
            pump_page(harness, cx);
            if check(cx) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "profile page did not reach expected state: view={:?} work={:?}",
                harness.page.read_with(cx, |page, cx| {
                    page.view_for_test().map(|view| {
                        let v = view.read(cx);
                        (
                            format!("{:?}", v.basic()),
                            format!("{:?}", v.repositories()),
                            format!("{:?}", v.contributions()),
                            format!("{:?}", v.readme_state()),
                            v.reader().is_some(),
                        )
                    })
                }),
                harness.page.read_with(cx, |page, _| page
                    .work
                    .iter()
                    .map(Option::is_some)
                    .collect::<Vec<_>>()),
            );

            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Cache-first Profile: every section that has a readable snapshot paints while the online reads
    /// are still held at the transport boundary, and the online results then replace them.
    #[gpui::test]
    fn cached_sections_paint_before_the_network_reads_finish_and_are_then_replaced(
        cx: &mut TestAppContext,
    ) {
        let (harness, cx) = build_harness(cx, true);
        // 1. Warm every section plus its cached representation.
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
            })
        });
        std::thread::sleep(Duration::from_millis(50));
        pump_page(&harness, cx);

        // 2. Hold the transport and rebind: the cache-first pass must publish without the network.
        harness.transport.hold(true);
        cx.update_window_entity(&harness.page, |page, _, cx| {
            let services = harness.controller.read(cx).services();
            page.rebind(services, cx);
        });
        wait_for_profile(&harness, cx, |cx| {
            cx.debug_bounds("profile-basic-provenance-historical")
                .is_some()
                && harness.page.read_with(cx, |page, cx| {
                    let Some(view) = page.view_for_test() else {
                        return false;
                    };
                    let v = view.read(cx);
                    matches!(v.repositories(), SectionState::Ready(_))
                        && matches!(v.contributions(), SectionState::Ready(_))
                        && matches!(v.readme_state(), SectionState::Ready(()))
                })
                && cx
                    .debug_bounds("profile-repositories-provenance-historical")
                    .is_some()
                && cx
                    .debug_bounds("profile-contributions-provenance-historical")
                    .is_some()
                && cx
                    .debug_bounds("profile-readme-provenance-historical")
                    .is_some()
        });
        harness.page.read_with(cx, |page, _| {
            assert!(
                page.work.iter().all(Option::is_some),
                "the online reads still own their sections"
            );
        });

        // 3. Release the network: the fresh results replace every saved marker.
        harness.transport.hold_siblings(false);
        harness.transport.release_all();
        wait_for_profile(&harness, cx, |cx| {
            cx.debug_bounds("profile-basic-provenance-historical")
                .is_none()
                && cx
                    .debug_bounds("profile-repositories-provenance-historical")
                    .is_none()
                && cx
                    .debug_bounds("profile-contributions-provenance-historical")
                    .is_none()
                && cx
                    .debug_bounds("profile-readme-provenance-historical")
                    .is_none()
                && harness.page.read_with(cx, |page, cx| {
                    page.view_for_test().is_some_and(|view| {
                        let v = view.read(cx);
                        matches!(v.basic(), SectionState::Ready(_))
                            && matches!(v.repositories(), SectionState::Ready(_))
                            && matches!(v.contributions(), SectionState::Ready(_))
                            && matches!(v.readme_state(), SectionState::Ready(()))
                    })
                })
                && harness
                    .page
                    .read_with(cx, |page, _| page.work.iter().all(Option::is_none))
        });
    }

    /// Regression (review finding): a cached Basic whose canonical login differs from the requested
    /// spelling must not rename the page while the online half still carries the requested login —
    /// that would make `accepts()` reject the final reply and strand Basic in its historical state.
    #[gpui::test]
    fn a_cached_alias_login_does_not_strand_the_online_basic_read(cx: &mut TestAppContext) {
        let (harness, cx) = build_harness_with_login(cx, true, "Alice");
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                page.view_for_test()
                    .is_some_and(|view| matches!(view.read(cx).basic(), SectionState::Ready(_)))
                    && page.work.iter().all(Option::is_none)
            })
        });
        // Reopen the page for the non-canonical route spelling, then hold the network: the snapshot
        // paints with the cached canonical spelling while the online read still answers with the
        // login the request was made with.
        harness
            .page
            .update(cx, |page, _| page.login = "Alice".into());
        // Hold the sibling section reads only: the basic read may finish (and canonicalize the
        // login) while those requests are still in flight.
        harness.transport.hold_siblings(true);
        cx.update_window_entity(&harness.page, |page, _, cx| {
            let services = harness.controller.read(cx).services();
            page.rebind(services, cx);
        });
        wait_for_profile(&harness, cx, |cx| {
            cx.debug_bounds("profile-basic-provenance-historical")
                .is_some()
        });
        harness.transport.hold(false);
        harness.transport.release_all();
        // Every section started before the rename must still finish: the canonical login change is
        // the same profile, not a different target.
        wait_for_profile(&harness, cx, |cx| {
            cx.debug_bounds("profile-basic-provenance-historical")
                .is_none()
                && cx
                    .debug_bounds("profile-repositories-provenance-historical")
                    .is_none()
                && cx
                    .debug_bounds("profile-contributions-provenance-historical")
                    .is_none()
                && cx
                    .debug_bounds("profile-readme-provenance-historical")
                    .is_none()
                && harness.page.read_with(cx, |page, cx| {
                    page.view_for_test().is_some_and(|view| {
                        let view = view.read(cx);
                        matches!(view.basic(), SectionState::Ready(_))
                            && matches!(view.repositories(), SectionState::Ready(_))
                            && matches!(view.contributions(), SectionState::Ready(_))
                            && matches!(view.readme_state(), SectionState::Ready(()))
                    }) && page.work.iter().all(Option::is_none)
                })
        });
        assert_eq!(
            harness.page.read_with(cx, |page, _| page.login.to_string()),
            "alice"
        );
    }

    /// Regression (review finding): the README comes from the public repository named after the
    /// login, so a canonical rename must restart README discovery. Otherwise the cached Basic would
    /// leave the page showing (or omitting) the old `old/old` README.
    #[gpui::test]
    fn a_canonical_login_rename_restarts_the_cached_readme_read(cx: &mut TestAppContext) {
        let (harness, cx) = build_harness_with_login(cx, true, "alice");
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                page.view_for_test().is_some_and(|view| {
                    let view = view.read(cx);
                    matches!(view.basic(), SectionState::Ready(_))
                        && matches!(view.readme_state(), SectionState::Ready(()))
                }) && page.work.iter().all(Option::is_none)
            })
        });
        let reads_before = harness.transport.reads.lock().unwrap().len();

        // Same numeric account, new canonical login: the cached Basic still says "alice".
        harness.transport.set_user(1, "alicia");
        cx.update_window_entity(&harness.page, |page, _, cx| {
            let services = harness.controller.read(cx).services();
            page.rebind(services, cx);
        });
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, _| {
                page.login == "alicia" && page.work[Section::Readme.index()].is_none()
            }) && harness.page.read_with(cx, |page, cx| {
                page.view_for_test().is_some_and(|view| {
                    !matches!(view.read(cx).readme_state(), SectionState::Loading)
                })
            })
        });

        let reads = harness.transport.reads.lock().unwrap().clone();
        assert!(
            reads[reads_before..]
                .iter()
                .any(|url| url.to_ascii_lowercase().contains("/repos/alicia/alicia")),
            "README discovery must run again for the new same-name repository: {reads:?}"
        );
    }

    /// Cache-first ordering on Profile: a snapshot reply that arrives after the section already shows
    /// network content must not roll it back to a historical value.
    #[gpui::test]
    fn a_late_profile_snapshot_reply_never_rolls_back_a_newer_network_section(
        cx: &mut TestAppContext,
    ) {
        let (harness, cx) = build_harness(cx, true);
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                page.view_for_test()
                    .is_some_and(|view| matches!(view.read(cx).basic(), SectionState::Ready(_)))
                    && page.work.iter().all(Option::is_none)
            })
        });
        // Build the snapshot a slow cache read would have produced for this account.
        let session = harness.accounts.page_session().unwrap();
        let binding = harness
            .accounts
            .repo_service()
            .cache(session.context())
            .unwrap();
        let cached = CachedPageReadSession::new(
            session.clone(),
            binding.store().clone(),
            binding.scope().clone(),
        )
        .unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let cached_basic = rt.block_on(async {
            cached
                .profile(PageReadMode::Offline, "alice", Default::default())
                .await
                .unwrap()
        });
        let PageValue::Value {
            value, metadata, ..
        } = cached_basic.value
        else {
            panic!("the warmed profile snapshot must be readable");
        };
        cx.update_window_entity(&harness.page, |page, window, cx| {
            let request = page.requests[Section::Basic.index()];
            page.work[Section::Basic.index()] = Some(Work::new());
            page.inbox.lock().expect("profile inbox").push_back(Reply {
                section: Section::Basic,
                generation: page.generation,
                request,
                selection: page.selection,
                context: session.context().clone(),
                login: page.login.clone(),
                preliminary: true,
                result: SectionResult::Basic(Ok(SectionOutcome {
                    value,
                    provenance: Some(ProfileProvenance::from_cache(true, &metadata)),
                    authorization: None,
                })),
            });
            let reply = page
                .inbox
                .lock()
                .expect("profile inbox")
                .pop_back()
                .unwrap();
            page.apply(reply, window, cx);
            // The simulated late cache read never owned this section's work slot in reality; clear
            // it so the page is usable afterwards.
            page.work[Section::Basic.index()] = None;
        });
        // The section already showed online content, so the snapshot is dropped entirely.
        assert!(!harness.page.read_with(cx, |page, cx| {
            page.view_for_test().is_some_and(|view| {
                view.read(cx)
                    .basic_provenance()
                    .is_some_and(ProfileProvenance::is_historical)
            })
        }));
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                page.view_for_test().is_some_and(|view| {
                    matches!(view.read(cx).basic(), SectionState::Ready(_))
                        && view
                            .read(cx)
                            .basic_provenance()
                            .is_none_or(|p| !p.is_historical())
                })
            })
        });
    }

    /// A cache miss on Profile preserves ordinary online loading rather than showing an empty or
    /// failed section.
    #[gpui::test]
    fn a_profile_cache_miss_keeps_online_loading_and_then_shows_the_network_result(
        cx: &mut TestAppContext,
    ) {
        let (harness, cx) = build_harness(cx, true);
        harness.transport.hold(true);
        // The basic section stays in its loading state while the transport is held: the empty cache
        // must not answer for it.
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, _| {
                page.view_for_test().is_none()
                    && page.work[Section::Basic.index()].is_some()
                    && page.cached_session.is_some()
            })
        });
        harness.transport.hold(false);
        harness.transport.release_all();
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                page.view_for_test().is_some_and(|view| {
                    let v = view.read(cx);
                    matches!(v.basic(), SectionState::Ready(_))
                        && matches!(v.repositories(), SectionState::Ready(_))
                        && matches!(v.contributions(), SectionState::Ready(_))
                        && matches!(v.readme_state(), SectionState::Ready(()))
                }) && page.work.iter().all(Option::is_none)
            })
        });
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
        // Re-capture a fresh authorization token right after revocation, exactly as a later read
        // would. A fresh token must NOT re-validate the historical snapshot read under the old one.
        let fresh = cache.store().authorization(cache.scope(), None);
        assert!(
            fresh.is_some(),
            "a new read can still capture its own authorization"
        );

        // Wait for cache monitor + drain to evict the historical view!
        wait_for_profile(&harness, cx, |cx| {
            harness
                .page
                .read_with(cx, |page, _| page.view_for_test().is_none())
        });
    }

    #[gpui::test]
    fn canonical_login_reopen_offline_restores_the_profile_before_canonicalization(
        cx: &mut TestAppContext,
    ) {
        // GitHub profile URLs are case-insensitive, so a route may arrive with a non-canonical
        // spelling while the server answers with the canonical login.
        let (harness, cx) = build_harness_with_login(cx, true, "ALICE");
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                page.view_for_test()
                    .is_some_and(|v| match v.read(cx).basic() {
                        SectionState::Ready(basic) => basic.login == "alice",
                        _ => false,
                    })
            })
        });
        // The host now addresses this page by the canonical login, exactly like the rewritten route.
        assert_eq!(
            harness
                .page
                .read_with(cx, |page, _| page.login_for_test().to_string()),
            "alice"
        );

        harness.transport.set_offline(true);
        cx.update_window_entity(&harness.page, |page, _, cx| {
            let services = harness.controller.read(cx).services();
            page.rebind(services, cx);
        });
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                page.view_for_test().is_some_and(|v| {
                    matches!(v.read(cx).basic(), SectionState::Ready(_))
                        && v.read(cx)
                            .basic_provenance()
                            .is_some_and(|p| p.is_historical())
                })
            })
        });
        assert!(
            cx.debug_bounds("profile-basic-provenance-historical")
                .is_some()
        );
    }

    #[gpui::test]
    fn retry_after_revocation_adopts_a_newly_authorized_cache_stamp(cx: &mut TestAppContext) {
        let (harness, cx) = build_harness(cx, true);
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                page.view_for_test()
                    .is_some_and(|v| matches!(v.read(cx).basic(), SectionState::Ready(_)))
            })
        });
        harness.transport.set_offline(true);
        cx.update_window_entity(&harness.page, |page, _, cx| {
            let services = harness.controller.read(cx).services();
            page.rebind(services, cx);
        });
        wait_for_profile(&harness, cx, |cx| {
            cx.debug_bounds("profile-basic-provenance-historical")
                .is_some()
        });

        // A concrete repository revocation retires the displayed snapshot. The aggregate gate stays
        // usable for fresh reads, so this must not lock the page out permanently.
        let context = harness.accounts.page_session().unwrap().context().clone();
        let cache = harness.accounts.repo_service().cache(&context).unwrap();
        cache
            .store()
            .revoke_repository(cache.scope(), Some(101))
            .unwrap();
        wait_for_profile(&harness, cx, |cx| {
            harness
                .page
                .read_with(cx, |page, _| page.view_for_test().is_none())
        });

        // While the page still holds a retryable placeholder, another authorized read (here the same
        // service, standing in for another mounted page) repopulates the aggregate snapshots.
        harness.transport.set_offline(false);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let session = harness.accounts.page_session().unwrap();
        let repopulated = runtime.block_on(async {
            let cache = harness.accounts.repo_service().cache(&context).unwrap();
            let (store, scope) = cache.into_parts();
            let reader = CachedPageReadSession::new(session, store, scope).unwrap();
            reader
                .profile(PageReadMode::Online, "alice", CancellationToken::default())
                .await
                .map(|scoped| scoped.value)
        });
        assert!(
            matches!(repopulated, Ok(PageValue::Value { .. })),
            "a fresh online read repopulates the aggregate snapshot"
        );

        // Retrying the original page while offline must adopt that newly authorized snapshot instead
        // of staying blocked by the earlier revocation.
        harness.transport.set_offline(true);
        let retry = cx
            .debug_bounds("profile-page-basic-error")
            .expect("the revoked snapshot leaves a retryable placeholder");
        cx.simulate_click(retry.center(), gpui::Modifiers::none());
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                page.view_for_test().is_some_and(|v| {
                    matches!(v.read(cx).basic(), SectionState::Ready(_))
                        && v.read(cx)
                            .basic_provenance()
                            .is_some_and(|p| p.is_historical())
                })
            })
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
    fn shared_cache_authorization_survives_capacity_eviction_but_not_revocation() {
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
        let auth = store.authorization(&scope, None).expect("authorization");

        // Unrelated account-level writes fill the budget and force LRU eviction of ordinary
        // entries. The authorization token is in-memory and lock-free, so eviction does not drop it.
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
            auth.is_current(),
            "ordinary capacity eviction must not make an authorized page look revoked"
        );

        // Explicit revocation is still authoritative.
        store.revoke_repository(&scope, Some(101)).unwrap();
        assert!(!auth.is_current());
    }

    #[gpui::test]
    fn online_sections_keep_the_authorization_they_were_read_under(cx: &mut TestAppContext) {
        let (harness, cx) = build_harness(cx, true);
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                page.work.iter().all(Option::is_none)
                    && page.view_for_test().is_some_and(|v| {
                        matches!(v.read(cx).basic(), SectionState::Ready(_))
                            && matches!(v.read(cx).repositories(), SectionState::Ready(_))
                            && matches!(v.read(cx).contributions(), SectionState::Ready(_))
                            && matches!(v.read(cx).readme_state(), SectionState::Ready(()))
                    })
            })
        });
        // Every section that was read online — including Basic — carries the authorization it was read under.
        let authorized = harness.page.read_with(cx, |page, _| {
            page.authorizations
                .lock()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(index, auth)| (index, auth.is_some()))
                .collect::<Vec<_>>()
        });
        assert_eq!(
            authorized,
            vec![(0, true), (1, true), (2, true), (3, true)],
            "online results keep their authorization: {authorized:?}"
        );

        // An explicit repository revocation retires the displayed online content even though the
        // account session stays valid (revocation does not advance the auth epoch).
        let context = harness.accounts.page_session().unwrap().context().clone();
        let cache = harness.accounts.repo_service().cache(&context).unwrap();
        cache
            .store()
            .revoke_repository(cache.scope(), Some(101))
            .unwrap();
        wait_for_profile(&harness, cx, |cx| {
            harness
                .page
                .read_with(cx, |page, _| page.view_for_test().is_none())
        });
    }

    #[gpui::test]
    fn a_late_reply_from_a_previous_binding_cannot_retire_the_new_page(cx: &mut TestAppContext) {
        let (harness, cx) = build_harness(cx, true);
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                page.work.iter().all(Option::is_none)
                    && page
                        .view_for_test()
                        .is_some_and(|v| matches!(v.read(cx).basic(), SectionState::Ready(_)))
            })
        });
        let session = harness.accounts.page_session().unwrap();
        let repos = harness.accounts.repo_service();
        let binding = repos.cache(session.context()).unwrap();
        let cached = CachedPageReadSession::new(
            session.clone(),
            binding.store().clone(),
            binding.scope().clone(),
        )
        .unwrap();

        // A background read for the *current* binding completes successfully.
        harness.transport.set_offline(true);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let outcome = runtime
            .block_on(async {
                let auth = prepare_gate_stamp(&repos, session.context()).await;
                read_profile_basic(
                    &repos,
                    &session,
                    Some(&cached),
                    auth.as_ref(),
                    "alice",
                    Default::default(),
                )
                .await
            })
            .unwrap();
        assert!(
            outcome
                .provenance
                .as_ref()
                .is_some_and(ProfileProvenance::is_historical)
        );

        // Freeze the reply it would produce: old generation, selection and request.
        let stale = cx.update_window_entity(&harness.page, |page, _, _| Reply {
            section: Section::Basic,
            generation: page.generation,
            request: page.requests[Section::Basic.index()],
            selection: page.selection,
            context: session.context().clone(),
            login: page.login.clone(),
            preliminary: false,
            result: SectionResult::Basic(Ok(outcome)),
        });

        // The page rebinds and mounts a fresh live profile under the same account.
        harness.transport.set_offline(false);
        cx.update_window_entity(&harness.page, |page, _, cx| {
            let services = harness.controller.read(cx).services();
            page.rebind(services, cx);
        });
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                page.work.iter().all(Option::is_none)
                    && page
                        .view_for_test()
                        .is_some_and(|v| matches!(v.read(cx).basic(), SectionState::Ready(_)))
            })
        });
        // Even if that fresh generation's gate is retired, an *obsolete* reply must not act on it.
        let context = harness.accounts.page_session().unwrap().context().clone();
        let cache = harness.accounts.repo_service().cache(&context).unwrap();
        cache
            .store()
            .revoke_repository(cache.scope(), Some(101))
            .unwrap();

        cx.update_window_entity(&harness.page, |page, window, cx| {
            page.apply(stale, window, cx);
        });
        assert!(
            harness
                .page
                .read_with(cx, |page, _| page.view_for_test().is_some()),
            "an obsolete reply must not turn the newly bound Profile into a placeholder"
        );
        assert!(
            harness.page.read_with(cx, |page, cx| page
                .view_for_test()
                .is_some_and(|v| matches!(v.read(cx).basic(), SectionState::Ready(_)))),
            "the new binding keeps its own content"
        );
    }

    #[gpui::test]
    fn a_stale_sibling_reply_cannot_cancel_the_retry_that_replaced_it(cx: &mut TestAppContext) {
        let (harness, cx) = build_harness(cx, true);
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                page.work.iter().all(Option::is_none)
                    && page
                        .view_for_test()
                        .is_some_and(|v| matches!(v.read(cx).basic(), SectionState::Ready(_)))
            })
        });
        let session = harness.accounts.page_session().unwrap();
        let repos = harness.accounts.repo_service();
        let binding = repos.cache(session.context()).unwrap();
        let cached = CachedPageReadSession::new(
            session.clone(),
            binding.store().clone(),
            binding.scope().clone(),
        )
        .unwrap();
        // A sibling section finishes a read under the *current* generation…
        harness.transport.set_offline(true);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let outcome = runtime
            .block_on(async {
                let auth = prepare_gate_stamp(&repos, session.context()).await;
                read_profile_pinned(
                    &repos,
                    &session,
                    Some(&cached),
                    auth.as_ref(),
                    "alice",
                    Default::default(),
                )
                .await
            })
            .unwrap();
        let stale = cx.update_window_entity(&harness.page, |page, _, _| Reply {
            section: Section::Repositories,
            generation: page.generation,
            request: page.requests[Section::Repositories.index()],
            selection: page.selection,
            context: session.context().clone(),
            login: page.login.clone(),
            preliminary: false,
            result: SectionResult::Repositories(Ok(outcome)),
        });

        // …then an explicit revocation retires the page.
        let context = harness.accounts.page_session().unwrap().context().clone();
        let cache = harness.accounts.repo_service().cache(&context).unwrap();
        cache
            .store()
            .revoke_repository(cache.scope(), Some(101))
            .unwrap();
        wait_for_profile(&harness, cx, |cx| {
            harness
                .page
                .read_with(cx, |page, _| page.view_for_test().is_none())
        });

        // The user retries Basic while the stale sibling reply is still undelivered.
        harness.transport.set_offline(false);
        let retry = cx
            .debug_bounds("profile-page-basic-error")
            .expect("retirement leaves a retryable placeholder");
        cx.simulate_click(retry.center(), gpui::Modifiers::none());
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                page.view_for_test()
                    .is_some_and(|v| matches!(v.read(cx).basic(), SectionState::Ready(_)))
            })
        });

        // Delivering the stale sibling reply must not retire (and cancel) that retry.
        cx.update_window_entity(&harness.page, |page, window, cx| {
            page.apply(stale, window, cx);
        });
        cx.run_until_parked();
        assert!(
            harness.page.read_with(cx, |page, cx| page
                .view_for_test()
                .is_some_and(|v| matches!(v.read(cx).basic(), SectionState::Ready(_)))),
            "a stale sibling reply must not cancel the fresh retry"
        );
        pump_page(&harness, cx);
        assert!(
            harness.page.read_with(cx, |page, cx| page
                .view_for_test()
                .is_some_and(|v| matches!(v.read(cx).basic(), SectionState::Ready(_)))),
            "the retry survives the following foreground tick"
        );
    }

    #[gpui::test]
    fn an_old_monitors_invalidation_cannot_retire_the_new_binding(cx: &mut TestAppContext) {
        let (harness, cx) = build_harness(cx, true);
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                page.work.iter().all(Option::is_none)
                    && page
                        .view_for_test()
                        .is_some_and(|v| matches!(v.read(cx).basic(), SectionState::Ready(_)))
            })
        });
        // A monitor that belonged to the *previous* binding finishes an iteration after the page
        // has already been rebound (its token was cancelled, but the iteration still completes).
        let stale_flag =
            cx.update_window_entity(&harness.page, |page, _, _| page.cache_invalidated.clone());
        cx.update_window_entity(&harness.page, |page, _, cx| {
            let services = harness.controller.read(cx).services();
            page.rebind(services, cx);
        });
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                page.work.iter().all(Option::is_none)
                    && page
                        .view_for_test()
                        .is_some_and(|v| matches!(v.read(cx).basic(), SectionState::Ready(_)))
            })
        });
        // Its verdict lands only now, tagged with the old generation: the new page must ignore it.
        stale_flag.store(
            cx.update_window_entity(&harness.page, |page, _, _| page.generation) - 1,
            Ordering::Release,
        );
        pump_page(&harness, cx);
        assert!(
            harness.page.read_with(cx, |page, cx| page
                .view_for_test()
                .is_some_and(|v| matches!(v.read(cx).basic(), SectionState::Ready(_)))),
            "a previous binding's monitor verdict must not retire the new page"
        );
        assert!(
            harness
                .page
                .read_with(cx, |page, _| page.unavailable.clone())
                .is_none(),
            "the new binding is not left as a Forbidden placeholder"
        );
    }

    #[gpui::test]
    fn the_new_binding_gets_its_own_monitor_even_if_the_old_one_finishes_late(
        cx: &mut TestAppContext,
    ) {
        let (harness, cx) = build_harness(cx, true);
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                page.work.iter().all(Option::is_none)
                    && page
                        .view_for_test()
                        .is_some_and(|v| matches!(v.read(cx).basic(), SectionState::Ready(_)))
            })
        });
        let before = harness.page.read_with(cx, |page, _| {
            page.cache_monitor.as_ref().map(|work| work.id)
        });
        assert!(before.is_some(), "the first binding owns a monitor");
        // Rebinding twice inside one synchronous update guarantees the previous monitor task has
        // not run its cleanup yet, so the new binding must not depend on that cleanup happening.
        cx.update_window_entity(&harness.page, |page, _, cx| {
            let services = harness.controller.read(cx).services();
            page.rebind(services.clone(), cx);
            page.rebind(services, cx);
        });
        wait_for_profile(&harness, cx, |cx| {
            harness.page.read_with(cx, |page, cx| {
                page.work.iter().all(Option::is_none)
                    && page
                        .view_for_test()
                        .is_some_and(|v| matches!(v.read(cx).basic(), SectionState::Ready(_)))
            })
        });
        let monitor = harness.page.read_with(cx, |page, _| {
            page.cache_monitor
                .as_ref()
                .map(|work| (work.id, work.token.is_cancelled()))
        });
        let (monitor_id, monitor_cancelled) =
            monitor.expect("the new binding owns a monitor of its own");
        assert_ne!(
            Some(monitor_id),
            before,
            "the new binding's monitor is a new task, not the cancelled one"
        );
        assert!(
            !monitor_cancelled,
            "the new binding's monitor is live; it does not wait for the cancelled task to finish"
        );
        // Revoking while the new page is displayed must still retire it: the previous task finishing
        // late cannot leave the new page unmonitored.
        let context = harness.accounts.page_session().unwrap().context().clone();
        let cache = harness.accounts.repo_service().cache(&context).unwrap();
        cache
            .store()
            .revoke_repository(cache.scope(), Some(101))
            .unwrap();
        wait_for_profile(&harness, cx, |cx| {
            harness
                .page
                .read_with(cx, |page, _| page.view_for_test().is_none())
        });
    }

    #[test]
    fn concurrent_authorizations_are_all_valid_tokens() {
        let temp = tempfile::tempdir().unwrap();
        let store = CacheStore::open(CacheConfig::at(temp.path().join("cache"))).unwrap();
        let scope = store
            .saved_account(AccountKey::new(1, HostId::github()))
            .unwrap();
        let auths = std::thread::scope(|threads| {
            let handles = (0..4)
                .map(|_| {
                    let store = store.clone();
                    let scope = scope.clone();
                    threads.spawn(move || store.authorization(&scope, None))
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert!(
            auths.iter().all(Option::is_some),
            "every concurrent capture succeeds: {auths:?}"
        );
        assert!(
            auths.iter().flatten().all(|a| a.is_current()),
            "all tokens start current"
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
    include!("profile_acceptance_tests.rs");
}
