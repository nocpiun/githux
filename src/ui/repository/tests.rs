//! Tests for the repository page.
//!
//! They drive the **real** data pipeline — `AccountService` → `RepoService`
//! session → repository/branches/revision/tree/README/blob → `PreparedContent`
//! → `ReaderView` — over the very same fixture the native preview browses
//! (`src/bin_support/repository_fixture.rs`), a temporary SQLite directory and
//! one synthetic PAT. No request leaves the machine and no real credential is
//! read.
//!
//! Because the preview and these tests share one fixture, a scene the owner can
//! click here is a scene that is asserted here. The GPUI test platform cannot
//! prove real Windows input, IME or a real GitHub account; those remain manual
//! acceptance items recorded in the task devlog.

#[path = "extras_tests.rs"]
mod extras_tests;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    AppContext, Entity, IntoElement, Modifiers, ParentElement, Render, Styled, TestAppContext,
    VisualTestContext, px,
};

use super::navigation::{
    MAX_TREE_ROWS, ReadmeState, RepositoryNavigation, RowState, TreeModel, TreeRow, TreeRowKind,
};
use super::{
    PageAction, ReaderStatus, RepositoryPage, RepositoryPageServices, RepositoryTab, Selection,
};
use crate::auth::InMemoryCredentialStore;
use crate::data::AccountService;
use crate::domain::error::GitHubError;
use crate::domain::repository::{
    Completeness, DirectoryTree, IncompleteReason, ObjectSha, RepoSnapshot, RepositoryError,
    RepositoryIdentity, RepositoryLocator, RevisionSpec, SnapshotDirectory, TreeEntry,
    TreeEntryKind,
};
use crate::github::{GitHubClient, GitHubConfig};
use crate::network::executor::BackgroundExecutor;
use crate::network::transport::Transport;
use crate::repository_modules::fixture::{FixtureTransport, LONG_TREE_FILES, NAME, OWNER, Scene};
use crate::storage::accounts::AccountDatabaseConfig;
use crate::ui::reader::DocumentKind;

fn locator() -> RepositoryLocator {
    RepositoryLocator::new(OWNER, NAME).expect("fixture locator")
}

// --------------------------------------------------------------- test setup --

pub(super) struct Fixture {
    pub(super) transport: Arc<FixtureTransport>,
    accounts: Arc<AccountService>,
    executor: Arc<BackgroundExecutor>,
    _temp: tempfile::TempDir,
    _runtime: tokio::runtime::Runtime,
}

impl Fixture {
    /// Opens the real service stack over the shared fixture transport and signs
    /// in a synthetic account, as the host would have done before mounting.
    fn new(scene: Scene) -> Self {
        let temp = tempfile::tempdir().expect("temporary account directory");
        let executor = Arc::new(BackgroundExecutor::new(4));
        let transport = Arc::new(FixtureTransport::new(scene));
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
                    transport.clone() as Arc<dyn Transport>,
                )),
                executor.clone(),
            )
            .await
            .expect("open account service");
            Arc::new(accounts)
        });
        Self {
            transport,
            accounts,
            executor,
            _temp: temp,
            _runtime: runtime,
        }
    }

    fn services(&self) -> RepositoryPageServices {
        RepositoryPageServices::new(self.accounts.clone(), self.executor.clone())
    }

    fn login(&self, id: u64) {
        // The fixture answers `/user` with the identity the test set, so the
        // real login path verifies the account the test expects.
        self.transport.set_identity(id, OWNER);
        self._runtime.block_on(async {
            self.accounts
                .login(crate::auth::token::Pat::from_str("synthetic-test-token"))
                .await
                .expect("synthetic login");
        });
    }

    fn switch_scene(&self, scene: Scene) {
        self.transport.set_scene(scene);
    }

    /// Signs in a *different* synthetic account, so A→B is a real change.
    pub(super) fn login_as(&self, id: u64, login: &str) {
        self.transport.set_identity(id, login);
        self._runtime.block_on(async {
            self.accounts
                .login(crate::auth::token::Pat::from_str("synthetic-second-token"))
                .await
                .expect("second login");
        });
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
    let locator = locator();
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
    for _ in 0..600 {
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

pub(super) fn frames(cx: &mut VisualTestContext) {
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

fn press(page: &Entity<RepositoryPage>, cx: &mut VisualTestContext, key: &str) {
    page.update_in(cx, |page, window, cx| page.press_tree_key(key, window, cx));
    cx.run_until_parked();
}

fn rows(page: &Entity<RepositoryPage>, cx: &mut VisualTestContext) -> Vec<TreeRow> {
    page.update(cx, |page, _| page.refresh_rows().as_ref().clone())
}

/// The happy path in the Normal scene: summary, one fixed commit, README.
pub(super) fn normal(
    cx: &mut TestAppContext,
) -> (Fixture, Entity<RepositoryPage>, &mut VisualTestContext) {
    let fixture = Fixture::new(Scene::Normal);
    fixture.login(1);
    let (page, cx) = open(cx, &fixture, RepositoryNavigation::default());
    activate(&page, cx);
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    (fixture, page, cx)
}

// -------------------------------------------------------------------- tests --

#[gpui::test]
fn the_normal_fixture_renders_its_readme_at_a_real_path_and_single_selection(
    cx: &mut TestAppContext,
) {
    let (fixture, page, cx) = normal(cx);

    let (details, branch, commit) = page.read_with(cx, |page, _| {
        (
            page.details().cloned(),
            page.selected_branch().map(str::to_string),
            page.fixed_commit().map(|sha| sha.as_str().to_string()),
        )
    });
    let details = details.expect("repository summary reached the page");
    assert_eq!(details.identity.locator().owner(), OWNER);
    assert_eq!(details.identity.locator().name(), NAME);
    assert_eq!(
        details.description.as_deref(),
        Some("A tiny double-entry ledger, kept small enough to browse.")
    );
    assert_eq!(branch.as_deref(), Some("main"));
    assert_eq!(commit.as_deref().map(|sha| &sha[..7]), Some("c0ffee0"));

    // The Code tab is selected and is the only tab with content.
    assert_eq!(
        page.read_with(cx, |page, _| page.tab()),
        RepositoryTab::Code
    );
    assert!(RepositoryTab::Code.has_content());
    assert!(!RepositoryTab::Issues.has_content());

    // The README is the real Markdown reader at its descriptor path.
    assert_eq!(
        page.read_with(cx, |page, _| page.readme_path().map(str::to_string)),
        Some("README.md".to_string())
    );
    assert_eq!(
        page.read_with(cx, |page, _| page.current_path().map(str::to_string)),
        Some("README.md".to_string())
    );
    let reader = page
        .read_with(cx, |page, _| page.reader().cloned())
        .unwrap();
    reader.read_with(cx, |reader, _| {
        assert_eq!(reader.kind(), &DocumentKind::Markdown);
        assert_eq!(reader.context().path(), "README.md");
        assert_eq!(
            reader.context().commit(),
            "c0ffee0000000000000000000000000000000001"
        );
        assert!(
            reader.markdown().is_some(),
            "a markdown document is mounted"
        );
    });

    // The tree contains real entries only — no synthesised "README" pseudo-file.
    let rows = rows(&page, cx);
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "assets",
            "docs",
            "src",
            "link-to-readme",
            "README.md",
            "vendor",
        ],
        "folders first, then files; no pseudo README row"
    );
    // The README file row is the single selected row.
    let readme_row = rows.iter().find(|row| row.path == "README.md").unwrap();
    assert_eq!(
        readme_row.path,
        page.read_with(cx, |page, _| page.current_path().unwrap().to_string())
    );
    assert!(cx.debug_bounds("repository-tab-code").is_some());
    assert!(cx.debug_bounds("repository-tab-active-code").is_some());
    assert!(cx.debug_bounds("repository-description").is_some());

    // Nothing in the run was an unmodelled request.
    assert!(
        fixture.transport.mismatches().is_empty(),
        "{:?}",
        fixture.transport.mismatches()
    );
}

#[gpui::test]
fn expanding_nested_directories_and_reopening_the_same_file_always_succeeds(
    cx: &mut TestAppContext,
) {
    let (fixture, page, cx) = normal(cx);

    // Expand docs, then docs/guides, then docs/guides/nested: one tree request each.
    for (path, expect) in [
        ("docs", "docs/guide.md"),
        // One level at a time: `guides` first exposes the `nested` directory,
        // and the deep file only appears once `nested` itself is opened.
        ("docs/guides", "docs/guides/nested"),
        ("docs/guides/nested", "docs/guides/nested/deep-note.md"),
    ] {
        page.update(cx, |page, cx| page.toggle_directory(path, cx));
        settle(&page, cx, |page| page.tree(path).is_some());
        assert!(
            rows(&page, cx).iter().any(|row| row.path == expect),
            "expanding {path} exposed {expect}"
        );
    }
    let doc_trees = fixture.transport.count("/git/trees/");
    assert_eq!(
        doc_trees, 4,
        "root + docs + guides + nested, one request each"
    );

    // Collapse and re-expand: no new request, the rows come right back.
    page.update(cx, |page, cx| page.toggle_directory("docs", cx));
    let collapsed = rows(&page, cx);
    assert!(!collapsed.iter().any(|row| row.path == "docs/guide.md"));
    assert!(
        !collapsed.iter().any(|row| row.path == "docs/guides"),
        "collapsing a parent hides its whole subtree"
    );
    page.update(cx, |page, cx| page.toggle_directory("docs", cx));
    settle(&page, cx, |page| page.tree("docs").is_some());
    assert_eq!(
        fixture.transport.count("/git/trees/"),
        doc_trees,
        "re-expanding a loaded directory does not re-download it"
    );

    // Open src/main.rs, then the README again, then the same file three times.
    page.update(cx, |page, cx| page.toggle_directory("src", cx));
    settle(&page, cx, |page| page.tree("src").is_some());
    page.update(cx, |page, cx| page.select_file("src/main.rs", cx));
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
            && page.selection() == &Selection::File("src/main.rs".into())
    });
    let reader = page
        .read_with(cx, |page, _| page.reader().cloned())
        .unwrap();
    reader.read_with(cx, |reader, _| {
        assert_eq!(reader.kind(), &DocumentKind::Source { language: None });
        assert_eq!(reader.context().path(), "src/main.rs");
    });

    for _ in 0..3 {
        page.update(cx, |page, cx| page.select_file("src/main.rs", cx));
        settle(&page, cx, |page| {
            page.reader_status() == &ReaderStatus::Showing
                && page.selection() == &Selection::File("src/main.rs".into())
        });
        let reader = page
            .read_with(cx, |page, _| page.reader().cloned())
            .unwrap();
        reader.read_with(cx, |reader, _| {
            assert_eq!(reader.context().path(), "src/main.rs");
        });
    }

    // README → source → README keeps working and lands on the real path.
    page.update(cx, |page, cx| page.show_readme(cx));
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing && page.selection() == &Selection::Readme
    });
    assert_eq!(
        page.read_with(cx, |page, _| page.current_path().map(str::to_string)),
        Some("README.md".to_string())
    );
    assert!(
        fixture.transport.mismatches().is_empty(),
        "{:?}",
        fixture.transport.mismatches()
    );
}

