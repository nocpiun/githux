//! The native Shell: title bar, sidebar, session tab strip, palette and page host.
//!
//! The Shell owns the single `RuntimeHandle`, drains it on a light foreground tick, rebuilds
//! pages when the account binding revision changes, polls every mounted page (including inactive
//! ones, so revocation is observed) and retires pages whose tab closed.

pub mod accounts;
pub mod avatar;
pub mod badge;
pub mod compact_status;
pub mod foundation;
pub mod geometry;
pub mod global_work_items;
pub mod home;
pub mod notifications;
pub mod page_layout;
pub mod profile;
pub mod reader;
pub mod relative_time;
pub mod repository;
pub mod repository_overview;
pub mod settings;
pub mod tooltip;
pub mod work_item_row;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Mutex;
use std::time::Duration;

use nsh_core::desktop::TitleBarStyle;
use nsh_core::element::ImageSrc;
use nsh_core::element::desktop::set_title_bar;
use nsh_core::prelude::*;

use crate::app::pages::{
    HomePage, NavigationSink, NotificationsPage, PageNavigation, PageView, ProfilePage,
    WorkItemsPage,
};
use crate::app::runtime::{PageContext, RepositoryList, RuntimeHandle, Snapshot};
use crate::domain::RepoSummary;
use crate::ui::foundation::{
    controls::{self, Icon},
    theme as t,
};
use crate::ui::geometry as g;
use crate::ui::repository::RepositoryPage;
use crate::ui::settings::SettingsView;
use crate::workspace::{Route, TabId};

/// The one application runtime, installed by `app::run` before the first frame.
static RUNTIME: Mutex<Option<RuntimeHandle>> = Mutex::new(None);

pub fn install_runtime(runtime: RuntimeHandle) {
    *RUNTIME.lock().expect("shell runtime") = Some(runtime);
}

struct ShellState {
    runtime: RuntimeHandle,
    snapshot: Snapshot,
    pages: HashMap<u64, Box<dyn PageView>>,
    settings: Option<SettingsView>,
    avatars: Option<Rc<crate::app::avatar::AvatarLoader>>,
    binding_revision: u64,
    active_tab: Option<TabId>,
    pending: Rc<RefCell<Vec<PageNavigation>>>,
    palette_open: bool,
    palette_query: Signal<String>,
    sidebar_collapsed: bool,
}

impl ShellState {
    fn active_route(&self) -> Route {
        self.runtime.sessions().active().route.clone()
    }
}

fn tick(state: &Rc<RefCell<ShellState>>, revision: Signal<u64>) {
    let mut st = state.borrow_mut();
    let snapshot = st.runtime.snapshot();
    let mut changed = snapshot != st.snapshot;
    st.snapshot = snapshot;
    let snapshot = st.snapshot.clone();
    {
        let sessions = st.runtime.sessions_mut();
        sessions.retain_accounts(&snapshot.accounts);
        sessions.show(snapshot.active.clone());
    }
    if snapshot.binding_revision != st.binding_revision {
        st.binding_revision = snapshot.binding_revision;
        st.active_tab = None;
        st.avatars = None;
        for (_, mut page) in st.pages.drain() {
            page.retire();
        }
        if let Some(mut settings) = st.settings.take() {
            settings.retire();
        }
    }
    let navs = std::mem::take(&mut *st.pending.borrow_mut());
    changed |= !navs.is_empty();
    for nav in navs {
        apply_navigation(&mut st, nav);
    }
    if st.avatars.is_none()
        && let Some(services) = st.runtime.services()
        && let Ok(context) = services.page_context(
            st.runtime.sessions(),
            st.runtime.sessions().active_id(),
            None,
        )
    {
        st.avatars = Some(Rc::new(crate::app::avatar::AvatarLoader::new(context)));
    }
    if let Some(avatars) = &st.avatars {
        changed |= avatars.poll();
    }
    let mounted = (st.pages.len(), st.settings.is_some());
    ensure_active_page(&mut st);
    changed |= mounted != (st.pages.len(), st.settings.is_some());
    let active = st.runtime.sessions().active_id();
    let active_changed = st.active_tab != Some(active);
    st.active_tab = Some(active);
    let mut pages = std::mem::take(&mut st.pages);
    for (id, page) in pages.iter_mut() {
        if active_changed {
            page.set_active(*id == active.get());
        }
        let before = page.host_presentation();
        page.poll(&st.runtime);
        changed |= before != page.host_presentation();
    }
    st.pages = pages;
    let mut settings = st.settings.take();
    if let Some(settings) = settings.as_mut() {
        settings.poll(&mut st.runtime);
    }
    st.settings = settings;
    drop(st);
    if changed {
        revision.update(|n| *n = n.wrapping_add(1));
    }
}

