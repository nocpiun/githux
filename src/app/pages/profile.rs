//! Production Profile page host on Native Shell.
//!
//! One [`PageContext`] per mount, one [`PageSlot`] per section, and the
//! [`ProfileView`] built from the real server identity once the basic profile
//! arrives. Each section has its own request, cancellation token and captured
//! `CacheAuthorization`; publication re-validates them, so a revocation or an
//! account switch retires both in-flight replies and displayed sections in the
//! same drain (LESSONS L022). README Markdown is parsed on the background
//! executor; the UI thread only installs the prepared document and polls its
//! image bridge.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nsh_core::element::{ImageSrc, create_root};
use nsh_core::prelude::*;

#[path = "profile/reads.rs"]
mod reads;

use crate::app::runtime::{PageContext, PageSlot, PageTicket, RuntimeHandle, Sessions};
use crate::content::images::ImageProvider;
use crate::data::page_cache::CachedPageReadSession;
use crate::data::{PageReadSession, ProfileReadme, RepositoryReadContext};
use crate::domain::profile::{
    ContributionCalendar, ContributionRange, ProfileBasic, ProfilePinned,
};
use crate::domain::repository::RepositoryIdentity;
use crate::storage::cache::CacheAuthorization;
use crate::ui::profile::{
    PresentationError, ProfileAction, ProfileActionKind, ProfileBinding, ProfileProvenance,
    ProfileTarget, ProfileView, SectionState,
};
use crate::ui::reader::{DocumentKind, LinkTarget, ReaderAction, ReaderDocument};
use reads::{
    SectionOutcome, read_profile_basic, read_profile_contributions, read_profile_pinned,
    read_profile_readme,
};

use super::{NavigationSink, PageNavigation, PageView};
use crate::workspace::Route;

/// The last rolling inclusive UTC year. GitHub accepts at most 366 days between
/// endpoints; a leap-year interval therefore moves the start forward one day
/// instead of requesting an invalid 367-day inclusive range.
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

/// Host-provided README image provider factory.
///
/// The Shell owns the application executor and the shared external-image client,
/// so it builds the existing `RepositoryImageService::with_cache(...)` for the
/// captured session and repository. The page still owns the authorization guard:
/// it wraps every provider in the `CacheAuthorization` captured for that reply.
pub type ReadmeImageProviderFactory =
    Rc<dyn Fn(&RepositoryReadContext, &RepositoryIdentity) -> Option<Arc<dyn ImageProvider>>>;

/// What the README read produced, already classified for presentation.
#[derive(Clone)]
enum ReadmePayload {
    Document {
        document: Arc<ReaderDocument>,
        repository: RepositoryIdentity,
        provenance: Option<ProfileProvenance>,
        authorization: Option<CacheAuthorization>,
    },
    /// A confirmed non-error absence: no eligible public same-name repository or
    /// no root README. The section is omitted entirely.
    Absent,
}

enum Reply {
    Basic {
        request: u64,
        result: Result<SectionOutcome<ProfileBasic>, PresentationError>,
    },
    Repositories {
        request: u64,
        result: Result<SectionOutcome<ProfilePinned>, PresentationError>,
    },
    Contributions {
        request: u64,
        result:
            Result<(SectionOutcome<ContributionCalendar>, ContributionRange), PresentationError>,
    },
    Readme {
        request: u64,
        result: Result<ReadmePayload, PresentationError>,
    },
}

type BasicSlot = Result<SectionOutcome<ProfileBasic>, PresentationError>;
type RepositoriesSlot = Result<SectionOutcome<ProfilePinned>, PresentationError>;
type ContributionsSlot =
    Result<(SectionOutcome<ContributionCalendar>, ContributionRange), PresentationError>;
type ReadmeSlot = Result<ReadmePayload, PresentationError>;

pub struct ProfilePage {
    context: PageContext,
    login: String,
    navigation: NavigationSink,
    actions: Rc<RefCell<VecDeque<ProfileAction>>>,
    range: ContributionRange,
    session: Option<PageReadSession>,
    cached_session: Option<CachedPageReadSession>,
    avatar_image: Option<ImageSrc>,
    avatars: Option<Rc<crate::app::avatar::AvatarLoader>>,
    provider_factory: Option<ReadmeImageProviderFactory>,
    basic_slot: PageSlot<BasicSlot>,
    repositories_slot: PageSlot<RepositoriesSlot>,
    contributions_slot: PageSlot<ContributionsSlot>,
    readme_slot: PageSlot<ReadmeSlot>,
    basic_ticket: Option<PageTicket>,
    repositories_ticket: Option<PageTicket>,
    contributions_ticket: Option<PageTicket>,
    readme_ticket: Option<PageTicket>,
    inbox: Arc<std::sync::Mutex<VecDeque<Reply>>>,
    requests: [u64; 4],
    working: [bool; 4],
    shown: [bool; 4],
    view_owner: Option<Owner>,
    view: Option<ProfileView>,
    unavailable: Option<PresentationError>,
    avatar_signal: Signal<u64>,
    last_clock: std::time::Instant,
    active: bool,
    retired: bool,
}

