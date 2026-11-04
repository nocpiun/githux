//! Repository page offline composition.
//!
//! These tests drive the **real** page over the real account service, real
//! `CachedRepositorySession` / `CachedPageReadSession` / `RepositoryImageService`
//! compositions and a real persistent `CacheStore` in a temporary directory. The
//! only injected boundaries are the deterministic GitHub transport, a temporary
//! SQLite account directory and (for external pictures) an injected byte source.
//!
//! The scenario is the product one: read a repository online, close the app
//! (release the page, close the account service and the store), reopen both, then
//! browse the same repository, commit, directory, README, file and README picture
//! with the transport refusing every request. Anything never read before must
//! stay an honest miss, never an invented empty result.
//!
//! The GPUI test platform cannot prove real Windows input, a real PAT or real
//! GitHub; that boundary is recorded in the task devlog.

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    AppContext, Entity, IntoElement, Modifiers, ParentElement, Styled, TestAppContext,
    VisualTestContext,
};

use super::{
    ReaderStatus, RepositoryNavigation, RepositoryPage, RepositoryPageServices, Selection,
};
use super::{frames, locator, setup};
use crate::auth::InMemoryCredentialStore;
use crate::content::images::ImageTarget;
use crate::data::{AccountService, RepositoryCacheOrigin};
use crate::github::{GitHubClient, GitHubConfig};
use crate::network::cancel::CancellationToken;
use crate::network::executor::BackgroundExecutor;
use crate::network::transport::{
    HttpRequest, HttpResponse, Transport, TransportError, json_response,
};
use crate::repository_modules::fixture::{FixtureTransport, NAME, OWNER, Scene};
use crate::storage::accounts::AccountDatabaseConfig;
use crate::storage::cache::{CacheConfig, CacheStore};
use crate::ui::reader::ImageKey;

const MAIN_COMMIT: &str = "c0ffee0000000000000000000000000000000001";
const FEATURE_COMMIT: &str = "c0ffee0000000000000000000000000000000002";
const MAIN_TREE: &str = "a000000000000000000000000000000000000001";
const README_BLOB: &str = "b000000000000000000000000000000000000001";
const DIAGRAM_BLOB: &str = "b0000000000000000000000000000000000000c1";

fn png_bytes() -> Vec<u8> {
    let image = image::RgbaImage::from_fn(8, 4, |x, y| {
        image::Rgba([(x * 30) as u8, (y * 60) as u8, 120, 255])
    });
    let mut cursor = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut cursor, image::ImageFormat::Png)
        .expect("encode the fixture README picture");
    cursor.into_inner()
}

fn b64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Wraps `FixtureTransport` with an offline switch and optional README picture
/// modeling so the shared fixture module does not need to be modified.
pub(super) struct OfflineTransport {
    pub(super) inner: Arc<FixtureTransport>,
    offline: Mutex<bool>,
    readme_image: Mutex<bool>,
}

impl OfflineTransport {
    fn new(scene: Scene) -> Self {
        Self {
            inner: Arc::new(FixtureTransport::new(scene)),
            offline: Mutex::new(false),
            readme_image: Mutex::new(false),
        }
    }

    pub(super) fn set_offline(&self, offline: bool) {
        *self.offline.lock().unwrap() = offline;
    }

    pub(super) fn set_readme_image(&self, enabled: bool) {
        *self.readme_image.lock().unwrap() = enabled;
    }

    pub(super) fn set_identity(&self, id: u64, login: &str) {
        self.inner.set_identity(id, login);
    }

    pub(super) fn recorded(&self) -> Vec<String> {
        self.inner.recorded()
    }

    pub(super) fn star_method_count(&self, method: crate::network::transport::HttpMethod) -> usize {
        self.inner.star_method_count(method)
    }