fn apply_navigation(st: &mut ShellState, nav: PageNavigation) {
    match nav {
        PageNavigation::Open(route) => {
            st.runtime.sessions_mut().open(route);
        }
        PageNavigation::Repository { route, intent } => {
            let tab = st.runtime.sessions_mut().open(route);
            if let Some(repository) = st.runtime.sessions_mut().repository_mut(tab) {
                *repository = intent;
            }
        }
        PageNavigation::External(url) => {
            spawn(async move {
                let _ = open_url(url).await;
            });
        }
        PageNavigation::CanonicalProfile { tab, login } => {
            let identity = st.runtime.sessions().identity(tab);
            if let Some(identity) = identity {
                st.runtime
                    .sessions_mut()
                    .canonicalize_profile(&identity, login);
            }
        }
    }
}

fn ensure_active_page(st: &mut ShellState) {
    let tab = st.runtime.sessions().active_id();
    if st.pages.contains_key(&tab.get()) {
        return;
    }
    let route = st.active_route();
    if route == Route::Settings {
        if st.settings.is_none() {
            st.settings = Some(accounts_view(st));
        }
        return;
    }
    let Some(services) = st.runtime.services() else {
        if st.settings.is_none() {
            st.settings = Some(accounts_view(st));
        }
        return;
    };
    let Ok(context) = services.page_context(st.runtime.sessions(), tab, None) else {
        return;
    };
    if let Some(page) = build_page(st, context, route, tab) {
        st.pages.insert(tab.get(), page);
    }
}

fn accounts_view(st: &ShellState) -> SettingsView {
    let pending = st.pending.clone();
    let mut view = SettingsView::new(Rc::new(move |nav| pending.borrow_mut().push(nav)));
    if let Some(avatars) = &st.avatars {
        view.install_avatar_provider(avatars.clone());
    }
    view
}

fn build_page(
    st: &ShellState,
    context: PageContext,
    route: Route,
    tab: TabId,
) -> Option<Box<dyn PageView>> {
    let pending = st.pending.clone();
    let navigation: NavigationSink = Rc::new(move |nav| pending.borrow_mut().push(nav));
    match route {
        Route::Home => {
            let mut page = HomePage::new(context, navigation);
            if let Some(avatars) = st.avatars.clone() {
                page.set_avatar_resolver(Some(Rc::new(move |actor| {
                    avatars
                        .image(actor.id, actor.avatar_url.as_deref())
                        .map(|image| ImageSrc::from(image.source.clone()))
                })));
            }
            Some(Box::new(page))
        }
        Route::Profile => {
            let login = st
                .snapshot
                .accounts
                .iter()
                .find(|a| Some(&a.identity.key) == st.snapshot.active.as_ref())
                .map(|a| a.identity.login.clone())
                .unwrap_or_default();
            Some(Box::new(profile_page(
                context,
                login,
                navigation,
                st.avatars.clone(),
            )))
        }
        Route::UserProfile { login, .. } => Some(Box::new(profile_page(
            context,
            login,
            navigation,
            st.avatars.clone(),
        ))),
        Route::Repository { namespace, name } => {
            let intent = st
                .runtime
                .sessions()
                .session(tab)
                .and_then(|s| s.repository.clone())
                .unwrap_or_default();
            let executor = context.services().executor();
            let mut page = RepositoryPage::new(context, namespace, name, intent, navigation);
            page.set_image_executor(executor);
            page.set_avatar_loader(st.avatars.clone());
            Some(Box::new(page))
        }
        Route::Notifications => Some(Box::new(NotificationsPage::new(context, navigation))),
        Route::Issues | Route::PullRequests => {
            Some(Box::new(WorkItemsPage::new(context, route, navigation)))
        }
        Route::Settings | Route::PullRequest { .. } => Some(Box::new(EmptyPage)),
    }
}

