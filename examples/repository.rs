//! Deterministic, headless data preview of the Native Shell repository page.
//!
//! Run with `cargo run --example repository` (Windows/macOS/Linux; no window, no network).
//!
//! This is development scaffolding, not the product page: the real visual preview is the Native
//! Shell application the integrator assembles. The example drives the **real** pipeline
//! (`AccountService` → `RepoService` session → summary/branches/revision/tree/README/blob →
//! prepared reader content → `RepositoryController`) over the same request-routed fixture the
//! suite uses, with a temporary SQLite directory, a real `CacheStore` and one synthetic PAT. It
//! prints what each scenario resolves so wiring regressions are visible without a GUI.
//!
//! The scenario list is the fixture's own; a scene the owner can inspect here is a scene the
//! tests assert. Statements are one per line and end with the terminal state of the page.

#[path = "../src/auth/mod.rs"]
mod auth;
#[path = "../src/content/mod.rs"]
mod content;
#[path = "../src/data/mod.rs"]
mod data;
#[path = "../src/domain/mod.rs"]
mod domain;
#[path = "../src/github/mod.rs"]
mod github;
#[path = "../src/network/mod.rs"]
mod network;
#[path = "../src/storage/mod.rs"]
mod storage;

pub mod app {
    #[path = "../../src/app/runtime/mod.rs"]
    mod runtime_impl;
    pub use runtime_impl as runtime;

    pub mod pages {
        use super::runtime::RuntimeHandle;
        use crate::workspace::{RepositoryNavigation, Route, TabId};
        use nsh_core::element::AnyElement;
        use std::rc::Rc;

        #[derive(Clone, Debug, PartialEq, Eq)]
        pub enum PageNavigation {
            Open(Route),
            Repository {
                route: Route,
                intent: RepositoryNavigation,
            },
            External(String),
            CanonicalProfile { tab: TabId, login: String },
        }

        pub type NavigationSink = Rc<dyn Fn(PageNavigation)>;

        pub trait PageView {
            fn element(&self) -> AnyElement;
            fn poll(&mut self, runtime: &RuntimeHandle);
            fn refresh(&mut self, runtime: &RuntimeHandle);
            fn set_active(&mut self, active: bool);
            fn retire(&mut self);
        }
    }
}

pub mod workspace {
    pub use crate::app::runtime::sessions::{
        PageIdentity, RepositoryNavigation, Route, RouteKey, Session, Sessions, TabId, Workspace,
    };
}

pub mod ui {
    pub mod foundation {
        #[path = "../../src/ui/foundation/mod.rs"]
        mod foundation_impl;
        pub use foundation_impl::*;
    }
    pub mod reader {
        #[path = "../../src/ui/reader/mod.rs"]
        mod reader_impl;
        pub use reader_impl::*;
    }
    pub mod repository {
        #[path = "../../src/ui/repository/mod.rs"]
        mod repository_impl;
        pub use repository_impl::*;
    }
    pub mod repository_overview {
        #[path = "../../src/ui/repository_overview/mod.rs"]
        mod repository_overview_impl;
        pub use repository_overview_impl::*;
    }
}

#[path = "../src/bin_support/repository_fixture.rs"]
mod repository_fixture;
use repository_fixture as fixture;

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::app::runtime::{Backend, ExitGate, RuntimeHandle};
use crate::auth::InMemoryCredentialStore;
use crate::data::AccountService;
use crate::github::{GitHubClient, GitHubConfig};
use crate::network::executor::BackgroundExecutor;
use crate::network::transport::Transport;
use crate::storage::accounts::AccountDatabaseConfig;
use crate::storage::cache::{CacheConfig, CacheStore};
use crate::ui::repository::{ReaderStatus, RepositoryController, RepositoryNavigation, RepositoryTab};
use crate::workspace::Route;
use fixture::{FixtureTransport, NAME, OWNER, Scene};

