//! Home / Feed page regressions on Native Shell.
//!
//! Real `AccountService` (SQLite account database, real `CacheStore`, in-memory
//! credentials), a scripted transport that answers by request identity instead of
//! a shared FIFO (LESSONS L010), the real `RuntimeHandle`/`PageContext` contract
//! and the real `HomePage`. The page is mounted in a headless NSH app, so the
//! assertions run against the elements the native view tree actually receives.
//!
//! No GitHub network, real PAT, GPU window or screenshot is involved.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

use crate::app::runtime::{Backend, Command, ExitGate, RuntimeHandle};
use crate::auth::{InMemoryCredentialStore, token::Pat};
use crate::data::AccountService;
use crate::github::{GitHubClient, GitHubConfig};
use crate::network::cancel::CancellationToken;
use crate::network::executor::BackgroundExecutor;
use crate::network::transport::{
    HttpMethod, HttpRequest, HttpResponse, Transport, TransportError,
};
use crate::storage::accounts::AccountDatabaseConfig;
use crate::storage::cache::{CacheConfig, CacheStore};
use crate::workspace::Route;

use super::{HomePage, NavigationSink, PageNavigation, PageView};

/// Statics only carry `Send` data: the backend, the observed navigation and the
/// commands the test asks the mounted page to apply. The page itself stays on the
/// engine thread.
static SLOT: Mutex<Option<BackendSlot>> = Mutex::new(None);
static NAVIGATION: Mutex<Vec<String>> = Mutex::new(Vec::new());
static COMMANDS: Mutex<Vec<TestCommand>> = Mutex::new(Vec::new());
static STATUS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static EMPTY_FIRST_PAGE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static LOCK: Mutex<()> = Mutex::new(());

struct BackendSlot {
    backend: Arc<Backend>,
}

#[derive(Clone, Debug)]
enum TestCommand {
    Select(crate::domain::AccountKey),
    Refresh,
}

fn response(status: u16, body: &str) -> HttpResponse {
    HttpResponse {
        status,
        headers: Vec::new(),
        body: body.to_string(),
    }
}

fn json(status: u16, value: serde_json::Value) -> HttpResponse {
    let mut response = response(status, &value.to_string());
    response
        .headers
        .push(("content-type".into(), "application/json".into()));
    response
}

/// Answers by request identity. Concurrent Home/Feed/enrichment reads therefore
/// cannot steal each other's responses.
struct ScriptedTransport {
    offline: Arc<std::sync::atomic::AtomicBool>,
    /// Which synthetic user the driver of `/user` reports, so two accounts can be
    /// logged in against one transport.
    user: Arc<Mutex<u64>>,
}

impl Transport for ScriptedTransport {
    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        _cancel: CancellationToken,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>>
    {
        Box::pin(async move {
            if self.offline.load(std::sync::atomic::Ordering::Acquire) {
                return Err(TransportError::Network { message: "synthetic offline".into() });
            }
            let user = *self.user.lock().expect("scripted user");
            Ok(script(&request, user))
        })
    }

    fn execute_bounded<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
        max_body_bytes: usize,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>>
    {
        Box::pin(async move {
            let value = self.execute(request, cancel).await?;
            if value.body.len() > max_body_bytes {
                return Err(TransportError::ResponseTooLarge {
                    limit: max_body_bytes,
                });
            }
            Ok(value)
        })
    }
}

fn login_for(user: u64) -> &'static str {
    if user == 1 { "alice" } else { "zoe" }
}