    fn custom_answer(&self, request: &HttpRequest) -> Option<HttpResponse> {
        if !*self.readme_image.lock().unwrap() {
            return None;
        }
        let url = request.url.as_str();
        // Fixed-commit resolution used by `RepositoryImageService`.
        if url.ends_with(&format!("/repos/{OWNER}/{NAME}/commits/{MAIN_COMMIT}")) {
            return Some(json_response(
                200,
                &serde_json::json!({
                    "sha": MAIN_COMMIT,
                    "commit": {
                        "message": "fixture commit on main",
                        "tree": { "sha": MAIN_TREE },
                        "author": { "name": OWNER, "email": null, "date": "2026-10-20T10:00:00Z" },
                        "committer": { "name": OWNER, "email": null, "date": "2026-10-20T10:00:00Z" }
                    }
                })
                .to_string(),
            ));
        }
        if url.ends_with(&format!("/repos/{OWNER}/{NAME}/commits/{FEATURE_COMMIT}")) {
            return Some(json_response(
                200,
                &serde_json::json!({
                    "sha": FEATURE_COMMIT,
                    "commit": {
                        "message": "fixture commit on feature/docs-polish",
                        "tree": { "sha": "a000000000000000000000000000000000000002" },
                        "author": { "name": OWNER, "email": null, "date": "2026-10-20T10:00:00Z" },
                        "committer": { "name": OWNER, "email": null, "date": "2026-10-20T10:00:00Z" }
                    }
                })
                .to_string(),
            ));
        }
        let readme_md =
            "# Ledger\n\nA tiny double-entry ledger.\n\n![Ledger diagram](diagram.png)\n";
        if url.contains(&format!("/repos/{OWNER}/{NAME}/readme?ref={MAIN_COMMIT}")) {
            return Some(json_response(
                200,
                &serde_json::json!({
                    "type": "file",
                    "path": "README.md",
                    "sha": README_BLOB,
                    "size": readme_md.len(),
                    "encoding": "base64"
                })
                .to_string(),
            ));
        }
        if url.ends_with(&format!("/repos/{OWNER}/{NAME}/git/trees/{MAIN_TREE}")) {
            let png = png_bytes();
            return Some(json_response(
                200,
                &serde_json::json!({
                    "sha": MAIN_TREE,
                    "truncated": false,
                    "tree": [
                        { "path": "README.md", "mode": "100644", "type": "blob", "sha": README_BLOB, "size": readme_md.len() },
                        { "path": "diagram.png", "mode": "100644", "type": "blob", "sha": DIAGRAM_BLOB, "size": png.len() },
                        { "path": "docs", "mode": "040000", "type": "tree", "sha": "a000000000000000000000000000000000000010" },
                        { "path": "src", "mode": "040000", "type": "tree", "sha": "a000000000000000000000000000000000000020" }
                    ]
                })
                .to_string(),
            ));
        }
        if url.ends_with(&format!("/repos/{OWNER}/{NAME}/git/blobs/{README_BLOB}")) {
            let bytes = readme_md.as_bytes();
            return Some(json_response(
                200,
                &serde_json::json!({
                    "sha": README_BLOB,
                    "encoding": "base64",
                    "size": bytes.len(),
                    "content": b64(bytes)
                })
                .to_string(),
            ));
        }
        if url.ends_with(&format!("/repos/{OWNER}/{NAME}/git/blobs/{DIAGRAM_BLOB}")) {
            let png = png_bytes();
            return Some(json_response(
                200,
                &serde_json::json!({
                    "sha": DIAGRAM_BLOB,
                    "encoding": "base64",
                    "size": png.len(),
                    "content": b64(&png)
                })
                .to_string(),
            ));
        }
        None
    }
}

impl Transport for OfflineTransport {
    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(TransportError::Cancelled);
            }
            if *self.offline.lock().unwrap() {
                return Err(TransportError::Network {
                    message: "offline fixture: connection refused".into(),
                });
            }
            if let Some(custom) = self.custom_answer(&request) {
                return Ok(custom);
            }
            self.inner.execute(request, cancel).await
        })
    }

    fn execute_bounded<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
        max_body_bytes: usize,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            let response = self.execute(request, cancel).await?;
            if response.body.len() > max_body_bytes {
                return Err(TransportError::ResponseTooLarge {
                    limit: max_body_bytes,
                });
            }
            Ok(response)
        })
    }
}

pub(super) struct CachedFixture {
    pub(super) transport: Arc<OfflineTransport>,
    accounts: Option<Arc<AccountService>>,
    credentials: Arc<InMemoryCredentialStore>,
    executor: Arc<BackgroundExecutor>,
    store: Option<CacheStore>,
    temp: tempfile::TempDir,
    runtime: tokio::runtime::Runtime,
}

impl CachedFixture {
    /// Opens the real cached service stack over the controlled offline transport.
    pub(super) fn new(scene: Scene) -> Self {
        let temp = tempfile::tempdir().expect("temporary account directory");
        let executor = Arc::new(BackgroundExecutor::new(4));
        let transport = Arc::new(OfflineTransport::new(scene));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("host runtime");
        let credentials = Arc::new(InMemoryCredentialStore::new());
        let creds_clone = credentials.clone();
        let (accounts, store) = runtime.block_on(async {
            let cache = CacheStore::open(CacheConfig::at(temp.path().join("cache")))
                .expect("open the app cache store");
            let accounts = AccountService::open_with_cache(
                AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3")),
                creds_clone,
                Arc::new(GitHubClient::new(
                    GitHubConfig::default(),
                    transport.clone() as Arc<dyn Transport>,
                )),
                executor.clone(),
                cache.clone(),
            )
            .await
            .expect("open the cached account service");
            (Arc::new(accounts), cache)
        });
        Self {
            transport,
            accounts: Some(accounts),
            credentials,
            executor,
            store: Some(store),
            temp,
            runtime,
        }
    }

