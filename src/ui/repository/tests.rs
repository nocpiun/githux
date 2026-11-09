//! Behavioral tests for the NSH repository page controller.
//!
//! They drive the **real** data pipeline — `AccountService` → `RepoService` session →
//! repository/branches/revision/tree/README/blob → prepared reader content — over the same
//! deterministic fixture the page was developed against (`src/bin_support/repository_fixture.rs`),
//! a temporary SQLite directory, a real `CacheStore` and one synthetic PAT. No request leaves the
//! machine and no real credential is read.
//!
//! The controller is the UI-framework-free half of the page: these tests poll it directly instead
//! of through a window. The NSH view wiring has its own headless test (`headless_tests`), and the
//! limits of both are recorded in the task devlog.

#[path = "headless_tests.rs"]
mod headless_tests;
#[path = "offline_tests.rs"]
mod offline_tests;
#[path = "star_tests.rs"]
mod star_tests;

use std::sync::Arc;
use std::time::Duration;

use super::navigation::{RepositoryNavigation, RowState, TreeRowKind};
use super::{PageAction, ReaderStatus, RepositoryController, RepositoryTab, Selection, TreeRow};
use crate::app::runtime::{Backend, ExitGate, RuntimeHandle};
use crate::auth::InMemoryCredentialStore;
use crate::data::AccountService;
use crate::github::{GitHubClient, GitHubConfig};
use crate::network::executor::BackgroundExecutor;
use crate::network::transport::Transport;
use crate::repository_modules::fixture::{FixtureTransport, NAME, OWNER, Scene};
use crate::storage::accounts::AccountDatabaseConfig;
use crate::storage::cache::{CacheConfig, CacheStore};
use crate::workspace::{Route, Sessions};

// --------------------------------------------------------------- test setup --

/// One service stack over the shared fixture transport.
pub(super) struct Fixture {
    pub(super) transport: Arc<FixtureTransport>,
    pub(super) executor: Arc<BackgroundExecutor>,
    cache: Option<CacheStore>,
    credentials: Arc<InMemoryCredentialStore>,
    db_path: std::path::PathBuf,
    runtime: tokio::runtime::Runtime,
    _temp: tempfile::TempDir,
}

impl Fixture {
    pub(super) fn new(scene: Scene, cached: bool) -> Self {
        let temp = tempfile::tempdir().expect("temporary account directory");
        let executor = Arc::new(BackgroundExecutor::new(4));
        let transport = Arc::new(FixtureTransport::new(scene));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("host runtime");
        let cache = cached.then(|| {
            CacheStore::open(CacheConfig::at(temp.path().join("cache"))).expect("open cache")
        });
        Self {
            transport,
            executor,
            cache,
            credentials: Arc::new(InMemoryCredentialStore::new()),
            db_path: temp.path().join("accounts.sqlite3"),
            runtime,
            _temp: temp,
        }
    }

    /// Opens the real account service exactly like application startup, over the fixture transport.
    fn open_accounts(&self) -> AccountService {
        let transport = self.transport.clone();
        let executor = self.executor.clone();
        let credentials = self.credentials.clone();
        let cache = self.cache.clone();
        let db = self.db_path.clone();
        self.runtime.block_on(async move {
            let client = Arc::new(GitHubClient::new(
                GitHubConfig::default(),
                transport as Arc<dyn Transport>,
            ));
            match cache {
                Some(cache) => {
                    AccountService::open_with_cache(
                        AccountDatabaseConfig::at(db),
                        credentials,
                        client,
                        executor,
                        cache,
                    )
                    .await
                }
                None => {
                    AccountService::open(
                        AccountDatabaseConfig::at(db),
                        credentials,
                        client,
                        executor,
                    )
                    .await
                }
            }
            .expect("open account service")
        })
    }

    /// A fresh runtime handle with one signed-in synthetic account, as the shell assembles it.
    pub(super) fn handle(&self, id: u64) -> RuntimeHandle {
        let accounts = self.open_accounts();
        self.transport.set_identity(id, OWNER);
        self.runtime.block_on(async {
            accounts
                .login(crate::auth::token::Pat::from_str("synthetic-test-token"))
                .await
                .expect("synthetic login")
        });
        RuntimeHandle::from_backend(Backend::new(
            accounts,
            self.executor.clone(),
            ExitGate::default(),
        ))
    }

