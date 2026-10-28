//! Tests for the repository page.
//!
//! They drive the **real** data pipeline — `AccountService` → `RepoService`
//! session → repository/branches/revision/tree/README/blob → `PreparedContent`
//! → `ReaderView` — over a deterministic, request-routed transport, a temporary
//! SQLite directory and one synthetic PAT. No request leaves the machine and no
//! real credential is read.
//!
//! What the GPUI test platform can prove is covered here: observable state
//! transitions, generation/authorization boundaries, tree laziness, the reader
//! states and the rendered tree bounds. Real Windows input, IME and a real
//! GitHub account remain manual acceptance items (see the task devlog).

use std::collections::{HashMap, VecDeque};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    AppContext, Entity, IntoElement, Modifiers, ParentElement, Render, Styled, TestAppContext,
    VisualTestContext, px,
};

use super::navigation::{
    MAX_TREE_ROWS, ReadmeState, RepositoryNavigation, TreeModel, TreeRow, TreeRowKind,
};
use super::{PageAction, ReaderStatus, RepositoryPage, RepositoryPageServices, Selection};
use crate::auth::InMemoryCredentialStore;
use crate::content::PreparedBody;
use crate::data::AccountService;
use crate::domain::error::GitHubError;
use crate::domain::repository::{
    Completeness, DirectoryTree, IncompleteReason, ObjectSha, ReadmeDiscovery, RepoSnapshot,
    RepositoryError, RepositoryIdentity, RepositoryLocator, SnapshotDirectory, TreeEntry,
    TreeEntryKind,
};
use crate::github::{GitHubClient, GitHubConfig};
use crate::network::cancel::CancellationToken;
use crate::network::executor::BackgroundExecutor;
use crate::network::transport::{
    HttpRequest, HttpResponse, Transport, TransportError, json_response,
};
use crate::storage::accounts::AccountDatabaseConfig;
use crate::ui::reader::DocumentKind;

const COMMIT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const COMMIT_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const ROOT_A: &str = "1111111111111111111111111111111111111111";
const ROOT_B: &str = "1212121212121212121212121212121212121212";
const DOCS_TREE: &str = "2222222222222222222222222222222222222222";
const SRC_TREE: &str = "2323232323232323232323232323232323232323";
const README_BLOB: &str = "3333333333333333333333333333333333333333";
const GUIDE_BLOB: &str = "4444444444444444444444444444444444444444";
const MAIN_BLOB: &str = "5555555555555555555555555555555555555555";
const LARGE_BLOB: &str = "6666666666666666666666666666666666666666";
const BINARY_BLOB: &str = "7777777777777777777777777777777777777777";

const README_TEXT: &str =
    "# Fixture README\n\nA [guide](docs/guide.md) and an [external](https://example.com).\n";
const GUIDE_TEXT: &str = "# Guide\n\nThe guide body.\n";
const SOURCE_TEXT: &str = "fn main() {\n    println!(\"你好 🌍\");\n}\n";

// ---------------------------------------------------------------- transport --

struct Step {
    status: u16,
    body: String,
    headers: Vec<(String, String)>,
    gate: Option<tokio::sync::oneshot::Receiver<()>>,
    entered: Option<tokio::sync::oneshot::Sender<()>>,
}

/// A request-routed scripted transport.
///
/// Responses are keyed by the request's URL path, so concurrent requests (root
/// tree plus README discovery, for example) are deterministic instead of
/// depending on completion order.
#[derive(Default)]
struct Scripted {
    steps: Mutex<HashMap<String, VecDeque<Step>>>,
    requests: Mutex<Vec<String>>,
}

impl Scripted {
    fn new() -> Self {
        Self::default()
    }

    fn push(&self, key: &str, status: u16, body: serde_json::Value) {
        self.steps
            .lock()
            .unwrap()
            .entry(key.to_string())
            .or_default()
            .push_back(Step {
                status,
                body: body.to_string(),
                headers: vec![("content-type".to_string(), "application/json".to_string())],
                gate: None,
                entered: None,
            });
    }