fn script(request: &HttpRequest, user: u64) -> HttpResponse {
    let url = reqwest::Url::parse(&request.url).expect("fixture request URL");
    assert_eq!(
        request.method,
        HttpMethod::Get,
        "unexpected write in a read fixture: {}",
        request.url
    );
    let login = login_for(user);
    let empty_first = EMPTY_FIRST_PAGE.load(std::sync::atomic::Ordering::Acquire);
    let path = url.path().to_string();
    let lower = path.to_ascii_lowercase();
    match path.as_str() {
        "/user" => json(200, serde_json::json!({"id": user, "login": login})),
        "/user/following" => json(
            200,
            serde_json::json!([
                {"id": 7, "login": "bob", "avatar_url": null},
                {"id": 8, "login": "carol", "avatar_url": null}
            ]),
        ),
        "/search/issues" => json(
            200,
            serde_json::json!({
                // One open item and one closed item: the page must show only the
                // open one even if a non-conforming response leaks a closed row.
                "total_count": 2, "incomplete_results": false,
                "items": [{
                    "id": 900, "number": 12,
                    "title": format!("Involved issue of {login}"),
                    "html_url": "https://github.com/alice/demo/issues/12",
                    "state": "open", "updated_at": "2026-10-28T10:00:00Z",
                    "repository_url": "https://api.github.com/repos/alice/demo",
                    "pull_request": null
                }, {
                    "id": 901, "number": 13, "title": "Synthetic closed issue",
                    "html_url": "https://github.com/alice/demo/issues/13",
                    "state": "closed", "updated_at": "2026-10-28T09:00:00Z",
                    "repository_url": "https://api.github.com/repos/alice/demo",
                    "pull_request": null
                }]
            }),
        ),
        "/repos/alice/demo" => json(
            200,
            serde_json::json!({
                "id": 10, "name": "demo", "full_name": "alice/demo",
                "owner": {"id": 1, "login": "alice", "avatar_url": null},
                "visibility": "public", "private": false, "default_branch": "main",
                "fork": false, "archived": false, "stargazers_count": 0,
                "forks_count": 0, "open_issues_count": 0, "size": 1,
                "description": "Synthetic repository description",
                "html_url": "https://github.com/alice/demo"
            }),
        ),
        "/repos/alice/demo/pulls/5" => json(
            200,
            serde_json::json!({
                "id": 555, "number": 5, "state": "closed", "merged": true,
                "title": "Synthetic merged pull request",
                "html_url": "https://github.com/alice/demo/pull/5",
                "user": {"id": 8, "login": "carol", "avatar_url": null},
                "body": "A detailed pull request description that exceeds the Feed preview limit. ".repeat(8)
            }),
        ),
        _ => {
            if lower.ends_with("/received_events") {
                let page_two = url.query_pairs().any(|(k, v)| k == "page" && v == "2");
                let events = if !page_two && empty_first {
                    // Every retained kind is filtered out here (push, unfollowed
                    // star), so the page is empty although the source has a next
                    // page.
                    serde_json::json!([{
                        "id": "evt-2", "type": "PushEvent",
                        "actor": {"id": 7, "login": "bob", "avatar_url": null},
                        "repo": {"id": 10, "name": "alice/demo", "url": "https://api.github.com/repos/alice/demo"},
                        "payload": {"ref": "refs/heads/main", "before": "a", "head": "b", "size": 1},
                        "created_at": "2026-10-28T08:00:00Z"
                    }, {
                        "id": "evt-3", "type": "WatchEvent",
                        "actor": {"id": 9, "login": "dave", "avatar_url": null},
                        "repo": {"id": 11, "name": "alice/other", "url": "https://api.github.com/repos/alice/other"},
                        "payload": {"action": "started"},
                        "created_at": "2026-10-28T07:30:00Z"
                    }])
                } else if page_two {
                    serde_json::json!([{
                        "id": "evt-5", "type": "WatchEvent",
                        "actor": {"id": 7, "login": "bob", "avatar_url": null},
                        "repo": {"id": 10, "name": "alice/demo", "url": "https://api.github.com/repos/alice/demo"},
                        "payload": {"action": "started"},
                        "created_at": "2026-10-28T05:00:00Z"
                    }])
                } else {
                    serde_json::json!([{
                        "id": "evt-1", "type": "WatchEvent",
                        "actor": {"id": 7, "login": "bob", "avatar_url": null},
                        "repo": {"id": 10, "name": "alice/demo", "url": "https://api.github.com/repos/alice/demo"},
                        "payload": {"action": "started"},
                        "created_at": "2026-10-28T09:00:00Z"
                    }, {
                        "id": "evt-2", "type": "PushEvent",
                        "actor": {"id": 7, "login": "bob", "avatar_url": null},
                        "repo": {"id": 10, "name": "alice/demo", "url": "https://api.github.com/repos/alice/demo"},
                        "payload": {"ref": "refs/heads/main", "before": "a", "head": "b", "size": 1},
                        "created_at": "2026-10-28T08:00:00Z"
                    }, {
                        "id": "evt-3", "type": "WatchEvent",
                        "actor": {"id": 9, "login": "dave", "avatar_url": null},
                        "repo": {"id": 11, "name": "alice/other", "url": "https://api.github.com/repos/alice/other"},
                        "payload": {"action": "started"},
                        "created_at": "2026-10-28T07:30:00Z"
                    }, {
                        "id": "evt-4", "type": "PullRequestEvent",
                        "actor": {"id": 7, "login": "bob", "avatar_url": null},
                        "repo": {"id": 10, "name": "alice/demo", "url": "https://api.github.com/repos/alice/demo"},
                        "payload": {
                            "action": "merged", "number": 5,
                            "pull_request": {
                                "url": "https://api.github.com/repos/alice/demo/pulls/5",
                                "id": 555, "number": 5,
                                "head": {"ref": "feature", "sha": "aaaa"},
                                "base": {"ref": "main", "sha": "bbbb"}
                            }
                        },
                        "created_at": "2026-10-28T07:00:00Z"
                    }])
                };
                let mut headers = vec![("x-poll-interval".to_string(), "1".to_string())];
                if !page_two {
                    headers.push((
                        "link".to_string(),
                        format!(
                            "<https://api.github.com/users/{login}/received_events?per_page=100&page=2>; rel=\"next\""
                        ),
                    ));
                }
                return HttpResponse {
                    status: 200,
                    headers,
                    body: events.to_string(),
                };
            }
            json(599, serde_json::json!({}))
        }
    }
}