fn profile_page(
    context: PageContext,
    login: String,
    navigation: NavigationSink,
    avatars: Option<Rc<crate::app::avatar::AvatarLoader>>,
) -> ProfilePage {
    let captured = context.clone();
    let mut page = ProfilePage::new(context, login, navigation);
    page.set_avatar_loader(avatars);
    page.set_readme_image_provider(Some(Rc::new(move |read_context, repository| {
        use crate::network::reader_images::ReqwestExternalImages;
        if !captured.services().is_current() || captured.context() != read_context {
            return None;
        }
        let session = captured.repository_session().ok()?;
        let cache = captured
            .cache()
            .map(|binding| (binding.store().clone(), binding.scope().clone(), false));
        Some(std::sync::Arc::new(
            crate::data::reader_images::RepositoryImageService::with_cache(
                session,
                repository.clone(),
                captured.services().executor(),
                std::sync::Arc::new(ReqwestExternalImages::new().ok()?),
                crate::content::images::ImageLimits::default(),
                cache,
            ),
        )
            as std::sync::Arc<dyn crate::content::images::ImageProvider>)
    })));
    page
}

/// A route-only page for surfaces whose content is intentionally empty in the first release.
struct EmptyPage;

impl PageView for EmptyPage {
    fn element(&self) -> AnyElement {
        view().flex_1().bg(t::theme().background).into_element()
    }
    fn poll(&mut self, _runtime: &RuntimeHandle) {}
    fn refresh(&mut self, _runtime: &RuntimeHandle) {}
    fn set_active(&mut self, _active: bool) {}
    fn retire(&mut self) {}
}

fn navigate(
    state: &Rc<RefCell<ShellState>>,
    revision: Signal<u64>,
    nav: PageNavigation,
) -> impl Fn() + 'static {
    let state = state.clone();
    move || {
        state.borrow_mut().palette_open = false;
        let pending = state.borrow().pending.clone();
        pending.borrow_mut().push(nav.clone());
        revision.update(|n| *n = n.wrapping_add(1));
    }
}

#[component]
pub fn Shell() -> impl IntoElement {
    let runtime = RUNTIME
        .lock()
        .expect("shell runtime")
        .take()
        .expect("app::run installs the runtime before the first frame");
    set_title_bar(TitleBarStyle::Hidden, Some(px(t::TITLEBAR_HEIGHT)));
    let state = Rc::new(RefCell::new(ShellState {
        runtime,
        snapshot: Snapshot {
            startup: crate::app::runtime::Startup::Opening,
            accounts: Vec::new(),
            active: None,
            repositories: RepositoryList::Idle,
            busy: None,
            settling: false,
            completion: None,
            recovery_pending: Ok(false),
            cache_open_error: None,
            binding_revision: 0,
        },
        pages: HashMap::new(),
        settings: None,
        avatars: None,
        binding_revision: u64::MAX,
        active_tab: None,
        pending: Rc::new(RefCell::new(Vec::new())),
        palette_open: false,
        palette_query: signal(String::new()),
        sidebar_collapsed: false,
    }));
    let revision = signal(0u64);
    let cleanup_state = state.clone();
    cleanup(move || {
        let mut st = cleanup_state.borrow_mut();
        for page in st.pages.values_mut() {
            page.retire();
        }
        st.pages.clear();
        if let Some(mut settings) = st.settings.take() {
            settings.retire();
        }
        st.avatars = None;
        st.runtime.shutdown();
    });
    tick(&state, revision);
    let tick_state = state.clone();
    set_interval(Duration::from_millis(200), move || {
        tick(&tick_state, revision)
    });

    let key_state = state.clone();
    on_key("ctrl+k", move |_| {
        let mut st = key_state.borrow_mut();
        st.palette_open = !st.palette_open;
        revision.update(|n| *n = n.wrapping_add(1));
    });
    let close_state = state.clone();
    on_key("ctrl+w", move |_| {
        let active = close_state.borrow().runtime.sessions().active_id();
        close_tab(&close_state, revision, active);
    });
    let cycle_state = state.clone();
    on_key("ctrl+tab", move |_| {
        cycle_tab(&cycle_state, revision, 1);
    });
    let back_state = state.clone();
    on_key("shift+ctrl+tab", move |_| {
        cycle_tab(&back_state, revision, -1);
    });

    view()
        .flex_1()
        .flex_col()
        .bg(t::theme().background)
        .child(titlebar(state.clone(), revision))
        .child(
            view()
                .flex_1()
                .flex_row()
                .min_h_0()
                .child(sidebar(state.clone(), revision))
                .child(island(state.clone(), revision)),
        )
        .child(palette(state.clone(), revision))
}