    fn push_with_headers(
        &self,
        key: &str,
        status: u16,
        body: serde_json::Value,
        headers: &[(&str, &str)],
    ) {
        let mut all: Vec<(String, String)> =
            vec![("content-type".to_string(), "application/json".to_string())];
        all.extend(
            headers
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string())),
        );
        self.steps
            .lock()
            .unwrap()
            .entry(key.to_string())
            .or_default()
            .push_back(Step {
                status,
                body: body.to_string(),
                headers: all,
                gate: None,
                entered: None,
            });
    }

    /// Queues a response that is delivered only after the returned sender fires.
    fn block(
        &self,
        key: &str,
        status: u16,
        body: serde_json::Value,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, gate) = tokio::sync::oneshot::channel();
        self.steps
            .lock()
            .unwrap()
            .entry(key.to_string())
            .or_default()
            .push_back(Step {
                status,
                body: body.to_string(),
                headers: vec![("content-type".to_string(), "application/json".to_string())],
                gate: Some(gate),
                entered: Some(entered),
            });
        (ready, release)
    }

    fn count(&self, key: &str) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|url| path_of(url) == key)
            .count()
    }

    fn total(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    fn recorded(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

/// The URL path without the query, so `readme?ref=…` matches one key.
fn path_of(url: &str) -> String {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let path = after_scheme
        .split_once('/')
        .map(|(_, path)| format!("/{path}"))
        .unwrap_or_else(|| "/".to_string());
    path.split(['?', '#'])
        .next()
        .unwrap_or_default()
        .to_string()
}

impl Transport for Scripted {
    fn execute<'a>(
        &'a self,
        request: HttpRequest,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(TransportError::Cancelled);
            }
            self.requests.lock().unwrap().push(request.url.clone());
            let key = path_of(&request.url);
            let step = {
                let mut steps = self.steps.lock().unwrap();
                match steps.get_mut(&key) {
                    Some(queue) => queue.pop_front(),
                    None => None,
                }
            };
            let Some(step) = step else {
                // An unscripted request is a fixture mistake, never a silent
                // success: a typed 404 makes it visible in the state assertions.
                return Ok(json_response(
                    404,
                    &serde_json::json!({"message": format!("unscripted {key}")}).to_string(),
                ));
            };
            if let Some(entered) = step.entered {
                let _ = entered.send(());
            }
            if let Some(gate) = step.gate {
                let _ = gate.await;
            }
            Ok(HttpResponse {
                status: step.status,
                headers: step.headers,
                body: step.body,
            })
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

// ----------------------------------------------------------------- fixtures --

/// The repository API path; every scripted key is derived from it.
const REPO: &str = "/repos/octo/demo";

fn details_body(description: &str) -> serde_json::Value {
    serde_json::json!({
        "id": 42,
        "name": "demo",
        "owner": {"id": 7, "login": "octo"},
        "description": description,
        "visibility": "public",
        "private": false,
        "default_branch": "main",
        "fork": false,
        "archived": false,
        "stargazers_count": 12,
        "forks_count": 3,
        "open_issues_count": 2,
        "subscribers_count": 5,
        "size": 10,
        "html_url": "https://example.invalid/octo/demo",
        "clone_url": "https://example.invalid/octo/demo.git",
        "pushed_at": "2026-10-20T10:00:00Z"
    })
}

fn branches_body(names: &[(&str, &str)]) -> serde_json::Value {
    serde_json::Value::Array(
        names
            .iter()
            .map(|(name, sha)| {
                serde_json::json!({"name": name, "commit": {"sha": sha}, "protected": false})
            })
            .collect(),
    )
}

fn commit_body(sha: &str, tree: &str) -> serde_json::Value {
    serde_json::json!({
        "sha": sha,
        "commit": {
            "message": format!("fixture {sha}"),
            "tree": {"sha": tree},
            "author": {"name": "octo", "email": null, "date": "2026-10-20T10:00:00Z"},
            "committer": {"name": "octo", "email": null, "date": "2026-10-20T10:00:00Z"}
        }
    })
}

fn readme_body(path: &str, sha: &str, size: usize) -> serde_json::Value {
    serde_json::json!({"type": "file", "path": path, "sha": sha, "size": size})
}

fn blob_body(sha: &str, bytes: &[u8]) -> serde_json::Value {
    use base64::Engine;
    serde_json::json!({
        "sha": sha,
        "encoding": "base64",
        "size": bytes.len(),
        "content": base64::engine::general_purpose::STANDARD.encode(bytes)
    })
}

fn blob_key(sha: &str) -> String {
    format!("{REPO}/git/blobs/{sha}")
}

fn tree_key(sha: &str) -> String {
    format!("{REPO}/git/trees/{sha}")
}

fn file_entry(path: &str, sha: &str, size: u64) -> serde_json::Value {
    serde_json::json!({"path": path, "mode": "100644", "type": "blob", "sha": sha, "size": size})
}

fn dir_entry(path: &str, sha: &str) -> serde_json::Value {
    serde_json::json!({"path": path, "mode": "040000", "type": "tree", "sha": sha})
}

fn tree_body(sha: &str, truncated: bool, entries: Vec<serde_json::Value>) -> serde_json::Value {
    serde_json::json!({"sha": sha, "truncated": truncated, "tree": entries})
}

// --------------------------------------------------------------- test setup --

struct Fixture {
    script: Arc<Scripted>,
    accounts: Arc<AccountService>,
    executor: Arc<BackgroundExecutor>,
    _temp: tempfile::TempDir,
    _runtime: tokio::runtime::Runtime,
}

impl Fixture {
    /// Opens the real service stack over the scripted transport and signs in a
    /// synthetic account, as the host would have done before mounting the page.
    fn new(script: Arc<Scripted>) -> Self {
        let temp = tempfile::tempdir().expect("temporary account directory");
        let executor = Arc::new(BackgroundExecutor::new(4));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("host runtime");
        let accounts = runtime.block_on(async {
            let accounts = AccountService::open(
                AccountDatabaseConfig::at(temp.path().join("accounts.sqlite3")),
                Arc::new(InMemoryCredentialStore::new()),
                Arc::new(GitHubClient::new(
                    GitHubConfig::default(),
                    script.clone() as Arc<dyn Transport>,
                )),
                executor.clone(),
            )
            .await
            .expect("open account service");
            Arc::new(accounts)
        });
        Self {
            script,
            accounts,
            executor,
            _temp: temp,
            _runtime: runtime,
        }
    }

    fn services(&self) -> RepositoryPageServices {
        RepositoryPageServices::new(self.accounts.clone(), self.executor.clone())
    }

    fn locator(&self) -> RepositoryLocator {
        RepositoryLocator::new("octo", "demo").expect("locator")
    }

    fn login(&self, id: u64) {
        self.script.push(
            "/user",
            200,
            serde_json::json!({"id": id, "login": format!("user{id}")}),
        );
        self._runtime.block_on(async {
            self.accounts
                .login(crate::auth::token::Pat::from_str("synthetic-test-token"))
                .await
                .expect("synthetic login");
        });
    }

    /// The full happy path script: repository, branches, revision, root tree,
    /// README descriptor and the README blob.
    fn script_first_screen(&self) {
        self.script.push(REPO, 200, details_body("A fixture"));
        self.script.push(
            &format!("{REPO}/branches"),
            200,
            branches_body(&[("main", COMMIT_A)]),
        );
        self.script.push(
            &format!("{REPO}/commits/main"),
            200,
            commit_body(COMMIT_A, ROOT_A),
        );
        self.script.push(
            &tree_key(ROOT_A),
            200,
            tree_body(
                ROOT_A,
                false,
                vec![
                    file_entry("README.md", README_BLOB, README_TEXT.len() as u64),
                    dir_entry("docs", DOCS_TREE),
                    dir_entry("src", SRC_TREE),
                ],
            ),
        );
        self.script.push(
            &format!("{REPO}/readme"),
            200,
            readme_body("README.md", README_BLOB, README_TEXT.len()),
        );
        self.script.push(
            &blob_key(README_BLOB),
            200,
            blob_body(README_BLOB, README_TEXT.as_bytes()),
        );
    }

    /// Expands `docs` and reads `docs/guide.md`.
    fn script_docs_and_guide(&self) {
        self.script.push(
            &tree_key(DOCS_TREE),
            200,
            tree_body(
                DOCS_TREE,
                false,
                vec![file_entry("guide.md", GUIDE_BLOB, GUIDE_TEXT.len() as u64)],
            ),
        );
        self.script.push(
            &blob_key(GUIDE_BLOB),
            200,
            blob_body(GUIDE_BLOB, GUIDE_TEXT.as_bytes()),
        );
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

fn open<'a>(
    cx: &'a mut TestAppContext,
    fixture: &Fixture,
    intent: RepositoryNavigation,
) -> (Entity<RepositoryPage>, &'a mut VisualTestContext) {
    setup(cx);
    let services = fixture.services();
    let locator = fixture.locator();
    let (page, cx) =
        cx.add_window_view(|window, cx| RepositoryPage::new(services, locator, intent, window, cx));
    cx.update(|window, _| window.activate_window());
    (page, cx)
}

/// Runs the page's own drain plus a few frames until `done` holds.
fn settle(
    page: &Entity<RepositoryPage>,
    cx: &mut VisualTestContext,
    mut done: impl FnMut(&RepositoryPage) -> bool,
) {
    for _ in 0..500 {
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

fn frames(cx: &mut VisualTestContext) {
    for _ in 0..2 {
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        cx.update(|window, cx| window.simulate_next_frame(cx));
    }
    cx.run_until_parked();
}

fn activate(page: &Entity<RepositoryPage>, cx: &mut VisualTestContext) {
    page.update(cx, |page, cx| page.activate(cx));
}

/// The happy path: summary, branches, one fixed commit, README markdown.
fn loaded(cx: &mut TestAppContext) -> (Fixture, Entity<RepositoryPage>, &mut VisualTestContext) {
    let script = Arc::new(Scripted::new());
    let fixture = Fixture::new(script.clone());
    fixture.login(1);
    fixture.script_first_screen();
    let (page, cx) = open(cx, &fixture, RepositoryNavigation::default());
    activate(&page, cx);
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    (fixture, page, cx)
}

fn release(sender: tokio::sync::oneshot::Sender<()>) {
    let _ = sender.send(());
}

/// Pumps the page until the scripted request behind `ready` was actually
/// entered, so a test can act while that request is still in flight.
fn wait_gate(
    page: &Entity<RepositoryPage>,
    cx: &mut VisualTestContext,
    mut ready: tokio::sync::oneshot::Receiver<()>,
) {
    for _ in 0..500 {
        page.update_in(cx, |page, window, cx| page.drain(window, cx));
        cx.run_until_parked();
        frames(cx);
        std::thread::sleep(Duration::from_millis(2));
        match ready.try_recv() {
            Ok(()) => return,
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                panic!("the scripted request ended without reaching its gate")
            }
        }
    }
    panic!("the scripted request never reached its gate");
}

// -------------------------------------------------------------------- tests --

#[gpui::test]
fn real_pipeline_renders_the_readme_then_a_selected_source_file(cx: &mut TestAppContext) {
    let (fixture, page, cx) = loaded(cx);

    let (details, branch, commit) = page.read_with(cx, |page, _| {
        (
            page.details().cloned(),
            page.selected_branch().map(str::to_string),
            page.fixed_commit().map(|sha| sha.as_str().to_string()),
        )
    });
    let rows: Vec<TreeRow> = page.read_with(cx, |page, _| page.rows().to_vec());
    let details = details.expect("repository summary reached the page");
    assert_eq!(details.identity.locator().name(), "demo");
    assert_eq!(details.stars, 12);
    assert_eq!(branch.as_deref(), Some("main"));
    assert_eq!(
        commit.as_deref(),
        Some(COMMIT_A),
        "the branch resolved to one commit"
    );
    assert_eq!(
        rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
        vec!["docs", "src", "README.md"],
        "directories sort before files"
    );
    assert!(matches!(
        rows[0].kind,
        TreeRowKind::Directory { expanded: false }
    ));

    // The README is the real Markdown reader over the prepared blob.
    let reader = page
        .read_with(cx, |page, _| page.reader().cloned())
        .unwrap();
    reader.read_with(cx, |reader, _| {
        assert_eq!(reader.kind(), &DocumentKind::Markdown);
        assert_eq!(reader.context().path(), "README.md");
        assert_eq!(reader.context().commit(), COMMIT_A);
        assert!(
            reader.markdown().is_some(),
            "a markdown document is mounted"
        );
    });
    assert!(cx.debug_bounds("repository-page-root").is_some());
    assert!(cx.debug_bounds("repository-readme-row").is_some());

    // Expanding a directory fetches exactly one more tree, keyed by its own SHA.
    fixture.script_docs_and_guide();
    let before = fixture.script.total();
    page.update(cx, |page, cx| page.toggle_directory("docs", cx));
    settle(&page, cx, |page| page.tree("docs").is_some());
    assert_eq!(
        fixture.script.count(&tree_key(DOCS_TREE)),
        1,
        "one lazy tree request for the opened directory"
    );
    assert_eq!(
        fixture.script.total(),
        before + 1,
        "no other request was made: {:?}",
        fixture.script.recorded()
    );
    let rows: Vec<TreeRow> = page.read_with(cx, |page, _| page.rows().to_vec());
    assert_eq!(
        rows.iter().map(|row| row.path.as_str()).collect::<Vec<_>>(),
        vec!["docs", "docs/guide.md", "src", "README.md"],
        "the child row sits under its expanded parent"
    );
    assert_eq!(rows[1].depth, 1);

    // Selecting a file renders the read-only source view at the same commit.
    page.update(cx, |page, cx| page.select_file("docs/guide.md", cx));
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
            && page.selection() == &Selection::File("docs/guide.md".into())
    });
    let reader = page
        .read_with(cx, |page, _| page.reader().cloned())
        .unwrap();
    reader.read_with(cx, |reader, _| {
        assert_eq!(
            reader.kind(),
            &DocumentKind::Markdown,
            "a .md file renders as Markdown"
        );
        assert_eq!(reader.context().path(), "docs/guide.md");
        assert_eq!(reader.context().commit(), COMMIT_A);
    });
    assert_eq!(
        fixture
            .script
            .recorded()
            .iter()
            .filter(|url| url.contains("/git/trees/") && url.contains(ROOT_A))
            .count(),
        1,
        "the root tree was read once and reused"
    );
    assert_eq!(
        page.read_with(cx, |page, _| page.navigation().path),
        Some("docs/guide.md".into())
    );
}

#[gpui::test]
fn a_switched_branch_discards_the_slower_older_snapshot(cx: &mut TestAppContext) {
    let script = Arc::new(Scripted::new());
    let fixture = Fixture::new(script.clone());
    fixture.login(1);
    fixture.script.push(REPO, 200, details_body("A fixture"));
    fixture.script.push(
        &format!("{REPO}/branches"),
        200,
        branches_body(&[("main", COMMIT_A), ("release", COMMIT_B)]),
    );
    let (ready_a, release_a) = fixture.script.block(
        &format!("{REPO}/commits/main"),
        200,
        commit_body(COMMIT_A, ROOT_A),
    );
    fixture.script.push(
        &format!("{REPO}/commits/release"),
        200,
        commit_body(COMMIT_B, ROOT_B),
    );
    fixture.script.push(
        &tree_key(ROOT_B),
        200,
        tree_body(ROOT_B, false, vec![file_entry("other.txt", MAIN_BLOB, 5)]),
    );
    fixture.script.push(
        &format!("{REPO}/readme"),
        404,
        serde_json::json!({"message": "Not Found"}),
    );
    fixture.script.push(
        &tree_key(ROOT_B),
        200,
        tree_body(ROOT_B, false, vec![file_entry("other.txt", MAIN_BLOB, 5)]),
    );

    let (page, cx) = open(cx, &fixture, RepositoryNavigation::default());
    activate(&page, cx);
    // `main` is gated inside the transport, so the page has no snapshot yet.
    wait_gate(&page, cx, ready_a);
    assert!(page.read_with(cx, |page, _| page.fixed_commit().is_none()));

    page.update(cx, |page, cx| page.select_branch("release", cx));
    settle(&page, cx, |page| {
        page.fixed_commit().map(|sha| sha.as_str()) == Some(COMMIT_B)
    });

    // The older A response lands after B is already on screen.
    release(release_a);
    for _ in 0..40 {
        page.update_in(cx, |page, window, cx| page.drain(window, cx));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(2));
    }
    let commit = page.read_with(cx, |page, _| {
        page.fixed_commit().map(|sha| sha.as_str().to_string())
    });
    assert_eq!(commit.as_deref(), Some(COMMIT_B), "A must not replace B");
    let rows: Vec<TreeRow> = page.read_with(cx, |page, _| page.rows().to_vec());
    assert_eq!(
        rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
        vec!["other.txt"]
    );
}

#[gpui::test]
fn a_later_completion_for_an_abandoned_file_does_not_replace_the_current_one(
    cx: &mut TestAppContext,
) {
    let (fixture, page, cx) = loaded(cx);
    fixture.script.push(
        &tree_key(DOCS_TREE),
        200,
        tree_body(
            DOCS_TREE,
            false,
            vec![
                file_entry("guide.md", GUIDE_BLOB, GUIDE_TEXT.len() as u64),
                file_entry("main.rs", MAIN_BLOB, SOURCE_TEXT.len() as u64),
            ],
        ),
    );
    fixture.script.push(
        &blob_key(MAIN_BLOB),
        200,
        blob_body(MAIN_BLOB, SOURCE_TEXT.as_bytes()),
    );
    let (ready_guide, release_guide) = fixture.script.block(
        &blob_key(GUIDE_BLOB),
        200,
        blob_body(GUIDE_BLOB, GUIDE_TEXT.as_bytes()),
    );

    page.update(cx, |page, cx| page.toggle_directory("docs", cx));
    settle(&page, cx, |page| page.tree("docs").is_some());
    page.update(cx, |page, cx| page.select_file("docs/guide.md", cx));
    wait_gate(&page, cx, ready_guide);
    // The second selection completes first.
    page.update(cx, |page, cx| page.select_file("docs/main.rs", cx));
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
            && page.selection() == &Selection::File("docs/main.rs".into())
    });
    release(release_guide);
    for _ in 0..40 {
        page.update_in(cx, |page, window, cx| page.drain(window, cx));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(2));
    }
    let selection = page.read_with(cx, |page, _| page.selection().clone());
    assert_eq!(selection, Selection::File("docs/main.rs".into()));
    let reader = page
        .read_with(cx, |page, _| page.reader().cloned())
        .unwrap();
    reader.read_with(cx, |reader, _| {
        assert_eq!(reader.context().path(), "docs/main.rs");
        match reader.kind() {
            DocumentKind::Source { .. } => {}
            other => panic!("expected the source view, got {other:?}"),
        }
    });
}