#[gpui::test]
fn switching_branches_shows_real_differences_and_back_again(cx: &mut TestAppContext) {
    let (fixture, page, cx) = normal(cx);
    let main_rows = rows(&page, cx);

    page.update(cx, |page, cx| page.select_branch("feature/docs-polish", cx));
    settle(&page, cx, |page| {
        page.selected_branch() == Some("feature/docs-polish")
            && page.reader_status() == &ReaderStatus::Showing
    });
    let commit = page.read_with(cx, |page, _| {
        page.fixed_commit().map(|sha| sha.as_str().to_string())
    });
    assert_eq!(
        commit.as_deref(),
        Some("c0ffee0000000000000000000000000000000002"),
        "the other branch resolved to its own commit"
    );
    let feature_rows = rows(&page, cx);
    assert!(main_rows.iter().any(|row| row.path == "src"));
    page.update(cx, |page, cx| page.toggle_directory("src", cx));
    settle(&page, cx, |page| page.tree("src").is_some());
    let feature_rows = {
        let mut all = feature_rows;
        all.extend(rows(&page, cx));
        all
    };
    assert!(
        feature_rows
            .iter()
            .any(|row| row.path == "src/feature_note.rs"),
        "a file only present on that branch appears, got {:?}",
        feature_rows
            .iter()
            .map(|row| row.path.as_str())
            .collect::<Vec<_>>()
    );
    let reader = page
        .read_with(cx, |page, _| page.reader().cloned())
        .unwrap();
    reader.read_with(cx, |reader, _| {
        assert_eq!(
            reader.context().commit(),
            "c0ffee0000000000000000000000000000000002"
        );
    });

    page.update(cx, |page, cx| page.select_branch("main", cx));
    settle(&page, cx, |page| {
        page.selected_branch() == Some("main") && page.reader_status() == &ReaderStatus::Showing
    });
    assert_eq!(
        page.read_with(cx, |page, _| page
            .fixed_commit()
            .map(|s| s.as_str().to_string()))
            .as_deref(),
        Some("c0ffee0000000000000000000000000000000001")
    );
    assert!(
        !rows(&page, cx)
            .iter()
            .any(|row| row.path == "src/feature_note.rs")
    );
    assert!(
        fixture.transport.mismatches().is_empty(),
        "{:?}",
        fixture.transport.mismatches()
    );
}