impl ProfilePage {
    /// The documented factory: real services, real navigation sink.
    pub fn new(context: PageContext, login: String, navigation: NavigationSink) -> Self {
        let mut page = Self {
            context,
            login,
            navigation,
            actions: Rc::new(RefCell::new(VecDeque::new())),
            range: rolling_year(),
            session: None,
            cached_session: None,
            avatar_image: None,
            avatars: None,
            provider_factory: None,
            basic_slot: PageSlot::default(),
            repositories_slot: PageSlot::default(),
            contributions_slot: PageSlot::default(),
            readme_slot: PageSlot::default(),
            basic_ticket: None,
            repositories_ticket: None,
            contributions_ticket: None,
            readme_ticket: None,
            inbox: Arc::new(std::sync::Mutex::new(VecDeque::new())),
            requests: [0; 4],
            working: [false; 4],
            shown: [false; 4],
            view_owner: None,
            view: None,
            unavailable: None,
            avatar_signal: signal(0u64),
            last_clock: std::time::Instant::now(),
            active: true,
            retired: false,
        };
        page.capture_session();
        page.start(Section::Basic);
        page
    }

    /// Host hook for the real persistent README image provider. Without it the
    /// reader renders its honest "no image service attached" placeholders; nothing
    /// here opens a second network or cache stack.
    pub fn set_readme_image_provider(&mut self, factory: Option<ReadmeImageProviderFactory>) {
        self.provider_factory = factory;
    }

    /// Host-supplied decoded header avatar. The account avatar loader owns
    /// fetching, decoding and its disk cache; this page only draws the result.
    pub fn set_avatar_loader(&mut self, loader: Option<Rc<crate::app::avatar::AvatarLoader>>) {
        self.avatars = loader;
    }

    pub fn set_avatar_image(&mut self, image: Option<ImageSrc>) {
        self.avatar_image = image;
    }

    fn capture_session(&mut self) {
        let Ok(session) = self.context.page_session() else {
            return;
        };
        if !self
            .context
            .services()
            .repositories()
            .is_current(session.context())
        {
            return;
        }
        self.cached_session = self.context.cache().cloned().and_then(|cache| {
            let (store, scope) = cache.into_parts();
            CachedPageReadSession::new(session.clone(), store, scope).ok()
        });
        self.session = Some(session);
    }

    fn authorization(&self) -> Option<CacheAuthorization> {
        let session = self.session.as_ref()?;
        let cache = self
            .context
            .services()
            .repositories()
            .cache(session.context())?;
        cache.store().authorization(cache.scope(), None)
    }