#[gpui::test]
fn an_authorization_change_clears_data_and_a_later_return_to_the_same_account_does_not_restore_it(
    cx: &mut TestAppContext,
) {
    let (fixture, page, cx) = loaded(cx);
    assert!(page.read_with(cx, |page, _| page.details().is_some()));

    // The host signs in a second account while the page is showing data.
    fixture.login(2);
    settle(&page, cx, |page| page.details().is_none());
    page.read_with(cx, |page, _| {
        assert!(!page.is_active(), "the page stopped and cleared its data");
        assert!(
            page.error().is_some(),
            "the change is explained, not silent"
        );
        assert_eq!(page.rows().len(), 0);
        assert!(page.reader().is_none());
    });

    // Returning to the first account (A → B → A) is a new authorization
    // context: the page must stay cleared until it is activated again.
    fixture
        ._runtime
        .block_on(fixture.accounts.set_active(crate::domain::AccountKey::new(
            1,
            crate::domain::HostId::github(),
        )))
        .expect("switch back");
    for _ in 0..10 {
        page.update_in(cx, |page, window, cx| page.drain(window, cx));
        cx.run_until_parked();
    }
    page.read_with(cx, |page, _| {
        assert!(page.details().is_none(), "old data does not come back");
        assert!(!page.is_active());
    });
}