fn main() -> std::process::ExitCode {
    let temp = tempfile::tempdir().expect("temporary directory");
    let executor = Arc::new(BackgroundExecutor::new(4));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("preview runtime");
    let mut failures = 0usize;
    for scene in Scene::ALL {
        let transport = Arc::new(FixtureTransport::new(scene));
        let cache = CacheStore::open(CacheConfig::at(temp.path().join(format!("cache-{scene:?}"))))
            .expect("open preview cache");
        // Every scene starts from clean durable state so a failure in one cannot leak into another.
        let database = AccountDatabaseConfig::at(
            temp.path().join(format!("accounts-{scene:?}.sqlite3")),
        );
        let credentials = Arc::new(InMemoryCredentialStore::new());
        let accounts = {
            let transport = transport.clone();
            let executor = executor.clone();
            let cache = cache.clone();
            runtime.block_on(async move {
                let accounts = AccountService::open_with_cache(
                    database,
                    credentials,
                    Arc::new(GitHubClient::new(
                        GitHubConfig::default(),
                        transport as Arc<dyn Transport>,
                    )),
                    executor,
                    cache,
                )
                .await
                .expect("open preview account service");
                accounts
                    .login(auth::token::Pat::from_str("synthetic-preview-token"))
                    .await
                    .expect("preview login");
                accounts
            })
        };
        let mut handle = RuntimeHandle::from_backend(Backend::new(
            accounts,
            executor.clone(),
            ExitGate::default(),
        ));
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
        let mut controller = RepositoryController::new(
            context,
            OWNER,
            NAME,
            crate::workspace::RepositoryNavigation::default(),
        )
        .with_image_executor(executor.clone());

        let settled = wait_for_terminal(&mut controller, &sessions);
        let model = controller.view_model();
        println!(
            "{:<28} {:<10} branch={:<24} commit={:<8} rows={:<4} tab={:?} reader={} error={} cached={} star={} overview={}",
            scene.label(),
            if settled { "settled" } else { "TIMEOUT" },
            model.branch.clone().unwrap_or_else(|| "-".into()),
            model
                .commit
                .as_deref()
                .map(|commit| commit.chars().take(7).collect::<String>())
                .unwrap_or_else(|| "-".into()),
            model.rows.len(),
            model.tab,
            model.reader_status.tag(),
            model
                .page_error
                .as_ref()
                .map(|error| error.title.clone())
                .unwrap_or_else(|| "-".into()),
            model.cached_label.is_some(),
            model.star.label,
            model.overview.is_some(),
        );
        if !settled {
            failures += 1;
        }
        // An in-page tab switch keeps the Code intent and re-mounts it afterwards.
        controller.select_tab(RepositoryTab::Issues);
        let _ = controller.tab();
        controller.select_tab(RepositoryTab::Code);
        controller.retire();
        let gate = handle.exit_gate();
        drop(handle);
        if !gate.wait(Duration::from_secs(10)) {
            eprintln!("{scene:?}: background teardown did not finish\n");
            failures += 1;
        }
        if !transport.mismatches().is_empty() {
            eprintln!(
                "{scene:?}: fixture answered an unmodelled request: {:?}",
                transport.mismatches()
            );
            failures += 1;
        }
    }
    if failures == 0 {
        println!("\nAll scenarios resolved through the real service pipeline.");
        std::process::ExitCode::SUCCESS
    } else {
        eprintln!("\n{failures} scenario(s) did not resolve; see the lines above.");
        std::process::ExitCode::FAILURE
    }
}

/// A scenario is terminal when it either shows a document or states why it cannot, with no
/// request still in flight. A timeout is a wiring failure, not an empty result.
fn wait_for_terminal(controller: &mut RepositoryController, sessions: &crate::workspace::Sessions) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        controller.poll(sessions);
        let model = controller.view_model();
        let terminal = match &model.reader_status {
            ReaderStatus::Showing | ReaderStatus::NoReadme | ReaderStatus::ReadmeUncertain => true,
            ReaderStatus::ReadmeUnavailable(_)
            | ReaderStatus::SelectionUnavailable { .. }
            | ReaderStatus::ContentFailed(_) => true,
            ReaderStatus::Loading => {
                model.page_error.is_some() || (model.branch.is_some() && model.rows.is_empty())
            }
        };
        if terminal {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

// Keep the page's own navigation type referenced so this preview documents the saved intent shape.
#[allow(dead_code)]
fn saved_intent() -> RepositoryNavigation {
    RepositoryNavigation::default()
}