#[gpui::test]
fn a_directory_toggle_keeps_the_reader_and_a_chevron_press_toggles_once(cx: &mut TestAppContext) {
    let (_fixture, page, cx) = normal(cx);
    let before = page.read_with(cx, |page, _| page.current_path().map(str::to_string));
    assert_eq!(before, Some("README.md".to_string()));

    // Pressing the directory row toggles the expansion, not the reader.
    page.update(cx, |page, cx| page.toggle_directory("src", cx));
    settle(&page, cx, |page| page.tree("src").is_some());
    assert_eq!(
        page.read_with(cx, |page, _| page.current_path().map(str::to_string)),
        Some("README.md".to_string()),
        "opening a directory must not clear the open document"
    );
    assert!(page.read_with(cx, |page, _| page.reader().is_some()));

    // Toggling twice returns to the exact state without double-toggling.
    page.update(cx, |page, cx| page.toggle_directory("src", cx));
    assert!(!page.read_with(cx, |page, _| page.expanded().contains("src")));
    page.update(cx, |page, cx| page.toggle_directory("src", cx));
    assert!(page.read_with(cx, |page, _| page.expanded().contains("src")));

    // Directory rows are folders; file rows are files; the chevron slot exists.
    let rows = rows(&page, cx);
    assert!(matches!(
        rows.iter()
            .find(|row| row.path == "src")
            .map(|row| row.kind),
        Some(TreeRowKind::Directory { expanded: true })
    ));
    assert_eq!(
        rows.iter()
            .find(|row| row.path == "src/main.rs")
            .map(|row| row.kind),
        Some(TreeRowKind::File)
    );
    assert_eq!(
        rows.iter()
            .find(|row| row.path == "link-to-readme")
            .map(|row| row.kind),
        Some(TreeRowKind::Symlink)
    );
    assert_eq!(
        rows.iter()
            .find(|row| row.path == "vendor")
            .map(|row| row.kind),
        Some(TreeRowKind::Submodule)
    );
}

#[gpui::test]
fn the_tree_is_keyboard_navigable_without_taking_the_reader_selection(cx: &mut TestAppContext) {
    let (_fixture, page, cx) = normal(cx);
    page.update(cx, |page, cx| page.focus_row("README.md", cx));

    // Down/Up walk visible rows; the cursor is a path, not a loose index.
    press(&page, cx, "down");
    let down = page.read_with(cx, |page, _| page.cursor().map(str::to_string));
    assert_eq!(down.as_deref(), Some("vendor"), "README.md is the last row");
    press(&page, cx, "up");
    assert_eq!(
        page.read_with(cx, |page, _| page.cursor().map(str::to_string)),
        Some("README.md".to_string())
    );

    // Right expands a directory; Enter on a file opens it.
    page.update(cx, |page, cx| page.focus_row("src", cx));
    press(&page, cx, "right");
    settle(&page, cx, |page| page.tree("src").is_some());
    assert!(page.read_with(cx, |page, _| page.expanded().contains("src")));
    press(&page, cx, "right");
    assert_eq!(
        page.read_with(cx, |page, _| page.cursor().map(str::to_string)),
        Some("src/main.rs".to_string()),
        "right steps into the first child"
    );
    press(&page, cx, "enter");
    settle(&page, cx, |page| {
        page.selection() == &Selection::File("src/main.rs".into())
    });

    // Left climbs back to the parent, then collapses it.
    press(&page, cx, "left");
    assert_eq!(
        page.read_with(cx, |page, _| page.cursor().map(str::to_string)),
        Some("src".to_string())
    );
    press(&page, cx, "left");
    assert!(!page.read_with(cx, |page, _| page.expanded().contains("src")));

    // A modified keystroke is not consumed by the tree.
    let consumed = page.update_in(cx, |page, window, cx| {
        let event = gpui::KeyDownEvent {
            keystroke: gpui::Keystroke {
                modifiers: Modifiers {
                    control: true,
                    ..Default::default()
                },
                key: "a".into(),
                key_char: None,
            },
            is_held: false,
            prefer_character_input: false,
        };
        let before = page.cursor().map(str::to_string);
        page.press_tree_key_event(&event, window, cx);
        (before, page.cursor().map(str::to_string))
    });
    let _ = cx;
    assert_eq!(consumed.0, consumed.1, "Ctrl+A stays with the reader");
}

#[gpui::test]
fn the_readme_relative_link_follows_the_guide_at_the_same_commit(cx: &mut TestAppContext) {
    let (fixture, page, cx) = normal(cx);
    let actions: Arc<Mutex<Vec<PageAction>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = actions.clone();
    page.update(cx, |page, _| {
        page.on_action(std::rc::Rc::new(move |action, _window, _cx| {
            sink.lock().unwrap().push(action);
        }));
    });

    let reader = page
        .read_with(cx, |page, _| page.reader().cloned())
        .unwrap();
    reader.update_in(cx, |reader, window, cx| {
        reader.handle_link("docs/guide.md", window, cx);
    });
    // The ancestor `docs` is expanded lazily, then the file is read.
    settle(&page, cx, |page| {
        page.selection() == &Selection::File("docs/guide.md".into())
            && page.reader_status() == &ReaderStatus::Showing
    });
    assert!(
        page.read_with(cx, |page, _| page.expanded().contains("docs")),
        "the ancestor directory of the linked file is expanded"
    );
    let reader = page
        .read_with(cx, |page, _| page.reader().cloned())
        .unwrap();
    reader.read_with(cx, |reader, _| {
        assert_eq!(reader.context().path(), "docs/guide.md");
        assert_eq!(
            reader.context().commit(),
            "c0ffee0000000000000000000000000000000001"
        );
    });

    // An external URL is reported to the host, never opened by the page.
    let reader = page
        .read_with(cx, |page, _| page.reader().cloned())
        .unwrap();
    reader.update_in(cx, |reader, window, cx| {
        reader.handle_link("https://www.rust-lang.org/", window, cx);
    });
    cx.run_until_parked();
    let actions = actions.lock().unwrap().clone();
    assert!(
        actions.iter().any(
            |action| matches!(action, PageAction::OpenExternal(url) if url == "https://www.rust-lang.org/")
        ),
        "the host received the external URL, got {actions:?}"
    );
    assert!(
        fixture.transport.mismatches().is_empty(),
        "{:?}",
        fixture.transport.mismatches()
    );
}

#[gpui::test]
fn switching_to_another_tab_clears_the_code_area_and_returning_restores_it(
    cx: &mut TestAppContext,
) {
    let (_fixture, page, cx) = normal(cx);
    page.update(cx, |page, cx| page.select_tab(RepositoryTab::Issues, cx));
    frames(cx);
    assert_eq!(
        page.read_with(cx, |page, _| page.tab()),
        RepositoryTab::Issues
    );
    assert!(
        cx.debug_bounds("repository-tab-empty").is_some(),
        "undesigned tab content stays empty"
    );
    assert!(
        cx.debug_bounds("repository-tree-column").is_none(),
        "Code's tree is not drawn under another tab"
    );
    assert!(cx.debug_bounds("repository-reader-column").is_none());
    // Navigation intent survives the detour.
    assert_eq!(
        page.read_with(cx, |page, _| page.current_path().map(str::to_string)),
        Some("README.md".to_string())
    );

    page.update(cx, |page, cx| page.select_tab(RepositoryTab::Code, cx));
    frames(cx);
    assert!(cx.debug_bounds("repository-tree-column").is_some());
    assert!(page.read_with(cx, |page, _| page.reader().is_some()));
}