#[gpui::test]
fn dropping_the_view_stops_late_success_and_errors_from_publishing(cx: &mut TestAppContext) {
    let script = Arc::new(Scripted::new());
    let fixture = Fixture::new(script.clone());
    fixture.login(1);
    fixture.script.push(REPO, 200, details_body("A fixture"));
    fixture.script.push(
        &format!("{REPO}/branches"),
        200,
        branches_body(&[("main", COMMIT_A)]),
    );
    let (ready, release_a) = script.block(
        &format!("{REPO}/commits/main"),
        200,
        commit_body(COMMIT_A, ROOT_A),
    );
    let (page, cx) = open(cx, &fixture, RepositoryNavigation::default());
    activate(&page, cx);
    wait_gate(&page, cx, ready);

    // Dropping the entity while a read is in flight must cancel it and leave no
    // way for the reply to reach a view.
    drop(page);
    release(release_a);
    for _ in 0..20 {
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(2));
    }
    // Re-opening the window content with a fresh page under the same services
    // must start from scratch rather than inherit the released entity's state.
    fixture.script_first_screen();
    let (fresh, cx) = open(cx, &fixture, RepositoryNavigation::default());
    activate(&fresh, cx);
    settle(&fresh, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
}

#[gpui::test]
fn a_deactivated_page_ignores_a_success_that_lands_afterwards(cx: &mut TestAppContext) {
    let script = Arc::new(Scripted::new());
    let fixture = Fixture::new(script.clone());
    fixture.login(1);
    fixture.script.push(REPO, 200, details_body("A fixture"));
    fixture.script.push(
        &format!("{REPO}/branches"),
        200,
        branches_body(&[("main", COMMIT_A)]),
    );
    let (ready, release_a) = script.block(
        &format!("{REPO}/commits/main"),
        200,
        commit_body(COMMIT_A, ROOT_A),
    );
    let (page, cx) = open(cx, &fixture, RepositoryNavigation::default());
    activate(&page, cx);
    wait_gate(&page, cx, ready);
    page.update(cx, |page, cx| page.deactivate(cx));
    release(release_a);
    for _ in 0..20 {
        page.update_in(cx, |page, window, cx| page.drain(window, cx));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(2));
    }
    page.read_with(cx, |page, _| {
        assert!(
            page.fixed_commit().is_none(),
            "the gated revision result never published"
        );
        assert!(
            !page.is_active(),
            "the page stopped its browsing generation"
        );
        assert_eq!(page.reader_status(), &ReaderStatus::Loading);
    });
}