    fn accounts(&self) -> &Arc<AccountService> {
        self.accounts.as_ref().expect("an open account service")
    }

    fn services(&self) -> RepositoryPageServices {
        RepositoryPageServices::new(self.accounts().clone(), self.executor.clone())
    }

    pub(super) fn login(&self, id: u64) {
        self.transport.set_identity(id, OWNER);
        self.runtime.block_on(async {
            self.accounts()
                .login(crate::auth::token::Pat::from_str("synthetic-offline-token"))
                .await
                .expect("synthetic login");
        });
    }

    /// Closes the account service and reopens it against the same persistent
    /// cache store and credential store.
    pub(super) fn restart(&mut self) {
        let accounts = self.accounts.take().expect("an open account service");
        self.runtime
            .block_on(async { accounts.close().await })
            .expect("close the account service");
        drop(accounts);
        let cache = self.store.take().expect("an open cache store");
        let creds_clone = self.credentials.clone();
        let accounts = self.runtime.block_on(async {
            AccountService::open_with_cache(
                AccountDatabaseConfig::at(self.temp.path().join("accounts.sqlite3")),
                creds_clone,
                Arc::new(GitHubClient::new(
                    GitHubConfig::default(),
                    self.transport.clone() as Arc<dyn Transport>,
                )),
                self.executor.clone(),
                cache.clone(),
            )
            .await
            .expect("reopen the cached account service")
        });
        self.accounts = Some(Arc::new(accounts));
        self.store = Some(cache);
    }
}

struct OfflineHost {
    page: Option<Entity<RepositoryPage>>,
}

impl gpui::Render for OfflineHost {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        gpui::div()
            .size_full()
            .children(self.page.clone().map(|page| page.into_any_element()))
    }
}

fn open_cached<'a>(
    cx: &'a mut TestAppContext,
    fixture: &CachedFixture,
    intent: RepositoryNavigation,
) -> (
    Entity<OfflineHost>,
    Entity<RepositoryPage>,
    &'a mut VisualTestContext,
) {
    setup(cx);
    let services = fixture.services();
    let locator = locator();
    let (host, cx) = cx.add_window_view(|window, cx| {
        let page = cx.new(|cx| RepositoryPage::new(services, locator, intent, window, cx));
        page.update(cx, |page, cx| page.activate(cx));
        OfflineHost { page: Some(page) }
    });
    cx.update(|window, _| window.activate_window());
    let page = host.read_with(cx, |host, _| host.page.clone().expect("a mounted page"));
    (host, page, cx)
}