    fn start(&mut self, section: Section) {
        if self.retired {
            return;
        }
        let index = section.index();
        if self.working[index] {
            return;
        }
        if self.session.is_none() {
            self.capture_session();
        }
        let Some(session) = self.session.clone() else {
            return;
        };
        let context = session.context().clone();
        if !self.context.services().repositories().is_current(&context) {
            return;
        }
        // Capture this read's authorization token *before* it runs. Both calls
        // are in-process (lock-free, disk-free), so this never blocks the UI.
        let authorization = self.authorization();
        // A cache binding whose token cannot be captured fails closed: publishing
        // the read anyway would present protected cached work as unguarded output.
        if self.cached_session.is_some() && authorization.is_none() {
            self.working[index] = false;
            self.present_failure(section, PresentationError::Unavailable);
            return;
        }
        self.requests[index] = self.requests[index].saturating_add(1);
        let request = self.requests[index];
        self.working[index] = true;
        if let Some(view) = self.view.as_mut() {
            let request = view.request();
            match section {
                Section::Basic => {
                    view.update_basic_with_provenance(&request, SectionState::Loading, None);
                }
                Section::Repositories => {
                    view.update_repositories_with_provenance(&request, SectionState::Loading, None);
                }
                Section::Contributions => {
                    let range = self.range.clone();
                    view.update_contributions_with_provenance(
                        &request,
                        &range,
                        SectionState::Loading,
                        None,
                    );
                }
                Section::Readme => {}
            }
        }
        let ticket = match section {
            Section::Basic => {
                let ticket = self.basic_slot.begin(self.context.clone());
                self.basic_ticket = Some(ticket.clone());
                ticket
            }
            Section::Repositories => {
                let ticket = self.repositories_slot.begin(self.context.clone());
                self.repositories_ticket = Some(ticket.clone());
                ticket
            }
            Section::Contributions => {
                let ticket = self.contributions_slot.begin(self.context.clone());
                self.contributions_ticket = Some(ticket.clone());
                ticket
            }
            Section::Readme => {
                let ticket = self.readme_slot.begin(self.context.clone());
                self.readme_ticket = Some(ticket.clone());
                ticket
            }
        };
        let cancel = ticket.cancellation();
        let generator = self.context.services().clone();
        let repos = self.context.services().repositories();
        let cached = self.cached_session.clone();
        let login = self.login.clone();
        let range = self.range.clone();
        let basic = self.view.as_ref().and_then(|view| match view.basic() {
            SectionState::Ready(basic) => Some(basic),
            _ => None,
        });
        let inbox = self.inbox.clone();
        let spawned = generator.spawn(async move {
            let authorization = authorization.as_ref();
            let result = match section {
                Section::Basic => Reply::Basic {
                    request,
                    result: read_profile_basic(
                        &repos,
                        &session,
                        cached.as_ref(),
                        authorization,
                        &login,
                        cancel,
                    )
                    .await,
                },
                Section::Repositories => Reply::Repositories {
                    request,
                    result: read_profile_pinned(
                        &repos,
                        &session,
                        cached.as_ref(),
                        authorization,
                        &login,
                        cancel,
                    )
                    .await,
                },
                Section::Contributions => Reply::Contributions {
                    request,
                    result: read_profile_contributions(
                        &repos,
                        &session,
                        cached.as_ref(),
                        authorization,
                        &login,
                        &range,
                        cancel,
                    )
                    .await,
                },
                Section::Readme => Reply::Readme {
                    request,
                    result: match basic {
                        Some(basic) => {
                            let read = read_profile_readme(
                                &repos,
                                &session,
                                cached.as_ref(),
                                authorization,
                                &basic,
                                cancel,
                            )
                            .await;
                            // Parse Markdown off the UI thread; the prepared
                            // document crosses the boundary as immutable data.
                            read.map(prepare_readme)
                        }
                        None => Err(PresentationError::InvalidData),
                    },
                },
            };
            inbox.lock().expect("profile inbox").push_back(result);
        });
        if spawned.is_err() {
            self.working[index] = false;
            self.present_failure(section, PresentationError::Unavailable);
        }
    }

    fn present_failure(&mut self, section: Section, error: PresentationError) {
        if section == Section::Basic {
            self.unavailable = Some(error);
            return;
        }
        let Some(view) = self.view.as_mut() else {
            return;
        };
        let request = view.request();
        match section {
            Section::Repositories => {
                view.update_repositories_with_provenance(
                    &request,
                    SectionState::Error(error),
                    None,
                );
            }
            Section::Contributions => {
                let range = self.range.clone();
                view.update_contributions_with_provenance(
                    &request,
                    &range,
                    SectionState::Error(error),
                    None,
                );
            }
            Section::Readme => {
                view.set_readme_error(&request, error, None, None);
            }
            Section::Basic => {}
        }
    }