#[gpui::test]
fn partial_branches_and_a_truncated_tree_are_never_reported_as_complete(cx: &mut TestAppContext) {
    let script = Arc::new(Scripted::new());
    let fixture = Fixture::new(script.clone());
    fixture.login(1);
    fixture.script.push(REPO, 200, details_body("A fixture"));
    // Page one, then a failing page two: the branch list stays partial.
    fixture.script.push_with_headers(
        &format!("{REPO}/branches"),
        200,
        branches_body(&[("main", COMMIT_A)]),
        &[(
            "link",
            "<https://api.github.com/repos/octo/demo/branches?per_page=100&page=2>; rel=\"next\"",
        )],
    );
    fixture.script.push(
        &format!("{REPO}/branches"),
        404,
        serde_json::json!({"message": "Not Found"}),
    );
    fixture.script.push(
        &format!("{REPO}/commits/main"),
        200,
        commit_body(COMMIT_A, ROOT_A),
    );
    fixture.script.push(
        &tree_key(ROOT_A),
        200,
        tree_body(ROOT_A, true, vec![file_entry("README.md", README_BLOB, 10)]),
    );
    fixture.script.push(
        &format!("{REPO}/readme"),
        200,
        readme_body("README.md", README_BLOB, README_TEXT.len()),
    );
    fixture.script.push(
        &blob_key(README_BLOB),
        200,
        blob_body(README_BLOB, README_TEXT.as_bytes()),
    );

    let (page, cx) = open(cx, &fixture, RepositoryNavigation::default());
    activate(&page, cx);
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    page.read_with(cx, |page, _| {
        let branches = page.branches().expect("branches");
        assert_eq!(branches.branches.len(), 1, "the first page is kept");
        assert!(
            matches!(
                branches.completeness,
                Completeness::Partial { ref reasons }
                    if reasons.iter().any(|reason| matches!(
                        reason,
                        IncompleteReason::Failed(RepositoryError::GitHub(GitHubError::NotFound))
                    ))
            ),
            "a failing later page keeps the observed branch and reports Partial, got {:?}",
            branches.completeness
        );
        let tree = page.tree("").expect("root tree");
        assert_eq!(tree.truncated, Some(true));
        assert!(super::navigation::tree_incomplete(tree));
    });
    frames(cx);
    assert!(cx.debug_bounds("repository-branches-incomplete").is_some());
    assert!(cx.debug_bounds("repository-tree-incomplete").is_some());
}

#[gpui::test]
fn a_missing_readme_is_a_true_empty_state_with_a_usable_tree(cx: &mut TestAppContext) {
    let script = Arc::new(Scripted::new());
    let fixture = Fixture::new(script.clone());
    fixture.login(1);
    fixture.script.push(REPO, 200, details_body("A fixture"));
    fixture.script.push(
        &format!("{REPO}/branches"),
        200,
        branches_body(&[("main", COMMIT_A)]),
    );
    fixture.script.push(
        &format!("{REPO}/commits/main"),
        200,
        commit_body(COMMIT_A, ROOT_A),
    );
    // The repository is a confirmed empty tree: the README is genuinely absent.
    fixture
        .script
        .push(&tree_key(ROOT_A), 200, tree_body(ROOT_A, false, vec![]));
    fixture.script.push(
        &format!("{REPO}/readme"),
        404,
        serde_json::json!({"message": "Not Found"}),
    );
    fixture
        .script
        .push(&tree_key(ROOT_A), 200, tree_body(ROOT_A, false, vec![]));

    let (page, cx) = open(cx, &fixture, RepositoryNavigation::default());
    activate(&page, cx);
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::NoReadme
    });
    page.read_with(cx, |page, _| {
        assert_eq!(page.readme_state(), Some(&ReadmeState::Absent));
        assert!(page.reader().is_none(), "no document is invented");
        assert!(page.tree("").is_some(), "the tree stays usable");
        assert!(page.tree("").unwrap().entries.is_empty());
    });
    frames(cx);
    assert!(cx.debug_bounds("repository-no-readme").is_some());
    assert!(cx.debug_bounds("repository-tree-column").is_some());
}

#[gpui::test]
fn an_unconfirmed_readme_is_distinct_from_absence_and_from_failure(cx: &mut TestAppContext) {
    let script = Arc::new(Scripted::new());
    let fixture = Fixture::new(script.clone());
    fixture.login(1);
    fixture.script_first_screen_no_readme_blob();
    fixture.script.push(
        &format!("{REPO}/readme"),
        404,
        serde_json::json!({"message": "Not Found"}),
    );
    fixture.script.push(
        &tree_key(ROOT_A),
        200,
        tree_body(ROOT_A, false, vec![file_entry("LICENSE", MAIN_BLOB, 6)]),
    );

    let (page, cx) = open(cx, &fixture, RepositoryNavigation::default());
    activate(&page, cx);
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::ReadmeUncertain
    });
    page.read_with(cx, |page, _| {
        assert_eq!(page.readme_state(), Some(&ReadmeState::Uncertain));
        assert!(page.reader().is_none());
    });
    frames(cx);
    assert!(cx.debug_bounds("repository-readme-uncertain").is_some());

    // A failed discovery is its own state, with the reason and one retry.
    let script = Arc::new(Scripted::new());
    let fixture = Fixture::new(script.clone());
    fixture.login(1);
    fixture.script_first_screen_no_readme_blob();
    fixture.script.push(
        &format!("{REPO}/readme"),
        401,
        serde_json::json!({"message": "Bad credentials"}),
    );
    let (page, cx) = open(cx, &fixture, RepositoryNavigation::default());
    activate(&page, cx);
    settle(&page, cx, |page| {
        matches!(page.reader_status(), ReaderStatus::ReadmeUnavailable(_))
    });
    let error = page.read_with(cx, |page, _| match page.reader_status() {
        ReaderStatus::ReadmeUnavailable(error) => error.clone(),
        other => panic!("expected a README failure, got {other:?}"),
    });
    assert!(error.title.contains("credential") || error.title.contains("rejected"));
    assert!(
        !error.retryable,
        "a rejected credential is not fixed by retrying the same request"
    );
}

trait FixtureScript {
    fn script_first_screen_no_readme_blob(&self);
}

impl FixtureScript for Fixture {
    /// Summary, branches, revision and the root tree; README discovery is left
    /// to the test so it can answer 404 or fail explicitly.
    fn script_first_screen_no_readme_blob(&self) {
        self.script.push(REPO, 200, details_body("A fixture"));
        self.script.push(
            &format!("{REPO}/branches"),
            200,
            branches_body(&[("main", COMMIT_A)]),
        );
        self.script.push(
            &format!("{REPO}/commits/main"),
            200,
            commit_body(COMMIT_A, ROOT_A),
        );
        self.script.push(
            &tree_key(ROOT_A),
            200,
            tree_body(ROOT_A, false, vec![dir_entry("docs", DOCS_TREE)]),
        );
    }
}

#[gpui::test]
fn an_empty_or_unavailable_repository_is_not_shown_as_an_empty_directory(cx: &mut TestAppContext) {
    let script = Arc::new(Scripted::new());
    let fixture = Fixture::new(script.clone());
    fixture.login(1);
    fixture.script.push(REPO, 200, details_body("A fixture"));
    fixture.script.push(
        &format!("{REPO}/branches"),
        200,
        branches_body(&[("main", COMMIT_A)]),
    );
    fixture.script.push(
        &format!("{REPO}/commits/main"),
        409,
        serde_json::json!({"message": "Git Repository is empty."}),
    );

    let (page, cx) = open(cx, &fixture, RepositoryNavigation::default());
    activate(&page, cx);
    settle(&page, cx, |page| page.error().is_some());
    let error = page.read_with(cx, |page, _| page.error().cloned().unwrap());
    assert!(error.title.contains("no readable Git data"));
    assert!(page.read_with(cx, |page, _| page.rows().is_empty()));
    frames(cx);
    assert!(cx.debug_bounds("repository-error").is_some());
}