#[gpui::test]
fn a_directory_failure_is_scoped_to_that_row_and_one_retry_recovers(cx: &mut TestAppContext) {
    let fixture = Fixture::new(Scene::DirectoryFailure);
    fixture.login(1);
    let (page, cx) = open(cx, &fixture, RepositoryNavigation::default());
    activate(&page, cx);
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });

    page.update(cx, |page, cx| page.toggle_directory("docs", cx));
    settle(&page, cx, |page| page.tree_failure("docs").is_some());
    page.read_with(cx, |page, _| {
        let error = page.tree_failure("docs").expect("scoped failure");
        assert!(
            error.title.contains("unexpected status") || error.title.contains("failed"),
            "a scripted server failure is not dressed as a permission answer: {error:?}"
        );
        assert!(error.retryable);
        // Everything else stays usable and visible.
        assert!(page.tree("").is_some());
        assert!(page.details().is_some());
        assert_eq!(page.current_path(), Some("README.md"));
        assert!(page.reader().is_some());
        let row = page
            .rows()
            .iter()
            .find(|row| row.path == "docs")
            .expect("the docs row");
        assert!(matches!(row.state, RowState::Failed(_)));
    });
    assert!(
        page.read_with(cx, |page, _| page.error().is_none()),
        "a directory failure is not a page-level error"
    );

    page.update(cx, |page, cx| page.retry_directory("docs", cx));
    settle(&page, cx, |page| page.tree("docs").is_some());
    assert!(
        rows(&page, cx)
            .iter()
            .any(|row| row.path == "docs/guide.md")
    );
    assert!(page.read_with(cx, |page, _| page.tree_failure("docs").is_none()));
}

#[gpui::test]
fn a_file_failure_replaces_only_the_reader_and_one_retry_recovers(cx: &mut TestAppContext) {
    let fixture = Fixture::new(Scene::FileFailure);
    fixture.login(1);
    let (page, cx) = open(cx, &fixture, RepositoryNavigation::default());
    activate(&page, cx);
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    page.update(cx, |page, cx| page.toggle_directory("src", cx));
    settle(&page, cx, |page| page.tree("src").is_some());

    page.update(cx, |page, cx| page.select_file("src/main.rs", cx));
    settle(&page, cx, |page| {
        matches!(page.reader_status(), ReaderStatus::ContentFailed(_))
    });
    page.read_with(cx, |page, _| {
        assert!(page.reader().is_none(), "the failed document is not shown");
        // The other layers survive: summary, branch, tree and its rows.
        assert!(page.details().is_some());
        assert!(page.fixed_commit().is_some());
        assert!(page.tree("src").is_some());
        assert!(page.error().is_none(), "not a page-level failure");
    });

    page.update(cx, |page, cx| page.retry_reader(cx));
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    let reader = page
        .read_with(cx, |page, _| page.reader().cloned())
        .unwrap();
    reader.read_with(cx, |reader, _| {
        assert_eq!(reader.context().path(), "src/main.rs");
    });
}

#[gpui::test]
fn a_missing_readme_keeps_the_tree_usable_and_offers_no_fake_entry(cx: &mut TestAppContext) {
    let fixture = Fixture::new(Scene::NoReadme);
    fixture.login(1);
    let (page, cx) = open(cx, &fixture, RepositoryNavigation::default());
    activate(&page, cx);
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::NoReadme
    });
    page.read_with(cx, |page, _| {
        assert_eq!(page.readme_state(), Some(&ReadmeState::Absent));
        assert!(page.reader().is_none(), "no document is invented");
        assert!(page.readme_path().is_none());
        assert!(!page.can_return_to_readme());
        assert_eq!(page.error(), None, "a proven absence is not an error");
    });
    // "Absent" is only provable on a complete, empty tree, so the tree area is
    // still rendered and usable — it just has no rows to open.
    frames(cx);
    assert!(cx.debug_bounds("repository-no-readme").is_some());
    assert!(cx.debug_bounds("repository-tree-column").is_some());
    assert!(page.read_with(cx, |page, _| page.rows().is_empty()));
    assert_eq!(
        page.read_with(cx, |page, _| page.reader_status().tag()),
        "no-readme"
    );
}

#[gpui::test]
fn an_uncertain_readme_is_distinct_from_a_proven_absence(cx: &mut TestAppContext) {
    let fixture = Fixture::new(Scene::ReadmeUncertain);
    fixture.login(1);
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
    assert!(cx.debug_bounds("repository-tree-column").is_some());
}