    fn action(&mut self, action: ProfileAction) {
        let Some(view) = self.view.as_mut() else {
            return;
        };
        if !view.accepts(&action.request) {
            return;
        }
        match action.kind {
            ProfileActionKind::Profile => {
                let active = self.context.services().accounts().active_summary();
                (self.navigation)(PageNavigation::Open(Route::profile(
                    self.login.clone(),
                    active.as_ref(),
                )));
            }
            ProfileActionKind::Repository(repository) => {
                if let Some((owner, name)) = repository.name_with_owner.split_once('/') {
                    (self.navigation)(PageNavigation::Open(Route::Repository {
                        namespace: owner.to_string(),
                        name: name.to_string(),
                    }));
                }
            }
            ProfileActionKind::ExternalLink { url } => {
                (self.navigation)(PageNavigation::External(url));
            }
            ProfileActionKind::Readme(ReaderAction::Link(LinkTarget::External { url })) => {
                (self.navigation)(PageNavigation::External(url));
            }
            ProfileActionKind::Readme(ReaderAction::Link(LinkTarget::Repository(link))) => {
                // Profile README repository-relative links open the real
                // repository inside Githux, never a browser tab.
                (self.navigation)(PageNavigation::Open(Route::Repository {
                    namespace: link.owner,
                    name: link.name,
                }));
            }
            ProfileActionKind::Readme(ReaderAction::AuthorizationRetired) => {
                self.retire_displayed_sections();
            }
            ProfileActionKind::Readme(_) => {}
            ProfileActionKind::RetryBasic => {
                self.unavailable = None;
                self.start(Section::Basic);
            }
            ProfileActionKind::RetryRepositories => self.start(Section::Repositories),
            ProfileActionKind::RetryContributions => self.start(Section::Contributions),
            ProfileActionKind::RetryReadme => self.start(Section::Readme),
        }
    }

    fn apply_basic(
        &mut self,
        request: u64,
        result: Result<SectionOutcome<ProfileBasic>, PresentationError>,
        sessions: &Sessions,
    ) {
        if request != self.requests[0] {
            return;
        }
        self.working[0] = false;
        let Some(ticket) = self.basic_ticket.take() else {
            return;
        };
        if !self.basic_slot.publish(ticket, result.clone(), sessions) {
            return;
        }
        self.shown[0] = true;
        match result {
            Ok(outcome) => {
                let SectionOutcome {
                    value: basic,
                    provenance,
                    ..
                } = outcome;
                // Server id/login are authoritative. An earlier login-only action
                // is never used as a numeric identity; the Shell canonicalizes the
                // matching TabId after this page reports it.
                self.login = basic.login.clone();
                let target = ProfileTarget::new(self.account_key().host.clone(), basic.id);
                let binding = ProfileBinding::new(self.account_key(), self.auth_epoch());
                let view = self.ensure_view(target, binding);
                let request = view.request();
                let applied = view.update_basic_with_provenance(
                    &request,
                    SectionState::Ready(basic.clone()),
                    provenance,
                );
                if applied {
                    self.sync_avatar();
                }
                for section in [
                    Section::Repositories,
                    Section::Contributions,
                    Section::Readme,
                ] {
                    self.start(section);
                }
                (self.navigation)(PageNavigation::CanonicalProfile {
                    tab: self.context.identity().tab,
                    login: basic.login,
                });
            }
            Err(error) => {
                if let Some((owner, mut view)) = self.view_owner.take().zip(self.view.take()) {
                    view.drop_reader();
                    owner.dispose();
                }
                self.unavailable = Some(error);
            }
        }
    }

    fn account_key(&self) -> crate::domain::AccountKey {
        self.context.context().auth().account.clone()
    }

    fn auth_epoch(&self) -> crate::domain::AuthEpoch {
        self.context.context().auth().epoch
    }

    fn ensure_view(&mut self, target: ProfileTarget, binding: ProfileBinding) -> &mut ProfileView {
        if self.view.is_none() {
            let range = self.range.clone();
            let actions = self.actions.clone();
            let (owner, mut view) = create_root(move || ProfileView::new(target, binding, range));
            view.on_action(Rc::new(move |action| {
                actions.borrow_mut().push_back(action);
            }));
            self.view_owner = Some(owner);
            self.view = Some(view);
        }
        self.view.as_mut().expect("profile view")
    }

    fn sync_avatar(&mut self) {
        // The host supplies decoded avatar images through `set_avatar_image`;
        // without an account avatar loader the header keeps its stable initials.
        let Some(view) = self.view.as_mut() else {
            return;
        };
        let request = view.request();
        if let Some(image) = self.avatar_image.take() {
            view.set_avatar_image(&request, Some(image));
        }
        self.avatar_signal.update(|epoch| *epoch += 1);
    }

    fn apply_repositories(
        &mut self,
        request: u64,
        result: Result<SectionOutcome<ProfilePinned>, PresentationError>,
        sessions: &Sessions,
    ) {
        if request != self.requests[1] {
            return;
        }
        self.working[1] = false;
        let Some(ticket) = self.repositories_ticket.take() else {
            return;
        };
        if !self
            .repositories_slot
            .publish(ticket, result.clone(), sessions)
        {
            return;
        }
        self.shown[1] = true;
        let Some(view) = self.view.as_mut() else {
            return;
        };
        let profile_request = view.request();
        match result {
            Ok(outcome) => {
                view.update_repositories_with_provenance(
                    &profile_request,
                    SectionState::Ready(outcome.value),
                    outcome.provenance,
                );
            }
            Err(error) => {
                view.update_repositories_with_provenance(
                    &profile_request,
                    SectionState::Error(error),
                    None,
                );
            }
        }
    }