fn close_tab(state: &Rc<RefCell<ShellState>>, revision: Signal<u64>, id: TabId) {
    let mut st = state.borrow_mut();
    st.runtime.sessions_mut().close(id);
    if let Some(mut page) = st.pages.remove(&id.get()) {
        page.retire();
    }
    drop(st);
    revision.update(|n| *n = n.wrapping_add(1));
}

fn cycle_tab(state: &Rc<RefCell<ShellState>>, revision: Signal<u64>, by: isize) {
    let mut st = state.borrow_mut();
    st.runtime.sessions_mut().cycle(by);
    drop(st);
    revision.update(|n| *n = n.wrapping_add(1));
}

fn titlebar(state: Rc<RefCell<ShellState>>, revision: Signal<u64>) -> AnyElement {
    dynamic(move || {
        let _ = revision.get();
        let open = {
            let state = state.clone();
            move || {
                state.borrow_mut().palette_open = true;
                revision.update(|n| *n = n.wrapping_add(1));
            }
        };
        let collapse = {
            let state = state.clone();
            move || {
                let mut st = state.borrow_mut();
                st.sidebar_collapsed = !st.sidebar_collapsed;
                drop(st);
                revision.update(|n| *n = n.wrapping_add(1));
            }
        };
        view()
            .h(px(t::TITLEBAR_HEIGHT))
            .flex_row()
            .items_center()
            .gap_2()
            .px_2()
            .child(controls::icon_button(
                "Toggle sidebar",
                Icon::More,
                collapse,
            ))
            .child(view().flex_1().h_full().window_drag(true))
            .child(
                view()
                    .h(px(28.0))
                    .min_w(px(300.0))
                    .px_2()
                    .rounded(t::theme().radius)
                    .border_1()
                    .border_color(t::theme().border)
                    .bg(t::theme().muted)
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .role(AccessibilityRole::Button)
                    .label("Search")
                    .test_id("shell-search")
                    .on_press(move |_: &()| open())
                    .child(controls::icon(Icon::Search))
                    .child(
                        t::label("Search")
                            .text_size(px(12.0))
                            .text_color(t::theme().muted_foreground),
                    ),
            )
    })
    .into_element()
}

