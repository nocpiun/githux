//! Formal integration coverage for reopening the repository `Pull requests` list offline.
//!
//! The product promise this file owns: a pull request the reader actually found by paging the
//! repository list is still reachable from the same list after the app quits and reopens with
//! no network, and that is true *before* the failing network read finishes.
//!
//! This harness follows the repository's real restart shape: the cold phase builds its own
//! `TestAppContext`, account service and `CacheStore`, then tears all of them down and calls
//! `quit`. The warm phase opens a **fresh** `CacheStore` at the same directory, so a
//! successful reopen proves the old lease is gone rather than reusing an in-memory owner. Each
//! phase mounts the real repository page over the real account sessions and the shared
//! development fixture transport; only connectivity is scripted (offline failure, held answer).
//!
//! What this cannot prove is recorded in the task devlog: a real Windows window, a real PAT,
//! real GitHub rate limits and real IME/multi-DPI input.

use std::path::PathBuf;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    AppContext, Entity, IntoElement, ParentElement, Styled, TestAppContext, VisualTestContext,
};

use super::super::{
    PageAction, ReaderStatus, RepositoryNavigation, RepositoryPage, RepositoryPageServices,
    RepositoryTab,
};
use crate::auth::InMemoryCredentialStore;
use crate::data::AccountService;
use crate::domain::repository::RepositoryLocator;
use crate::domain::repository_pulls::PullFilter;
use crate::github::{GitHubClient, GitHubConfig};
use crate::network::cancel::CancellationToken;
use crate::network::executor::BackgroundExecutor;
use crate::network::transport::{HttpRequest, HttpResponse, Transport, TransportError};
use crate::repository_modules::fixture::{FixtureTransport, NAME, OWNER, Scene};
use crate::storage::accounts::AccountDatabaseConfig;
use crate::storage::cache::{CacheConfig, CacheStore};

/// The repository pull list path, so a test can inspect page requests.
const PULLS: &str = "/pulls?";

/// The fixture transport with scripted connectivity.
///
/// `offline` fails every request like a dropped connection. `hold` keeps matching requests
/// pending until released, which is how a stored paint is observed while the network read has
/// not finished yet.
struct ScriptedTransport {
    inner: Arc<FixtureTransport>,
    offline: Mutex<bool>,
    held: Mutex<Option<String>>,
    release: tokio::sync::watch::Sender<bool>,
    seen: Mutex<Vec<String>>,
}

impl ScriptedTransport {
    fn new() -> Self {
        Self {
            inner: Arc::new(FixtureTransport::new(Scene::Normal)),
            offline: Mutex::new(false),
            held: Mutex::new(None),
            release: tokio::sync::watch::channel(true).0,
            seen: Mutex::new(Vec::new()),
        }
    }

    fn set_offline(&self, offline: bool) {
        *self.offline.lock().unwrap() = offline;
    }

    fn hold(&self, needle: &str) {
        *self.held.lock().unwrap() = Some(needle.to_string());
        self.release.send_replace(false);
    }

    fn release_all(&self) {
        *self.held.lock().unwrap() = None;
        self.release.send_replace(true);
    }

    fn pull_requests(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|url| url.contains(PULLS))
            .cloned()
            .collect()
    }

    async fn run(
        &self,
        request: HttpRequest,
        cancel: CancellationToken,
        bound: Option<usize>,
    ) -> Result<HttpResponse, TransportError> {
        if cancel.is_cancelled() {
            return Err(TransportError::Cancelled);
        }
        self.seen.lock().unwrap().push(request.url.clone());
        let held = self.held.lock().unwrap().clone();
        if let Some(needle) = held
            && request.url.contains(&needle)
        {
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
                _ = cancel.clone().cancelled() => return Err(TransportError::Cancelled),
            }
        }
        if *self.offline.lock().unwrap() {
            return Err(TransportError::Network {
                message: "scripted offline: connection refused".into(),
            });
        }
        match bound {
            Some(limit) => self.inner.execute_bounded(request, cancel, limit).await,
            None => self.inner.execute(request, cancel).await,
        }
    }
}