fn release_page(host: &Entity<OfflineHost>, cx: &mut VisualTestContext) {
    host.update(cx, |host, cx| {
        if let Some(page) = host.page.clone() {
            page.update(cx, |page, cx| page.deactivate(cx));
        }
        host.page = None;
        cx.notify();
    });
    frames(cx);
    cx.run_until_parked();
    for _ in 0..40 {
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn settle(
    page: &Entity<RepositoryPage>,
    cx: &mut VisualTestContext,
    mut done: impl FnMut(&RepositoryPage) -> bool,
) {
    for _ in 0..800 {
        page.update_in(cx, |page, window, cx| page.drain(window, cx));
        cx.run_until_parked();
        frames(cx);
        std::thread::sleep(Duration::from_millis(2));
        if page.read_with(cx, |page, _| done(page)) {
            return;
        }
    }
    panic!(
        "page did not settle; error={:?} status={:?}",
        page.read_with(cx, |page, _| page.error().cloned()),
        page.read_with(cx, |page, _| page.reader_status().clone())
    );
}

fn reader_text(page: &Entity<RepositoryPage>, cx: &mut VisualTestContext) -> String {
    page.read_with(cx, |page, cx| {
        let reader = page.reader().expect("a reader is showing").read(cx);
        if let Some(doc) = reader.markdown() {
            return doc.read(cx).document_text().to_string();
        }
        if let Some(src) = reader.source() {
            return src.read(cx).text().to_string();
        }
        panic!("neither markdown nor source view present");
    })
}

fn image_key(path: &str) -> ImageKey {
    ImageTarget::Repository {
        owner: OWNER.into(),
        name: NAME.into(),
        commit: MAIN_COMMIT.into(),
        path: path.into(),
    }
    .key()
}

fn wait_for_image(
    page: &Entity<RepositoryPage>,
    cx: &mut VisualTestContext,
    key: &ImageKey,
    expected: &str,
) {
    for _ in 0..800 {
        page.update_in(cx, |page, window, cx| page.drain(window, cx));
        cx.run_until_parked();
        frames(cx);
        cx.background_executor
            .advance_clock(Duration::from_millis(25));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(2));
        let state = page.read_with(cx, |page, cx| {
            page.reader()
                .and_then(|reader| reader.read(cx).image_state(key))
        });
        if state.as_deref() == Some(expected) {
            return;
        }
    }
    panic!("image never reached {expected}");
}

/// Reads the README once, online, and returns the request count afterwards.
fn warm_up(cx: &mut TestAppContext, fixture: &CachedFixture) -> usize {
    {
        let (host, page, cx) = open_cached(cx, fixture, RepositoryNavigation::default());
        settle(&page, cx, |page| {
            page.reader_status() == &ReaderStatus::Showing
        });
        page.read_with(cx, |page, _| {
            assert_eq!(
                page.details_provenance()
                    .map(|provenance| provenance.origin),
                Some(RepositoryCacheOrigin::Network),
                "an online read is reported as a network observation"
            );
            assert!(
                !page.is_showing_cached_snapshot(),
                "a network page shows no cached marker"
            );
        });
        // Visit one nested directory and one file so both are persisted.
        page.update(cx, |page, cx| page.toggle_directory("src", cx));
        settle(&page, cx, |page| page.tree("src").is_some());
        page.update(cx, |page, cx| page.select_file("src/main.rs", cx));
        settle(&page, cx, |page| {
            page.reader_status() == &ReaderStatus::Showing
                && page.selection() == &Selection::File("src/main.rs".into())
        });
        page.update(cx, |page, cx| page.show_readme(cx));
        settle(&page, cx, |page| {
            page.reader_status() == &ReaderStatus::Showing && page.selection() == &Selection::Readme
        });
        release_page(&host, cx);
    }
    fixture.transport.recorded().len()
}

/// The ordered product scenario: online warm-up, restart, offline reading of
/// exactly the visited repository content.
#[gpui::test]
fn visited_repository_content_reads_after_restart_without_network(cx: &mut TestAppContext) {
    let mut fixture = CachedFixture::new(Scene::Normal);
    fixture.login(1);
    let recorded_before_restart = warm_up(cx, &fixture);

    fixture.restart();
    fixture.transport.set_offline(true);

    let (_host, page, cx) = open_cached(cx, &fixture, RepositoryNavigation::default());
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });

    page.read_with(cx, |page, _| {
        assert_eq!(
            page.details_provenance()
                .map(|provenance| provenance.origin),
            Some(RepositoryCacheOrigin::Snapshot),
            "a reopened offline page reads a historical snapshot"
        );
        assert_eq!(
            page.revision_provenance()
                .map(|provenance| provenance.origin),
            Some(RepositoryCacheOrigin::Snapshot)
        );
        assert!(
            page.is_showing_cached_snapshot(),
            "the page states that it is showing cached data"
        );
        assert_eq!(page.selected_branch(), Some("main"));
        assert_eq!(
            page.fixed_commit().map(|sha| sha.as_str()),
            Some(MAIN_COMMIT)
        );
        assert_eq!(page.current_path(), Some("README.md"));
        assert!(page.tree("").is_some(), "the visited root tree is readable");
        assert_eq!(page.details().map(|details| details.stars), Some(128));
    });
    let text = reader_text(&page, cx);
    assert!(
        text.contains("A tiny double-entry ledger"),
        "the README body comes from the cached blob: {text}"
    );
    frames(cx);
    assert!(
        cx.debug_bounds("repository-cached-badge").is_some(),
        "the cached marker is visible"
    );

    // A visited nested directory and file stay readable.
    page.update(cx, |page, cx| page.toggle_directory("src", cx));
    settle(&page, cx, |page| page.tree("src").is_some());
    page.update(cx, |page, cx| page.select_file("src/main.rs", cx));
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
            && page.selection() == &Selection::File("src/main.rs".into())
    });
    let text = reader_text(&page, cx);
    assert!(
        text.contains("split_in_half"),
        "the visited source file is readable offline: {text}"
    );

    // A directory that was never read is an honest scoped miss, not an empty
    // listing, and it does not disturb what is already loaded.
    page.update(cx, |page, cx| page.toggle_directory("docs", cx));
    settle(&page, cx, |page| page.tree_failure("docs").is_some());
    page.read_with(cx, |page, _| {
        let failure = page.tree_failure("docs").expect("a scoped failure");
        assert!(
            !failure.title.is_empty(),
            "the miss is reported with a reason"
        );
        assert!(
            page.tree("src").is_some(),
            "the unrelated loaded directory is untouched"
        );
        assert!(
            page.reader().is_some(),
            "the already read document stays on screen"
        );
    });

    // Not one request left the machine after the restart.
    assert_eq!(
        fixture.transport.recorded().len(),
        recorded_before_restart,
        "an offline page issues no network request: {:?}",
        fixture.transport.recorded()
    );
}