fn sidebar(state: Rc<RefCell<ShellState>>, revision: Signal<u64>) -> AnyElement {
    dynamic(move || {
        let _ = revision.get();
        let st = state.borrow();
        if st.sidebar_collapsed {
            return view();
        }
        let route = st.active_route();
        let mut nav_rows = view().gap(px(2.0)).px_2().pt_2();
        for (label, icon, target) in [
            ("Home", Icon::Home, Route::Home),
            ("Settings", Icon::Settings, Route::Settings),
        ] {
            let active = route == target;
            let press = navigate(&state, revision, PageNavigation::Open(target));
            nav_rows = nav_rows.child(sidebar_row(label, icon, active, press));
        }

        let repository_group = match &st.snapshot.repositories {
            RepositoryList::Ready { outcome, .. } => {
                let mut list = scroll().flex_1().min_h_0().p(px(4.0));
                for repo in &outcome.repositories {
                    let target = Route::repository(repo);
                    let active = route.key() == target.key();
                    let press = navigate(&state, revision, PageNavigation::Open(target));
                    list = list.child(repo_row(repo, active, st.avatars.as_ref(), press));
                }
                list.into_element()
            }
            RepositoryList::Loading => view()
                .p(px(8.0))
                .child(
                    t::label("Loading repositories…")
                        .text_size(px(12.0))
                        .text_color(t::theme().muted_foreground),
                )
                .into_element(),
            RepositoryList::Failed(_) => view()
                .p(px(8.0))
                .child(
                    t::label("Repositories unavailable")
                        .text_size(px(12.0))
                        .text_color(t::theme().muted_foreground),
                )
                .into_element(),
            RepositoryList::Idle => view()
                .p(px(8.0))
                .child(
                    t::label("Repositories (0)")
                        .text_size(px(12.0))
                        .text_color(t::theme().muted_foreground),
                )
                .into_element(),
        };

        let account = st
            .snapshot
            .active
            .as_ref()
            .and_then(|key| st.snapshot.accounts.iter().find(|a| &a.identity.key == key))
            .map(|a| a.identity.login.clone())
            .unwrap_or_else(|| "Not signed in".to_string());
        let profile_press = navigate(&state, revision, PageNavigation::Open(Route::Profile));
        let footer = view()
            .px_2()
            .py_2()
            .border_t_1()
            .border_t_color(t::theme().border)
            .flex_row()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .on_press(move |_: &()| profile_press())
            .child(
                st.snapshot
                    .accounts
                    .iter()
                    .find(|a| Some(&a.identity.key) == st.snapshot.active.as_ref())
                    .map(|a| {
                        shell_avatar(
                            st.avatars.as_ref(),
                            a.identity.key.id,
                            a.identity.avatar_url.as_deref(),
                            &a.identity.login,
                        )
                    })
                    .unwrap_or_else(|| controls::icon(Icon::Repo).into_element()),
            )
            .child(t::label(account).text_size(px(13.0)).flex_1());

        view()
            .w(px(g::SIDEBAR_WIDTH))
            .h_full()
            .flex_col()
            .p(px(g::SIDEBAR_PADDING))
            .child(nav_rows)
            .child(
                view().px_2().pt_3().pb_1().child(
                    t::label(match &st.snapshot.repositories {
                        RepositoryList::Ready { outcome, .. } => {
                            format!("Repositories ({})", outcome.repositories.len())
                        }
                        _ => "Repositories".to_string(),
                    })
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t::theme().muted_foreground),
                ),
            )
            .child(repository_group)
            .child(footer)
    })
    .into_element()
}

fn sidebar_row(
    label: &'static str,
    icon: Icon,
    active: bool,
    on_press: impl Fn() + 'static,
) -> AnyElement {
    let bg = if active {
        t::theme().accent
    } else {
        AdaptiveColor::TRANSPARENT
    };
    view()
        .h(px(30.0))
        .px_2()
        .rounded(t::theme().radius)
        .flex_row()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(label)
        .test_id(if label == "Settings" {
            "shell-nav-Settings"
        } else {
            "shell-nav-Home"
        })
        .bg(bg)
        .hover(|s| s.bg(t::theme().accent))
        .on_press(move |_: &()| on_press())
        .child(controls::icon(icon))
        .child(t::label(label).text_size(px(13.0)))
        .into_element()
}