impl Transport for ScriptedTransport {
    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(self.run(request, cancel, None))
    }

    fn execute_bounded<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
        max_body_bytes: usize,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(self.run(request, cancel, Some(max_body_bytes)))
    }
}

/// One persisted app profile shared by both phases: the same cache directory, accounts
/// database and credential store, but never the same live store instance.
struct Profile {
    dir: tempfile::TempDir,
    transport: Arc<ScriptedTransport>,
    runtime: tokio::runtime::Runtime,
    credentials: Arc<InMemoryCredentialStore>,
}

impl Profile {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temporary app directory");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("host runtime");
        Self {
            dir,
            transport: Arc::new(ScriptedTransport::new()),
            runtime,
            credentials: Arc::new(InMemoryCredentialStore::new()),
        }
    }

    fn cache_root(&self) -> PathBuf {
        self.dir.path().join("cache")
    }

    fn accounts_config(&self) -> AccountDatabaseConfig {
        AccountDatabaseConfig::at(self.dir.path().join("accounts.sqlite3"))
    }

    /// A fresh store plus a new account service over it. Success here after a cold phase is
    /// the evidence that the previous process released the directory lease.
    fn open(&self, executor: &Arc<BackgroundExecutor>, store: CacheStore) -> Arc<AccountService> {
        let transport = self.transport.clone();
        let executor = executor.clone();
        let config = self.accounts_config();
        let credentials = self.credentials.clone();
        Arc::new(self.runtime.block_on(async {
            AccountService::open_with_cache(
                config,
                credentials,
                Arc::new(GitHubClient::new(
                    GitHubConfig::default(),
                    transport as Arc<dyn Transport>,
                )),
                executor,
                store,
            )
            .await
            .expect("open the cached account service")
        }))
    }

    fn login(&self, accounts: &AccountService) {
        self.transport.inner.set_identity(7, OWNER);
        self.runtime.block_on(async {
            accounts
                .login(crate::auth::token::Pat::from_str(
                    "synthetic-pull-offline-token",
                ))
                .await
                .expect("synthetic login");
        });
    }

    fn open_store(&self) -> CacheStore {
        CacheStore::open(CacheConfig::at(self.cache_root())).expect("open the app cache store")
    }
}

struct Host {
    page: Option<Entity<RepositoryPage>>,
}

impl gpui::Render for Host {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        gpui::div()
            .size_full()
            .children(self.page.clone().map(|page| page.into_any_element()))
    }
}

fn setup(cx: &mut TestAppContext) {
    cx.update(|cx| {
        ely_core::init_for_tests(cx);
        crate::platform::fonts::register(cx).expect("test fonts");
        ely_core::theme::Theme::update(cx, |theme| {
            theme.font_family = "Noto Sans".into();
            theme.mono_family = "Google Sans Code".into();
        });
    });
}

fn open(
    cx: &mut TestAppContext,
    accounts: Arc<AccountService>,
    executor: Arc<BackgroundExecutor>,
) -> (Entity<Host>, Entity<RepositoryPage>, &mut VisualTestContext) {
    let services = RepositoryPageServices::new(accounts, executor);
    let locator = RepositoryLocator::new(OWNER, NAME).expect("fixture locator");
    let (host, cx) = cx.add_window_view(|window, cx| {
        let page = cx.new(|cx| {
            RepositoryPage::new(
                services,
                locator,
                RepositoryNavigation::default(),
                window,
                cx,
            )
        });
        page.update(cx, |page, cx| page.activate(cx));
        Host { page: Some(page) }
    });
    cx.update(|window, _| window.activate_window());
    let page = host.read_with(cx, |host, _| host.page.clone().expect("a mounted page"));
    (host, page, cx)
}

fn frames(cx: &mut VisualTestContext) {
    for _ in 0..2 {
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
    }
    cx.run_until_parked();
}