struct Fixture {
    accounts: Vec<crate::domain::AccountKey>,
    offline: Arc<std::sync::atomic::AtomicBool>,
    user: Arc<Mutex<u64>>,
    executor: Arc<BackgroundExecutor>,
    service: Option<AccountService>,
    database: std::path::PathBuf,
    directory: tempfile::TempDir,
    cache_directory: tempfile::TempDir,
    credentials: Arc<InMemoryCredentialStore>,
}

impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let cache_directory = tempfile::tempdir().unwrap();
        let credentials = Arc::new(InMemoryCredentialStore::new());
        let offline = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let user = Arc::new(Mutex::new(1u64));
        let executor = Arc::new(BackgroundExecutor::new(4));
        let database = directory.path().join("accounts.sqlite3");
        let mut fixture = Self {
            accounts: Vec::new(),
            offline,
            user,
            executor,
            service: None,
            database,
            directory,
            cache_directory,
            credentials,
        };
        fixture.service = Some(fixture.open().await);
        fixture.login("synthetic-home-primary").await;
        fixture
    }

    fn client(&self) -> Arc<GitHubClient> {
        Arc::new(GitHubClient::new(
            GitHubConfig::default(),
            Arc::new(ScriptedTransport {
                offline: self.offline.clone(),
                user: self.user.clone(),
            }),
        ))
    }

    async fn try_open(&self) -> Option<AccountService> {
        let store =
            CacheStore::open(CacheConfig::at(self.cache_directory.path().to_path_buf())).unwrap();
        AccountService::open_with_cache(
            AccountDatabaseConfig::at(self.database.clone()),
            self.credentials.clone(),
            self.client(),
            self.executor.clone(),
            store,
        )
        .await
        .ok()
    }

    async fn open(&self) -> AccountService {
        self.try_open()
            .await
            .expect("the account database opens")
    }

    async fn login(&mut self, token: &str) {
        let service = self.service.as_ref().expect("fixture service");
        service.login(Pat::from_str(token)).await.unwrap();
        let key = service
            .active_summary()
            .expect("an active account")
            .identity
            .key
            .clone();
        if !self.accounts.contains(&key) {
            self.accounts.push(key);
        }
    }

    /// Reopens the account database (a restart) and revalidates the saved active
    /// account against the transport.
    async fn restart(&mut self) {
        self.service = None;
        // The previous backend closes its account database asynchronously, so a
        // restart waits for the single-writer lock instead of failing spuriously.
        let mut service = None;
        for _ in 0..80 {
            if let Some(opened) = self.try_open().await {
                service = Some(opened);
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let service = service.expect("the account database opens after the restart");
        if let Some(active) = service
            .registry_handle()
            .lock()
            .unwrap()
            .summaries()
            .first()
            .map(|summary| summary.identity.key.clone())
        {
            let _ = service.revalidate(active).await;
        }
        self.service = Some(service);
    }
}

struct Driver {
    runtime: RuntimeHandle,
    page: HomePage,
}

impl Driver {
    fn tick(&mut self) {
        let commands: Vec<TestCommand> = {
            let mut pending = COMMANDS.lock().expect("commands");
            pending.drain(..).collect()
        };
        for command in commands {
            match command {
                TestCommand::Select(account) => {
                    let result = self.runtime.submit(Command::Select(account.clone()));
                    if matches!(result, Err(crate::app::runtime::SubmitError::Busy)) {
                        // A sidebar read owned the runtime slot; retry next tick.
                        COMMANDS.lock().expect("commands").push(TestCommand::Select(account));
                    }
                }
                TestCommand::Refresh => {
                    self.page.refresh(&self.runtime);
                }
            }
        }
        self.page.poll(&self.runtime);
        let snapshot = self.runtime.snapshot();
        STATUS.lock().expect("status").push(format!(
            "revision {} active {:?}",
            snapshot.binding_revision,
            snapshot.active.as_ref().map(|key| key.id)
        ));
    }
}

#[derive(Clone, PartialEq, Props)]
struct HarnessProps {}

#[component]
fn HomeHarness(_props: HarnessProps) -> impl IntoElement {
    let driver: std::rc::Rc<std::cell::RefCell<Option<Driver>>> =
        std::rc::Rc::new(std::cell::RefCell::new(build_driver()));
    set_interval(Duration::from_millis(25), {
        let driver = driver.clone();
        move || {
            if let Ok(mut driver) = driver.try_borrow_mut()
                && let Some(driver) = driver.as_mut()
            {
                driver.tick();
            }
        }
    });
    let epoch = signal(0u64);
    set_interval(Duration::from_millis(120), move || epoch.update(|n| *n += 1));
    dynamic(move || {
        let _ = epoch.get();
        let body = match driver.try_borrow_mut() {
            Ok(driver) => match driver.as_ref() {
                Some(driver) => driver.page.element(),
                None => view()
                    .child(text("no driver"))
                    .into_element(),
            },
            Err(_) => view().into_element(),
        };
        view()
            .size_full()
            .bg(crate::ui::foundation::theme::theme().background)
            .child(body)
            .child(
                view()
                    .test_id("nav-log")
                    .absolute()
                    .top(px(0.0))
                    .left(px(0.0))
                    .child(
                        text(NAVIGATION.lock().expect("nav log").join("|"))
                            .text_size(px(8.0)),
                    )
                    .child(
                        text(STATUS.lock().expect("status").join("; "))
                            .text_size(px(8.0)),
                    ),
            )
            .into_element()
    })
    .into_element()
}

fn build_driver() -> Option<Driver> {
    let backend = SLOT.lock().ok()?.take()?.backend;
    let backend = Arc::try_unwrap(backend).ok()?;
    let mut runtime = RuntimeHandle::from_backend(backend);
    runtime.snapshot();
    let tab = runtime.sessions().workspace().active_id();
    let services = runtime.services()?;
    let context = services
        .page_context(runtime.sessions(), tab, None)
        .expect("a page context for the active account");
    let navigation: NavigationSink = std::rc::Rc::new(move |action: PageNavigation| {
        let label = match action {
            PageNavigation::Open(route) => match route {
                Route::Repository { namespace, name } => format!("open {namespace}/{name}"),
                Route::PullRequest {
                    namespace,
                    name,
                    number,
                } => format!("open {namespace}/{name} #{number}"),
                other => format!("open {}", other.title()),
            },
            PageNavigation::Repository { route, .. } => match route {
                Route::Repository { namespace, name } => format!("open {namespace}/{name}"),
                other => format!("open {}", other.title()),
            },
            PageNavigation::External(url) => format!("external {url}"),
            PageNavigation::CanonicalProfile { login, .. } => format!("canonical {login}"),
        };
        NAVIGATION.lock().expect("navigation log").push(label);
    });
    let page = HomePage::new(context, navigation);
    Some(Driver { runtime, page })
}

fn mount(fixture: &mut Fixture, empty_first_page: bool) -> Headless {
    EMPTY_FIRST_PAGE.store(empty_first_page, std::sync::atomic::Ordering::Release);
    *NAVIGATION.lock().unwrap() = Vec::new();
    *COMMANDS.lock().unwrap() = Vec::new();
    *STATUS.lock().unwrap() = Vec::new();
    let service = fixture.service.take().expect("fixture service");
    let backend = Backend::new(service, fixture.executor.clone(), ExitGate::default());
    *SLOT.lock().unwrap() = Some(BackendSlot {
        backend: Arc::new(backend),
    });
    let root =
        Root::new::<HomeHarness>(HarnessProps::default).tokens(crate::ui::foundation::theme::tokens());
    let mut app = Headless::start(EngineConfig::new(root)).expect("headless app starts");
    app.set_viewport(nsh_core::geometry::Size::new(
        nsh_core::geometry::px(1200.0),
        nsh_core::geometry::px(900.0),
    ))
    .unwrap();
    app
}

fn navigation() -> Vec<String> {
    NAVIGATION.lock().unwrap().clone()
}

#[test]
fn home_and_feed_render_real_account_data() {
    let _guard = LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut fixture = runtime.block_on(Fixture::new());
    let mut app = mount(&mut fixture, false);
    app.wait_for("Involved issue of alice")
        .expect("the open involved issue renders");
    app.wait_for("Synthetic merged pull request #5")
        .expect("the merged contribution renders");
    app.wait_for("Synthetic repository description")
        .expect("the star card description is enriched");
    assert!(
        !app.screen().shows("Synthetic closed issue"),
        "a closed search result must never become a visible row"
    );
    assert!(
        !app.screen().shows("alice/other"),
        "an unfollowed account's activity is withheld"
    );
    // The merged card credits the pull-request author, not the merging actor.
    app.wait_for("carol").expect("the credited author renders");
    // Relative time stays on the shared formatter's wording.
    assert!(
        app.screen()
            .texts()
            .iter()
            .any(|text| text.ends_with(" ago") || *text == "Just now" || *text == "Yesterday"),
        "relative time is rendered by the shared formatter: {:?}",
        app.screen().texts()
    );
    // No avatar loader is attached in this fixture, so the card keeps its stable
    // initials instead of inventing a picture.
    app.wait_for("id:feed-avatar-0")
        .expect("the avatar slot renders");
}

#[test]
fn read_more_opens_the_internal_pull_request_route() {
    let _guard = LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut fixture = runtime.block_on(Fixture::new());
    let mut app = mount(&mut fixture, false);
    app.wait_for("id:feed-pr-read-more-1")
        .expect("a truncated PR body offers Read more");
    app.press("id:feed-pr-read-more-1")
        .expect("the Read more link is pressable");
    app.wait_for("open alice/demo #5")
        .expect("Read more opens the internal pull-request route");
    assert!(
        !navigation().iter().any(|entry| entry.starts_with("external")),
        "Read more must never open a browser: {:?}",
        navigation()
    );
}

#[test]
fn a_filtered_empty_first_page_appends_the_next_source_page() {
    let _guard = LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut fixture = runtime.block_on(Fixture::new());
    let mut app = mount(&mut fixture, true);
    // The first source page retains no card at all (a push and an unfollowed
    // star); the page must still continue to the next older page on its own
    // instead of pretending the Feed ended (LESSONS L015).
    app.wait_for("id:feed-event-row-0")
        .expect("the next source page is read without user interaction");
    app.wait_for("bob")
        .expect("the appended star keeps its credited actor");
    assert!(navigation().is_empty(), "loading more must not navigate");
}

#[test]
fn account_switch_clears_displayed_home_data_in_one_drain() {
    let _guard = LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut fixture = runtime.block_on(Fixture::new());
    // A second account makes the switch observable.
    {
        let user = fixture.user.clone();
        *user.lock().unwrap() = 2;
    }
    runtime.block_on(fixture.login("synthetic-home-secondary"));
    {
        let user = fixture.user.clone();
        *user.lock().unwrap() = 1;
    }
    // Mount with the first account active so selecting the second is a real change.
    {
        let first = fixture.accounts[0].clone();
        let service = fixture.service.as_ref().expect("fixture service");
        runtime.block_on(service.set_active(first)).unwrap();
    }
    let second = fixture.accounts.last().expect("second account").clone();
    let mut app = mount(&mut fixture, false);
    app.wait_for("Involved issue of alice")
        .expect("the first account's Home renders");
    COMMANDS.lock().unwrap().push(TestCommand::Select(second));
    app.wait_for("The account or authorization changed. Refresh in the current account.")
        .expect("the displayed content is retired in the same drain");
    assert!(
        !app.screen().shows("Involved issue of alice"),
        "already displayed rows must not survive the account change"
    );
}

#[test]
fn offline_refresh_serves_the_saved_snapshot_and_marks_it_saved() {
    let _guard = LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut fixture = runtime.block_on(Fixture::new());
    let mut app = mount(&mut fixture, false);
    app.wait_for("Involved issue of alice")
        .expect("the online read warms the page cache");
    app.wait_for("Synthetic merged pull request #5")
        .expect("the Feed read warms the page cache");
    // Network reads now fail: the page must fall back to the saved snapshot and
    // keep saying so, instead of pretending it is a live result.
    fixture
        .offline
        .store(true, std::sync::atomic::Ordering::Release);
    // `X-Poll-Interval` keeps the Feed refresh disabled for about a second; the
    // page honours it, so wait that window out before asking for a refresh.
    std::thread::sleep(Duration::from_millis(1300));
    COMMANDS.lock().unwrap().push(TestCommand::Refresh);
    app.wait_for("id:home-historical")
        .expect("the saved Home snapshot is marked as saved");
    app.wait_for("id:feed-historical")
        .expect("the saved Feed snapshot is marked as saved");
    assert!(
        app.screen().shows("Involved issue of alice"),
        "the saved Home content stays readable offline"
    );
    assert!(
        app.screen().shows("Synthetic merged pull request #5"),
        "the saved Feed content stays readable offline"
    );
}