#[gpui::test]
fn an_empty_git_repository_is_not_shown_as_an_empty_directory(cx: &mut TestAppContext) {
    let fixture = Fixture::new(Scene::EmptyRepository);
    fixture.login(1);
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
fn a_complete_empty_branch_list_offers_no_commit(cx: &mut TestAppContext) {
    let fixture = Fixture::new(Scene::NoBranches);
    fixture.login(1);
    let (page, cx) = open(cx, &fixture, RepositoryNavigation::default());
    activate(&page, cx);
    settle(&page, cx, |page| page.error().is_some());
    let error = page.read_with(cx, |page, _| page.error().cloned().unwrap());
    assert!(error.title.contains("No branch"));
    assert!(page.read_with(cx, |page, _| page.selected_branch().is_none()));
    assert!(page.read_with(cx, |page, _| page.rows().is_empty()));
}

#[gpui::test]
fn a_truncated_tree_is_never_reported_as_complete(cx: &mut TestAppContext) {
    let fixture = Fixture::new(Scene::TruncatedTree);
    fixture.login(1);
    let (page, cx) = open(cx, &fixture, RepositoryNavigation::default());
    activate(&page, cx);
    settle(&page, cx, |page| page.tree("").is_some());
    let tree = page.read_with(cx, |page, _| page.tree("").cloned().unwrap());
    assert_eq!(tree.truncated, Some(true));
    assert!(super::navigation::tree_incomplete(&tree));
    // Unknown Git types are rows that are not opened as text.
    assert_eq!(
        page.read_with(cx, |page, _| {
            page.rows()
                .iter()
                .find(|row| row.path == "odd-entry")
                .map(|row| row.kind)
        }),
        Some(TreeRowKind::Unknown)
    );
    frames(cx);
    assert!(cx.debug_bounds("repository-tree-incomplete").is_some());
}

#[gpui::test]
fn the_long_tree_stays_bounded_and_scrolls_only_its_own_region(cx: &mut TestAppContext) {
    let fixture = Fixture::new(Scene::LongTree);
    fixture.login(1);
    let (page, cx) = open(cx, &fixture, RepositoryNavigation::default());
    activate(&page, cx);
    settle(&page, cx, |page| page.tree("").is_some());
    assert_eq!(rows(&page, cx).len(), LONG_TREE_FILES);
    frames(cx);
    assert!(cx.debug_bounds("repository-row-file-0000.txt").is_some());
    assert!(
        cx.debug_bounds("repository-row-file-0300.txt").is_none(),
        "off-screen rows are not built"
    );

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
}

#[gpui::test]
fn resetting_to_normal_after_each_scene_restores_full_browsing(cx: &mut TestAppContext) {
    let fixture = Fixture::new(Scene::DirectoryFailure);
    fixture.login(1);
    let (page, cx) = open(cx, &fixture, RepositoryNavigation::default());
    activate(&page, cx);
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    page.update(cx, |page, cx| page.toggle_directory("docs", cx));
    settle(&page, cx, |page| page.tree_failure("docs").is_some());

    // A scene change plus a fresh activation must start clean.
    fixture.switch_scene(Scene::Normal);
    activate(&page, cx);
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    assert!(page.read_with(cx, |page, _| page.tree_failure("docs").is_none()));
    // Activation restored the saved expansion, so docs loads again and succeeds.
    settle(&page, cx, |page| page.tree("docs").is_some());
    assert!(
        rows(&page, cx)
            .iter()
            .any(|row| row.path == "docs/guide.md")
    );
    assert!(
        fixture.transport.mismatches().is_empty(),
        "{:?}",
        fixture.transport.mismatches()
    );
}

#[gpui::test]
fn the_reader_offers_a_back_to_readme_entry_only_when_it_is_useful(cx: &mut TestAppContext) {
    let (_fixture, page, cx) = normal(cx);
    assert!(
        !page.read_with(cx, |page, _| page.can_return_to_readme()),
        "already showing the README: no duplicate entry"
    );
    page.update(cx, |page, cx| page.toggle_directory("docs", cx));
    settle(&page, cx, |page| page.tree("docs").is_some());
    page.update(cx, |page, cx| page.select_file("docs/guide.md", cx));
    settle(&page, cx, |page| {
        page.selection() == &Selection::File("docs/guide.md".into())
            && page.reader_status() == &ReaderStatus::Showing
    });
    page.read_with(cx, |page, _| {
        assert_eq!(page.selection(), &Selection::File("docs/guide.md".into()));
        assert!(
            page.can_return_to_readme(),
            "a different file is shown and a README exists: selection={:?} readme={:?}",
            page.selection(),
            page.readme_state().and_then(|state| state.path())
        );
    });
    page.update(cx, |page, cx| page.show_readme(cx));
    settle(&page, cx, |page| {
        page.selection() == &Selection::Readme && page.reader_status() == &ReaderStatus::Showing
    });
    assert_eq!(
        page.read_with(cx, |page, _| page.current_path().map(str::to_string)),
        Some("README.md".to_string())
    );
}

#[gpui::test]
fn a_slow_older_request_never_replaces_a_newer_selection(cx: &mut TestAppContext) {
    let (_fixture, page, cx) = normal(cx);
    page.update(cx, |page, cx| page.toggle_directory("docs", cx));
    settle(&page, cx, |page| page.tree("docs").is_some());
    page.update(cx, |page, cx| page.toggle_directory("src", cx));
    settle(&page, cx, |page| page.tree("src").is_some());

    // Ask for guide, then immediately for main.rs: the second must win even if
    // the first lands later.
    page.update(cx, |page, cx| page.select_file("docs/guide.md", cx));
    page.update(cx, |page, cx| page.select_file("src/main.rs", cx));
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
            && page.selection() == &Selection::File("src/main.rs".into())
    });
    for _ in 0..30 {
        page.update_in(cx, |page, window, cx| page.drain(window, cx));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        page.read_with(cx, |page, _| page.selection().clone()),
        Selection::File("src/main.rs".into())
    );
    let reader = page
        .read_with(cx, |page, _| page.reader().cloned())
        .unwrap();
    reader.read_with(cx, |reader, _| {
        assert_eq!(reader.context().path(), "src/main.rs");
    });
}

#[gpui::test]
fn an_authorization_change_clears_data_and_a_later_return_to_the_same_account_does_not_restore_it(
    cx: &mut TestAppContext,
) {
    let (fixture, page, cx) = normal(cx);
    assert!(page.read_with(cx, |page, _| page.details().is_some()));

    // The host signs in a second account while the page is showing data.
    fixture.login_as(8, "second-user");
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
    // Switch back to the first synthetic account (id 1) after the second one.
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
fn a_deactivated_page_ignores_work_that_lands_afterwards(cx: &mut TestAppContext) {
    let fixture = Fixture::new(Scene::Normal);
    fixture.login(1);
    let (page, cx) = open(cx, &fixture, RepositoryNavigation::default());
    activate(&page, cx);
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });
    let commit_before = page.read_with(cx, |page, _| {
        page.fixed_commit().map(|sha| sha.as_str().to_string())
    });

    // Deactivate while a directory read is in flight; its result must not land.
    page.update(cx, |page, cx| page.toggle_directory("docs", cx));
    page.update(cx, |page, cx| page.deactivate(cx));
    for _ in 0..40 {
        page.update_in(cx, |page, window, cx| page.drain(window, cx));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(2));
    }
    page.read_with(cx, |page, _| {
        assert!(!page.is_active());
        assert!(
            page.fixed_commit().map(|sha| sha.as_str().to_string()) == commit_before,
            "a deactivated page keeps no new publication"
        );
        assert!(
            page.tree("docs").is_none(),
            "the in-flight directory read was discarded"
        );
    });
}

#[gpui::test]
fn rebinding_keeps_navigation_intent_but_reads_under_new_authorization(cx: &mut TestAppContext) {
    let fixture = Fixture::new(Scene::Normal);
    fixture.login(1);
    let intent = RepositoryNavigation {
        branch: Some("main".into()),
        path: Some("src/main.rs".into()),
        expanded: ["src".to_string()].into_iter().collect(),
    };
    let (page, cx) = open(cx, &fixture, intent.clone());
    activate(&page, cx);
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
            && page.selection() == &Selection::File("src/main.rs".into())
    });
    let old_requests = fixture.transport.recorded().len();

    // A second account is signed in, and the host rebinds the page to it.
    fixture.login_as(8, "second-user");
    let fresh = fixture.services();
    page.update(cx, |page, cx| page.rebind(fresh, cx));
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
            && page.selection() == &Selection::File("src/main.rs".into())
    });
    page.read_with(cx, |page, _| {
        assert_eq!(page.navigation(), intent, "intent survives the rebind");
        let context = page.context().expect("a new session");
        assert_eq!(
            context.auth().account.id,
            8,
            "a session for the new account"
        );
    });
    assert!(
        fixture.transport.recorded().len() > old_requests,
        "the rebind re-read everything instead of reusing old data"
    );
}