fn settle(
    page: &Entity<RepositoryPage>,
    cx: &mut VisualTestContext,
    mut done: impl FnMut(&RepositoryPage) -> bool,
) {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        page.update_in(cx, |page, window, cx| page.drain(window, cx));
        cx.run_until_parked();
        frames(cx);
        std::thread::sleep(Duration::from_millis(2));
        if page.read_with(cx, |page, _| done(page)) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the pull list did not settle; page error={:?} loaded={:?} busy={:?} rows={:?} model error={:?} tab={:?}",
            page.read_with(cx, |page, _| page.error().cloned()),
            page.read_with(cx, |page, _| page.pulls_model().map(|model| model.loaded())),
            page.read_with(cx, |page, _| page.pulls.busy()),
            page.read_with(cx, |page, _| page
                .pulls_model()
                .map(|model| model.rows().len())),
            page.read_with(cx, |page, _| page
                .pulls_model()
                .and_then(|model| model.error().map(|error| format!("{error:?}")))),
            page.read_with(cx, |page, _| page.tab()),
        );
    }
}

fn open_pulls(page: &Entity<RepositoryPage>, cx: &mut VisualTestContext) {
    page.update(cx, |page, cx| {
        page.select_tab(RepositoryTab::PullRequests, cx)
    });
    frames(cx);
}

fn numbers(page: &RepositoryPage) -> Vec<u64> {
    page.pulls_model()
        .map(|model| model.rows().iter().map(|row| row.number).collect())
        .unwrap_or_default()
}

/// Tears down a phase the way quitting the app does: remove the window, deactivate and drop the
/// page, close the account service, release the store lease and quit the GPUI context.
fn quit_phase(
    cx: TestAppContext,
    accounts: Arc<AccountService>,
    executor: Arc<BackgroundExecutor>,
    store: CacheStore,
    runtime: &tokio::runtime::Runtime,
) {
    // Dropping the context first drops every held entity, including the repository page and
    // its cache bindings.
    cx.quit();
    drop(cx);
    runtime.block_on(async { accounts.close().await }).ok();
    drop(accounts);
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while executor.in_flight() > 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "background reads did not finish before the restart"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(executor);
    drop(store);
    crate::app::accounts::wait_for_shutdown();
}