/// The explicit Refresh control re-verifies online and drops the cached marker.
#[gpui::test]
fn manual_refresh_from_a_cached_snapshot_restores_network_provenance(cx: &mut TestAppContext) {
    let mut fixture = CachedFixture::new(Scene::Normal);
    fixture.login(1);
    warm_up(cx, &fixture);

    fixture.restart();
    fixture.transport.set_offline(true);
    {
        let (_host, page, cx) = open_cached(cx, &fixture, RepositoryNavigation::default());
        settle(&page, cx, |page| {
            page.reader_status() == &ReaderStatus::Showing
        });
        assert!(page.read_with(cx, |page, _| page.is_showing_cached_snapshot()));
        page.update_in(cx, |page, window, cx| page.drain(window, cx));
        frames(cx);
        cx.run_until_parked();
        frames(cx);
        assert!(
            cx.debug_bounds("repository-refresh").is_some(),
            "the cached page offers one refresh"
        );

        // Back online: the one Refresh control re-verifies the whole page.
        fixture.transport.set_offline(false);
        let refresh = cx
            .debug_bounds("repository-refresh")
            .expect("the cached page offers one refresh");
        cx.simulate_click(refresh.center(), Modifiers::none());
        settle(&page, cx, |page| {
            page.reader_status() == &ReaderStatus::Showing && !page.is_showing_cached_snapshot()
        });
        page.read_with(cx, |page, _| {
            assert_eq!(
                page.details_provenance()
                    .map(|provenance| provenance.origin),
                Some(RepositoryCacheOrigin::Network)
            );
            assert_eq!(page.current_path(), Some("README.md"));
        });
    }
}

/// README pictures resolved at the fixed commit are reusable after a restart.
#[gpui::test]
fn readme_picture_read_at_the_fixed_commit_is_reused_after_restart(cx: &mut TestAppContext) {
    let mut fixture = CachedFixture::new(Scene::Normal);
    fixture.transport.set_readme_image(true);
    fixture.login(1);
    let key = image_key("diagram.png");
    {
        let (host, page, cx) = open_cached(cx, &fixture, RepositoryNavigation::default());
        settle(&page, cx, |page| {
            page.reader_status() == &ReaderStatus::Showing
        });
        wait_for_image(&page, cx, &key, "ready");
        release_page(&host, cx);
    }
    let recorded_before_restart = fixture.transport.recorded().len();

    fixture.restart();
    fixture.transport.set_offline(true);
    let (_host, page, cx) = open_cached(cx, &fixture, RepositoryNavigation::default());
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    wait_for_image(&page, cx, &key, "ready");
    assert_eq!(
        fixture.transport.recorded().len(),
        recorded_before_restart,
        "the cached picture needs no network request: {:?}",
        fixture.transport.recorded()
    );
}

/// Replacing the PAT isolates private cache content: until the new credential
/// proves access to this repository, a reopened page must not present the
/// previous credential's cached repository data as readable.
#[gpui::test]
fn replaced_credential_needs_its_own_repository_proof(cx: &mut TestAppContext) {
    let mut fixture = CachedFixture::new(Scene::Normal);
    fixture.login(1);
    warm_up(cx, &fixture);

    // Replace the PAT while online (replacement verification needs `/user`).
    let account = fixture
        .accounts()
        .active_context()
        .expect("an active account")
        .account;
    fixture
        .runtime
        .block_on(async {
            fixture
                .accounts()
                .replace(
                    account,
                    crate::auth::token::Pat::from_str("synthetic-replacement-token"),
                )
                .await
        })
        .expect("replace the PAT");
    fixture.restart();
    fixture.transport.set_offline(true);

    // Offline immediately after the replacement: the isolated cache must not be
    // handed back as if the new credential had verified this repository.
    {
        let (_host, page, cx) = open_cached(cx, &fixture, RepositoryNavigation::default());
        settle(&page, cx, |page| {
            page.error().is_some() || page.reader().is_some()
        });
        page.read_with(cx, |page, _| {
            assert!(
                page.reader_status() != &ReaderStatus::Showing,
                "the isolated cache is not presented as readable: {:?}",
                page.reader_status()
            );
            let error = page.error().expect("the denial is reported");
            assert!(
                !error.title.is_empty(),
                "a denied read is reported, not an empty repository"
            );
        });
    }
}