    /// Reopens the saved account over the same durable stores, as an application restart does.
    /// No login is issued: the credential and catalog already exist.
    pub(super) fn resume_handle(&self) -> RuntimeHandle {
        let accounts = self.open_accounts();
        assert!(
            !accounts.summaries().is_empty(),
            "the saved account must be restored from the durable catalog"
        );
        RuntimeHandle::from_backend(Backend::new(
            accounts,
            self.executor.clone(),
            ExitGate::default(),
        ))
    }

    /// Signs a *different* synthetic account in through the running handle's own service.
    pub(super) fn login_as(&self, handle: &RuntimeHandle, id: u64, login: &str) {
        self.transport.set_identity(id, login);
        let accounts = handle.services().expect("services").accounts().clone();
        self.runtime.block_on(async {
            accounts
                .login(crate::auth::token::Pat::from_str("synthetic-second-token"))
                .await
                .expect("second login")
        });
    }

    pub(super) fn switch_scene(&self, scene: Scene) {
        self.transport.set_scene(scene);
    }

    pub(super) fn cache(&self) -> CacheStore {
        self.cache.clone().expect("cached fixture")
    }

    pub(super) fn controller(
        &self,
        handle: &mut RuntimeHandle,
        intent: RepositoryNavigation,
    ) -> (RepositoryController, Sessions) {
        handle.snapshot();
        let tab = handle.sessions_mut().open(Route::Repository {
            namespace: OWNER.to_string(),
            name: NAME.to_string(),
        });
        let context = handle
            .services()
            .expect("services")
            .page_context(handle.sessions(), tab, None)
            .expect("page context");
        let sessions = handle.sessions().clone();
        let controller = RepositoryController::new(
            context,
            OWNER,
            NAME,
            crate::workspace::RepositoryNavigation {
                branch: intent.branch,
                file: intent.path,
            },
        );
        (controller, sessions)
    }
}

fn intent() -> RepositoryNavigation {
    RepositoryNavigation::default()
}

/// Drops a runtime handle and waits for its background teardown, so the next account service can
/// reopen the same SQLite/cache files (the real application does this at exit).
pub(super) fn close_handle(handle: RuntimeHandle) {
    let gate = handle.exit_gate();
    drop(handle);
    assert!(
        gate.wait(Duration::from_secs(10)),
        "background teardown finished within the exit gate"
    );
}