#[gpui::test]
fn unsupported_and_oversized_files_keep_their_honest_reader_states(cx: &mut TestAppContext) {
    let (fixture, page, cx) = loaded(cx);
    fixture.script.push(
        &tree_key(DOCS_TREE),
        200,
        tree_body(
            DOCS_TREE,
            false,
            vec![
                file_entry("binary.dat", BINARY_BLOB, 4),
                file_entry("huge.log", LARGE_BLOB, 3 * 1024 * 1024),
            ],
        ),
    );
    fixture.script.push(
        &blob_key(BINARY_BLOB),
        200,
        blob_body(BINARY_BLOB, &[0xff, 0xfe, 0x00, 0x01]),
    );
    page.update(cx, |page, cx| page.toggle_directory("docs", cx));
    settle(&page, cx, |page| page.tree("docs").is_some());

    page.update(cx, |page, cx| page.select_file("docs/binary.dat", cx));
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    let reader = page
        .read_with(cx, |page, _| page.reader().cloned())
        .unwrap();
    reader.read_with(cx, |reader, _| {
        assert!(reader.is_status(), "non-UTF-8 bytes are a status, not text");
    });

    // A file past the blob budget is refused locally, and still reports TooLarge.
    page.update(cx, |page, cx| page.select_file("docs/huge.log", cx));
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    let reader = page
        .read_with(cx, |page, _| page.reader().cloned())
        .unwrap();
    reader.read_with(cx, |reader, _| {
        assert!(reader.is_status());
        assert_eq!(reader.context().path(), "docs/huge.log");
    });
    assert_eq!(
        fixture.script.count(&blob_key(LARGE_BLOB)),
        0,
        "a known-oversized blob is not downloaded"
    );
}

#[gpui::test]
fn retry_is_explicit_bounded_and_scoped_to_the_failed_step(cx: &mut TestAppContext) {
    let script = Arc::new(Scripted::new());
    let fixture = Fixture::new(script.clone());
    fixture.login(1);
    fixture
        .script
        .push(REPO, 500, serde_json::json!({"message": "boom"}));
    let (page, cx) = open(cx, &fixture, RepositoryNavigation::default());
    activate(&page, cx);
    settle(&page, cx, |page| page.error().is_some());
    assert_eq!(fixture.script.count(REPO), 1, "no automatic retry");
    let error = page.read_with(cx, |page, _| page.error().cloned().unwrap());
    assert!(error.retryable);

    // The retry re-runs the failed step only, and succeeds on the second try.
    fixture.script_first_screen();
    page.update(cx, |page, cx| page.retry(cx));
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    assert_eq!(fixture.script.count(REPO), 2);
    assert_eq!(fixture.script.count(&format!("{REPO}/branches")), 1);
}

/// One window holding two independent pages, as two tabs in a Shell would.
struct TwoPages {
    first: Entity<RepositoryPage>,
    second: Entity<RepositoryPage>,
}

impl Render for TwoPages {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        gpui::div()
            .flex()
            .size_full()
            .child(gpui::div().flex_1().h_full().child(self.first.clone()))
            .child(gpui::div().flex_1().h_full().child(self.second.clone()))
    }
}

#[gpui::test]
fn two_pages_keep_independent_branch_path_and_tree_state(cx: &mut TestAppContext) {
    let script = Arc::new(Scripted::new());
    let fixture = Fixture::new(script.clone());
    fixture.login(1);
    // Both pages share one service and transport, so each key is scripted once
    // per consumer: a request pops its own scripted answer.
    let root_entries = vec![
        file_entry("README.md", README_BLOB, README_TEXT.len() as u64),
        dir_entry("docs", DOCS_TREE),
    ];
    for _ in 0..2 {
        fixture.script.push(REPO, 200, details_body("A fixture"));
        fixture.script.push(
            &format!("{REPO}/branches"),
            200,
            branches_body(&[("main", COMMIT_A), ("release", COMMIT_B)]),
        );
        fixture.script.push(
            &format!("{REPO}/commits/main"),
            200,
            commit_body(COMMIT_A, ROOT_A),
        );
        fixture.script.push(
            &tree_key(ROOT_A),
            200,
            tree_body(ROOT_A, false, root_entries.clone()),
        );
        fixture.script.push(
            &format!("{REPO}/readme"),
            200,
            readme_body("README.md", README_BLOB, README_TEXT.len()),
        );
        fixture.script.push(
            &blob_key(README_BLOB),
            200,
            blob_body(README_BLOB, README_TEXT.as_bytes()),
        );
    }
    // The second page then moves to `release` and expands `docs`.
    fixture.script.push(
        &format!("{REPO}/commits/release"),
        200,
        commit_body(COMMIT_B, ROOT_B),
    );
    fixture.script.push(
        &tree_key(ROOT_B),
        200,
        tree_body(
            ROOT_B,
            false,
            vec![
                file_entry("other.txt", MAIN_BLOB, 5),
                dir_entry("docs", DOCS_TREE),
            ],
        ),
    );
    fixture.script.push(
        &format!("{REPO}/readme"),
        200,
        readme_body("README.md", README_BLOB, README_TEXT.len()),
    );
    fixture.script.push(
        &blob_key(README_BLOB),
        200,
        blob_body(README_BLOB, README_TEXT.as_bytes()),
    );
    fixture.script.push(
        &tree_key(DOCS_TREE),
        200,
        tree_body(
            DOCS_TREE,
            false,
            vec![file_entry("guide.md", GUIDE_BLOB, GUIDE_TEXT.len() as u64)],
        ),
    );

    setup(cx);
    let services = fixture.services();
    let locator = fixture.locator();
    let (pair, cx) = cx.add_window_view(|window, cx| {
        let first = cx.new(|cx| {
            let mut page = RepositoryPage::new(
                services.clone(),
                locator.clone(),
                RepositoryNavigation::default(),
                window,
                cx,
            );
            page.activate(cx);
            page
        });
        let second = cx.new(|cx| {
            let mut page = RepositoryPage::new(
                services.clone(),
                locator.clone(),
                RepositoryNavigation::default(),
                window,
                cx,
            );
            page.activate(cx);
            page
        });
        TwoPages { first, second }
    });
    let (first, second) = pair.read_with(cx, |pair, _| (pair.first.clone(), pair.second.clone()));

    let pump = |first: &Entity<RepositoryPage>,
                second: &Entity<RepositoryPage>,
                cx: &mut VisualTestContext| {
        first.update_in(cx, |page, window, cx| page.drain(window, cx));
        second.update_in(cx, |page, window, cx| page.drain(window, cx));
        cx.run_until_parked();
        frames(cx);
        std::thread::sleep(Duration::from_millis(2));
    };
    let mut ready = false;
    for _ in 0..500 {
        pump(&first, &second, cx);
        if first.read_with(cx, |page, _| page.reader_status() == &ReaderStatus::Showing)
            && second.read_with(cx, |page, _| page.reader_status() == &ReaderStatus::Showing)
        {
            ready = true;
            break;
        }
    }
    assert!(ready, "both pages loaded their own README");

    // The second page switches branch and expands a directory.
    second.update(cx, |page, cx| page.select_branch("release", cx));
    let mut switched = false;
    for _ in 0..300 {
        pump(&first, &second, cx);
        if second.read_with(cx, |page, _| {
            page.fixed_commit().map(|sha| sha.as_str()) == Some(COMMIT_B)
        }) {
            switched = true;
            break;
        }
    }
    assert!(switched, "the second page reached the other branch");
    second.update(cx, |page, cx| page.toggle_directory("docs", cx));
    for _ in 0..200 {
        pump(&first, &second, cx);
        if second.read_with(cx, |page, _| page.tree("docs").is_some()) {
            break;
        }
    }

    // Scrolling one page's tree does not move the other page's tree.
    let second_tree = cx.debug_bounds("repository-tree-column");
    if let Some(bounds) = second_tree {
        cx.simulate_mouse_move(bounds.center(), None, Modifiers::none());
        cx.simulate_event(gpui::ScrollEvent {
            position: bounds.center(),
            delta: gpui::ScrollDelta::Lines(gpui::point(0.0, -30.0)),
            modifiers: Modifiers::none(),
            touch_phase: Default::default(),
            source: gpui::ScrollSource::Pointer,
        });
        frames(cx);
    }
    let first_offset = first.read_with(cx, |page, _| {
        page.tree_scroll.0.borrow().base_handle.offset()
    });
    assert_eq!(
        first_offset.y,
        px(0.0),
        "the first page's tree stayed where it was"
    );

    first.read_with(cx, |page, _| {
        assert_eq!(page.selected_branch(), Some("main"));
        assert_eq!(page.fixed_commit().map(|sha| sha.as_str()), Some(COMMIT_A));
        assert!(
            page.expanded().is_empty(),
            "the second page's expansion does not leak into the first"
        );
        assert_eq!(page.selection(), &Selection::Readme);
        assert!(!page.rows().iter().any(|row| row.path == "docs/guide.md"));
    });
    second.read_with(cx, |page, _| {
        assert_eq!(page.selected_branch(), Some("release"));
        assert_eq!(page.fixed_commit().map(|sha| sha.as_str()), Some(COMMIT_B));
        assert!(page.expanded().contains("docs"));
        let rows = page.rows();
        assert!(
            rows.iter().any(|row| row.path == "docs/guide.md"),
            "the second page kept its own expansion, got {rows:?}"
        );
    });
}

