//! Deterministic Native Shell real-service regressions for ProfilePage.
//!
//! Real `AccountService` (SQLite account database, real `CacheStore`, in-memory credentials),
//! a scripted transport that serves Profile REST and GraphQL requests by identity, the real
//! `RuntimeHandle`/`PageContext` contract and `ProfilePage`. The page is created, polled and
//! retired inside a headless NSH component on the engine thread, so `create_root`/signals see a
//! reactive runtime and the assertions run against the published view tree.
//!
//! Covers:
//! 1. Confirmed README absence (404 from the same-name repository) stays a quiet optional absence.
//! 2. Cached content is served offline and marked as a historical snapshot.
//! 3. Revocation clears already displayed content in the same drain (LESSONS L022).

use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nsh_core::prelude::*;
use nsh_engine::{EngineConfig, Root};
use nsh_testing::Headless;

use crate::app::pages::{NavigationSink, PageNavigation, PageView};
use crate::app::runtime::{Backend, ExitGate, RuntimeHandle};
use crate::auth::{InMemoryCredentialStore, token::Pat};
use crate::data::AccountService;
use crate::github::{GitHubClient, GitHubConfig};
use crate::network::cancel::CancellationToken;
use crate::network::executor::BackgroundExecutor;
use crate::network::transport::{HttpMethod, HttpRequest, HttpResponse, Transport, TransportError};
use crate::storage::accounts::AccountDatabaseConfig;
use crate::storage::cache::{CacheConfig, CacheStore};

use super::ProfilePage;

static LOCK: Mutex<()> = Mutex::new(());
static SLOT: Mutex<Option<Arc<Backend>>> = Mutex::new(None);
static TARGET_LOGIN: Mutex<String> = Mutex::new(String::new());
static COMMANDS: Mutex<Vec<TestCommand>> = Mutex::new(Vec::new());

#[derive(Clone, Debug)]
enum TestCommand {
    Refresh,
    RevokeRepository(u64),
}

fn json(status: u16, value: serde_json::Value) -> HttpResponse {
    let mut resp = HttpResponse {
        status,
        headers: Vec::new(),
        body: value.to_string(),
    };
    resp.headers
        .push(("content-type".into(), "application/json".into()));
    resp
}

fn graphql_ok(data: serde_json::Value) -> HttpResponse {
    json(200, serde_json::json!({ "data": data }))
}

const COMMIT_SHA: &str = "1111111111111111111111111111111111111111";
const TREE_SHA: &str = "2222222222222222222222222222222222222222";
const BLOB_SHA: &str = "3333333333333333333333333333333333333333";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReadmeMode {
    Present,
    NotFound,
}

struct ProfileScriptedTransport {
    offline: Arc<std::sync::atomic::AtomicBool>,
    readme_mode: Arc<Mutex<ReadmeMode>>,
}

