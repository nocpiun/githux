use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{AppContext, Context, Entity, IntoElement, Render, Subscription, Window};
use tokio::sync::watch;

use crate::app::accounts::AccountServices;
use crate::app::avatar::AvatarKey;
use crate::data::{PageReadSession, ProfileReadme, ProfileReadmeIneligible, RepositoryReadContext};
use crate::domain::blob::BlobLimits;
use crate::domain::error::GitHubError;
use crate::domain::profile::{
    ContributionCalendar, ContributionRange, ProfileBasic, ProfileError, ProfilePinned,
};
use crate::domain::repository::RepositoryError;
use crate::github::RepositoryLimits;
use crate::github::profile::ProfileLimits;
use crate::network::cancel::{CancellationToken, CancellationTokenSource};
use crate::ui::profile::{
    PresentationError, ProfileAction, ProfileActionKind, ProfileBinding, ProfileRequest,
    ProfileTarget, ProfileView, SectionState,
};
use crate::ui::reader::{LinkTarget, ReaderAction};
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

enum SectionResult {
    Basic(Result<ProfileBasic, ProfileError>),
    Repositories(Result<ProfilePinned, ProfileError>),
    Contributions(Result<ContributionCalendar, ProfileError>),
    Readme(Result<ProfileReadme, RepositoryError>),
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
    selection: u64,
    generation: u64,
    requests: [u64; 4],
    work: [Option<Work>; 4],
    inbox: Arc<Mutex<VecDeque<Reply>>>,
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
        let mut this = Self {
            tab,
            login,
            services: None,
            session: None,
            selection: 0,
            generation: 0,
            requests: [0; 4],
            work: [None, None, None, None],
            inbox: Arc::new(Mutex::new(VecDeque::new())),
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
        self.stop();
        self.services = services;
        self.selection = self.services.as_ref().map_or(0, AccountServices::selection);
        self.generation = self.generation.saturating_add(1);
        self.requests = [0; 4];
        self.request = None;
        self.unavailable = None;
        self.view = None;
        self._changed = None;
        let Some(services) = self.services.clone() else {
            return;
        };
        match services.accounts().page_session() {
            Ok(session) if services.repositories().is_current(session.context()) => {
                self.session = Some(session);
                self.start(Section::Basic, cx);
            }
            _ => {}
        }
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
                let _ = weak.update(cx, |page, cx| page.action(action, window, cx));
            }));
        });
        let request = view.read(cx).request();
        self.request = Some(request);
        self._changed = Some(cx.observe(&view, |_, _, cx| cx.notify()));
        self.view = Some(view);
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
        }
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
        let submitted = services.executor().handle().spawn(async move {
            let result = match section {
                Section::Basic => SectionResult::Basic(
                    session
                        .profile(&login, cancel)
                        .await
                        .map(|value| value.value),
                ),
                Section::Repositories => SectionResult::Repositories(
                    session
                        .profile_pinned(&login, &ProfileLimits::default(), cancel)
                        .await
                        .map(|value| value.value),
                ),
                Section::Contributions => SectionResult::Contributions(
                    session
                        .contribution_calendar(&login, &range, &ProfileLimits::default(), cancel)
                        .await
                        .map(|value| value.value),
                ),
                Section::Readme => SectionResult::Readme(match basic {
                    Some(basic) => session
                        .profile_readme(
                            &basic,
                            &RepositoryLimits::default(),
                            &BlobLimits::default(),
                            cancel,
                        )
                        .await
                        .map(|value| value.value),
                    None => Err(RepositoryError::InvalidInput {
                        field: "profile basic identity",
                    }),
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

    fn accepts(&self, reply: &Reply) -> bool {
        reply.generation == self.generation
            && reply.selection == self.selection
            && reply.request == self.requests[reply.section.index()]
            && reply.login == self.login
            && self
                .services
                .as_ref()
                .is_some_and(|services| services.repositories().is_current(&reply.context))
            && self
                .session
                .as_ref()
                .is_some_and(|session| session.context() == &reply.context)
    }

    fn drain(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn apply(&mut self, reply: Reply, window: &mut Window, cx: &mut Context<Self>) {
        if !self.accepts(&reply) {
            return;
        }
        self.work[reply.section.index()] = None;
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
                        let state = match result {
                            Ok(value) => SectionState::Ready(value),
                            Err(error) => SectionState::Error(error.into()),
                        };
                        view.update(cx, |view, cx| {
                            view.update_repositories(&request, state, cx);
                        });
                    }
                    Section::Contributions => {
                        let SectionResult::Contributions(result) = reply.result else {
                            return;
                        };
                        let state = match result {
                            Ok(value) => SectionState::Ready(value),
                            Err(error) => SectionState::Error(error.into()),
                        };
                        let range = self.range.clone();
                        view.update(cx, |view, cx| {
                            view.update_contributions(&request, &range, state, cx);
                        });
                    }
                    Section::Readme => {
                        let SectionResult::Readme(result) = reply.result else {
                            return;
                        };
                        match result {
                            Ok(ProfileReadme::Prepared { content, .. }) => {
                                view.update(cx, |view, cx| {
                                    view.update_readme(
                                        &request,
                                        SectionState::Ready(content),
                                        window,
                                        cx,
                                    );
                                });
                            }
                            Ok(ProfileReadme::Missing { .. }) => {
                                view.update(cx, |view, cx| {
                                    view.set_readme_unavailable(
                                        &request,
                                        "No root README.md exists in this user's public same-name repository.",
                                        cx,
                                    );
                                });
                            }
                            Ok(ProfileReadme::Unavailable { .. }) => {
                                view.update(cx, |view, cx| {
                                    view.set_readme_unavailable(
                                        &request,
                                        "The root README could not be confirmed from an incomplete repository tree.",
                                        cx,
                                    );
                                });
                            }
                            Ok(ProfileReadme::NotEligible { reason, .. }) => {
                                let message = match reason {
                                    ProfileReadmeIneligible::NotPublicSameNameRepository => {
                                        "This user has no visible public same-name profile repository."
                                    }
                                    ProfileReadmeIneligible::NoDefaultBranch => {
                                        "The public same-name profile repository has no default branch."
                                    }
                                };
                                view.update(cx, |view, cx| {
                                    view.set_readme_unavailable(&request, message, cx);
                                });
                            }
                            Err(error) => {
                                view.update(cx, |view, cx| {
                                    view.update_readme_with_context(
                                        &request,
                                        SectionState::Error(repository_error(error)),
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
            Ok(basic) => {
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
                        view.update_basic(&request, state, cx);
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
                self.unavailable = Some(error.into());
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
        self.inbox.lock().expect("profile inbox").clear();
        self.session = None;
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
            // A late avatar completion only changes presentation; no read is restarted.
            if let Some(basic) = match view.read(cx).basic() {
                SectionState::Ready(basic) => Some(basic.clone()),
                _ => None,
            } {
                self.sync_avatar(basic, cx);
            }
            return view.into_any_element();
        }
        if let Some(error) = &self.unavailable {
            return self.placeholder(error.message(), cx).into_any_element();
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