#[gpui::test]
fn rebinding_keeps_navigation_intent_but_reads_under_new_authorization(cx: &mut TestAppContext) {
    let script = Arc::new(Scripted::new());
    let fixture = Fixture::new(script.clone());
    fixture.login(1);
    fixture.script_first_screen();
    let intent = RepositoryNavigation {
        branch: Some("main".into()),
        path: Some("docs/guide.md".into()),
        expanded: ["docs".to_string()].into_iter().collect(),
    };
    fixture.script_docs_and_guide();
    let (page, cx) = open(cx, &fixture, intent.clone());
    activate(&page, cx);
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
            && page.selection() == &Selection::File("docs/guide.md".into())
    });
    let old_requests = fixture.script.total();

    // A second account is signed in, and the host rebinds the page to it.
    fixture.login(2);
    fixture.script.push(REPO, 200, details_body("A fixture"));
    fixture.script.push(
        &format!("{REPO}/branches"),
        200,
        branches_body(&[("main", COMMIT_A)]),
    );
    fixture.script.push(
        &format!("{REPO}/commits/main"),
        200,
        commit_body(COMMIT_A, ROOT_A),
    );
    fixture.script.push(
        &tree_key(ROOT_A),
        200,
        tree_body(
            ROOT_A,
            false,
            vec![
                file_entry("README.md", README_BLOB, README_TEXT.len() as u64),
                dir_entry("docs", DOCS_TREE),
            ],
        ),
    );
    fixture.script.push(
        &format!("{REPO}/readme"),
        200,
        readme_body("README.md", README_BLOB, README_TEXT.len()),
    );
    fixture.script_docs_and_guide();

    let fresh = fixture.services();
    page.update(cx, |page, cx| page.rebind(fresh, cx));
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
            && page.selection() == &Selection::File("docs/guide.md".into())
    });
    page.read_with(cx, |page, _| {
        assert_eq!(page.navigation(), intent, "intent survives the rebind");
        let context = page.context.as_ref().expect("a new session");
        assert_eq!(
            context.auth().account.id,
            2,
            "the new session belongs to the new account"
        );
        let reader = page.reader().expect("reader");
        let _ = reader;
    });
    assert!(
        fixture.script.total() > old_requests,
        "the rebind re-read everything instead of reusing old data"
    );
}

#[gpui::test]
fn a_long_tree_stays_bounded_and_scrolls_only_its_own_region(cx: &mut TestAppContext) {
    let script = Arc::new(Scripted::new());
    let fixture = Fixture::new(script.clone());
    fixture.login(1);
    fixture.script.push(REPO, 200, details_body("A fixture"));
    fixture.script.push(
        &format!("{REPO}/branches"),
        200,
        branches_body(&[("main", COMMIT_A)]),
    );
    fixture.script.push(
        &format!("{REPO}/commits/main"),
        200,
        commit_body(COMMIT_A, ROOT_A),
    );
    let entries: Vec<serde_json::Value> = (0..3000)
        .map(|index| file_entry(&format!("file-{index:04}.txt"), MAIN_BLOB, 3))
        .collect();
    fixture
        .script
        .push(&tree_key(ROOT_A), 200, tree_body(ROOT_A, false, entries));
    fixture.script.push(
        &format!("{REPO}/readme"),
        404,
        serde_json::json!({"message": "Not Found"}),
    );
    fixture.script.push(
        &tree_key(ROOT_A),
        200,
        tree_body(
            ROOT_A,
            false,
            (0..3000)
                .map(|index| file_entry(&format!("file-{index:04}.txt"), MAIN_BLOB, 3))
                .collect(),
        ),
    );

    let (page, cx) = open(cx, &fixture, RepositoryNavigation::default());
    activate(&page, cx);
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::ReadmeUncertain
    });
    let rows = page.update(cx, |page, _| page.refresh_rows());
    assert_eq!(rows.len(), 3000);
    assert!(page.read_with(cx, |page, _| !page.tree_capped()));
    frames(cx);
    assert!(cx.debug_bounds("repository-tree-row-0").is_some());
    assert!(
        cx.debug_bounds("repository-tree-row-2500").is_none(),
        "off-screen rows are not built"
    );

    // Wheel input over the tree moves the tree only; the reader column has no
    // competing scroll surface, so nothing else can move.
    let tree = cx.debug_bounds("repository-tree-column").unwrap();
    cx.simulate_mouse_move(tree.center(), None, Modifiers::none());
    cx.simulate_event(gpui::ScrollEvent {
        position: tree.center(),
        delta: gpui::ScrollDelta::Lines(gpui::point(0.0, -40.0)),
        modifiers: Modifiers::none(),
        touch_phase: Default::default(),
        source: gpui::ScrollSource::Pointer,
    });
    frames(cx);
    let offset = page.read_with(cx, |page, _| {
        page.tree_scroll.0.borrow().base_handle.offset()
    });
    assert!(
        offset.y < px(0.0),
        "the tree viewport scrolled, offset {offset:?}"
    );
    assert!(cx.debug_bounds("repository-readme-row").is_some());
    assert!(cx.debug_bounds("repository-reader-column").is_some());
}