impl Transport for ProfileScriptedTransport {
    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        _cancel: CancellationToken,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>>
    {
        Box::pin(async move {
            if self.offline.load(std::sync::atomic::Ordering::Acquire) {
                return Err(TransportError::Network {
                    message: "synthetic offline".into(),
                });
            }
            let mode = *self.readme_mode.lock().expect("readme_mode");
            Ok(script(&request, mode))
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

fn script(request: &HttpRequest, readme_mode: ReadmeMode) -> HttpResponse {
    let url = reqwest::Url::parse(&request.url).expect("url");
    let path = url.path().to_string();
    let lower = path.to_ascii_lowercase();

    if request.method == HttpMethod::Get && path == "/user" {
        return json(200, serde_json::json!({ "id": 1, "login": "alice" }));
    }

    if lower == "/users/alice" {
        return json(
            200,
            serde_json::json!({
                "id": 1,
                "login": "alice",
                "name": "Alice Example",
                "bio": "Rust and native systems enthusiast",
                "avatar_url": null,
                "followers": 42,
                "following": 10,
                "company": "Githux Org",
                "location": "Earth",
                "website_url": "https://alice.example.com",
            }),
        );
    }

    if lower.ends_with("/graphql") {
        let body = request.body.as_deref().unwrap_or_default();
        if body.contains("pinnedItems") || body.contains("itemShowcase") {
            return graphql_ok(serde_json::json!({
                "user": {
                    "pinnedItems": {
                        "totalCount": 1,
                        "pageInfo": { "hasNextPage": false },
                        "nodes": [{
                            "__typename": "Repository",
                            "databaseId": 101,
                            "name": "core",
                            "nameWithOwner": "alice/core",
                            "description": "Core systems and engine library",
                            "url": "https://github.com/alice/core",
                            "isPrivate": false,
                            "isFork": false,
                            "stargazerCount": 99,
                            "forkCount": 12,
                            "primaryLanguage": { "name": "Rust" },
                            "owner": { "login": "alice" }
                        }]
                    },
                    "itemShowcase": {
                        "hasPinnedItems": true,
                        "items": {
                            "totalCount": 1,
                            "pageInfo": { "hasNextPage": false },
                            "nodes": [{
                                "__typename": "Repository",
                                "databaseId": 101,
                                "name": "core",
                                "nameWithOwner": "alice/core",
                                "description": "Core systems and engine library",
                                "url": "https://github.com/alice/core",
                                "isPrivate": false,
                                "isFork": false,
                                "stargazerCount": 99,
                                "forkCount": 12,
                                "primaryLanguage": { "name": "Rust" },
                                "owner": { "login": "alice" }
                            }]
                        }
                    }
                }
            }));
        }

        if body.contains("contributionCalendar") {
            return graphql_ok(serde_json::json!({
                "user": {
                    "contributionsCollection": {
                        "contributionCalendar": {
                            "totalContributions": 18,
                            "weeks": [{
                                "contributionDays": [{
                                    "date": "2026-01-01",
                                    "contributionCount": 18,
                                    "contributionLevel": "FOURTH_QUARTILE",
                                    "color": "#216e39"
                                }]
                            }],
                            "months": []
                        }
                    }
                }
            }));
        }
    }

    if lower == "/repos/alice/alice" {
        if readme_mode == ReadmeMode::NotFound {
            return json(404, serde_json::json!({ "message": "Not Found" }));
        }
        return json(
            200,
            serde_json::json!({
                "id": 77,
                "name": "alice",
                "full_name": "alice/alice",
                "owner": { "id": 1, "login": "alice" },
                "visibility": "public",
                "private": false,
                "default_branch": "main",
                "fork": false,
                "archived": false,
                "stargazers_count": 5,
                "forks_count": 0,
                "open_issues_count": 0,
                "size": 1,
                "html_url": "https://github.com/alice/alice"
            }),
        );
    }

    if lower.contains("/repos/alice/alice/commits/") {
        return json(
            200,
            serde_json::json!({
                "sha": COMMIT_SHA,
                "commit": {
                    "message": "profile readme commit",
                    "tree": { "sha": TREE_SHA },
                    "author": null,
                    "committer": null
                }
            }),
        );
    }

    if lower.ends_with(&format!("/repos/alice/alice/git/trees/{TREE_SHA}")) {
        return json(
            200,
            serde_json::json!({
                "sha": TREE_SHA,
                "truncated": false,
                "tree": [
                    {
                        "path": "README.md",
                        "type": "blob",
                        "mode": "100644",
                        "sha": BLOB_SHA,
                        "size": 64
                    }
                ]
            }),
        );
    }

    if lower.ends_with(&format!("/repos/alice/alice/git/blobs/{BLOB_SHA}")) {
        use base64::Engine;
        let content_text = "# Alice Profile\n\nWelcome to Alice's profile readme text.";
        let encoded = base64::engine::general_purpose::STANDARD.encode(content_text.as_bytes());
        return json(
            200,
            serde_json::json!({
                "sha": BLOB_SHA,
                "encoding": "base64",
                "size": content_text.len(),
                "content": encoded
            }),
        );
    }

    json(
        599,
        serde_json::json!({ "error": "unmodelled fixture request" }),
    )
}

struct Fixture {
    offline: Arc<std::sync::atomic::AtomicBool>,
    readme_mode: Arc<Mutex<ReadmeMode>>,
    executor: Arc<BackgroundExecutor>,
    service: Option<AccountService>,
    database: std::path::PathBuf,
    _directory: tempfile::TempDir,
    cache_directory: tempfile::TempDir,
    credentials: Arc<InMemoryCredentialStore>,
}

impl Fixture {
    async fn new(readme_mode: ReadmeMode) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let cache_directory = tempfile::tempdir().unwrap();
        let credentials = Arc::new(InMemoryCredentialStore::new());
        let offline = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let readme_mode = Arc::new(Mutex::new(readme_mode));
        let executor = Arc::new(BackgroundExecutor::new(4));
        let database = directory.path().join("accounts.sqlite3");
        let mut fixture = Self {
            offline,
            readme_mode,
            executor,
            service: None,
            database,
            _directory: directory,
            cache_directory,
            credentials,
        };
        fixture.service = Some(fixture.open().await);
        fixture.login().await;
        fixture
    }

    fn client(&self) -> Arc<GitHubClient> {
        Arc::new(GitHubClient::new(
            GitHubConfig::default(),
            Arc::new(ProfileScriptedTransport {
                offline: self.offline.clone(),
                readme_mode: self.readme_mode.clone(),
            }),
        ))
    }

    async fn open(&self) -> AccountService {
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
        .expect("the account database opens")
    }

    async fn login(&mut self) {
        let service = self.service.as_ref().expect("fixture service");
        service
            .login(Pat::from_str("synthetic-profile-primary"))
            .await
            .unwrap();
    }
}

struct Driver {
    runtime: RuntimeHandle,
    page: ProfilePage,
}

impl Driver {
    fn tick(&mut self) {
        let commands: Vec<TestCommand> = {
            let mut pending = COMMANDS.lock().expect("commands");
            pending.drain(..).collect()
        };
        for command in commands {
            match command {
                TestCommand::Refresh => self.page.refresh(&self.runtime),
                TestCommand::RevokeRepository(repo_id) => {
                    if let Some(services) = self.runtime.services()
                        && let Ok(session) = services.repositories().session()
                        && let Some(cache) = services.repositories().cache(session.context())
                    {
                        let _ = cache
                            .store()
                            .revoke_repository(cache.scope(), Some(repo_id));
                    }
                }
            }
        }
        self.page.poll(&self.runtime);
    }
}

#[derive(Clone, PartialEq, Props)]
struct ProfileHarnessProps {}

#[component]
fn ProfileRealHarness(_props: ProfileHarnessProps) -> impl IntoElement {
    let driver: std::rc::Rc<std::cell::RefCell<Option<Driver>>> =
        std::rc::Rc::new(std::cell::RefCell::new(build_driver()));
    // The Profile page's `element()` is a static placeholder until the first section
    // creates the view, so it reads no signal and cannot drive its own re-render.
    // A host revision (bumped on every poll) forces the region to re-evaluate and
    // pick up the newly created view, exactly as the Shell repaints each frame.
    let revision = signal(0u64);
    set_interval(Duration::from_millis(20), {
        let driver = driver.clone();
        move || {
            if let Ok(mut driver) = driver.try_borrow_mut()
                && let Some(driver) = driver.as_mut()
            {
                driver.tick();
            }
            revision.update(|n| *n += 1);
        }
    });
    dynamic(move || {
        let _ = revision.get();
        let body = match driver.try_borrow() {
            Ok(driver) => match driver.as_ref() {
                Some(driver) => driver.page.element(),
                None => view().child(text("no driver")).into_element(),
            },
            Err(_) => view().into_element(),
        };
        view()
            .size_full()
            .bg(crate::ui::foundation::theme::theme().background)
            .child(body)
            .into_element()
    })
    .into_element()
}

fn build_driver() -> Option<Driver> {
    let backend = SLOT.lock().ok()?.take()?;
    let backend = Arc::try_unwrap(backend).ok()?;
    let mut runtime = RuntimeHandle::from_backend(backend);
    runtime.snapshot();
    let tab = runtime.sessions().workspace().active_id();
    let services = runtime.services()?;
    let context = services
        .page_context(runtime.sessions(), tab, None)
        .expect("a page context for the active account");
    let navigation: NavigationSink = Rc::new(|_action: PageNavigation| {});
    let login = TARGET_LOGIN.lock().expect("login").clone();
    let page = ProfilePage::new(context, login, navigation);
    Some(Driver { runtime, page })
}

fn mount(fixture: &mut Fixture, target_login: &str) -> Headless {
    *TARGET_LOGIN.lock().unwrap() = target_login.to_string();
    *COMMANDS.lock().unwrap() = Vec::new();
    let service = fixture.service.take().expect("fixture service");
    let backend = Backend::new(service, fixture.executor.clone(), ExitGate::default());
    *SLOT.lock().unwrap() = Some(Arc::new(backend));
    let root = Root::new::<ProfileRealHarness>(ProfileHarnessProps::default)
        .tokens(crate::ui::foundation::theme::tokens());
    let mut app = Headless::start(EngineConfig::new(root)).expect("headless app starts");
    app.set_viewport(nsh_core::geometry::Size::new(
        nsh_core::geometry::px(1200.0),
        nsh_core::geometry::px(900.0),
    ))
    .unwrap();
    app
}

#[test]
fn profile_real_service_renders_all_sections_with_readme() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut fixture = runtime.block_on(Fixture::new(ReadmeMode::Present));
    let mut app = mount(&mut fixture, "alice");

    app.wait_for("Alice Example")
        .expect("basic profile renders the display name");
    app.wait_for("@alice")
        .expect("basic profile renders the login");
    app.wait_for("Core systems and engine library")
        .expect("pinned repository description renders");
    app.wait_for("id:profile-readme-section")
        .expect("the README section renders when present");
    app.wait_for("id:profile-heatmap")
        .expect("the contribution heatmap renders");
}

#[test]
fn profile_confirmed_readme_absence_omits_the_whole_section() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut fixture = runtime.block_on(Fixture::new(ReadmeMode::NotFound));
    let mut app = mount(&mut fixture, "alice");

    app.wait_for("Alice Example")
        .expect("profile basic renders");
    app.wait_for("Core systems and engine library")
        .expect("pinned repositories render");
    app.wait_for("id:profile-heatmap")
        .expect("contributions render");
    // L014: a confirmed absence must omit the whole optional section, no card, no error.
    assert!(
        !app.screen().shows("id:profile-readme-section"),
        "an absent README must not leave an empty section"
    );
    assert!(
        !app.screen().shows("Profile README could not be loaded"),
        "an absent README is not an actionable error"
    );
    assert!(
        !app.screen().shows("id:profile-readme-error"),
        "an absent README must not show an error marker"
    );
}

#[test]
fn profile_offline_refresh_serves_cached_historical_content() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut fixture = runtime.block_on(Fixture::new(ReadmeMode::Present));
    let mut app = mount(&mut fixture, "alice");