/// Two pages in one window, as two Shell tabs would be.
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
fn two_pages_keep_independent_branch_and_path_state(cx: &mut TestAppContext) {
    let fixture = Fixture::new(Scene::Normal);
    fixture.login(1);
    setup(cx);
    let services = fixture.services();
    let locator = locator();
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
    for _ in 0..600 {
        pump(&first, &second, cx);
        if first.read_with(cx, |page, _| page.reader_status() == &ReaderStatus::Showing)
            && second.read_with(cx, |page, _| page.reader_status() == &ReaderStatus::Showing)
        {
            ready = true;
            break;
        }
    }
    assert!(ready, "both pages loaded their own README");

    second.update(cx, |page, cx| page.select_branch("feature/docs-polish", cx));
    let mut switched = false;
    for _ in 0..400 {
        pump(&first, &second, cx);
        if second.read_with(cx, |page, _| {
            page.selected_branch() == Some("feature/docs-polish")
        }) && second.read_with(cx, |page, _| page.reader_status() == &ReaderStatus::Showing)
        {
            switched = true;
            break;
        }
    }
    assert!(switched, "the second page reached the other branch");

    first.read_with(cx, |page, _| {
        assert_eq!(page.selected_branch(), Some("main"));
        assert_eq!(
            page.fixed_commit().map(|sha| sha.as_str()),
            Some("c0ffee0000000000000000000000000000000001")
        );
        assert!(page.expanded().is_empty());
    });
    second.read_with(cx, |page, _| {
        assert_eq!(page.selected_branch(), Some("feature/docs-polish"));
        assert_eq!(
            page.fixed_commit().map(|sha| sha.as_str()),
            Some("c0ffee0000000000000000000000000000000002")
        );
    });
}

#[gpui::test]
fn the_tree_cap_is_stated_rather_than_silently_dropped(cx: &mut TestAppContext) {
    let _ = cx;
    // The cap lives in the flattened model, so it can be driven directly.
    let mut model = TreeModel::default();
    let identity = RepositoryIdentity {
        id: 1,
        locator: RepositoryLocator::new("octo", "demo").unwrap(),
    };
    let snapshot = RepoSnapshot {
        repository: identity,
        requested: RevisionSpec::Commit(ObjectSha::new("a".repeat(40)).unwrap()),
        commit: ObjectSha::new("a".repeat(40)).unwrap(),
        root_tree: ObjectSha::new("b".repeat(40)).unwrap(),
        message: "fixture".into(),
        author: None,
        committer: None,
    };
    let entries: Vec<TreeEntry> = (0..MAX_TREE_ROWS + 50)
        .map(|index| TreeEntry {
            path: crate::domain::repository::RepositoryPath::new(format!("f{index}")).unwrap(),
            sha: ObjectSha::new("c".repeat(40)).unwrap(),
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
fn a_partial_branch_list_is_reported_as_partial(cx: &mut TestAppContext) {
    let _ = cx;
    let completeness = Completeness::Partial {
        reasons: vec![IncompleteReason::ServerTruncated],
    };
    assert!(super::navigation::incompleteness_note(&completeness).is_some());
    assert!(super::navigation::incompleteness_note(&Completeness::Complete).is_none());
    let error = RepositoryError::GitHub(GitHubError::NotFound);
    let page_error = super::navigation::PageError::from_repository(&error);
    assert!(!page_error.title.contains("not visible"));
}

#[gpui::test]
fn an_already_shown_file_survives_an_unrelated_directory_load(cx: &mut TestAppContext) {
    let (fixture, page, cx) = normal(cx);
    page.update(cx, |page, cx| page.toggle_directory("src", cx));
    settle(&page, cx, |page| page.tree("src").is_some());
    page.update(cx, |page, cx| page.select_file("src/main.rs", cx));
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
            && page.selection() == &Selection::File("src/main.rs".into())
    });
    let reader = page
        .read_with(cx, |page, _| page.reader().cloned())
        .unwrap();
    let blobs_before = fixture.transport.count("/git/blobs/");

    // Expanding an unrelated directory must not reload the shown file.
    page.update(cx, |page, cx| page.toggle_directory("docs", cx));
    settle(&page, cx, |page| page.tree("docs").is_some());
    for _ in 0..40 {
        page.update_in(cx, |page, window, cx| page.drain(window, cx));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        page.read_with(cx, |page, _| page.reader().cloned().map(|r| r.entity_id())),
        Some(reader.entity_id()),
        "the same reader entity is still mounted"
    );
    assert_eq!(
        page.read_with(cx, |page, _| page.reader_status().clone()),
        ReaderStatus::Showing,
        "the reader never fell back to Loading"
    );
    assert_eq!(
        fixture.transport.count("/git/blobs/"),
        blobs_before,
        "the already shown blob was not downloaded again"
    );

    // A failing unrelated directory leaves it alone too.
    fixture.switch_scene(Scene::DirectoryFailure);
    page.update(cx, |page, cx| page.toggle_directory("assets", cx));
    settle(&page, cx, |page| page.tree("assets").is_some());
    assert_eq!(
        page.read_with(cx, |page, _| page.reader().cloned().map(|r| r.entity_id())),
        Some(reader.entity_id())
    );
}

#[gpui::test]
fn a_superseded_branch_resolution_is_rejected_even_for_the_same_branch_name(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::new(Scene::Normal);
    fixture.login(1);
    let (page, cx) = open(cx, &fixture, RepositoryNavigation::default());
    activate(&page, cx);
    settle(&page, cx, |page| {
        page.reader_status() == &ReaderStatus::Showing
    });

    // A → B → A: the first A reply must be rejected once a newer resolution
    // exists, even though both name the same branch.
    page.update(cx, |page, cx| page.select_branch("feature/docs-polish", cx));
    page.update(cx, |page, cx| page.select_branch("main", cx));
    settle(&page, cx, |page| {
        page.selected_branch() == Some("main") && page.reader_status() == &ReaderStatus::Showing
    });
    let accepted = page.read_with(cx, |page, _| {
        page.fixed_commit().map(|sha| sha.as_str().to_string())
    });
    assert_eq!(
        accepted.as_deref(),
        Some("c0ffee0000000000000000000000000000000001"),
        "the current main resolution is the one shown"
    );
    // Late replies from the superseded requests cannot change it.
    for _ in 0..40 {
        page.update_in(cx, |page, window, cx| page.drain(window, cx));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        page.read_with(cx, |page, _| page
            .fixed_commit()
            .map(|sha| sha.as_str().to_string()))
            .as_deref(),
        Some("c0ffee0000000000000000000000000000000001")
    );
    assert_eq!(
        page.read_with(cx, |page, _| page.selected_branch().map(str::to_string))
            .as_deref(),
        Some("main")
    );
    assert!(page.read_with(cx, |page, _| page.error().is_none()));
}

#[gpui::test]
fn a_missing_ancestor_directory_ends_in_a_terminal_state_not_endless_loading(
    cx: &mut TestAppContext,
) {
    let (fixture, page, cx) = normal(cx);
    // `assets` *is* loaded here, so its entries are known: a nonexistent child
    // can never produce a response.
    page.update(cx, |page, cx| page.toggle_directory("assets", cx));
    settle(&page, cx, |page| page.tree("assets").is_some());
    let blobs_before = fixture.transport.count("/git/blobs/");

    page.update(cx, |page, cx| {
        page.select_file("assets/missing/file.md", cx)
    });
    for _ in 0..40 {
        page.update_in(cx, |page, window, cx| page.drain(window, cx));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(2));
    }
    let status = page.read_with(cx, |page, _| page.reader_status().clone());
    assert!(
        matches!(
            status,
            ReaderStatus::ContentFailed(_) | ReaderStatus::SelectionUnavailable { .. }
        ),
        "a nonexistent ancestor produces a terminal status, got {status:?}"
    );
    assert_eq!(
        fixture.transport.count("/git/blobs/"),
        blobs_before,
        "nothing was downloaded for a path that cannot exist"
    );
}