/// Polls the controller until `done`, giving the real background executor time to answer.
pub(super) fn pump(
    controller: &mut RepositoryController,
    sessions: &Sessions,
    mut done: impl FnMut(&RepositoryController) -> bool,
) {
    for _ in 0..3000 {
        controller.poll(sessions);
        if done(controller) {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!(
        "controller did not settle; error={:?} status={:?} branch={:?}",
        controller.error(),
        controller.reader_status(),
        controller.selected_branch()
    );
}

pub(super) fn settle_ready(controller: &mut RepositoryController, sessions: &Sessions) {
    pump(controller, sessions, |controller| {
        controller.reader_status() == &ReaderStatus::Showing
    });
}

pub(super) fn rows(controller: &mut RepositoryController) -> Vec<TreeRow> {
    controller.refresh_rows().as_ref().clone()
}

// -------------------------------------------------------------------- tests --

#[test]
fn the_normal_fixture_reaches_its_readme_at_a_real_path_and_single_selection() {
    let fixture = Fixture::new(Scene::Normal, false);
    let mut handle = fixture.handle(1);
    let (mut controller, sessions) = fixture.controller(&mut handle, intent());
    settle_ready(&mut controller, &sessions);

    let details = controller.details().expect("repository summary");
    assert_eq!(details.identity.locator().owner(), OWNER);
    assert_eq!(details.identity.locator().name(), NAME);
    assert_eq!(
        details.description.as_deref(),
        Some("A tiny double-entry ledger, kept small enough to browse.")
    );
    assert_eq!(controller.selected_branch(), Some("main"));
    let commit = controller.fixed_commit().expect("fixed commit");
    assert_eq!(&commit.as_str()[..7], "c0ffee0");
    assert_eq!(controller.tab(), RepositoryTab::Code);
    assert_eq!(controller.readme_path(), Some("README.md"));
    assert_eq!(controller.current_path(), Some("README.md"));
    assert!(!controller.can_return_to_readme());

    let rows = rows(&mut controller);
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
    assert_eq!(
        super::row_kind(&rows, "docs"),
        Some(TreeRowKind::Directory { expanded: false })
    );
    assert!(
        fixture.transport.mismatches().is_empty(),
        "{:?}",
        fixture.transport.mismatches()
    );
}

#[test]
fn nested_directories_load_once_and_reopening_the_same_file_works() {
    let fixture = Fixture::new(Scene::Normal, false);
    let mut handle = fixture.handle(1);
    let (mut controller, sessions) = fixture.controller(&mut handle, intent());
    settle_ready(&mut controller, &sessions);

    for (path, expect) in [
        ("docs", "docs/guide.md"),
        ("docs/guides", "docs/guides/nested"),
        ("docs/guides/nested", "docs/guides/nested/deep-note.md"),
    ] {
        controller.toggle_directory(path);
        pump(&mut controller, &sessions, |controller| {
            controller.tree(path).is_some()
        });
        assert!(
            rows(&mut controller).iter().any(|row| row.path == expect),
            "expanding {path} exposed {expect}"
        );
    }
    let tree_reads = fixture.transport.count("/git/trees/");
    for _ in 0..3 {
        controller.select_file("docs/guides/nested/deep-note.md");
        pump(&mut controller, &sessions, |controller| {
            controller.reader_status() == &ReaderStatus::Showing
                && controller.current_path() == Some("docs/guides/nested/deep-note.md")
        });
    }
    assert_eq!(
        fixture.transport.count("/git/trees/"),
        tree_reads,
        "re-opening a file must not re-download the tree"
    );
    // Re-opening the same document keeps a single reader mount.
    controller.select_file("docs/guides/nested/deep-note.md");
    pump(&mut controller, &sessions, |controller| {
        controller.current_path() == Some("docs/guides/nested/deep-note.md")
    });
    assert!(controller.can_return_to_readme());
    controller.show_readme();
    pump(&mut controller, &sessions, |controller| {
        controller.current_path() == Some("README.md")
    });
}

#[test]
fn branch_switches_resolve_real_differences_and_a_to_b_to_a_ignores_the_old_reply() {
    let fixture = Fixture::new(Scene::Normal, false);
    let mut handle = fixture.handle(1);
    let (mut controller, sessions) = fixture.controller(&mut handle, intent());
    settle_ready(&mut controller, &sessions);
    let main_commit = controller.fixed_commit().expect("main commit").clone();
    controller.select_branch("feature/docs-polish");
    pump(&mut controller, &sessions, |controller| {
        controller.selected_branch() == Some("feature/docs-polish")
            && controller.reader_status() == &ReaderStatus::Showing
    });
    let feature_commit = controller.fixed_commit().expect("feature commit").clone();
    assert_ne!(main_commit, feature_commit);
    assert!(
        !rows(&mut controller).is_empty(),
        "the feature branch resolves its own fixed-commit tree"
    );
    let feature_readme = controller.readme_path().map(str::to_string);
    assert!(
        feature_readme.is_some(),
        "the feature branch README is discovered"
    );

    // A → B → A: the newest resolution wins even though A's name repeats.
    controller.select_branch("main");
    controller.select_branch("feature/docs-polish");
    controller.select_branch("main");
    pump(&mut controller, &sessions, |controller| {
        controller.selected_branch() == Some("main")
            && controller.reader_status() == &ReaderStatus::Showing
    });
    assert_eq!(controller.fixed_commit(), Some(&main_commit));
}

#[test]
fn a_branch_switch_does_not_restart_the_repository_overview() {
    let fixture = Fixture::new(Scene::Normal, false);
    let mut handle = fixture.handle(1);
    let (mut controller, sessions) = fixture.controller(&mut handle, intent());
    pump(&mut controller, &sessions, |controller| {
        controller.overview().is_some_and(|overview| {
            overview.releases().value().is_some()
                && overview.contributors().value().is_some()
                && overview.languages().value().is_some()
        })
    });
    let releases_before = fixture.transport.count("/releases");
    let languages_before = fixture.transport.count("/languages");
    let branch_before = controller.selected_branch().map(str::to_string);
    controller.select_branch("feature/docs-polish");
    pump(&mut controller, &sessions, |controller| {
        controller.selected_branch() == Some("feature/docs-polish")
            && controller.reader_status() == &ReaderStatus::Showing
    });
    assert_ne!(branch_before.as_deref(), Some("feature/docs-polish"));
    assert_eq!(fixture.transport.count("/releases"), releases_before);
    assert_eq!(fixture.transport.count("/languages"), languages_before);
}

#[test]
fn readme_absence_uncertainty_empty_repository_and_empty_branch_list_stay_distinct() {
    for (scene, expect_status) in [
        (Scene::NoReadme, ReaderStatus::NoReadme),
        (Scene::ReadmeUncertain, ReaderStatus::ReadmeUncertain),
    ] {
        let fixture = Fixture::new(scene, false);
        let mut handle = fixture.handle(1);
        let (mut controller, sessions) = fixture.controller(&mut handle, intent());
        pump(&mut controller, &sessions, |controller| {
            controller.reader_status() == &expect_status
        });
    }

    let fixture = Fixture::new(Scene::EmptyRepository, false);
    let mut handle = fixture.handle(1);
    let (mut controller, sessions) = fixture.controller(&mut handle, intent());
    pump(&mut controller, &sessions, |controller| {
        controller.selected_branch().is_none() && controller.error().is_some()
    });
    assert!(
        controller
            .error()
            .expect("page error")
            .title
            .contains("no readable Git data")
    );

    let fixture = Fixture::new(Scene::NoBranches, false);
    let mut handle = fixture.handle(1);
    let (mut controller, sessions) = fixture.controller(&mut handle, intent());
    pump(&mut controller, &sessions, |controller| {
        controller.error().is_some() && controller.selected_branch().is_none()
    });
    let error = controller.error().expect("no branch error");
    assert!(
        error
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("empty branch list"))
    );
    assert!(
        fixture.transport.mismatches().is_empty(),
        "{:?}",
        fixture.transport.mismatches()
    );
}

#[test]
fn a_scoped_directory_failure_keeps_the_rest_of_the_page_and_retries_once() {
    let fixture = Fixture::new(Scene::DirectoryFailure, false);
    let mut handle = fixture.handle(1);
    let (mut controller, sessions) = fixture.controller(&mut handle, intent());
    settle_ready(&mut controller, &sessions);
    controller.toggle_directory("docs");
    pump(&mut controller, &sessions, |controller| {
        controller.tree_failure("docs").is_some()
    });
    // The reader keeps showing the README and the other rows stay usable.
    assert_eq!(controller.reader_status(), &ReaderStatus::Showing);
    assert_eq!(controller.current_path(), Some("README.md"));
    let visible = rows(&mut controller);
    assert!(visible.iter().any(|row| row.path == "src"));
    assert_eq!(
        super::row_state(&visible, "docs"),
        Some(RowState::Failed(
            controller.tree_failure("docs").expect("failure").clone()
        ))
    );
    controller.retry_directory("docs");
    pump(&mut controller, &sessions, |controller| {
        controller.tree("docs").is_some()
    });
    assert!(
        rows(&mut controller)
            .iter()
            .any(|row| row.path == "docs/guide.md")
    );
}

#[test]
fn a_confirmed_empty_root_is_not_a_failure_and_a_truncated_tree_says_so() {
    let fixture = Fixture::new(Scene::NoReadme, false);
    let mut handle = fixture.handle(1);
    let (mut controller, sessions) = fixture.controller(&mut handle, intent());
    pump(&mut controller, &sessions, |controller| {
        controller.tree("").is_some()
    });
    let root = controller.tree("").expect("root tree");
    assert!(
        root.entries.is_empty(),
        "a confirmed complete empty tree is a real (not failed) empty root"
    );
    let model = controller.view_model();
    assert!(model.empty_root, "the page states the commit has no files");
    assert!(model.root_failure.is_none());

    let fixture = Fixture::new(Scene::TruncatedTree, false);
    let mut handle = fixture.handle(1);
    let (mut controller, sessions) = fixture.controller(&mut handle, intent());
    pump(&mut controller, &sessions, |controller| {
        controller.tree("").is_some()
    });
    let model = controller.view_model();
    assert!(
        model.tree_incomplete,
        "a truncated listing is never a complete one"
    );
}

#[test]
fn switching_the_in_page_tab_keeps_the_code_intent_and_returns_to_the_same_file() {
    let fixture = Fixture::new(Scene::Normal, false);
    let mut handle = fixture.handle(1);
    let (mut controller, sessions) = fixture.controller(&mut handle, intent());
    settle_ready(&mut controller, &sessions);
    controller.select_file("docs/guide.md");
    pump(&mut controller, &sessions, |controller| {
        controller.current_path() == Some("docs/guide.md")
    });
    controller.select_tab(RepositoryTab::Issues);
    assert_eq!(controller.tab(), RepositoryTab::Issues);
    // Leaving Code drops the displayed document but keeps the navigation intent.
    let model = controller.view_model();
    assert_eq!(model.selection, Selection::File("docs/guide.md".into()));
    controller.select_tab(RepositoryTab::Code);
    pump(&mut controller, &sessions, |controller| {
        controller.tab() == RepositoryTab::Code
            && controller.current_path() == Some("docs/guide.md")
            && controller.reader_status() == &ReaderStatus::Showing
    });
}

#[test]
fn an_authorization_change_clears_every_displayed_value_and_the_queued_reply() {
    let fixture = Fixture::new(Scene::Delayed, false);
    let mut handle = fixture.handle(1);
    let (mut controller, sessions) = fixture.controller(&mut handle, intent());
    settle_ready(&mut controller, &sessions);

    // Start a branch resolution, then sign another account in before it is drained.
    controller.select_branch("feature/docs-polish");
    fixture.login_as(&handle, 8, "new");
    controller.poll(&sessions);
    assert!(
        controller.authorization_lost(),
        "a different active account under the same page must invalidate it"
    );
    assert!(controller.details().is_none());
    assert!(controller.branch.is_none());
    assert!(controller.reader_status() != &ReaderStatus::Showing);
    assert!(
        controller
            .error()
            .is_some_and(|error| error.title.contains("authorization changed")),
        "{:?}",
        controller.error()
    );
    // Draining again must not restore the old page from a queued reply.
    controller.poll(&sessions);
    assert!(controller.details().is_none());
}

#[test]
fn a_reader_link_to_another_repository_is_reported_and_never_followed() {
    let fixture = Fixture::new(Scene::Normal, false);
    let mut handle = fixture.handle(1);
    let (mut controller, sessions) = fixture.controller(&mut handle, intent());
    settle_ready(&mut controller, &sessions);
    controller.reader_action(crate::ui::reader::ReaderAction::Link(
        crate::ui::reader::LinkTarget::Repository(crate::ui::reader::RepositoryLink {
            owner: "someone".into(),
            name: "elsewhere".into(),
            path: "README.md".into(),
            commit: controller
                .fixed_commit()
                .expect("commit")
                .as_str()
                .to_string(),
            anchor: None,
        }),
    ));
    match controller.take_action() {
        Some(PageAction::LinkNotFollowed { path, reason }) => {
            assert_eq!(path, "README.md");
            assert!(reason.contains("different repository"));
        }
        other => panic!("expected a refused link, got {other:?}"),
    }
    assert_eq!(controller.current_path(), Some("README.md"));
}

#[test]
fn overview_actions_are_revalidated_against_the_displayed_values() {
    let fixture = Fixture::new(Scene::Normal, false);
    let mut handle = fixture.handle(1);
    let (mut controller, sessions) = fixture.controller(&mut handle, intent());
    pump(&mut controller, &sessions, |controller| {
        controller.overview().is_some_and(|overview| {
            overview
                .latest_release()
                .is_some_and(|state| state.value().is_some())
        })
    });
    let overview = controller.overview().expect("overview").clone();
    let binding = overview.binding().clone();
    let release = overview
        .latest_release()
        .and_then(crate::ui::repository_overview::SectionState::value)
        .and_then(|latest| latest.release.clone())
        .expect("latest release");
    controller.overview_action(
        crate::ui::repository_overview::OverviewAction::OpenRelease {
            binding: binding.clone(),
            release_id: release.id,
            tag_name: release.tag_name.clone(),
            html_url: release.html_url.clone(),
        },
    );
    assert!(matches!(
        controller.take_action(),
        Some(PageAction::Overview(
            crate::ui::repository_overview::OverviewAction::OpenRelease { .. }
        ))
    ));
    // A stale action (different release id) is inert.
    controller.overview_action(
        crate::ui::repository_overview::OverviewAction::OpenRelease {
            binding,
            release_id: release.id.wrapping_add(1),
            tag_name: "other".into(),
            html_url: None,
        },
    );
    assert!(controller.take_action().is_none());
}

#[test]
fn tree_keyboard_navigation_moves_by_visible_rows_and_activates() {
    let fixture = Fixture::new(Scene::Normal, false);
    let mut handle = fixture.handle(1);
    let (mut controller, sessions) = fixture.controller(&mut handle, intent());
    settle_ready(&mut controller, &sessions);
    controller.focus_first_row();
    assert_eq!(controller.cursor(), Some("assets"));
    controller.move_cursor(1);
    assert_eq!(controller.cursor(), Some("docs"));
    controller.move_cursor(-1);
    assert_eq!(controller.cursor(), Some("assets"));
    controller.cursor_right();
    pump(&mut controller, &sessions, |controller| {
        !controller.expanded().is_empty()
    });
    assert!(controller.expanded().contains("assets"));
}