/// Extras use the cached page session, so a cached overview is explicitly
/// historical and still offered a refresh.
#[gpui::test]
fn cached_extras_report_a_historical_snapshot(cx: &mut TestAppContext) {
    let mut fixture = CachedFixture::new(Scene::Normal);
    fixture.login(1);
    {
        let (host, page, cx) = open_cached(cx, &fixture, RepositoryNavigation::default());
        settle(&page, cx, |page| {
            page.reader_status() == &ReaderStatus::Showing
        });
        // Give the four extras endpoints time to publish.
        for _ in 0..200 {
            page.update_in(cx, |page, window, cx| page.drain(window, cx));
            frames(cx);
            let ready = page.read_with(cx, |page, cx| {
                page.overview().is_some_and(|view| {
                    let view = view.read(cx);
                    !view.releases().is_loading() && !view.contributors().is_loading()
                })
            });
            if ready {
                break;
            }
            std::thread::sleep(Duration::from_millis(3));
        }
        release_page(&host, cx);
    }
    fixture.restart();
    fixture.transport.set_offline(true);
    let (_host, page, cx) = open_cached(cx, &fixture, RepositoryNavigation::default());
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    for _ in 0..300 {
        page.update_in(cx, |page, window, cx| page.drain(window, cx));
        frames(cx);
        let historical = page.read_with(cx, |page, cx| {
            page.overview().is_some_and(|view| {
                let view = view.read(cx);
                view.releases().is_historical() && view.contributors().is_historical()
            })
        });
        if historical {
            break;
        }
        std::thread::sleep(Duration::from_millis(3));
    }
    page.read_with(cx, |page, cx| {
        let view = page.overview().expect("overview mounted").read(cx);
        assert!(
            view.releases().is_historical(),
            "a cached release list is a historical snapshot"
        );
        assert!(
            view.releases().sync_label().is_some(),
            "the historical snapshot keeps its last verified time"
        );
        assert!(view.languages().is_historical());
    });
    assert!(
        page.read_with(cx, |page, _| page.extras.counts.is_none()),
        "cached OPEN counts are withheld instead of drawn as freshly verified badges"
    );
    assert!(page.read_with(cx, |page, _| page.extras.counts_historical));
    frames(cx);
    assert!(
        cx.debug_bounds("overview-releases-cached").is_some(),
        "the cached release group is labelled"
    );
    assert!(
        cx.debug_bounds("overview-contributors-cached").is_some(),
        "the cached contributor group is labelled"
    );
}

/// Offline Star never sends a write or fakes a toggled count; once back online
/// and refreshed, Star/Unstar uses the authoritative server confirmation.
#[gpui::test]
fn offline_star_is_disabled_without_requests_and_recovers_online(cx: &mut TestAppContext) {
    let mut fixture = CachedFixture::new(Scene::StarUnstarred);
    fixture.login(1);
    {
        let (host, page, cx) = open_cached(cx, &fixture, RepositoryNavigation::default());
        settle(&page, cx, |page| {
            page.reader_status() == &ReaderStatus::Showing && page.starred_state().is_some()
        });
        release_page(&host, cx);
    }
    let recorded_before_restart = fixture.transport.recorded().len();
    let puts_before = fixture
        .transport
        .star_method_count(crate::network::transport::HttpMethod::Put);

    fixture.restart();
    fixture.transport.set_offline(true);
    {
        let (host, page, cx) = open_cached(cx, &fixture, RepositoryNavigation::default());
        settle(&page, cx, |page| {
            page.reader_status() == &ReaderStatus::Showing
        });
        frames(cx);
        assert!(page.read_with(cx, |page, _| page.is_showing_cached_snapshot()));
        assert_eq!(page.read_with(cx, |page, _| page.starred_state()), None);
        assert_eq!(
            page.read_with(cx, |page, _| page.details().unwrap().stars),
            128
        );
        // Pressing the Star button while showing a cached snapshot issues no
        // request and never flips the count optimistically.
        let star = cx.debug_bounds("repository-star").expect("Star button");
        cx.simulate_click(star.center(), Modifiers::none());
        page.update_in(cx, |page, window, cx| page.drain(window, cx));
        frames(cx);
        assert_eq!(page.read_with(cx, |page, _| page.starred_state()), None);
        assert_eq!(
            page.read_with(cx, |page, _| page.details().unwrap().stars),
            128
        );
        assert_eq!(
            fixture.transport.recorded().len(),
            recorded_before_restart,
            "offline Star click sends no request"
        );
        assert_eq!(
            fixture
                .transport
                .star_method_count(crate::network::transport::HttpMethod::Put),
            puts_before
        );

        // Refresh online: Star status is observed again and a real click writes
        // and re-reads the authoritative summary count.
        fixture.transport.set_offline(false);
        let refresh = cx
            .debug_bounds("repository-refresh")
            .expect("refresh button");
        cx.simulate_click(refresh.center(), Modifiers::none());
        settle(&page, cx, |page| {
            page.reader_status() == &ReaderStatus::Showing
                && !page.is_showing_cached_snapshot()
                && page.starred_state()
                    == Some(crate::domain::repository_actions::StarredState::Unstarred)
        });
        frames(cx);
        let star = cx.debug_bounds("repository-star").expect("Star button");
        cx.simulate_click(star.center(), Modifiers::none());
        settle(&page, cx, |page| {
            page.starred_state() == Some(crate::domain::repository_actions::StarredState::Starred)
                && !page.star_is_busy()
        });
        assert_eq!(
            page.read_with(cx, |page, _| page.details().unwrap().stars),
            129
        );
        assert_eq!(
            fixture
                .transport
                .star_method_count(crate::network::transport::HttpMethod::Put),
            puts_before + 1
        );
        assert!(!page.read_with(cx, |page, _| page.fork_mutations_enabled()));
        release_page(&host, cx);
    }
}