#[gpui::test]
fn activation_clears_previous_data_even_when_the_session_cannot_be_acquired(
    cx: &mut TestAppContext,
) {
    let (fixture, page, cx) = normal(cx);
    assert!(page.read_with(cx, |page, _| page.details().is_some()));
    // Remove the only account, then activate: `session()` fails.
    fixture._runtime.block_on(async {
        fixture
            .accounts
            .remove_saved(crate::domain::AccountKey::new(
                1,
                crate::domain::HostId::github(),
            ))
            .await
            .expect("remove the account");
    });
    page.update(cx, |page, cx| page.deactivate(cx));
    activate(&page, cx);
    frames(cx);
    page.read_with(cx, |page, _| {
        assert!(!page.is_active());
        assert!(page.error().is_some(), "the failure is reported");
        assert!(
            page.details().is_none(),
            "the previous account's summary is gone"
        );
        assert!(
            page.selected_branch().is_none(),
            "no branch from the previous session"
        );
        assert!(page.rows().is_empty());
        assert!(page.reader().is_none());
    });
    assert!(
        cx.debug_bounds("repository-body").is_none()
            || cx.debug_bounds("repository-tree-column").is_none(),
        "old content is not still drawn"
    );
}

/// Keeps the `TreeEntryKind` import used even when no test names it directly.
fn _entry_kind(kind: &TreeEntryKind) -> bool {
    matches!(kind, TreeEntryKind::File { .. })
}

/// Click the right-hand blank part of a real row, beyond the label.
fn click_row(page: &Entity<RepositoryPage>, cx: &mut VisualTestContext, selector: &'static str) {
    frames(cx);
    let bounds = cx.debug_bounds(selector).expect(selector);
    cx.simulate_click(
        gpui::point(bounds.right() - px(12.0), bounds.center().y),
        Modifiers::none(),
    );
    page.update_in(cx, |page, window, cx| page.drain(window, cx));
}

#[gpui::test]
fn normal_fixture_mouse_and_keyboard_events_browse_repeatably(cx: &mut TestAppContext) {
    let (fixture, page, cx) = normal(cx);
    cx.simulate_resize(gpui::size(px(1000.0), px(700.0)));
    frames(cx);
    let column = cx.debug_bounds("repository-tree-column").unwrap();
    let folder = cx.debug_bounds("repository-row-docs").unwrap();
    assert!(
        folder.size.width > column.size.width - px(4.0),
        "full-width row hit area"
    );
    let chevron = cx.debug_bounds("repository-chevron-docs").unwrap();
    cx.simulate_click(chevron.center(), Modifiers::none());
    settle(&page, cx, |p| p.tree("docs").is_some());
    assert!(
        page.read_with(cx, |p, _| p.expanded().contains("docs")),
        "one click expands once"
    );
    click_row(&page, cx, "repository-row-docs");
    assert!(!page.read_with(cx, |p, _| p.expanded().contains("docs")));
    click_row(&page, cx, "repository-row-docs");
    click_row(&page, cx, "repository-row-docs/guide.md");
    settle(&page, cx, |p| {
        p.reader_status() == &ReaderStatus::Showing && p.current_path() == Some("docs/guide.md")
    });
    click_row(&page, cx, "repository-row-src");
    settle(&page, cx, |p| p.tree("src").is_some());
    for _ in 0..3 {
        click_row(&page, cx, "repository-row-src/main.rs");
        settle(&page, cx, |p| {
            p.reader_status() == &ReaderStatus::Showing && p.current_path() == Some("src/main.rs")
        });
    }
    // Real keyboard dispatch reaches the focused tree viewport.
    cx.simulate_keystrokes("left left right right enter");
    settle(&page, cx, |p| p.reader_status() == &ReaderStatus::Showing);
    click_row(&page, cx, "repository-row-README.md");
    settle(&page, cx, |p| {
        p.reader_status() == &ReaderStatus::Showing && p.current_path() == Some("README.md")
    });
    assert!(!page.read_with(cx, |p, _| p.can_return_to_readme()));
    assert!(fixture.transport.mismatches().is_empty());
}

#[gpui::test]
fn nested_preferred_readme_selects_its_real_node(cx: &mut TestAppContext) {
    let fixture = Fixture::new(Scene::ReadmeInDocs);
    fixture.login(1);
    let (page, cx) = open(cx, &fixture, RepositoryNavigation::default());
    activate(&page, cx);
    settle(&page, cx, |p| {
        p.reader_status() == &ReaderStatus::Showing && p.tree("docs").is_some()
    });
    assert_eq!(
        page.read_with(cx, |p, _| p.current_path().map(str::to_string)),
        Some("docs/README.md".into())
    );
    assert!(rows(&page, cx).iter().any(|r| r.path == "docs/README.md"));
    click_row(&page, cx, "repository-row-docs/README.md");
    settle(&page, cx, |p| p.reader_status() == &ReaderStatus::Showing);
    assert!(!page.read_with(cx, |p, _| p.can_return_to_readme()));
}

#[gpui::test]
fn empty_binary_and_large_files_use_the_real_reader_states(cx: &mut TestAppContext) {
    let (fixture, page, cx) = normal(cx);
    click_row(&page, cx, "repository-row-assets");
    settle(&page, cx, |p| p.tree("assets").is_some());
    for path in ["assets/empty.txt", "assets/logo.bin", "assets/huge.log"] {
        page.update(cx, |p, cx| p.select_file(path, cx));
        settle(&page, cx, |p| p.reader_status() == &ReaderStatus::Showing);
        page.read_with(cx, |p, cx| {
            assert!(p.reader().unwrap().read(cx).is_status(), "{path}")
        });
    }
    assert_eq!(
        fixture
            .transport
            .count("/git/blobs/b000000000000000000000000000000000000007"),
        0
    );
    page.update(cx, |p, cx| p.toggle_directory("assets/empty-folder", cx));
    settle(&page, cx, |p| p.tree("assets/empty-folder").is_some());
    assert!(matches!(
        rows(&page, cx)
            .iter()
            .find(|r| r.path == "assets/empty-folder")
            .unwrap()
            .state,
        RowState::Empty
    ));
}