    app.wait_for("Alice Example").expect("online basic renders");
    app.wait_for("id:profile-readme-section")
        .expect("online readme renders");
    assert!(
        !app.screen().shows("id:profile-basic-provenance-historical"),
        "a fresh online read is not a historical snapshot"
    );

    fixture
        .offline
        .store(true, std::sync::atomic::Ordering::Release);
    COMMANDS.lock().unwrap().push(TestCommand::Refresh);

    app.wait_for("id:profile-basic-provenance-historical")
        .expect("the cached profile is marked as a historical snapshot");
    app.wait_for("Core systems and engine library")
        .expect("cached pinned content stays readable offline");
    app.wait_for("id:profile-readme-section")
        .expect("cached README stays readable offline");
    assert!(
        app.screen().shows("Alice Example"),
        "cached basic content stays readable offline"
    );
}

#[test]
fn profile_revocation_clears_already_displayed_content_immediately() {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut fixture = runtime.block_on(Fixture::new(ReadmeMode::Present));
    let mut app = mount(&mut fixture, "alice");

    app.wait_for("Alice Example")
        .expect("online profile renders");
    app.wait_for("Core systems and engine library")
        .expect("pinned repositories render");

    COMMANDS
        .lock()
        .unwrap()
        .push(TestCommand::RevokeRepository(101));

    // LESSONS L022: the revoked aggregate retires the displayed sections and the page
    // falls back to the retryable Forbidden placeholder (same wording as the accepted GPUI page).
    app.wait_for("This section is not available with the current authorization.")
        .expect("revocation clears displayed content and shows a retryable placeholder");
    assert!(
        !app.screen().shows("Core systems and engine library"),
        "revoked private content must not stay on screen"
    );
}