fn repo_row(
    repo: &RepoSummary,
    active: bool,
    avatars: Option<&Rc<crate::app::avatar::AvatarLoader>>,
    on_press: impl Fn() + 'static,
) -> AnyElement {
    let bg = if active {
        t::theme().accent
    } else {
        AdaptiveColor::TRANSPARENT
    };
    view()
        .h(px(28.0))
        .px_2()
        .rounded(t::theme().radius)
        .flex_row()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .child(shell_avatar(
            avatars,
            repo.owner.id,
            repo.owner.avatar_url.as_deref(),
            &repo.owner.login,
        ))
        .label(repo.full_name.clone())
        .bg(bg)
        .hover(|s| s.bg(t::theme().accent))
        .on_press(move |_: &()| on_press())
        .child(
            t::label(repo.owner.login.clone())
                .text_size(px(12.0))
                .text_color(t::theme().muted_foreground),
        )
        .child(t::label(format!("/{}", repo.name)).text_size(px(12.0)))
        .into_element()
}

fn island(state: Rc<RefCell<ShellState>>, revision: Signal<u64>) -> AnyElement {
    view()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .p(px(g::ISLAND_INSET))
        .child(
            view()
                .flex_1()
                .min_h_0()
                .rounded(t::theme().radius)
                .border_1()
                .border_color(t::theme().border)
                .bg(t::theme().card)
                .overflow_hidden()
                .child(tab_strip(state.clone(), revision))
                .child(page_host(state, revision)),
        )
        .into_element()
}

fn tab_strip(state: Rc<RefCell<ShellState>>, revision: Signal<u64>) -> AnyElement {
    dynamic(move || {
        let _ = revision.get();
        let st = state.borrow();
        let active = st.runtime.sessions().active_id();
        let sessions: Vec<(TabId, String)> = st
            .runtime
            .sessions()
            .sessions()
            .iter()
            .map(|s| (s.id, s.route.title()))
            .collect();
        let mut strip = view()
            .h(px(g::TAB_STRIP_HEIGHT))
            .flex_row()
            .items_center()
            .gap_1()
            .px(px(g::TAB_STRIP_INSET))
            .border_b_1()
            .border_b_color(t::theme().border);
        for (id, title) in sessions {
            let is_active = id == active;
            let activate = {
                let state = state.clone();
                move || {
                    state.borrow_mut().runtime.sessions_mut().activate(id);
                    revision.update(|n| *n = n.wrapping_add(1));
                }
            };
            let close = {
                let state = state.clone();
                move || close_tab(&state, revision, id)
            };
            strip = strip.child(tab_button(title, is_active, activate, close));
        }
        drop(st);
        let refresh = {
            let state = state.clone();
            move || {
                let mut st = state.borrow_mut();
                let id = st.runtime.sessions().active_id().get();
                if let Some(mut page) = st.pages.remove(&id) {
                    page.refresh(&st.runtime);
                    st.pages.insert(id, page);
                }
                drop(st);
                revision.update(|n| *n = n.wrapping_add(1));
            }
        };
        strip.child(view().flex_1()).child(controls::icon_button(
            "Refresh page",
            Icon::Refresh,
            refresh,
        ))
    })
    .into_element()
}

fn tab_button(
    title: String,
    active: bool,
    on_activate: impl Fn() + 'static,
    on_close: impl Fn() + 'static,
) -> AnyElement {
    view()
        .h(px(g::TAB_HEIGHT))
        .w(px(g::TAB_WIDTH))
        .px_2()
        .rounded(t::theme().radius)
        .flex_row()
        .items_center()
        .gap_1()
        .overflow_hidden()
        .bg(if active {
            t::theme().accent
        } else {
            AdaptiveColor::TRANSPARENT
        })
        .cursor_pointer()
        .role(AccessibilityRole::Tab)
        .label(title.clone())
        .on_press(move |_: &()| on_activate())
        .child(
            t::label(title)
                .text_size(px(12.0))
                .number_of_lines(1)
                .flex_1(),
        )
        .child(controls::icon_button("Close tab", Icon::Close, on_close))
        .into_element()
}