#[gpui::test]
fn a_tree_past_the_row_cap_reports_the_bound_instead_of_growing(_cx: &mut TestAppContext) {
    // The cap is a property of the flattened model, so it can be driven without
    // downloading a repository of that size.
    let mut model = TreeModel::default();
    let identity = RepositoryIdentity {
        id: 1,
        locator: RepositoryLocator::new("octo", "demo").unwrap(),
    };
    let snapshot = RepoSnapshot {
        repository: identity.clone(),
        requested: crate::domain::repository::RevisionSpec::Commit(
            ObjectSha::new(COMMIT_A).unwrap(),
        ),
        commit: ObjectSha::new(COMMIT_A).unwrap(),
        root_tree: ObjectSha::new(ROOT_A).unwrap(),
        message: "fixture".into(),
        author: None,
        committer: None,
    };
    let entries: Vec<TreeEntry> = (0..MAX_TREE_ROWS + 50)
        .map(|index| TreeEntry {
            path: crate::domain::repository::RepositoryPath::new(format!("f{index}")).unwrap(),
            sha: ObjectSha::new(MAIN_BLOB).unwrap(),
            size: Some(1),
            kind: TreeEntryKind::File { executable: false },
        })
        .collect();
    model.set_root(DirectoryTree {
        directory: SnapshotDirectory::root(&snapshot),
        entries,
        truncated: Some(false),
        completeness: Completeness::Complete,
    });
    let rows = model.rows(&Default::default());
    assert_eq!(rows.len(), MAX_TREE_ROWS);
    assert!(
        model.capped(),
        "the cap is stated rather than silently dropped"
    );
}

#[gpui::test]
fn reader_links_follow_same_commit_files_and_hand_external_urls_to_the_host(
    cx: &mut TestAppContext,
) {
    let (fixture, page, cx) = loaded(cx);
    fixture.script.push(
        &tree_key(DOCS_TREE),
        200,
        tree_body(
            DOCS_TREE,
            false,
            vec![file_entry("guide.md", GUIDE_BLOB, GUIDE_TEXT.len() as u64)],
        ),
    );
    fixture.script.push(
        &blob_key(GUIDE_BLOB),
        200,
        blob_body(GUIDE_BLOB, GUIDE_TEXT.as_bytes()),
    );
    // Returning to the README re-reads it at the fixed commit: this page keeps
    // no document cache, as the scope requires.
    fixture.script.push(
        &format!("{REPO}/readme"),
        200,
        readme_body("README.md", README_BLOB, README_TEXT.len()),
    );
    fixture.script.push(
        &blob_key(README_BLOB),
        200,
        blob_body(README_BLOB, README_TEXT.as_bytes()),
    );
    fixture.script.push(
        &tree_key(DOCS_TREE),
        200,
        tree_body(
            DOCS_TREE,
            false,
            vec![file_entry("guide.md", GUIDE_BLOB, GUIDE_TEXT.len() as u64)],
        ),
    );
    fixture.script.push(
        &blob_key(GUIDE_BLOB),
        200,
        blob_body(GUIDE_BLOB, GUIDE_TEXT.as_bytes()),
    );
    let actions: Arc<Mutex<Vec<PageAction>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = actions.clone();
    page.update(cx, |page, _| {
        page.on_action(std::rc::Rc::new(move |action, _window, _cx| {
            sink.lock().unwrap().push(action);
        }));
    });

    page.update(cx, |page, cx| page.toggle_directory("docs", cx));
    settle(&page, cx, |page| page.tree("docs").is_some());
    page.update(cx, |page, cx| page.select_file("docs/guide.md", cx));
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
            && page.selection() == &Selection::File("docs/guide.md".into())
    });

    // The README's relative link resolves inside this commit; the page follows it
    // only because the target already exists in the loaded tree.
    page.update(cx, |page, cx| page.show_readme(cx));
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing && page.selection() == &Selection::Readme
    });
    let reader = page
        .read_with(cx, |page, _| page.reader().cloned())
        .unwrap();
    reader.update_in(cx, |reader, window, cx| {
        reader.handle_link("docs/guide.md", window, cx);
    });
    settle(&page, cx, |page| {
        page.selection() == &Selection::File("docs/guide.md".into())
    });

    // An external URL is reported, never opened by the page.
    let reader = page
        .read_with(cx, |page, _| page.reader().cloned())
        .unwrap();
    reader.update_in(cx, |reader, window, cx| {
        reader.handle_link("https://example.com/docs", window, cx);
    });
    cx.run_until_parked();
    let actions = actions.lock().unwrap().clone();
    assert!(
        actions
            .iter()
            .any(|action| matches!(action, PageAction::OpenExternal(url) if url == "https://example.com/docs")),
        "the host received the external URL, got {actions:?}"
    );
    assert!(
        actions.iter().all(
            |action| !matches!(action, PageAction::OpenExternal(url) if url.starts_with("file:"))
        ),
        "no local or scripted protocol is handed over"
    );
}

#[gpui::test]
fn a_failed_directory_read_is_scoped_and_does_not_clear_the_rest_of_the_page(
    cx: &mut TestAppContext,
) {
    let (fixture, page, cx) = loaded(cx);
    fixture.script.push(
        &tree_key(DOCS_TREE),
        403,
        serde_json::json!({"message": "Forbidden"}),
    );
    page.update(cx, |page, cx| page.toggle_directory("docs", cx));
    settle(&page, cx, |page| page.tree_failure("docs").is_some());
    page.read_with(cx, |page, _| {
        let error = page.tree_failure("docs").expect("scoped failure");
        assert!(error.title.contains("denied"));
        assert!(page.tree("").is_some(), "the root listing is still shown");
        assert!(page.details().is_some());
        assert_eq!(page.selection(), &Selection::Readme);
        assert!(
            page.expanded().contains("docs"),
            "the failed directory stays open so its row can say it is unavailable"
        );
        let row = page
            .rows()
            .iter()
            .find(|row| row.path == "docs")
            .expect("the directory row")
            .clone();
        assert_eq!(row.note, Some("unavailable"));
    });
    assert!(page.read_with(cx, |page, _| page.reader().is_some()));
}