    fn apply_contributions(
        &mut self,
        request: u64,
        result: ContributionsSlot,
        sessions: &Sessions,
    ) {
        if request != self.requests[2] {
            return;
        }
        self.working[2] = false;
        let Some(ticket) = self.contributions_ticket.take() else {
            return;
        };
        if !self
            .contributions_slot
            .publish(ticket, result.clone(), sessions)
        {
            return;
        }
        self.shown[2] = true;
        let Some(view) = self.view.as_mut() else {
            return;
        };
        let request = view.request();
        match result {
            Ok((outcome, range)) => {
                // An offline snapshot can belong to an earlier rolling year; adopt
                // the interval it was stored under so the section, its provenance
                // and the visible range stay consistent with the restored data.
                if range != self.range {
                    self.range = range.clone();
                    view.set_range(range.clone());
                }
                view.update_contributions_with_provenance(
                    &request,
                    &range,
                    SectionState::Ready(outcome.value),
                    outcome.provenance,
                );
            }
            Err(error) => {
                let range = self.range.clone();
                view.update_contributions_with_provenance(
                    &request,
                    &range,
                    SectionState::Error(error),
                    None,
                );
            }
        }
    }

    fn apply_readme(&mut self, request: u64, result: ReadmeSlot, sessions: &Sessions) {
        if request != self.requests[3] {
            return;
        }
        self.working[3] = false;
        let Some(ticket) = self.readme_ticket.take() else {
            return;
        };
        if !self.readme_slot.publish(ticket, result.clone(), sessions) {
            return;
        }
        self.shown[3] = true;
        let Some(view) = self.view.as_mut() else {
            return;
        };
        let profile_request = view.request();
        match result {
            Ok(ReadmePayload::Document {
                document,
                repository,
                provenance,
                authorization,
            }) => {
                let provider = self
                    .provider_factory
                    .as_ref()
                    .and_then(|factory| factory(self.session.as_ref()?.context(), &repository));
                let guard = capability_guard(
                    self.context.services().repositories(),
                    self.session
                        .as_ref()
                        .map(|session| session.context().clone()),
                    authorization,
                );
                view.set_readme_document(&profile_request, document, provider, guard, provenance);
            }
            Ok(ReadmePayload::Absent) => {
                view.set_readme_unavailable_with_provenance(
                    &profile_request,
                    "No root README.md exists in this user's public same-name repository.",
                    None,
                );
            }
            Err(error) => {
                if error == PresentationError::NotFound {
                    view.set_readme_unavailable_with_provenance(
                        &profile_request,
                        "No visible public same-name profile repository or root README.",
                        None,
                    );
                } else {
                    view.set_readme_error(&profile_request, error, None, None);
                }
            }
        }
    }

    /// Drops every authorization-bound value the page is showing, including the
    /// mounted README reader and its image provider, and leaves a retryable
    /// placeholder. Advancing each slot generation makes every outstanding reply
    /// fail its publication check, so a stale sibling reply cannot retire the retry
    /// the user starts after this cleanup.
    fn retire_displayed_sections(&mut self) {
        self.cancel_work();
        if let Some((owner, mut view)) = self.view_owner.take().zip(self.view.take()) {
            view.drop_reader();
            owner.dispose();
        }
        self.unavailable = Some(PresentationError::Forbidden);
    }

    fn cancel_work(&mut self) {
        self.basic_slot.cancel();
        self.repositories_slot.cancel();
        self.contributions_slot.cancel();
        self.readme_slot.cancel();
        self.basic_ticket = None;
        self.repositories_ticket = None;
        self.contributions_ticket = None;
        self.readme_ticket = None;
        self.working = [false; 4];
        self.shown = [false; 4];
        self.inbox.lock().expect("profile inbox").clear();
    }

    fn clear_stale_display(&mut self, sessions: &Sessions) {
        if self.view.is_none() {
            return;
        }
        let mut stale = false;
        for (index, slot_is_valid) in [
            (0usize, self.basic_slot.is_valid(sessions)),
            (1, self.repositories_slot.is_valid(sessions)),
            (2, self.contributions_slot.is_valid(sessions)),
            (3, self.readme_slot.is_valid(sessions)),
        ] {
            if self.shown[index] && !slot_is_valid {
                stale = true;
            }
        }
        if stale {
            self.retire_displayed_sections();
        }
    }