fn page_host(state: Rc<RefCell<ShellState>>, revision: Signal<u64>) -> AnyElement {
    dynamic(move || {
        let _ = revision.get();
        let st = state.borrow();
        let active = st.runtime.sessions().active_id();
        if let Some(page) = st.pages.get(&active.get()) {
            return page.element();
        }
        if let Some(settings) = &st.settings
            && (st.active_route() == Route::Settings || st.snapshot.active.is_none())
        {
            return settings.element();
        }
        view()
            .flex_1()
            .items_center()
            .justify_center()
            .child(t::label("No page").text_color(t::theme().muted_foreground))
            .into_element()
    })
    .into_element()
}

fn palette(state: Rc<RefCell<ShellState>>, revision: Signal<u64>) -> AnyElement {
    dynamic(move || {
        let _ = revision.get();
        let open = state.borrow().palette_open;
        if !open {
            return view().into_element();
        }
        let query = state.borrow().palette_query;
        let needle = query.get().to_lowercase();
        let mut entries = view().flex_col().gap(px(2.0));
        let mut choices: Vec<(String, Route)> = [
            ("Home", Route::Home),
            ("Settings", Route::Settings),
            ("Profile", Route::Profile),
            ("Notifications", Route::Notifications),
            ("Issues", Route::Issues),
            ("Pull requests", Route::PullRequests),
        ]
        .into_iter()
        .map(|(label, route)| (label.to_owned(), route))
        .collect();
        if let RepositoryList::Ready { outcome, .. } = &state.borrow().snapshot.repositories {
            choices.extend(
                outcome
                    .repositories
                    .iter()
                    .map(|repo| (repo.full_name.clone(), Route::repository(repo))),
            );
        }
        for (label, route) in choices
            .into_iter()
            .filter(|(label, _)| label.to_lowercase().contains(&needle))
        {
            let press = navigate(&state, revision, PageNavigation::Open(route));
            entries = entries.child(
                view()
                    .h(px(32.0))
                    .px_2()
                    .rounded(t::theme().radius)
                    .flex_row()
                    .items_center()
                    .cursor_pointer()
                    .role(AccessibilityRole::Button)
                    .label(label.clone())
                    .hover(|s| s.bg(t::theme().accent))
                    .on_press(move |_: &()| press())
                    .child(t::label(label).text_size(px(13.0))),
            );
        }
        let close = {
            let state = state.clone();
            move || {
                state.borrow_mut().palette_open = false;
                revision.update(|n| *n = n.wrapping_add(1));
            }
        };
        view()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .items_center()
            .justify_center()
            .bg(AdaptiveColor::from(rgba(0x00000080)))
            .child(
                view()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .on_press(move |_: &()| close()),
            )
            .child(
                view()
                    .w(px(420.0))
                    .p_2()
                    .rounded(t::theme().radius)
                    .border_1()
                    .border_color(t::theme().border)
                    .bg(t::theme().popover)
                    .child(
                        nsh_core::element::input()
                            .value(query)
                            .auto_focus(true)
                            .placeholder("Search pages and repositories")
                            .test_id("shell-palette-query")
                            .on_change(move |text: &str| query.set(text.to_owned())),
                    )
                    .child(scroll().max_h(px(400.0)).child(entries)),
            )
            .into_element()
    })
    .into_element()
}

fn shell_avatar(
    loader: Option<&Rc<crate::app::avatar::AvatarLoader>>,
    id: u64,
    url: Option<&str>,
    login: &str,
) -> AnyElement {
    let source = loader.and_then(|loader| loader.image(id, url));
    crate::ui::avatar::avatar(crate::ui::avatar::AvatarView {
        id: "avatar",
        selector: "shell-avatar",
        url: source.as_ref().map(|image| image.source.as_str()),
        name: login,
        slot: crate::ui::avatar::AvatarSlot::Sm,
    })
}
impl Drop for ShellState {
    fn drop(&mut self) {
        self.avatars = None;
        self.runtime.shutdown();
    }
}

#[cfg(test)]
mod integration_tests;