/// Revoking repository cache permission or rejecting the credential immediately
/// clears the displayed page state and blocks offline reads across restarts.
#[gpui::test]
fn cache_revocation_and_credential_rejection_clear_displayed_data_and_block_offline(
    cx: &mut TestAppContext,
) {
    let mut fixture = CachedFixture::new(Scene::Normal);
    fixture.login(1);
    warm_up(cx, &fixture);

    fixture.restart();
    fixture.transport.set_offline(true);
    {
        let (host, page, cx) = open_cached(cx, &fixture, RepositoryNavigation::default());
        settle(&page, cx, |page| {
            page.reader_status() == &ReaderStatus::Showing
        });
        assert!(page.read_with(cx, |page, _| page.details().is_some()));

        // Explicitly revoke the repository in the cache store while the page is
        // mounted: the next drain/render check must clear the displayed data.
        let store = fixture.store.as_ref().expect("cache store").clone();
        let scope = store
            .saved_account(crate::domain::AccountKey::new(
                1,
                crate::domain::HostId::github(),
            ))
            .expect("saved scope");
        store.revoke_repository(&scope, Some(42)).expect("revoke");

        settle(&page, cx, |page| {
            !page.is_active() && page.details().is_none()
        });
        page.read_with(cx, |page, _| {
            assert!(page.reader().is_none(), "revoked reader content is cleared");
            assert!(page.overview().is_none(), "revoked overview is cleared");
            assert!(page.error().is_some(), "revocation is explained");
        });
        release_page(&host, cx);
    }

    // Reopening after explicit revocation while still offline cannot resurrect
    // the revoked repository.
    fixture.restart();
    fixture.transport.set_offline(true);
    {
        let (host, page, cx) = open_cached(cx, &fixture, RepositoryNavigation::default());
        settle(&page, cx, |page| page.error().is_some());
        page.read_with(cx, |page, _| {
            assert_ne!(page.reader_status(), &ReaderStatus::Showing);
            assert!(page.details().is_none());
        });
        release_page(&host, cx);
    }
}

/// A queued reply from before a manual refresh must not repopulate the page, and
/// the refresh must invalidate the previous branch/revision identity.
#[gpui::test]
fn manual_refresh_invalidates_queued_replies_from_the_previous_generation(cx: &mut TestAppContext) {
    let fixture = CachedFixture::new(Scene::Normal);
    fixture.login(1);
    let (host, page, cx) = open_cached(cx, &fixture, RepositoryNavigation::default());
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    assert!(!page.read_with(cx, |page, _| page.is_showing_cached_snapshot()));

    // Capture the current generation identities and a real snapshot, then queue a
    // stale revision reply that would restore the previous branch state.
    let (snapshot, old_branch_request, old_generation) = page.read_with(cx, |page, _| {
        (
            page.branch
                .as_ref()
                .expect("a loaded branch")
                .snapshot
                .clone(),
            page.branch_request,
            page.generation,
        )
    });
    page.update_in(cx, |page, window, cx| {
        page.inbox
            .lock()
            .expect("inbox")
            .push_back(super::super::Message {
                generation: old_generation,
                branch_request: old_branch_request,
                context: page.context.clone().expect("context"),
                body: super::super::Body::Revision {
                    branch: "main".into(),
                    request: old_branch_request,
                    result: Ok((snapshot.clone(), None)),
                },
            });
        page.refresh_repository(cx);
        page.drain(window, cx);
    });
    page.read_with(cx, |page, _| {
        assert!(
            page.generation != old_generation,
            "refresh advances the page generation"
        );
        assert!(
            page.branch_request != old_branch_request,
            "refresh advances the branch request identity"
        );
        assert!(
            page.details().is_none() && page.branch.is_none(),
            "a queued reply from the previous generation cannot repopulate the page"
        );
    });
    // The refresh itself completes normally under the new generation.
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing && page.details().is_some()
    });
    release_page(&host, cx);
}