#[gpui::test]
fn root_retry_uses_the_snapshot_and_child_partial_is_never_empty(cx: &mut TestAppContext) {
    let fixture = Fixture::new(Scene::RootFailure);
    fixture.login(1);
    let (page, cx) = open(cx, &fixture, RepositoryNavigation::default());
    activate(&page, cx);
    settle(&page, cx, |p| {
        p.tree_failure("").is_some() && p.reader_status() == &ReaderStatus::Showing
    });
    let reader = page.read_with(cx, |p, _| p.reader().cloned()).unwrap();
    page.update(cx, |p, cx| p.retry_directory("", cx));
    settle(&page, cx, |p| p.tree("").is_some());
    assert_eq!(
        reader.entity_id(),
        page.read_with(cx, |p, _| p.reader().unwrap().entity_id())
    );
    assert!(page.read_with(cx, |p, _| p.tree_failure("").is_none()));
    fixture.switch_scene(Scene::TruncatedTree);
    page.update(cx, |p, cx| p.toggle_directory("docs", cx));
    settle(&page, cx, |p| p.tree("docs").is_some());
    assert!(matches!(
        rows(&page, cx)
            .iter()
            .find(|r| r.path == "docs")
            .unwrap()
            .state,
        RowState::Incomplete
    ));
}

#[gpui::test]
fn superseded_same_file_success_and_failure_cannot_publish(cx: &mut TestAppContext) {
    let (_fixture, page, cx) = normal(cx);
    page.update(cx, |p, cx| p.toggle_directory("src", cx));
    settle(&page, cx, |p| p.tree("src").is_some());
    page.update(cx, |p, cx| p.select_file("src/main.rs", cx));
    let old = page.read_with(cx, |p, _| p.selection_request);
    page.update(cx, |p, cx| p.show_readme(cx));
    page.update(cx, |p, cx| p.select_file("src/main.rs", cx));
    settle(&page, cx, |p| {
        p.reader_status() == &ReaderStatus::Showing && p.current_path() == Some("src/main.rs")
    });
    let reader = page.read_with(cx, |p, _| p.reader().cloned()).unwrap();
    page.update_in(cx, |p, window, cx| {
        let branch = p.branch.as_ref().unwrap();
        let reply = super::ContentReply {
            branch: branch.name.clone(),
            commit: branch.commit().clone(),
            role: super::Role::File("src/main.rs".into()),
            selection_request: old,
            path: "src/main.rs".into(),
            result: Err(GitHubError::Unauthorized.into()),
        };
        p.apply_content(reply, window, cx);
    });
    assert_eq!(
        page.read_with(cx, |p, _| p.reader_status().clone()),
        ReaderStatus::Showing
    );
    assert_eq!(
        reader.entity_id(),
        page.read_with(cx, |p, _| p.reader().unwrap().entity_id())
    );
}

#[gpui::test]
fn old_branch_identity_rejects_success_and_error_after_returning_to_same_name(
    cx: &mut TestAppContext,
) {
    let (_fixture, page, cx) = normal(cx);
    let (snapshot, old) = page.read_with(cx, |p, _| {
        (
            p.branch.as_ref().unwrap().snapshot.clone(),
            p.branch_request,
        )
    });
    page.update(cx, |p, cx| p.select_branch("feature/docs-polish", cx));
    page.update(cx, |p, cx| p.select_branch("main", cx));
    settle(&page, cx, |p| {
        p.reader_status() == &ReaderStatus::Showing && p.selected_branch() == Some("main")
    });
    let reader = page.read_with(cx, |p, _| p.reader().unwrap().entity_id());
    page.update(cx, |p, cx| {
        p.apply_revision("main".into(), old, Ok(snapshot), cx);
        p.apply_revision(
            "main".into(),
            old,
            Err(GitHubError::Unauthorized.into()),
            cx,
        );
        assert_eq!(p.reader().unwrap().entity_id(), reader);
        assert!(p.error().is_none());
    });
}

#[gpui::test]
fn enumeration_failure_preserves_saved_branch_and_recovery_preserves_reader(
    cx: &mut TestAppContext,
) {
    let (fixture, page, cx) = normal(cx);
    let listing = page.read_with(cx, |p, _| p.branches().cloned()).unwrap();
    page.update(cx, |p, cx| {
        p.intent.branch = Some("feature/docs-polish".into());
        p.branch = None;
        p.apply_branches(Err(GitHubError::Timeout { seconds: 1 }.into()), cx);
    });
    settle(&page, cx, |p| {
        p.selected_branch() == Some("feature/docs-polish")
            && p.reader_status() == &ReaderStatus::Showing
    });
    let reader = page.read_with(cx, |p, _| p.reader().unwrap().entity_id());
    page.update(cx, |p, cx| p.apply_branches(Ok(listing), cx));
    assert_eq!(
        reader,
        page.read_with(cx, |p, _| p.reader().unwrap().entity_id())
    );
    assert!(fixture.transport.mismatches().is_empty());
}

#[gpui::test]
fn failed_file_stays_failed_when_an_unrelated_directory_finishes(cx: &mut TestAppContext) {
    let fixture = Fixture::new(Scene::FileFailure);
    fixture.login(1);
    let (page, cx) = open(cx, &fixture, RepositoryNavigation::default());
    activate(&page, cx);
    settle(&page, cx, |p| p.reader_status() == &ReaderStatus::Showing);
    page.update(cx, |p, cx| p.toggle_directory("src", cx));
    settle(&page, cx, |p| p.tree("src").is_some());
    page.update(cx, |p, cx| p.select_file("src/main.rs", cx));
    settle(&page, cx, |p| {
        matches!(p.reader_status(), ReaderStatus::ContentFailed(_))
    });
    let count = fixture.transport.count("/git/blobs/");
    page.update(cx, |p, cx| p.toggle_directory("assets", cx));
    settle(&page, cx, |p| p.tree("assets").is_some());
    assert_eq!(count, fixture.transport.count("/git/blobs/"));
    assert!(page.read_with(cx, |p, _| matches!(
        p.reader_status(),
        ReaderStatus::ContentFailed(_)
    )));
    page.update(cx, |p, cx| p.retry_reader(cx));
    settle(&page, cx, |p| p.reader_status() == &ReaderStatus::Showing);
}

#[gpui::test]
fn retries_do_not_overlap_enumeration_or_reset_successful_repository_data(cx: &mut TestAppContext) {
    let (fixture, page, cx) = normal(cx);
    let overview = page.read_with(cx, |p, _| p.overview().unwrap().entity_id());
    page.update(cx, |p, cx| {
        p.retry_branches(cx);
        p.retry_branches(cx);
    });
    settle(&page, cx, |p| !p.branches_loading);
    assert_eq!(
        fixture.transport.count("/branches?"),
        2,
        "initial plus one bounded retry"
    );
    page.update(cx, |p, cx| {
        p.branch = None;
        p.intent.branch = Some("main".into());
        p.error = Some(super::PageError::new("failed revision", None, true));
        p.retry(cx);
    });
    settle(&page, cx, |p| {
        p.reader_status() == &ReaderStatus::Showing && p.branch.is_some()
    });
    assert_eq!(
        overview,
        page.read_with(cx, |p, _| p.overview().unwrap().entity_id())
    );
    assert_eq!(fixture.transport.count("/releases/latest"), 1);
}