/// The product scenario: a pull request that only exists on the second page is found online,
/// then found again in the same list after the app quits and reopens with no network. The
/// stored rows appear while the failing read is still pending, and `Cached`/Retry only after it
/// failed.
#[test]
fn a_second_page_pull_request_is_reachable_after_reopening_offline() {
    let profile = Profile::new();
    let executor = Arc::new(BackgroundExecutor::new(4));
    {
        let mut cx = TestAppContext::build(gpui::TestDispatcher::new(0), None);
        setup(&mut cx);
        let store = profile.open_store();
        let accounts = profile.open(&executor, store.clone());
        profile.login(&accounts);
        {
            let (host, page, vcx) = open(&mut cx, accounts.clone(), executor.clone());
            settle(&page, vcx, |page| {
                page.reader_status() == &ReaderStatus::Showing
            });
            open_pulls(&page, vcx);
            settle(&page, vcx, |page| {
                page.pulls_model().is_some_and(|model| model.loaded()) && !page.pulls.busy()
            });
            assert_eq!(page.read_with(vcx, |page, _| numbers(page)), vec![12, 11]);
            // Read the second page; #10 exists only there.
            page.update(vcx, |page, cx| page.request_pulls(false, cx));
            settle(&page, vcx, |page| {
                page.pulls_model()
                    .is_some_and(|model| model.rows().len() == 3 && model.next().is_none())
                    && !page.pulls.busy()
            });
            assert_eq!(
                page.read_with(vcx, |page, _| numbers(page)),
                vec![12, 11, 10]
            );
            assert_eq!(profile.transport.pull_requests().len(), 2);
            host.update(vcx, |host, cx| {
                host.page = None;
                cx.notify();
            });
            drop(page);
            drop(host);
        }
        quit_phase(cx, accounts, executor.clone(), store, &profile.runtime);
    }

    // A brand-new cache store at the same directory: this only succeeds once the cold phase
    // released the exclusive lease, so it is a real restart rather than the same live owner.
    let store = CacheStore::open(CacheConfig::at(profile.cache_root()))
        .expect("the app cache store reopens on disk");
    profile.transport.set_offline(true);
    profile.transport.hold(PULLS);
    let mut cx = TestAppContext::build(gpui::TestDispatcher::new(0), None);
    setup(&mut cx);
    let executor = Arc::new(BackgroundExecutor::new(4));
    let accounts = profile.open(&executor, store.clone());
    {
        let (_host, page, vcx) = open(&mut cx, accounts.clone(), executor.clone());
        settle(&page, vcx, |page| {
            page.reader_status() == &ReaderStatus::Showing
        });
        open_pulls(&page, vcx);
        // Page one and the visited page two are restored from disk while the network attempt
        // is still pending.
        settle(&page, vcx, |page| numbers(page) == vec![12, 11, 10]);
        page.read_with(vcx, |page, _| {
            let model = page.pulls_model().expect("a restored model");
            assert!(model.is_historical(), "the rows are a stored prefix");
            assert!(
                !model.is_partial(),
                "the stored prefix ends at the real end of the served list"
            );
        });
        frames(vcx);
        assert!(
            vcx.debug_bounds("repository-pulls-cached").is_none(),
            "a pending verification is not a fallback"
        );
        assert!(vcx.debug_bounds("repository-pulls-retry").is_none());
        assert!(
            vcx.debug_bounds("repository-pull-10").is_some(),
            "the restored page-two row is already drawn"
        );
        let requested = profile.transport.pull_requests();
        assert!(
            requested
                .iter()
                .all(|url| url.contains("page=1") || url.contains("page=2")),
            "restoring never fetches a page that was not read before: {requested:?}"
        );

        // The network fails now: only then does the list offer `Cached` and one retry, and the
        // restored rows stay usable.
        profile.transport.release_all();
        settle(&page, vcx, |page| {
            page.pulls_model()
                .is_some_and(|model| model.error().is_some())
                && !page.pulls.busy()
        });
        frames(vcx);
        assert!(vcx.debug_bounds("repository-pulls-cached").is_some());
        assert!(vcx.debug_bounds("repository-pulls-retry").is_some());
        assert_eq!(
            page.read_with(vcx, |page, _| numbers(page)),
            vec![12, 11, 10]
        );

        // The restored page-two row is still a real, verified open action.
        let actions = Rc::new(std::cell::RefCell::new(Vec::new()));
        {
            let actions = actions.clone();
            page.update(vcx, |page, _| {
                page.on_action(Rc::new(move |action, _, _| {
                    actions.borrow_mut().push(action)
                }));
            });
        }
        let title = vcx
            .debug_bounds("repository-pull-title-10")
            .expect("the restored row title");
        vcx.simulate_click(title.center(), gpui::Modifiers::none());
        assert!(
            matches!(
                &actions.borrow()[..],
                [PageAction::OpenPullRequest { locator, number: 10, .. }]
                    if locator.owner() == OWNER && locator.name() == NAME
            ),
            "{:?}",
            actions.borrow()
        );

        // A real retry replaces the stored prefix. The network is back, page one is held: the
        // old rows stay readable, and the fallback marker plus its button disappear as soon as
        // the retry starts.
        profile.transport.set_offline(false);
        profile.transport.hold("page=1");
        let retry = vcx
            .debug_bounds("repository-pulls-retry")
            .expect("the failed read offers one retry");
        let reads = profile.transport.pull_requests().len();
        vcx.simulate_click(retry.center(), gpui::Modifiers::none());
        frames(vcx);
        settle(&page, vcx, |page| page.pulls.busy());
        assert_eq!(
            page.read_with(vcx, |page, _| numbers(page)),
            vec![12, 11, 10],
            "the stored rows stay visible while the retry is pending"
        );
        assert!(
            vcx.debug_bounds("repository-pulls-cached").is_none(),
            "a running retry is not a fallback"
        );
        assert!(
            vcx.debug_bounds("repository-pulls-retry").is_none(),
            "the retry control hides while it runs"
        );
        assert!(
            profile.transport.pull_requests().len() > reads,
            "the retry actually issues a new read"
        );

        // Releasing the held first page publishes the verified list in place: the stored
        // page-two row is gone and the list is no longer historical.
        profile.transport.release_all();
        settle(&page, vcx, |page| {
            page.pulls_model()
                .is_some_and(|model| model.loaded() && !model.is_historical())
                && !page.pulls.busy()
        });
        assert_eq!(
            page.read_with(vcx, |page, _| numbers(page)),
            vec![12, 11],
            "the network first page replaces the stored prefix"
        );
        page.read_with(vcx, |page, _| {
            let model = page.pulls_model().expect("a verified model");
            assert!(!model.is_historical());
            assert!(!model.is_cached_empty());
        });
        frames(vcx);
        assert!(vcx.debug_bounds("repository-pulls-cached").is_none());
        assert!(vcx.debug_bounds("repository-pulls-retry").is_none());
        assert!(vcx.debug_bounds("repository-pull-10").is_none());
        assert!(vcx.debug_bounds("repository-pull-12").is_some());
        drop(page);
        drop(_host);
    }
    quit_phase(cx, accounts, executor, store, &profile.runtime);
}