/// A gate probe that is still outstanding when the page re-activates must not
/// leave the new generation permanently unable to probe again, and a denial must
/// remain recoverable through activation.
#[gpui::test]
fn activation_resets_gate_probe_bookkeeping_and_recovers_from_denial(cx: &mut TestAppContext) {
    let fixture = CachedFixture::new(Scene::Normal);
    fixture.login(1);
    let (host, page, cx) = open_cached(cx, &fixture, RepositoryNavigation::default());
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });

    // Simulate an outstanding probe plus a denial, as if a stale reply had been
    // dropped by the generation check in `apply`.
    page.update_in(cx, |page, window, cx| {
        page.gate_probe_in_flight = true;
        page.gate_denied = true;
        let stale_request = page.gate_probe_request;
        page.activate(cx);
        assert!(
            !page.gate_probe_in_flight,
            "activation clears the in-flight flag"
        );
        assert!(!page.gate_denied, "activation clears the previous denial");
        assert!(
            page.gate_probe_request != stale_request,
            "activation advances the probe identity so stale replies are inert"
        );
        // A reply for the superseded probe must not settle the new one.
        page.gate_probe_in_flight = true;
        page.apply_gate(stale_request, false, cx);
        assert!(
            page.gate_probe_in_flight,
            "a stale gate reply cannot clear a newer in-flight probe"
        );
        page.apply_gate(page.gate_probe_request, true, cx);
        assert!(!page.gate_probe_in_flight);
        assert!(!page.gate_denied);
        let _ = (window, cx);
    });
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    release_page(&host, cx);
}

/// A page whose summary is fresh online but whose branch/revision readings fell
/// back to a snapshot must report the cached age, not "just now".
#[gpui::test]
fn cached_marker_reports_the_oldest_historical_source_not_a_fresh_summary(cx: &mut TestAppContext) {
    let fixture = CachedFixture::new(Scene::Normal);
    fixture.login(1);
    let (_host, page, cx) = open_cached(cx, &fixture, RepositoryNavigation::default());
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    let now = crate::data::reader_image_cache::now_ms();
    let old = now - 3 * 24 * 60 * 60 * 1000;
    page.update(cx, |page, cx| {
        // Fresh summary (Network), stale branch list and revision (Snapshot).
        page.details_provenance = Some(super::super::RepositoryCacheProvenance {
            origin: RepositoryCacheOrigin::Network,
            fetched_at_ms: now,
            verified_at_ms: now,
            completeness: crate::domain::cache::CacheCompleteness::Complete,
        });
        page.branches_provenance = Some(super::super::RepositoryCacheProvenance {
            origin: RepositoryCacheOrigin::Snapshot,
            fetched_at_ms: old,
            verified_at_ms: old,
            completeness: crate::domain::cache::CacheCompleteness::Complete,
        });
        if let Some(branch) = page.branch.as_mut() {
            branch.snapshot_provenance = Some(super::super::RepositoryCacheProvenance {
                origin: RepositoryCacheOrigin::Snapshot,
                fetched_at_ms: old + 60_000,
                verified_at_ms: old + 60_000,
                completeness: crate::domain::cache::CacheCompleteness::Complete,
            });
        }
        cx.notify();
    });
    page.read_with(cx, |page, _| {
        assert!(page.is_showing_cached_snapshot());
        assert_eq!(
            page.historical_verified_at_ms(),
            Some(old),
            "the oldest cached source decides the reported age"
        );
        assert_ne!(
            page.historical_verified_at_ms(),
            Some(now),
            "a fresh network summary must not be reported as the cached age"
        );
    });
    frames(cx);
    assert!(cx.debug_bounds("repository-cached-badge").is_some());
}