    fn tick_clock(&mut self) {
        if !self.active || self.last_clock.elapsed() < Duration::from_secs(30) {
            return;
        }
        self.last_clock = std::time::Instant::now();
        if let Some(view) = self.view.as_mut() {
            view.set_now(SystemTime::now());
        }
    }
}

fn capability_guard(
    repos: crate::data::RepoService,
    context: Option<RepositoryReadContext>,
    authorization: Option<CacheAuthorization>,
) -> Arc<dyn Fn() -> bool + Send + Sync> {
    Arc::new(move || {
        authorization
            .as_ref()
            .is_none_or(CacheAuthorization::is_current)
            && context
                .as_ref()
                .is_none_or(|context| repos.is_current(context))
    })
}

/// Classify a section read into presentation intent. Markdown parsing happens
/// here, on the background executor, never on the UI thread.
fn prepare_readme(outcome: SectionOutcome<ProfileReadme>) -> ReadmePayload {
    let SectionOutcome {
        value,
        provenance,
        authorization,
    } = outcome;
    match value {
        ProfileReadme::Prepared {
            repository,
            content,
            ..
        } => ReadmePayload::Document {
            document: ReaderDocument::from_prepared(content, DocumentKind::Markdown),
            repository,
            provenance,
            authorization,
        },
        ProfileReadme::Missing { .. } | ProfileReadme::Unavailable { .. } => ReadmePayload::Absent,
        ProfileReadme::NotEligible { .. } => ReadmePayload::Absent,
    }
}

impl PageView for ProfilePage {
    fn host_presentation(&self) -> Option<String> {
        Some(format!(
            "{}:{}:{:?}",
            self.view.is_some(),
            self.session.is_some(),
            self.unavailable
        ))
    }
    fn element(&self) -> AnyElement {
        if let Some(view) = &self.view {
            return view.element();
        }
        let message = match &self.unavailable {
            Some(error) => error.message().to_string(),
            None if self.session.is_none() => {
                "Sign in to read public profile information. There is no offline page cache."
                    .to_string()
            }
            None => "Loading profile…".to_string(),
        };
        view()
            .test_id("profile-page-unavailable")
            .size_full()
            .items_center()
            .justify_center()
            .p_6()
            .child(
                text(message)
                    .text_center()
                    .text_size(px(13.0))
                    .text_color(crate::ui::foundation::theme::theme().muted_foreground),
            )
            .into_element()
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
        let replies: Vec<Reply> = self
            .inbox
            .lock()
            .expect("profile inbox")
            .drain(..)
            .collect();
        for reply in replies {
            match reply {
                Reply::Basic { request, result } => self.apply_basic(request, result, sessions),
                Reply::Repositories { request, result } => {
                    self.apply_repositories(request, result, sessions)
                }
                Reply::Contributions { request, result } => {
                    self.apply_contributions(request, result, sessions)
                }
                Reply::Readme { request, result } => self.apply_readme(request, result, sessions),
            }
        }
        if let Some(view) = self.view.as_mut() {
            if let Some(avatars) = &self.avatars {
                let image = if let SectionState::Ready(basic) = view.basic() {
                    avatars
                        .image(basic.id, basic.avatar_url.as_deref())
                        .map(|image| ImageSrc::from(image.source.clone()))
                } else {
                    None
                };
                view.set_avatar_image(&view.request(), image);
            }
            view.poll_reader();
        }
        self.tick_clock();
    }

    fn refresh(&mut self, _runtime: &RuntimeHandle) {
        self.start(Section::Basic);
    }

    fn set_active(&mut self, active: bool) {
        self.active = active;
        if !active && let Some(view) = self.view.as_ref() {
            view.suspend();
        }
        if active {
            self.last_clock = std::time::Instant::now();
        }
    }

    fn retire(&mut self) {
        if self.retired {
            return;
        }
        self.retired = true;
        self.cancel_work();
        if let Some((owner, mut view)) = self.view_owner.take().zip(self.view.take()) {
            view.drop_reader();
            owner.dispose();
        }
        self.session = None;
        self.cached_session = None;
    }
}

#[cfg(test)]
#[path = "profile/acceptance.rs"]
mod acceptance;