/// Both filters restore their own list on the same disk; the current scope keeps them apart.
#[test]
fn the_two_filters_restore_their_own_lists() {
    let profile = Profile::new();
    let executor = Arc::new(BackgroundExecutor::new(4));
    {
        let mut cx = TestAppContext::build(gpui::TestDispatcher::new(0), None);
        setup(&mut cx);
        let store = profile.open_store();
        let accounts = profile.open(&executor, store.clone());
        profile.login(&accounts);
        {
            let (host, page, vcx) = open(&mut cx, accounts.clone(), executor.clone());
            settle(&page, vcx, |page| {
                page.reader_status() == &ReaderStatus::Showing
            });
            open_pulls(&page, vcx);
            settle(&page, vcx, |page| {
                page.pulls_model().is_some_and(|model| model.loaded()) && !page.pulls.busy()
            });
            page.update(vcx, |page, cx| page.filter_pulls(PullFilter::Closed, cx));
            settle(&page, vcx, |page| {
                page.pulls_model()
                    .is_some_and(|model| model.loaded() && model.filter() == PullFilter::Closed)
                    && !page.pulls.busy()
            });
            assert_eq!(page.read_with(vcx, |page, _| numbers(page)), vec![9, 8, 7]);
            host.update(vcx, |host, cx| {
                host.page = None;
                cx.notify();
            });
            drop(page);
            drop(host);
        }
        quit_phase(cx, accounts, executor.clone(), store, &profile.runtime);
    }

    let store = CacheStore::open(CacheConfig::at(profile.cache_root()))
        .expect("the app cache store reopens on disk");
    profile.transport.set_offline(true);
    let mut cx = TestAppContext::build(gpui::TestDispatcher::new(0), None);
    setup(&mut cx);
    let executor = Arc::new(BackgroundExecutor::new(4));
    let accounts = profile.open(&executor, store.clone());
    {
        let (_host, page, vcx) = open(&mut cx, accounts.clone(), executor.clone());
        settle(&page, vcx, |page| {
            page.reader_status() == &ReaderStatus::Showing
        });
        open_pulls(&page, vcx);
        settle(&page, vcx, |page| numbers(page) == vec![12, 11]);
        page.update(vcx, |page, cx| page.filter_pulls(PullFilter::Closed, cx));
        settle(&page, vcx, |page| numbers(page) == vec![9, 8, 7]);
        page.update(vcx, |page, cx| page.filter_pulls(PullFilter::Open, cx));
        settle(&page, vcx, |page| numbers(page) == vec![12, 11]);
        assert!(
            !page.read_with(vcx, |page, _| numbers(page).contains(&9)),
            "the open list never shows a closed row"
        );
        drop(page);
        drop(_host);
    }
    quit_phase(cx, accounts, executor, store, &profile.runtime);
}
